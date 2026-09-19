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
/// this is about the final `do_cut`/`do_gif`-source export encoder.
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

/// Source video stream's bitrate in kbps, for matching the cut's output
/// bitrate to it. Falls back to the container's overall bitrate (slightly
/// over-estimates video-only bitrate since it includes audio, but only
/// used when the stream itself has no `bit_rate` tag — common for MKV).
fn source_video_bitrate_kbps(info: &Value, vstream: &Value) -> Option<u64> {
    vstream["bit_rate"].as_str()
        .and_then(|b| b.parse::<u64>().ok())
        .or_else(|| info["format"]["bit_rate"].as_str().and_then(|b| b.parse::<u64>().ok()))
        .map(|b| (b / 1000).max(100))
}

/// `-b:v`/`-maxrate`/`-bufsize` targeting the given bitrate (maxrate at
/// 1.5x, bufsize at 2x, room for encoder-side VBR fluctuation around the
/// target without ballooning file size). `None` falls back to `-b:v 0`,
/// i.e. unconstrained — quality-only rate control (CRF/CQ), used when the
/// source bitrate couldn't be determined.
fn bitrate_args(br_kbps: Option<u64>) -> Vec<String> {
    match br_kbps {
        Some(br) => vec![
            "-b:v".into(), format!("{br}k"),
            "-maxrate".into(), format!("{}k", br * 3 / 2),
            "-bufsize".into(), format!("{}k", br * 2),
        ],
        None => vec!["-b:v".into(), "0".into()],
    }
}

fn nvenc_extra(br_kbps: Option<u64>, profile: &str) -> Vec<String> {
    ["-preset", "p6", "-tune", "hq", "-rc", "vbr", "-cq", "18"]
        .into_iter().map(String::from)
        .chain(bitrate_args(br_kbps))
        .chain(["-spatial_aq", "1", "-temporal_aq", "1", "-aq-strength", "8", "-rc-lookahead", "32"]
            .into_iter().map(String::from))
        .chain(["-profile:v".to_string(), profile.to_string()])
        .collect()
}

/// CPU h264/hevc: bitrate-targeted (ABR + VBV) when the source bitrate is
/// known, so the cut lands close to it; CRF-only quality mode otherwise
/// (`-b:v 0` doesn't mean anything to libx264/libx265 the way it does to
/// NVENC's CQ mode, so this needs its own fallback rather than reusing
/// `bitrate_args`' `None` case).
fn cpu_h26x_extra(br_kbps: Option<u64>) -> Vec<String> {
    let mut v = vec!["-preset".to_string(), "slow".to_string()];
    match br_kbps {
        Some(br) => v.extend(bitrate_args(Some(br))),
        None => v.extend(["-crf".to_string(), "18".to_string()]),
    }
    v
}

pub fn encode_args(path: &str) -> Vec<String> {
    let info = ffprobe(path);
    let mut v: Vec<String> = vec![];
    let mut a: Vec<String> = vec![];

    // NVENC only covers h264/hevc; everything else falls through to a CPU
    // encoder even when NVENC is available (same as the external reference).
    let use_nvenc = nvenc_available();
    let lossless = ["flac","pcm_s16le","pcm_s24le","pcm_s32le","pcm_f32le","alac"];
    let a_map: &[(&str, &str)] = &[
        ("aac","aac"), ("mp3","libmp3lame"), ("opus","libopus"), ("vorbis","libvorbis"),
        ("flac","flac"), ("ac3","ac3"), ("eac3","eac3"),
        ("pcm_s16le","pcm_s16le"), ("pcm_s24le","pcm_s24le"), ("alac","alac"),
    ];

    if let Some(streams) = info["streams"].as_array() {
        for s in streams {
            let ct = s["codec_type"].as_str().unwrap_or("");
            let cn = s["codec_name"].as_str().unwrap_or("");

            if ct == "video" && v.is_empty() {
                let pfmt = s["pix_fmt"].as_str().unwrap_or("yuv420p");
                let br = source_video_bitrate_kbps(&info, s);
                let (enc, extra): (&str, Vec<String>) = match cn {
                    "h264" if use_nvenc => ("h264_nvenc", nvenc_extra(br, "high")),
                    "hevc" if use_nvenc => ("hevc_nvenc", nvenc_extra(br, "main")),
                    "h264" => ("libx264", cpu_h26x_extra(br)),
                    "hevc" => ("libx265", cpu_h26x_extra(br)),
                    "vp9"  => ("libvpx-vp9", ["-crf","33"].into_iter().map(String::from).chain(bitrate_args(br)).collect()),
                    "vp8"  => ("libvpx",     ["-crf","10"].into_iter().map(String::from).chain(bitrate_args(br)).collect()),
                    "av1"  => ("libaom-av1", ["-crf","30"].into_iter().map(String::from).chain(bitrate_args(br)).collect()),
                    "mpeg4"      => ("mpeg4",      vec!["-qscale:v".to_string(), "3".to_string()]),
                    "mpeg2video" => ("mpeg2video", vec!["-qscale:v".to_string(), "3".to_string()]),
                    "prores"     => ("prores_ks",  vec!["-profile:v".to_string(), "3".to_string()]),
                    _ if use_nvenc => ("h264_nvenc", nvenc_extra(br, "high")),
                    _ => ("libx264", cpu_h26x_extra(br)),
                };
                v = ["-c:v", enc].iter().copied().map(String::from)
                    .chain(extra)
                    .chain(["-pix_fmt".to_string(), pfmt.to_string()])
                    .collect();
                debug!("v_enc={enc}  pix_fmt={pfmt}  nvenc={use_nvenc}  src_bitrate={br:?}kbps");
            }
            if ct == "audio" && a.is_empty() {
                let enc = a_map.iter().find(|(k,_)| *k == cn).map(|(_,v)| *v).unwrap_or("aac");
                if lossless.contains(&cn) {
                    a = ["-c:a", enc].iter().map(|s| s.to_string()).collect();
                } else {
                    let br = s["bit_rate"].as_str()
                        .and_then(|b| b.parse::<u64>().ok())
                        .map(|b| (b / 1000).max(64))
                        .unwrap_or(192);
                    a = ["-c:a", enc, "-b:a", &format!("{br}k")]
                        .iter().map(|s| s.to_string()).collect();
                }
                debug!("a_enc={enc}");
            }
        }
    }

    if v.is_empty() {
        // No video stream was found to read a source bitrate from, so this
        // path always falls back to quality-only rate control.
        let (enc, extra) = if use_nvenc {
            ("h264_nvenc", nvenc_extra(None, "high"))
        } else {
            ("libx264", cpu_h26x_extra(None))
        };
        v = ["-c:v", enc].iter().copied().map(String::from)
            .chain(extra)
            .chain(["-pix_fmt".to_string(), "yuv420p".to_string()])
            .collect();
    }
    if a.is_empty() {
        a = ["-c:a","aac","-b:a","192k"].iter().map(|s| s.to_string()).collect();
    }
    v.extend(a);
    v
}
