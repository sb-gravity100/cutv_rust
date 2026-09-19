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

use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use log::{debug, warn};

/// Bytes sampled from each end of the source file for `video_cache_key` —
/// bounded so hashing a multi-GB video stays fast (worst case ~2MiB read)
/// instead of hashing the whole file.
const HASH_SAMPLE_BYTES: u64 = 1024 * 1024;

/// Cheap content-based cache key for a source video: file length + a sample
/// of bytes from the start and end, hashed. Not cryptographic, but collision
/// odds are irrelevant here — worst case is a redundant re-transcode, never
/// wrong playback. Lets two different paths pointing at the same content
/// (a renamed/copied file) reuse the same scrub proxy instead of
/// re-transcoding.
fn video_cache_key(path: &str) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    let mut hasher = DefaultHasher::new();
    len.hash(&mut hasher);

    let head_len = HASH_SAMPLE_BYTES.min(len) as usize;
    let mut head = vec![0u8; head_len];
    file.read_exact(&mut head).ok()?;
    head.hash(&mut hasher);

    if len > HASH_SAMPLE_BYTES {
        let tail_len = HASH_SAMPLE_BYTES.min(len) as usize;
        file.seek(SeekFrom::End(-(tail_len as i64))).ok()?;
        let mut tail = vec![0u8; tail_len];
        file.read_exact(&mut tail).ok()?;
        tail.hash(&mut hasher);
    }

    Some(format!("{:016x}", hasher.finish()))
}

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
///
/// The proxy is named after `video_cache_key(path)`, not the source
/// filename or process id: if a proxy for this exact content already exists
/// in the temp dir (built earlier this session, or left over from a
/// previous session — including a crashed one, since nothing but a clean
/// app exit deletes them, see `cleanup_all_proxies`) it's reused as-is and
/// no transcode runs at all.
pub fn spawn_proxy(path: String, ctx: egui::Context) -> Arc<Mutex<Option<PathBuf>>> {
    let result: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let result2 = result.clone();

    thread::spawn(move || {
        let key = video_cache_key(&path).unwrap_or_else(|| {
            // File couldn't be hashed (unreadable, vanished, ...) — fall
            // back to a pid-based name so the transcode below at least has
            // somewhere to write; no cross-session reuse in this case.
            format!("nohash_{}_{}", std::process::id(), std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
        });
        let out_path = std::env::temp_dir().join(format!("cutv_proxy_{key}.mp4"));

        if out_path.is_file() {
            debug!("scrub proxy: reusing cached proxy for {path} -> {out_path:?}");
            *result2.lock().unwrap() = Some(out_path);
            ctx.request_repaint();
            return;
        }

        // Transcode to a .tmp sibling and rename into place only on success,
        // so a half-written file from an interrupted/crashed transcode is
        // never mistaken for a valid cached proxy on a later run.
        let tmp_path = out_path.with_extension("mp4.tmp");
        let t0 = std::time::Instant::now();
        debug!("scrub proxy: transcoding {path} -> {out_path:?} (GOP={PROXY_GOP})");

        // Try NVENC first (this machine has an NVIDIA GPU — confirmed via
        // `ffmpeg -encoders`), fall back to libx264 ultrafast if it's
        // unavailable or fails. Either way: short fixed GOP is what
        // actually matters here, not the codec/encoder choice.
        let ok = run_transcode(&path, &tmp_path, true) || run_transcode(&path, &tmp_path, false);

        if ok && std::fs::rename(&tmp_path, &out_path).is_ok() {
            debug!("scrub proxy: ready in {:?}", t0.elapsed());
            *result2.lock().unwrap() = Some(out_path);
            ctx.request_repaint();
        } else {
            warn!("scrub proxy: transcode failed (both nvenc and libx264) — staying on the original file, seeks will stay slow");
            let _ = std::fs::remove_file(&tmp_path);
        }
    });

    result
}

/// Deletes every scrub-proxy temp file in the system temp dir — this
/// session's and any leftover from a previous one (including a crashed
/// session, which never runs `CutvApp`'s `Drop`). Called once on normal app
/// exit. Proxies are deliberately *not* deleted as videos are switched
/// mid-session (see `app.rs::load_video`) so that switching back to an
/// already-opened video reuses its existing proxy instead of
/// re-transcoding; this is what actually clears them out.
pub fn cleanup_all_proxies() {
    let dir = std::env::temp_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            debug!("scrub proxy cleanup: failed to read {dir:?}: {e}");
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("cutv_proxy_") {
            let path = entry.path();
            if let Err(e) = std::fs::remove_file(&path) {
                debug!("scrub proxy cleanup: failed to remove {path:?}: {e}");
            } else {
                debug!("scrub proxy cleanup: removed {path:?}");
            }
        }
    }
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
        "-f", "mp4", // output path is a `.mp4.tmp` sibling during transcode (see spawn_proxy) — ffmpeg can't infer the container from that extension, so force it
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
