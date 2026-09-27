use anyhow::{anyhow, Result};
use log::debug;
use serde_json::Value;
use std::process::Command;
use std::sync::OnceLock;

pub struct VideoInfo {
    pub duration: f64,
    pub fps:      f64,
    pub width:    u32,
    pub height:   u32,
}

pub fn ffprobe(path: &str) -> Value {
    debug!("ffprobe: {path}");
    let out = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_streams",
            "-show_format",
            path,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
        }
        Ok(o) => {
            log::warn!("ffprobe stderr: {}", String::from_utf8_lossy(&o.stderr));
            Value::Null
        }
        Err(e) => { log::warn!("ffprobe exec: {e}"); Value::Null }
    }
}

pub fn probe_video(path: &str) -> Result<VideoInfo> {
    let info = ffprobe(path);
    let streams = info["streams"]
        .as_array()
        .ok_or_else(|| anyhow!("ffprobe returned no streams for: {path}"))?;

    for s in streams {
        if s["codec_type"].as_str() != Some("video") { continue; }
        let width  = s["width"].as_u64().ok_or_else(|| anyhow!("no width"))? as u32;
        let height = s["height"].as_u64().ok_or_else(|| anyhow!("no height"))? as u32;
        let fps    = crate::util::parse_fps(s["r_frame_rate"].as_str().unwrap_or("30/1"));
        let dur    = info["format"]["duration"].as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| s["duration"].as_str().and_then(|v| v.parse().ok()))
            .ok_or_else(|| anyhow!("no duration in: {path}"))?;
        debug!("probe: {width}x{height} @ {fps:.3}fps  dur={dur:.3}s");
        return Ok(VideoInfo { duration: dur, fps, width, height });
    }
    Err(anyhow!("no video stream found in: {path}"))
}

/// Whether the `PATH` ffmpeg has NVENC support (`h264_nvenc` in `-encoders`).
/// Cached — shells out at most once per process. Unrelated to GStreamer's
/// own (currently broken) `nvcodec` feature, which is playback-decode only;
/// this is about the final export encoder (`export.rs`'s NVENC selection).
pub fn nvenc_available() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let out = Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output();
        let available = match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains("h264_nvenc"),
            Err(e) => { log::warn!("nvenc probe exec: {e}"); false }
        };
        debug!("nvenc_available={available}");
        available
    })
}

/// ffmpeg's `atempo` filter only accepts a 0.5-2.0 rate per stage; chain
/// multiple stages to cover this app's full speed range (0.1x-10x). Used as
/// a libavfilter graph-string fragment by `export.rs` (pitch-preserving
/// speed change) — same string shape as the CLI `-af` value this replaced.
pub fn atempo_chain(rate: f64) -> String {
    let mut r = rate.max(0.01);
    let mut stages = Vec::new();
    while r > 2.0 { stages.push("atempo=2.0".to_string()); r /= 2.0; }
    while r < 0.5 { stages.push("atempo=0.5".to_string()); r /= 0.5; }
    stages.push(format!("atempo={r:.6}"));
    stages.join(",")
}
