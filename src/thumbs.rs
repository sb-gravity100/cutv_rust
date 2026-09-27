// Timeline thumbnail generation — independent of the player (Player owns
// playback; this decodes its own single frames in-process via
// `ffmpeg-next` instead of shelling out to `ffmpeg` per thumbnail).
// Extracted out of the old video.rs when that was replaced by player.rs.

use std::sync::{Arc, Mutex};
use std::thread;

use ffmpeg_next as ff;
use ff::{format, media, software::scaling};
use log::{debug, warn};

pub struct ThumbData {
    pub t:   f64,
    pub rgb: Vec<u8>,
    pub w:   u32,
    pub h:   u32,
}

const N:  u32 = 24;
const TW: u32 = 72;
const TH: u32 = 44;

/// ffmpeg's global microsecond time base (`AV_TIME_BASE`), used by
/// `format::context::Input::seek`'s stream_index=-1 convention — same
/// constant/pattern `export.rs` uses.
const AV_TIME_BASE: i64 = 1_000_000;

pub fn spawn_thumbs(
    path: String, duration: f64, ctx: egui::Context,
) -> Arc<Mutex<Vec<ThumbData>>> {
    let pending: Arc<Mutex<Vec<ThumbData>>> = Arc::new(Mutex::new(Vec::new()));
    let p2 = pending.clone();

    thread::spawn(move || {
        if let Err(e) = ff::init() {
            warn!("thumbs: ffmpeg-next init failed: {e}");
            return;
        }
        let mut ictx = match format::input(&path) {
            Ok(c) => c,
            Err(e) => { warn!("thumbs: open {path} failed: {e}"); return; }
        };
        let v_index = match ictx.streams().best(media::Type::Video) {
            Some(s) => s.index(),
            None => { warn!("thumbs: no video stream in {path}"); return; }
        };
        let ist_time_base = ictx.stream(v_index).expect("just looked up").time_base();
        let params = ictx.stream(v_index).expect("just looked up").parameters();
        let mut decoder = match ff::codec::context::Context::from_parameters(params)
            .and_then(|c| c.decoder().video())
        {
            Ok(d) => d,
            Err(e) => { warn!("thumbs: decoder open failed: {e}"); return; }
        };
        let mut scaler = match scaling::Context::get(
            decoder.format(), decoder.width(), decoder.height(),
            format::Pixel::RGB24, TW, TH, scaling::Flags::BILINEAR,
        ) {
            Ok(s) => s,
            Err(e) => { warn!("thumbs: scaler init failed: {e}"); return; }
        };

        for i in 0..N {
            let t = duration * (i as f64 + 0.5) / N as f64;
            match grab_frame_at(&mut ictx, v_index, ist_time_base, &mut decoder, &mut scaler, t) {
                Some(rgb) => {
                    p2.lock().unwrap().push(ThumbData { t, rgb, w: TW, h: TH });
                    ctx.request_repaint();
                    debug!("thumb {}/{N}  t={t:.1}s", i + 1);
                }
                None => debug!("thumb {}/{N}  t={t:.1}s  -> no frame decoded, skipped", i + 1),
            }
        }
        debug!("all thumbnails done");
    });

    pending
}

/// Seeks to `t`, decodes forward from the preceding keyframe, and returns
/// the first frame at or past `t` scaled to `TW`x`TH` RGB24 — same
/// approximate accuracy `-ss` (before `-i`, decoded rather than
/// keyframe-snapped) gave the old CLI path. `MAX_FRAMES` bounds how far
/// forward a pathological seek (e.g. a very sparse-keyframe source) is
/// allowed to decode before giving up, so one bad thumbnail can't hang the
/// whole strip.
fn grab_frame_at(
    ictx: &mut format::context::Input,
    v_index: usize,
    ist_time_base: ff::Rational,
    decoder: &mut ff::decoder::Video,
    scaler: &mut scaling::Context,
    t: f64,
) -> Option<Vec<u8>> {
    const MAX_FRAMES: u32 = 500;

    let seek_ts = (t.max(0.0) * AV_TIME_BASE as f64).round() as i64;
    if let Err(e) = ictx.seek(seek_ts, ..seek_ts) {
        warn!("thumbs: seek({t:.3}) failed: {e}");
        return None;
    }
    decoder.flush();

    let mut frame = ff::frame::Video::empty();
    let mut scanned = 0u32;
    for (stream, packet) in ictx.packets() {
        if stream.index() != v_index { continue; }
        if decoder.send_packet(&packet).is_err() { continue; }
        while decoder.receive_frame(&mut frame).is_ok() {
            scanned += 1;
            let Some(pts) = frame.pts() else { continue };
            let frame_t = pts as f64 * ist_time_base.numerator() as f64 / ist_time_base.denominator() as f64;
            if frame_t >= t - 1e-3 || scanned >= MAX_FRAMES {
                let mut rgb = ff::frame::Video::empty();
                if let Err(e) = scaler.run(&frame, &mut rgb) {
                    warn!("thumbs: scale failed at t={t:.3}: {e}");
                    return None;
                }
                // scaler output is RGB24 packed (stride may still exceed
                // width*3 for alignment) — copy row-by-row like player.rs's
                // sample_to_frame does for the same reason.
                let (w, h) = (rgb.width() as usize, rgb.height() as usize);
                let stride = rgb.stride(0);
                let data = rgb.data(0);
                let mut out = Vec::with_capacity(w * h * 3);
                for row in 0..h {
                    let start = row * stride;
                    out.extend_from_slice(&data[start..start + w * 3]);
                }
                return Some(out);
            }
        }
        if scanned >= MAX_FRAMES { break; }
    }
    None
}
