// Background scrub-proxy transcode — the real fix for "accurate and fast
// like regular video editors" on long-GOP source video (see PLAN.md/
// conversation 2026-09-19). A frame-exact seek has to decode forward from
// the keyframe before it to the target; on a source with a long GOP (this
// project hit a ~250-frame/4.2s GOP in testing) that's 500ms+ per seek no
// matter how fast decode/convert are — gating/coalescing rapid requests
// only stops that cost from compounding, it doesn't remove it. Professional
// NLEs solve this the same way: transcode a short-GOP ("all the seeks are
// cheap") proxy in the background for scrubbing/preview, and only touch the
// original file for the final export. `do_cut` in app.rs already does that
// (encode_args always targets `self.path`, the original) — this module just
// gives `Player` a fast file to point at instead.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use log::{debug, warn};

/// Every frame in the proxy is a keyframe within this many frames of any
/// other — i.e. the worst-case seek only ever has to decode this many
/// frames forward, regardless of the source's own GOP structure. Small
/// enough to feel instant, large enough not to bloat the proxy file size
/// pointlessly (true all-intra, GOP=1, is markedly bigger for little
/// perceptible seek-speed gain once you're already down from hundreds of
/// frames to single digits).
const PROXY_GOP: u32 = 8;

/// Kicks off a background transcode of `path` into a short-GOP proxy file
/// and returns a handle that resolves to its path once ready. The caller
/// (app.rs) keeps using the original file until then, then swaps `Player`
/// over to the proxy at the current position — playback is usable
/// immediately, just without fast accurate seeking until the proxy lands.
pub fn spawn_proxy(path: String, ctx: egui::Context) -> Arc<Mutex<Option<PathBuf>>> {
    let result: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let result2 = result.clone();

    thread::spawn(move || {
        let out_path = std::env::temp_dir()
            .join(format!("cutv_proxy_{}.mp4", std::process::id()));

        let t0 = std::time::Instant::now();
        debug!("scrub proxy: transcoding {path} -> {out_path:?} (GOP={PROXY_GOP})");

        // Try NVENC first (this machine has an NVIDIA GPU — confirmed via
        // `ffmpeg -encoders`), fall back to libx264 ultrafast if it's
        // unavailable or fails. Either way: short fixed GOP is what
        // actually matters here, not the codec/encoder choice.
        let ok = run_transcode(&path, &out_path, true) || run_transcode(&path, &out_path, false);

        if ok {
            debug!("scrub proxy: ready in {:?}", t0.elapsed());
            *result2.lock().unwrap() = Some(out_path);
            ctx.request_repaint();
        } else {
            warn!("scrub proxy: transcode failed (both nvenc and libx264) — staying on the original file, seeks will stay slow");
        }
    });

    result
}

fn run_transcode(path: &str, out_path: &std::path::Path, try_nvenc: bool) -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-i", path]);
    if try_nvenc {
        cmd.args(["-c:v", "h264_nvenc", "-preset", "p1", "-rc", "vbr", "-cq", "23"]);
    } else {
        cmd.args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "20"]);
    }
    cmd.args([
        "-g", &PROXY_GOP.to_string(),
        "-keyint_min", &PROXY_GOP.to_string(),
        "-sc_threshold", "0", // disable scene-cut adaptive keyframes — GOP must stay fixed/short
        "-c:a", "aac", "-b:a", "160k",
        "-movflags", "+faststart",
    ]);
    cmd.arg(out_path);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    no_window(&mut cmd);

    match cmd.output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            let which = if try_nvenc { "nvenc" } else { "libx264" };
            debug!("scrub proxy: {which} transcode failed: {}", String::from_utf8_lossy(&o.stderr));
            false
        }
        Err(e) => {
            warn!("scrub proxy: failed to spawn ffmpeg: {e}");
            false
        }
    }
}

fn no_window(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
}
