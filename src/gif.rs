// GIF export: raw RGBA frames are pulled from the source via an ffmpeg
// subprocess (same pattern as thumbs.rs's frame grabs), cropped/scaled/
// fps-capped by ffmpeg's `-vf`, and fed to `gifski` for quantization/
// encoding. Two threads run concurrently — one reads ffmpeg's stdout and
// calls `Collector::add_frame_rgba`, the other drives `Writer::write` —
// because gifski's collector blocks once its internal queue is full until
// the writer is actively consuming (see gifski::new's doc comment).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use gifski::collector::{ImgVec, RGBA8};
use gifski::progress::ProgressReporter;
use gifski::{Repeat, Settings};
use log::{debug, warn};

use crate::crop::CropRect;

const GIF_WIDTH: u32 = 640;
const GIF_FPS: f64 = 20.0;
// gifski's `quality` (1-100): lower = lossier/smaller, higher = closer to
// source. 35 trades noticeably more compression for visible quantization/
// dithering artifacts, in exchange for much smaller files at 20fps.
const GIF_QUALITY: u8 = 35;

pub enum GifOutcome {
    Done(String),
    Err(String),
}

struct GifProgress {
    done: usize,
    total: usize,
    shared: Arc<Mutex<f32>>,
    ctx: egui::Context,
}

impl ProgressReporter for GifProgress {
    fn increase(&mut self) -> bool {
        self.done += 1;
        let frac = (self.done as f32 / self.total.max(1) as f32).clamp(0.0, 1.0);
        *self.shared.lock().unwrap() = frac;
        self.ctx.request_repaint();
        true
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_gif_export(
    path: String,
    in_t: f64,
    out_t: f64,
    src_w: u32,
    src_h: u32,
    crop: Option<CropRect>,
    out_path: String,
    progress: Arc<Mutex<f32>>,
    result: Arc<Mutex<Option<GifOutcome>>>,
    ctx: egui::Context,
) {
    thread::spawn(move || {
        // The crop rect's own size *is* the pre-scale frame size — no need
        // to separately track "cropped dimensions" elsewhere.
        let base_w = crop.map_or(src_w, |r| r.w.round().max(1.0) as u32);
        let base_h = crop.map_or(src_h, |r| r.h.round().max(1.0) as u32);
        let gif_w = GIF_WIDTH.min(base_w).max(2);
        let gif_h = ((base_h as f64 * gif_w as f64 / base_w as f64).round() as u32).max(2);

        let mut filters = Vec::new();
        if let Some(r) = crop {
            filters.push(r.to_vf());
        }
        filters.push(format!("scale={gif_w}:{gif_h}:flags=lanczos"));
        filters.push(format!("fps={GIF_FPS}"));
        let vf = filters.join(",");

        let dur = (out_t - in_t).max(0.001);
        let total_frames = ((dur * GIF_FPS).round() as usize).max(1);

        let args = vec![
            "-y".to_string(),
            "-ss".to_string(), format!("{in_t:.3}"),
            "-i".to_string(), path,
            "-t".to_string(), format!("{dur:.3}"),
            "-vf".to_string(), vf,
            "-an".to_string(),
            "-f".to_string(), "rawvideo".to_string(),
            "-pix_fmt".to_string(), "rgba".to_string(),
            "pipe:1".to_string(),
        ];
        debug!("ffmpeg gif frames: {}", args.join(" "));

        let mut cmd = Command::new("ffmpeg");
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                warn!("gif ffmpeg exec error: {e}");
                *result.lock().unwrap() = Some(GifOutcome::Err(e.to_string()));
                ctx.request_repaint();
                return;
            }
        };

        let stderr = child.stderr.take().expect("stderr was piped");
        let stderr_thread = thread::spawn(move || -> String {
            let mut buf = String::new();
            let _ = std::io::BufReader::new(stderr).read_to_string(&mut buf);
            buf
        });

        let mut stdout = child.stdout.take().expect("stdout was piped");

        let settings = Settings {
            // Frames are already scaled by ffmpeg's `-vf`, so pin gifski's
            // own (size-heuristic-driven) resize to a no-op by giving it
            // the exact dimensions we're about to feed it.
            width: Some(gif_w),
            height: Some(gif_h),
            quality: GIF_QUALITY,
            fast: false,
            repeat: Repeat::Infinite,
        };
        let (collector, writer) = match gifski::new(settings) {
            Ok(cw) => cw,
            Err(e) => {
                warn!("gifski init error: {e}");
                *result.lock().unwrap() = Some(GifOutcome::Err(e.to_string()));
                ctx.request_repaint();
                let _ = child.kill();
                return;
            }
        };

        let frame_size = gif_w as usize * gif_h as usize * 4;
        let frames_thread = thread::spawn(move || -> usize {
            let mut buf = vec![0u8; frame_size];
            let mut frame_index = 0usize;
            while stdout.read_exact(&mut buf).is_ok() {
                let pixels: Vec<RGBA8> = buf.chunks_exact(4)
                    .map(|c| RGBA8::new(c[0], c[1], c[2], c[3]))
                    .collect();
                let img = ImgVec::new(pixels, gif_w as usize, gif_h as usize);
                let pts = frame_index as f64 / GIF_FPS;
                if collector.add_frame_rgba(frame_index, img, pts).is_err() {
                    break;
                }
                frame_index += 1;
            }
            drop(collector);
            frame_index
        });

        let mut reporter = GifProgress {
            done: 0,
            total: total_frames,
            shared: progress.clone(),
            ctx: ctx.clone(),
        };
        let write_result = match std::fs::File::create(&out_path) {
            Ok(file) => writer.write(file, &mut reporter),
            Err(e) => Err(gifski::Error::Io(e)),
        };

        let encoded_frames = frames_thread.join().unwrap_or(0);
        let status = child.wait();
        let stderr_text = stderr_thread.join().unwrap_or_default();

        if let Ok(s) = &status {
            if !s.success() && encoded_frames == 0 {
                debug!("ffmpeg gif-frames exit {s}: {stderr_text}");
            }
        }
        let outcome = match write_result {
            Ok(()) if encoded_frames > 0 => {
                let name = Path::new(&out_path).file_name().unwrap_or_default().to_string_lossy().to_string();
                debug!("gif done: {name} ({encoded_frames} frames)");
                *progress.lock().unwrap() = 1.0;
                GifOutcome::Done(name)
            }
            Ok(()) => {
                warn!("gif produced no frames: {stderr_text}");
                GifOutcome::Err(if stderr_text.is_empty() { "no frames decoded".into() } else { stderr_text })
            }
            Err(e) => {
                warn!("gifski write error: {e}  ffmpeg stderr: {stderr_text}");
                GifOutcome::Err(e.to_string())
            }
        };
        *result.lock().unwrap() = Some(outcome);
        ctx.request_repaint();
    });
}
