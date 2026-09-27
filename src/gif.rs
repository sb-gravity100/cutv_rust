// GIF export: raw RGBA frames are decoded/filtered in-process via
// `ffmpeg-next` (same approach as `export.rs`'s final cut and `thumbs.rs`'s
// frame grabs, replacing the earlier spawned-`ffmpeg` subprocess),
// cropped/speed-adjusted/scaled/fps-capped by the same filter-graph chain
// `-vf` used to express, and fed to `gifski` for quantization/encoding.
// Two threads run concurrently — one decodes/filters frames and calls
// `Collector::add_frame_rgba`, the other drives `Writer::write` — because
// gifski's collector blocks once its internal queue is full until the
// writer is actively consuming (see gifski::new's doc comment).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Result};
use ffmpeg_next as ff;
use ff::{codec, format, frame, media, Rational};
use gifski::collector::{Collector, ImgVec, RGBA8};
use gifski::progress::ProgressReporter;
use gifski::{Repeat, Settings};
use log::{debug, warn};

use crate::crop::CropRect;

const GIF_WIDTH: u32 = 800;
const GIF_FPS: f64 = 20.0;
// gifski's `quality` (1-100): lower = lossier/smaller, higher = closer to
// source.
const GIF_QUALITY: u8 = 90;

/// ffmpeg's global microsecond time base (`AV_TIME_BASE`) — same
/// constant/pattern `export.rs`/`thumbs.rs` use for seeking.
const AV_TIME_BASE: i64 = 1_000_000;
/// Same tolerances `export.rs`'s trim logic uses — see its doc comments.
const TAIL_SLACK_SECS: f64 = 1.0;
const HEAD_EPS_SECS: f64 = 1e-4;

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
    speed: f64,
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
        if (speed - 1.0).abs() > 1e-6 {
            filters.push(format!("setpts={:.6}*PTS", 1.0 / speed));
        }
        filters.push(format!("scale={gif_w}:{gif_h}:flags=lanczos"));
        filters.push(format!("fps={GIF_FPS}"));
        // gifski needs packed RGBA8 — forced as an explicit trailing
        // `format=rgba` filter stage (text, same string-graph parser as
        // everything else here), same reasoning as export.rs's `aformat`
        // fix: a struct-based buffersink pixel-format setter is a
        // different, narrower API than this crate's text-graph path and
        // isn't worth the risk of hitting another version mismatch.
        filters.push("format=rgba".to_string());
        let vf = filters.join(",");

        let dur = (out_t - in_t).max(0.001);
        // The GIF's own output timeline is stretched/compressed by the
        // setpts filter above when speed != 1 — the source segment lasts
        // `dur` seconds, but the output runs for `dur / speed`.
        let out_dur = dur / speed.max(0.01);
        let total_frames = ((out_dur * GIF_FPS).round() as usize).max(1);

        let settings = Settings {
            // Frames are already scaled by the filter chain, so pin
            // gifski's own (size-heuristic-driven) resize to a no-op by
            // giving it the exact dimensions already produced.
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
                return;
            }
        };

        let frames_thread = thread::spawn(move || -> usize {
            match extract_frames(&path, in_t, out_t, &vf, collector) {
                Ok(n) => n,
                Err(e) => {
                    warn!("gif frame extraction failed: {e:#}");
                    0
                }
            }
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

        let outcome = match write_result {
            Ok(()) if encoded_frames > 0 => {
                let name = Path::new(&out_path).file_name().unwrap_or_default().to_string_lossy().to_string();
                debug!("gif done: {name} ({encoded_frames} frames)");
                *progress.lock().unwrap() = 1.0;
                GifOutcome::Done(name)
            }
            Ok(()) => {
                warn!("gif produced no frames");
                GifOutcome::Err("no frames decoded".into())
            }
            Err(e) => {
                warn!("gifski write error: {e}");
                GifOutcome::Err(e.to_string())
            }
        };
        *result.lock().unwrap() = Some(outcome);
        ctx.request_repaint();
    });
}

fn to_pts(secs: f64, tb: Rational) -> i64 {
    (secs * tb.denominator() as f64 / tb.numerator() as f64).round() as i64
}

fn ts_secs(pts: i64, tb: Rational) -> f64 {
    pts as f64 * tb.numerator() as f64 / tb.denominator() as f64
}

