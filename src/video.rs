use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use log::debug;

pub struct FrameData {
    pub ts:  f64,
    pub rgb: Vec<u8>,
    pub w:   u32,
    pub h:   u32,
}

// ── Playback decoder ──────────────────────────────────────────────────────────

pub struct VideoDecoder {
    stop:   Arc<AtomicBool>,
    child:  Arc<Mutex<Option<Child>>>,
    thread: Option<JoinHandle<()>>,
    pub rx: mpsc::Receiver<Option<FrameData>>,
}

impl VideoDecoder {
    pub fn spawn(
        path: &str, start_t: f64, w: u32, h: u32, fps: f64,
        ctx: egui::Context,
    ) -> Self {
        let stop  = Arc::new(AtomicBool::new(false));
        let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::sync_channel::<Option<FrameData>>(4);

        let stop2      = stop.clone();
        let child_arc2 = child_arc.clone();
        let path       = path.to_string();

        let thread = thread::spawn(move || {
            let mut cmd = play_cmd(&path, start_t, w, h);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => { log::error!("ffmpeg spawn failed: {e}"); return; }
            };
            let mut stdout = child.stdout.take().unwrap();
            *child_arc2.lock().unwrap() = Some(child);

            let fsz = (w * h * 3) as usize;
            let mut buf = vec![0u8; fsz];
            let mut idx = 0u64;
            debug!("decoder started  ss={start_t:.3}s  {w}x{h}");

            loop {
                if stop2.load(Ordering::Relaxed) { break; }
                match stdout.read_exact(&mut buf) {
                    Ok(()) => {
                        let ts = start_t + idx as f64 / fps;
                        idx += 1;
                        let frame = FrameData { ts, rgb: buf.clone(), w, h };
                        if tx.send(Some(frame)).is_err() { break; }
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        debug!("decoder eof: {e}");
                        let _ = tx.send(None);
                        break;
                    }
                }
            }

            if let Some(mut c) = child_arc2.lock().unwrap().take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            debug!("decoder exited  frames={idx}");
        });

        VideoDecoder { stop, child: child_arc, thread: Some(thread), rx }
    }

    pub fn stop(&mut self) {
        debug!("decoder stop requested");
        self.stop.store(true, Ordering::Relaxed);
        // Kill the child process so read_exact unblocks immediately
        if let Ok(mut g) = self.child.lock() {
            if let Some(mut c) = g.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        // Drain channel so the sender can unblock if queued
        while self.rx.try_recv().is_ok() {}
        if let Some(t) = self.thread.take() { let _ = t.join(); }
        debug!("decoder stopped");
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) { self.stop(); }
}

// ── Seek preview worker ───────────────────────────────────────────────────────

struct SeekReq { t: f64, w: u32, h: u32, path: String }

pub struct SeekWorker {
    req:        Arc<Mutex<Option<SeekReq>>>,
    pub result: Arc<Mutex<Option<FrameData>>>,
    _thread:    JoinHandle<()>,
}

impl SeekWorker {
    pub fn spawn(ctx: egui::Context) -> Self {
        let req:    Arc<Mutex<Option<SeekReq>>>   = Arc::new(Mutex::new(None));
        let result: Arc<Mutex<Option<FrameData>>> = Arc::new(Mutex::new(None));

        let req2    = req.clone();
        let result2 = result.clone();

        let thread = thread::spawn(move || loop {
            let r = req2.lock().unwrap().take();
            if let Some(SeekReq { t, w, h, path }) = r {
                debug!("seek preview  t={t:.3}s");
                let mut cmd = seek_cmd(&path, t, w, h);
                let frame_sz = (w * h * 3) as usize;
                match cmd.output() {
                    Ok(o) if o.stdout.len() == frame_sz => {
                        *result2.lock().unwrap() =
                            Some(FrameData { ts: t, rgb: o.stdout, w, h });
                        ctx.request_repaint();
                    }
                    Ok(o) => debug!("seek size mismatch: {} vs {frame_sz}", o.stdout.len()),
                    Err(e) => log::warn!("seek cmd failed: {e}"),
                }
            } else {
                thread::sleep(std::time::Duration::from_millis(8));
            }
        });

        SeekWorker { req, result, _thread: thread }
    }

