use anyhow::{anyhow, Result};
use log::debug;
use serde_json::Value;
use std::process::Command;

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

pub fn encode_args(path: &str) -> Vec<String> {
    let info = ffprobe(path);
    let mut v: Vec<String> = vec![];
    let mut a: Vec<String> = vec![];

    let v_map: &[(&str, &str, &[&str])] = &[
        ("h264",       "libx264",    &["-crf","18","-preset","slow"]),
        ("hevc",       "libx265",    &["-crf","18","-preset","slow"]),
        ("vp9",        "libvpx-vp9", &["-crf","33","-b:v","0"]),
        ("vp8",        "libvpx",     &["-crf","10","-b:v","0"]),
        ("av1",        "libaom-av1", &["-crf","30","-b:v","0"]),
        ("mpeg4",      "mpeg4",      &["-qscale:v","3"]),
        ("mpeg2video", "mpeg2video", &["-qscale:v","3"]),
        ("prores",     "prores_ks",  &["-profile:v","3"]),
    ];
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
                let (enc, extra) = v_map.iter()
                    .find(|(k,_,_)| *k == cn)
                    .map(|(_,e,x)| (*e, *x))
                    .unwrap_or(("libx264", &["-crf","18","-preset","slow"] as &[&str]));
                v = ["-c:v", enc].iter().copied()
                    .chain(extra.iter().copied())
                    .chain(["-pix_fmt", pfmt])
                    .map(String::from).collect();
                debug!("v_enc={enc}  pix_fmt={pfmt}");
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
        v = ["-c:v","libx264","-crf","18","-preset","slow","-pix_fmt","yuv420p"]
            .iter().map(|s| s.to_string()).collect();
    }
    if a.is_empty() {
        a = ["-c:a","aac","-b:a","192k"].iter().map(|s| s.to_string()).collect();
    }
    v.extend(a);
    v
}