/// Decodes `[in_t, out_t)` of `src_path`'s video stream, runs it through
/// the filter-spec string `vf` (crop/setpts/scale/fps/format, see
/// `spawn_gif_export`), and pushes each resulting RGBA frame into
/// `collector`. Returns the number of frames pushed.
fn extract_frames(src_path: &str, in_t: f64, out_t: f64, vf: &str, collector: Collector) -> Result<usize> {
    ff::init()?;
    let mut ictx = format::input(&src_path)?;
    let v_index = ictx.streams().best(media::Type::Video)
        .ok_or_else(|| anyhow!("no video stream found in: {src_path}"))?
        .index();

    let (mut decoder, mut graph, tb) = {
        let stream = ictx.stream(v_index).ok_or_else(|| anyhow!("video stream vanished"))?;
        let tb = stream.time_base();
        let decoder = codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .video()?;
        let graph = build_graph(&decoder, tb, vf)?;
        (decoder, graph, tb)
    };

    let seek_ts = (in_t.max(0.0) * AV_TIME_BASE as f64).round() as i64;
    ictx.seek(seek_ts, ..seek_ts)?;
    decoder.flush();
    let in_t_pts = to_pts(in_t, tb);

    let mut frame_index = 0usize;
    let mut done = false;

    for (stream, packet) in ictx.packets() {
        if stream.index() != v_index { continue; }
        decoder.send_packet(&packet)?;
        done = drain(&mut decoder, &mut graph, tb, in_t_pts, in_t, out_t, &collector, &mut frame_index)?;
        if done { break; }
    }
    if !done {
        decoder.send_eof()?;
        drain(&mut decoder, &mut graph, tb, in_t_pts, in_t, out_t, &collector, &mut frame_index)?;
    }
    graph.get("in").ok_or_else(|| anyhow!("filter graph missing 'in'"))?.source().flush()?;
    push_filtered(&mut graph, &collector, &mut frame_index)?;

    Ok(frame_index)
}

fn build_graph(decoder: &ff::decoder::Video, tb: Rational, spec: &str) -> Result<ff::filter::Graph> {
    let mut graph = ff::filter::Graph::new();
    let sar = decoder.aspect_ratio();
    let sar = if sar.numerator() == 0 { Rational::new(1, 1) } else { sar };
    let pix_fmt = decoder.format().descriptor().map(|d| d.name()).unwrap_or("yuv420p");
    let args = format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
        decoder.width(), decoder.height(), pix_fmt,
        tb.numerator(), tb.denominator(),
        sar.numerator(), sar.denominator(),
    );
    graph.add(&ff::filter::find("buffer").ok_or_else(|| anyhow!("no buffer filter"))?, "in", &args)?;
    graph.add(&ff::filter::find("buffersink").ok_or_else(|| anyhow!("no buffersink filter"))?, "out", "")?;
    graph.output("in", 0)?.input("out", 0)?.parse(spec)?;
    graph.validate()?;
    Ok(graph)
}

#[allow(clippy::too_many_arguments)]
fn drain(
    decoder: &mut ff::decoder::Video, graph: &mut ff::filter::Graph, tb: Rational, in_t_pts: i64,
    in_t: f64, out_t: f64, collector: &Collector, frame_index: &mut usize,
) -> Result<bool> {
    let mut frame = frame::Video::empty();
    let mut done = false;
    while decoder.receive_frame(&mut frame).is_ok() {
        let Some(pts) = frame.pts() else { continue };
        let t = ts_secs(pts, tb);
        if t > out_t + TAIL_SLACK_SECS { done = true; continue; }
        if t > out_t { continue; }
        if t < in_t - HEAD_EPS_SECS { continue; }
        frame.set_pts(Some(pts - in_t_pts));
        graph.get("in").ok_or_else(|| anyhow!("filter graph missing 'in'"))?.source().add(&frame)?;
        push_filtered(graph, collector, frame_index)?;
    }
    Ok(done)
}

fn push_filtered(graph: &mut ff::filter::Graph, collector: &Collector, frame_index: &mut usize) -> Result<()> {
    let mut filtered = frame::Video::empty();
    while graph.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
        let (w, h) = (filtered.width() as usize, filtered.height() as usize);
        let stride = filtered.stride(0);
        let data = filtered.data(0);
        let mut pixels: Vec<RGBA8> = Vec::with_capacity(w * h);
        for row in 0..h {
            let row_bytes = &data[row * stride..row * stride + w * 4];
            pixels.extend(row_bytes.as_chunks::<4>().0.iter().map(|c| RGBA8::new(c[0], c[1], c[2], c[3])));
        }
        let img = ImgVec::new(pixels, w, h);
        let pts = *frame_index as f64 / GIF_FPS;
        if collector.add_frame_rgba(*frame_index, img, pts).is_err() {
            break;
        }
        *frame_index += 1;
    }
    Ok(())
}
