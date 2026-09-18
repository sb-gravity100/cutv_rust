// Timeline thumbnail generation — independent of the player (Player/mpv
// owns playback; this just shells out to ffmpeg for single-frame grabs).
// Extracted out of the old video.rs when that was replaced by player.rs.

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use log::debug;

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
