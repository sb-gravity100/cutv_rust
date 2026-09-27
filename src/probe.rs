use anyhow::{anyhow, Result};
use ffmpeg_next as ff;
use ff::media;
use log::debug;
use std::sync::OnceLock;

pub struct VideoInfo {
    pub duration: f64,
    pub fps:      f64,
    pub width:    u32,
    pub height:   u32,
}

/// ffmpeg's global microsecond time base (`AV_TIME_BASE`) — same constant
/// `export.rs` uses for its own seek/duration math.
const AV_TIME_BASE: i64 = 1_000_000;

/// Reads duration/fps/width/height directly via `ffmpeg-next` (libavformat)
/// instead of shelling out to `ffprobe` — the same in-process approach
/// `export.rs` uses for the final cut. Opens a decoder just long enough to
/// read `width()`/`height()` (`codec::Parameters` doesn't expose them
/// directly in this crate version); no frames are actually decoded.
pub fn probe_video(path: &str) -> Result<VideoInfo> {
    ff::init()?;
    let ictx = ff::format::input(&path)?;
    let stream = ictx.streams().best(media::Type::Video)
        .ok_or_else(|| anyhow!("no video stream found in: {path}"))?;

    let rate = stream.rate();
    let fps = if rate.denominator() > 0 {
        rate.numerator() as f64 / rate.denominator() as f64
    } else {
        30.0
    };

    let container_dur = ictx.duration();
    let duration = if container_dur > 0 {
        container_dur as f64 / AV_TIME_BASE as f64
    } else {
        let tb = stream.time_base();
        let sdur = stream.duration();
        if sdur > 0 && tb.denominator() > 0 {
            sdur as f64 * tb.numerator() as f64 / tb.denominator() as f64
        } else {
            return Err(anyhow!("no duration in: {path}"));
        }
    };

    let decoder = ff::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    let width = decoder.width();
    let height = decoder.height();

    debug!("probe: {width}x{height} @ {fps:.3}fps  dur={duration:.3}s");
    Ok(VideoInfo { duration, fps, width, height })
}

/// Whether an NVENC h264 encoder is registered in the linked FFmpeg build.
/// Cached — the registry lookup is cheap but this also runs `ff::init()`,
/// which only needs to happen once. Unrelated to GStreamer's own
/// (currently broken) `nvcodec` feature, which is playback-decode only;
/// this is about `export.rs`'s and `proxy.rs`'s NVENC encoder selection.
pub fn nvenc_available() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let _ = ff::init();
        let available = ff::encoder::find_by_name("h264_nvenc").is_some();
        debug!("nvenc_available={available}");
        available
    })
}

/// ffmpeg's `atempo` filter only accepts a 0.5-2.0 rate per stage; chain
/// multiple stages to cover this app's full speed range (0.1x-10x). Used as
/// a libavfilter graph-string fragment by `export.rs` (pitch-preserving
/// speed change).
pub fn atempo_chain(rate: f64) -> String {
    let mut r = rate.max(0.01);
    let mut stages = Vec::new();
    while r > 2.0 { stages.push("atempo=2.0".to_string()); r /= 2.0; }
    while r < 0.5 { stages.push("atempo=0.5".to_string()); r /= 0.5; }
    stages.push(format!("atempo={r:.6}"));
    stages.join(",")
}