    pub fn request(&self, path: &str, t: f64, w: u32, h: u32) {
        *self.req.lock().unwrap() = Some(SeekReq { t, w, h, path: path.to_string() });
    }

    pub fn poll(&self) -> Option<FrameData> {
        self.result.lock().unwrap().take()
    }
}

// ── Thumbnail generator ────────────────────────────────────────────────────────

pub struct ThumbData {
    pub t:   f64,
    pub rgb: Vec<u8>,
    pub w:   u32,
    pub h:   u32,
}

pub fn spawn_thumbs(
    path: String, duration: f64, ctx: egui::Context,
) -> Arc<Mutex<Vec<ThumbData>>> {
    let pending: Arc<Mutex<Vec<ThumbData>>> = Arc::new(Mutex::new(Vec::new()));
    let p2 = pending.clone();
    const N:  u32 = 24;
    const TW: u32 = 72;
    const TH: u32 = 44;

    thread::spawn(move || {
        for i in 0..N {
            let t = duration * (i as f64 + 0.5) / N as f64;
            let mut cmd = seek_cmd(&path, t, TW, TH);
            if let Ok(o) = cmd.output() {
                let exp = (TW * TH * 3) as usize;
                if o.stdout.len() == exp {
                    p2.lock().unwrap().push(ThumbData { t, rgb: o.stdout, w: TW, h: TH });
                    ctx.request_repaint();
                    debug!("thumb {}/{N}  t={t:.1}s", i + 1);
                }
            }
        }
        debug!("all thumbnails done");
    });

    pending
}

// ── Proxy builder ─────────────────────────────────────────────────────────────

pub struct ProxyBuilder {
    pub ready: Arc<AtomicBool>,
    _thread:   JoinHandle<()>,
}

/// Transcodes `src` to a small H.264/ultrafast proxy at display resolution.
/// All playback, seeks, and thumbnails should use the proxy path, not the source.
pub fn spawn_proxy(src: String, proxy_path: String, dw: u32, dh: u32) -> ProxyBuilder {
    let ready  = Arc::new(AtomicBool::new(false));
    let ready2 = ready.clone();

    let thread = thread::spawn(move || {
        debug!("proxy encode start → {proxy_path}");
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-i", &src,
            "-vf", &format!("scale={dw}:{dh}"),
            "-c:v", "libx264", "-preset", "ultrafast", "-crf", "28",
            "-an",
            &proxy_path,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        no_window(&mut cmd);
        match cmd.status() {
            Ok(s) if s.success() => {
                ready2.store(true, Ordering::Relaxed);
                debug!("proxy ready: {proxy_path}");
            }
            Ok(s)  => log::warn!("proxy encode exit={s}"),
            Err(e) => log::warn!("proxy encode error: {e}"),
        }
    });

    ProxyBuilder { ready, _thread: thread }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn play_cmd(path: &str, ss: f64, w: u32, h: u32) -> Command {
    let mut c = Command::new("ffmpeg");
    c.args([
        "-re", // pace decode/output to the source's native frame rate — without this
               // ffmpeg dumps frames down the pipe as fast as it can decode them
        "-ss", &format!("{ss:.3}"),
        "-i", path,
        "-vf", &format!("scale={w}:{h}"),
        "-pix_fmt", "rgb24",
        "-f", "rawvideo",
        "-an",
        "pipe:1",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    no_window(&mut c);
    c
}

fn seek_cmd(path: &str, ss: f64, w: u32, h: u32) -> Command {
    let mut c = Command::new("ffmpeg");
    c.args([
        "-ss", &format!("{ss:.3}"),
        "-i", path,
        "-vframes", "1",
        "-vf", &format!("scale={w}:{h}"),
        "-pix_fmt", "rgb24",
        "-f", "rawvideo",
        "-an",
        "pipe:1",
    ])
    .stderr(Stdio::null());
    no_window(&mut c);
    c
}

fn no_window(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
}
