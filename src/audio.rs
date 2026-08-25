use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use log::debug;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

pub struct AudioPlayer {
    _stream: OutputStream,
    handle:  OutputStreamHandle,
    sink:    Option<Sink>,
    tmp:     PathBuf,
    pub ready: Arc<AtomicBool>,
    muted:   bool,
}

impl AudioPlayer {
    /// Constructs the player and immediately starts background audio extraction.
    pub fn new(video_path: &str) -> Option<Self> {
        let (stream, handle) = OutputStream::try_default()
            .map_err(|e| log::warn!("audio output init failed: {e}"))
            .ok()?;

        let tmp   = std::env::temp_dir().join(format!("cutv_{}.mp3", std::process::id()));
        let ready = Arc::new(AtomicBool::new(false));

        {
            let vp     = video_path.to_string();
            let tp     = tmp.clone();
            let ready2 = ready.clone();
            thread::spawn(move || {
                debug!("audio extract -> {tp:?}");
                let mut cmd = Command::new("ffmpeg");
                cmd.args([
                    "-y", "-i", &vp,
                    "-vn", "-c:a", "libmp3lame", "-q:a", "4",
                    tp.to_str().unwrap(),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
                no_window(&mut cmd);
                match cmd.status() {
                    Ok(s) if s.success() => {
                        ready2.store(true, Ordering::Relaxed);
                        debug!("audio ready");
                    }
                    Ok(s)  => log::warn!("audio extract exit: {s}"),
                    Err(e) => log::warn!("audio ffmpeg error: {e}"),
                }
            });
        }

        Some(AudioPlayer { _stream: stream, handle, sink: None, tmp, ready, muted: false })
    }

    /// Start (or restart) playback from `start_t` seconds.
    pub fn play(&mut self, start_t: f64) {
        if !self.ready.load(Ordering::Relaxed) {
            debug!("audio play skipped — not ready yet");
            return;
        }
        debug!("audio play  ss={start_t:.3}s");
        self.stop_sink();

        if let Ok(file) = File::open(&self.tmp) {
            match Decoder::new(BufReader::new(file)) {
                Ok(src) => match Sink::try_new(&self.handle) {
                    Ok(sink) => {
                        sink.set_volume(if self.muted { 0.0 } else { 1.0 });
                        sink.append(src);
                        if start_t > 0.01 {
                            if let Err(e) = sink.try_seek(Duration::from_secs_f64(start_t)) {
                                log::warn!("audio seek failed: {e}");
                            }
                        }
                        sink.play();
                        self.sink = Some(sink);
                    }
                    Err(e) => log::warn!("audio sink creation failed: {e}"),
                },
                Err(e) => log::warn!("audio decoder error: {e}"),
            }
        }
    }

    pub fn pause(&self) {
        if let Some(s) = &self.sink { s.pause(); debug!("audio paused"); }
    }

    pub fn set_mute(&mut self, m: bool) {
        self.muted = m;
        if let Some(s) = &self.sink { s.set_volume(if m { 0.0 } else { 1.0 }); }
        debug!("audio muted={m}");
    }

    fn stop_sink(&mut self) {
        if let Some(s) = self.sink.take() { s.stop(); }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop_sink();
        if self.tmp.exists() { let _ = std::fs::remove_file(&self.tmp); }
        debug!("AudioPlayer dropped");
    }
}

fn no_window(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
}
