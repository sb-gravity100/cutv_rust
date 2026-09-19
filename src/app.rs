use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::Local;
use egui::{
    Align, Align2, Color32, FontId, Frame, Layout, Margin, Pos2, Rect, RichText,
    Sense, Stroke, Vec2, pos2, vec2,
};
use log::{debug, trace};

use crate::crop::{self, CropRect, Handle as CropHandle};
use crate::gif::{self, GifOutcome};
use crate::player::Player;
use crate::probe::encode_args;
use crate::proxy::spawn_proxy;
use crate::thumbs::{ThumbData, spawn_thumbs};
use crate::util::fmt_tc;

// ── Palette ───────────────────────────────────────────────────────────────────

const BG:      Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
const BG_DRK:  Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
const BTN:     Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
const BTN_HV:  Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
const TXT:     Color32 = Color32::from_rgb(0xdd, 0xdd, 0xdd);
const TXT_DIM: Color32 = Color32::from_rgb(0x48, 0x48, 0x48);
const C_IN:    Color32 = Color32::from_rgb(0x4e, 0xff, 0x91);
const C_OUT:   Color32 = Color32::from_rgb(0xff, 0x50, 0x50);
const SEP_CLR: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);

const TL_H:    f32 = 58.0;
const THUMB_W: f32 = 72.0;
const THUMB_H: f32 = 44.0;

// Fixed vertical space taken by everything below the video (timeline,
// timecodes, transport, edit row, progress, status, separators/spacing).
// The video panel gets whatever height is left, so this must track any
// layout change to those rows.
const CHROME_H: f32 = TL_H            // timeline
    + 1.0 + 7.0 + 22.0 + 7.0          // hsep + timecodes row
    + 1.0 + 7.0 + 36.0 + 7.0          // hsep + transport row
    + 1.0 + 6.0 + 34.0                // hsep + edit row
    + 1.0 + 3.0 + 22.0;               // hsep + progress + status

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Drag { In, Out, Pos }

enum CutResult { Done(String), Err(String) }

// ── App ───────────────────────────────────────────────────────────────────────

pub struct CutvApp {
    ctx:      egui::Context,
    path:     String,       // original source — used for playback (Player) and do_cut()
    duration: f64,
    fps:      f64,
    src_w:    u32,       // original source dimensions — used for display aspect ratio
    src_h:    u32,
    transport_w: f32,    // remembered width of the transport button row, for centering

    player: Player,
    video_tex: Option<egui::TextureHandle>,

    // Scrub proxy (short-GOP transcode, see proxy.rs): `player` plays the
    // original file until this resolves, then gets swapped to point at the
    // proxy for fast accurate seeking. `proxy_active` guards against
    // re-swapping every frame once it has; the temp file gets deleted on
    // drop.
    pending_proxy: Arc<Mutex<Option<PathBuf>>>,
    proxy_active: Option<PathBuf>,

    // Measures decode throughput (frames actually delivered by Player::poll
    // per second), not the UI's own repaint rate — the useful number for
    // diagnosing "is the decoder keeping up" independent of egui's redraw
    // cadence. Recomputed once a second from a rolling count.
    frames_this_sec: u32,
    fps_window_start: f64, // egui input time the current 1s window started
    measured_fps: f32,

    cur_t:   f64,        // mirrors player.position each frame; authoritative between polls
    in_t:    f64,
    out_t:   f64,
    muted:   bool,

    thumb_texs:     Vec<(f64, egui::TextureHandle)>,
    pending_thumbs: Arc<Mutex<Vec<ThumbData>>>,

    tl_drag: Option<Drag>,

    crop_mode: bool,
    crop_rect: Option<CropRect>, // in source-pixel coords; applies to do_cut() whenever Some, independent of crop_mode
    crop_drag: Option<CropHandle>,

    frame_hold_dir: i32,   // active hold-to-repeat direction: -1, 0, or 1
    next_hold_tick: f64,   // egui input time the next repeat step fires at

    status:       String,
    cut_progress: f32,
    cut_progress_shared: Arc<Mutex<f32>>, // written by do_cut()/do_gif()'s worker thread as export progresses
    cut_result:   Arc<Mutex<Option<CutResult>>>,
    gif_result:   Arc<Mutex<Option<GifOutcome>>>,
}

impl CutvApp {
    pub fn new(
        path: String,
        duration: f64,
        fps: f64,
        src_w: u32,
        src_h: u32,
        ctx: egui::Context,
    ) -> Self {
        debug!("src {src_w}x{src_h}  fps={fps:.3}  dur={duration:.3}s");

        let pending_thumbs = spawn_thumbs(path.clone(), duration, ctx.clone());
        let player = Player::new(&path, duration, fps);
        let pending_proxy = spawn_proxy(path.clone(), ctx.clone());

        CutvApp {
            ctx,
            path,
            duration,
            fps,
            src_w,
            src_h,
            transport_w: 280.0,
            player,
            video_tex: None,
            pending_proxy,
            proxy_active: None,
            frames_this_sec: 0,
            fps_window_start: 0.0,
            measured_fps: 0.0,
            cur_t: 0.0,
            in_t: 0.0,
            out_t: duration,
            muted: false,
            thumb_texs: Vec::new(),
            pending_thumbs,
            tl_drag: None,
            crop_mode: false,
            crop_rect: None,
            crop_drag: None,
            frame_hold_dir: 0,
            next_hold_tick: 0.0,
            status: "Space play  ←→ ±5s  ,. ±1f  I/O in/out  M mute  Enter cut  G gif".into(),
            cut_progress: 0.0,
            cut_progress_shared: Arc::new(Mutex::new(0.0)),
            cut_result: Arc::new(Mutex::new(None)),
            gif_result: Arc::new(Mutex::new(None)),
        }
    }

    // ── Playback controls ─────────────────────────────────────────────────────

    fn toggle_play(&mut self) {
        if self.player.paused { self.player.play(); } else { self.player.pause(); }
    }

    fn seek(&mut self, t: f64) {
        let t = t.clamp(0.0, self.duration);
        self.player.seek(t);
        self.cur_t = t; // optimistic; corrected by the next poll() if the decoder lands elsewhere
    }

    fn step_frames(&mut self, n: i32) {
        self.player.step_frames(n);
    }

    /// Drives frame-by-frame hold-to-repeat for both the comma/period keys
    /// and the on-screen 1f buttons: steps once immediately on a fresh
    /// press, then repeats at a cadence scaled to the source frame rate
    /// while `dir` stays held.
    fn service_frame_hold(&mut self, dir: i32, now: f64, ctx: &egui::Context) {
        if dir == 0 {
            self.frame_hold_dir = 0;
            return;
        }
        // step_frames() is a frame-exact seek, which can take real time on a
        // long-GOP source (measured: 500ms+ for a ~250-frame GOP — it has to
        // decode forward from the keyframe to the exact target). This gate
        // applies to EVERY step, not just held repeats: a fresh press (the
        // `frame_hold_dir != dir` branch) used to bypass it entirely, so
        // pressing again before the previous step's seek landed thrashed
        // exactly the same way holding did — nothing ever completed.
        if self.player.is_seeking() {
            return;
        }
        if self.frame_hold_dir != dir {
            self.frame_hold_dir = dir;
            self.step_frames(dir);
            self.next_hold_tick = now + 0.35; // initial delay before repeat kicks in
        } else if now >= self.next_hold_tick {
            self.step_frames(dir);
            let interval = (2.5 / self.fps.max(1.0)).max(0.03);
            self.next_hold_tick = now + interval;
        }
        ctx.request_repaint();
    }

    fn set_in(&mut self) {
        self.in_t = self.cur_t.clamp(0.0, self.out_t);
        debug!("IN = {:.3}", self.in_t);
        self.status = format!("IN  =  {}", fmt_tc(self.in_t));
    }

    fn set_out(&mut self) {
        self.out_t = self.cur_t.clamp(self.in_t, self.duration);
        debug!("OUT = {:.3}", self.out_t);
        self.status = format!("OUT  =  {}", fmt_tc(self.out_t));
    }

    fn cut_from_start(&mut self) {
        self.in_t = 0.0;
        self.status = "IN set to start".into();
    }

    fn cut_to_end(&mut self) {
        self.out_t = self.duration;
        self.status = "OUT set to end".into();
    }

    fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.player.set_mute(self.muted);
    }

    fn toggle_crop_mode(&mut self) {
        self.crop_mode = !self.crop_mode;
        if self.crop_mode && self.crop_rect.is_none() {
            self.crop_rect = Some(CropRect::default_for(self.src_w, self.src_h));
        }
        self.status = if self.crop_mode {
            "Crop: drag handles to resize, drag inside to move".into()
        } else {
            "crop mode off — crop still applies to CUT until cleared".into()
        };
    }

    /// Clears any set crop entirely (not just the editing overlay).
    fn clear_crop(&mut self) {
        self.crop_rect = None;
        self.crop_mode = false;
        self.status = "crop cleared".into();
    }

    fn do_gif(&mut self) {
        if self.out_t - self.in_t < 0.1 {
            self.status = "IN/OUT too close (< 0.1 s)".into();
            return;
        }
        if !self.player.paused { self.player.pause(); }

        let p      = Path::new(&self.path);
        let stem   = p.file_stem().unwrap_or_default().to_string_lossy();
        let ts_str = Local::now().format("%Y%m%d-%H%M%S").to_string();
        let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let out_path = dir.join(format!("{stem}_gif_{ts_str}.gif"))
            .to_string_lossy().to_string();

        debug!("gif  [{:.3} → {:.3}]  -> {out_path}", self.in_t, self.out_t);
        self.status = format!("Exporting GIF → {}", Path::new(&out_path).file_name()
            .unwrap_or_default().to_string_lossy());
        *self.cut_progress_shared.lock().unwrap() = 0.0;

        gif::spawn_gif_export(
            self.path.clone(), self.in_t, self.out_t,
            self.src_w, self.src_h, self.crop_rect,
            out_path, self.cut_progress_shared.clone(),
            self.gif_result.clone(), self.ctx.clone(),
        );
    }

    fn do_cut(&mut self) {
        if self.out_t - self.in_t < 0.1 {
            self.status = "IN/OUT too close (< 0.1 s)".into();
            return;
        }
        if !self.player.paused { self.player.pause(); }

        let p      = Path::new(&self.path);
        let stem   = p.file_stem().unwrap_or_default().to_string_lossy();
        let ext    = p.extension().unwrap_or_default().to_string_lossy();
        let ts_str = Local::now().format("%Y%m%d-%H%M%S").to_string();
        // p.parent() is Some("") — not None — for a bare relative filename
        // like "sample.mp4" (no directory component), so a plain
        // `unwrap_or(".")` never kicks in and building the path by string
        // formatting produced a leading "/" (filesystem root) instead of
        // the source's actual directory. `.join()` on a real PathBuf
        // handles this correctly either way.
        let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let out_path = dir.join(format!("{stem}_cut_{ts_str}.{ext}"))
            .to_string_lossy().to_string();

        debug!("cut  [{:.3} → {:.3}]  -> {out_path}", self.in_t, self.out_t);
        self.status = format!("Cutting → {}", Path::new(&out_path).file_name()
            .unwrap_or_default().to_string_lossy());

        let path       = self.path.clone();
        let in_t       = self.in_t;
        let out_t      = self.out_t;
        let cut_dur    = (out_t - in_t).max(0.001);
        let crop_rect  = self.crop_rect;
        let cut_result = self.cut_result.clone();
        let cut_progress = self.cut_progress_shared.clone();
        let ctx        = self.ctx.clone();
        *cut_progress.lock().unwrap() = 0.0;

        thread::spawn(move || {
            let enc = encode_args(&path);
            let mut args = vec![
                "-y".to_string(),
                "-ss".to_string(), format!("{in_t:.3}"),
                "-i".to_string(), path,
                "-t".to_string(), format!("{:.3}", out_t - in_t),
            ];
            args.extend(enc);
            if let Some(r) = crop_rect {
                args.push("-vf".to_string());
                args.push(r.to_vf());
            }
            // Machine-readable progress on stdout, one `key=value` per
            // line, ending each stanza with `progress=continue`/`end` —
            // parsed below to drive the status-bar progress fill.
            args.push("-progress".to_string());
            args.push("pipe:1".to_string());
            args.push(out_path.clone());

            debug!("ffmpeg cut: {}", args.join(" "));
            let mut cmd = Command::new("ffmpeg");
            cmd.args(&args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            #[cfg(target_os = "windows")]
            { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); }

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("cut exec error: {e}");
                    *cut_result.lock().unwrap() = Some(CutResult::Err(e.to_string()));
                    ctx.request_repaint();
                    return;
                }
            };

            // stderr has to be drained concurrently with stdout, or ffmpeg
            // blocks writing to one full pipe while we're blocked reading
            // the other — a classic pipe deadlock. Collect it on its own
            // thread; only used for the error message if the cut fails.
            let stderr = child.stderr.take().expect("stderr was piped");
            let stderr_thread = thread::spawn(move || -> String {
                use std::io::Read;
                let mut buf = String::new();
                let _ = std::io::BufReader::new(stderr).read_to_string(&mut buf);
                buf
            });

            if let Some(stdout) = child.stdout.take() {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                    // out_time_us (or the older out_time_ms, also actually
                    // microseconds despite the name — a long-standing
                    // ffmpeg quirk kept for backward compatibility) is the
                    // encoder's current position into the cut.
                    if let Some(v) = line.strip_prefix("out_time_us=")
                        .or_else(|| line.strip_prefix("out_time_ms=")) {
                        if let Ok(us) = v.parse::<f64>() {
                            let frac = ((us / 1_000_000.0) / cut_dur).clamp(0.0, 1.0) as f32;
                            *cut_progress.lock().unwrap() = frac;
                            ctx.request_repaint();
                        }
                    }
                }
            }

            let status = child.wait();
            let stderr_text = stderr_thread.join().unwrap_or_default();

            let result = match status {
                Ok(s) if s.success() => {
                    let name = Path::new(&out_path)
                        .file_name().unwrap_or_default()
                        .to_string_lossy().to_string();
                    debug!("cut done: {name}");
                    *cut_progress.lock().unwrap() = 1.0;
                    CutResult::Done(name)
                }
                Ok(s) => {
                    log::warn!("cut failed (exit {s}): {stderr_text}");
                    CutResult::Err(stderr_text)
                }
                Err(e) => { log::warn!("cut wait error: {e}"); CutResult::Err(e.to_string()) }
            };
            *cut_result.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    // ── Timeline helpers ──────────────────────────────────────────────────────

    fn tl_hit(&self, x: f32, w: f32) -> Drag {
        let in_x  = (self.in_t  / self.duration) as f32 * w;
        let out_x = (self.out_t / self.duration) as f32 * w;
        if (x - in_x).abs()  < 10.0 { Drag::In  }
        else if (x - out_x).abs() < 10.0 { Drag::Out }
        else { Drag::Pos }
    }

    // ── Separator helper ──────────────────────────────────────────────────────

    fn hsep(ui: &mut egui::Ui) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(r, 0.0, SEP_CLR);
    }
}

// ── egui App trait ────────────────────────────────────────────────────────────

impl eframe::App for CutvApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t_update_start = std::time::Instant::now();

        // ── Swap to the scrub proxy once it's ready ───────────────────────────
        // do_cut() always uses self.path (the original) regardless — only
        // playback switches. Preserves position/play state across the swap.
        if self.proxy_active.is_none() {
            if let Some(proxy_path) = self.pending_proxy.lock().unwrap().take() {
                debug!("swapping playback to scrub proxy: {proxy_path:?}");
                let was_playing = !self.player.paused;
                let resume_at = self.player.position;
                let mut new_player = Player::new(&proxy_path.to_string_lossy(), self.duration, self.fps);
                new_player.seek(resume_at);
                if was_playing {
                    new_player.play();
                }
                new_player.set_mute(self.muted);
                self.player = new_player;
                self.video_tex = None; // old texture's size may not match; reload on next frame
                self.status = "Scrub proxy ready — seeking is now fast".into();
                self.proxy_active = Some(proxy_path);
            }
        }

        // ── Poll player state ─────────────────────────────────────────────────
        if let Some(f) = self.player.poll() {
            let t0 = std::time::Instant::now();
            let img = egui::ColorImage::from_rgb([f.w as usize, f.h as usize], &f.rgb);
            let t1 = std::time::Instant::now();
            match &mut self.video_tex {
                Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                None => {
                    self.video_tex =
                        Some(ctx.load_texture("video", img, egui::TextureOptions::LINEAR));
                }
            }
            trace!("frame upload: ColorImage {:?}  tex.set {:?}", t1 - t0, t0.elapsed() - (t1 - t0));
            self.frames_this_sec += 1;
        }
        let now = ctx.input(|i| i.time);
        if now - self.fps_window_start >= 1.0 {
            self.measured_fps = self.frames_this_sec as f32 / (now - self.fps_window_start).max(0.001) as f32;
            debug!("measured_fps={:.1}  paused={}", self.measured_fps, self.player.paused);
            self.frames_this_sec = 0;
            self.fps_window_start = now;
        }
        self.cur_t = self.player.position;
        if self.player.duration > 0.0 {
            self.duration = self.player.duration;
        }
        if !self.player.paused {
            ctx.request_repaint();
        }

        // ── Poll pending thumbnails ───────────────────────────────────────────
        {
            let mut pending = self.pending_thumbs.lock().unwrap();
            for td in pending.drain(..) {
                let img = egui::ColorImage::from_rgb([td.w as usize, td.h as usize], &td.rgb);
                let tex = ctx.load_texture(
                    format!("th{}", td.t as u32), img, egui::TextureOptions::LINEAR,
                );
                self.thumb_texs.push((td.t, tex));
            }
        }

        // ── Cut progress / result ─────────────────────────────────────────────
        self.cut_progress = *self.cut_progress_shared.lock().unwrap();
        if let Ok(mut g) = self.cut_result.lock() {
            if let Some(r) = g.take() {
                self.status = match r {
                    CutResult::Done(n)   => format!("Saved: {n}"),
                    CutResult::Err(msg)  => format!("Error: {}", &msg[..msg.len().min(80)]),
                };
                self.cut_progress = 0.0;
                *self.cut_progress_shared.lock().unwrap() = 0.0;
            }
        }
        if let Ok(mut g) = self.gif_result.lock() {
            if let Some(r) = g.take() {
                self.status = match r {
                    GifOutcome::Done(n)  => format!("Saved: {n}"),
                    GifOutcome::Err(msg) => format!("Error: {}", &msg[..msg.len().min(80)]),
                };
                self.cut_progress = 0.0;
                *self.cut_progress_shared.lock().unwrap() = 0.0;
            }
        }

        // ── Keyboard input ────────────────────────────────────────────────────
        // Comma/period use key_down (held, sampled every frame) instead of
        // key_pressed (fires once on the down-edge) so they can drive the
        // same hold-to-repeat cadence as the on-screen 1f buttons below.
        let (kspace, kleft, kright, kcomma_down, kperiod_down, ki, ko, km, kenter, kg) =
            ctx.input(|i| (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_down(egui::Key::Comma),
                i.key_down(egui::Key::Period),
                i.key_pressed(egui::Key::I),
                i.key_pressed(egui::Key::O),
                i.key_pressed(egui::Key::M),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::G),
            ));
        if kspace  { self.toggle_play(); }
        if kleft   { let t = self.cur_t - 5.0; self.seek(t); }
        if kright  { let t = self.cur_t + 5.0; self.seek(t); }
        if ki      { self.set_in(); }
        if ko      { self.set_out(); }
        if km      { self.toggle_mute(); }
        if kenter  { self.do_cut(); }
        if kg      { self.do_gif(); }

        let kb_hold_dir = match (kcomma_down, kperiod_down) {
            (true, false) => -1,
            (false, true) => 1,
            _ => 0,
        };

        // ── Draw UI ───────────────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(Frame::none()
                .fill(BG)
                .inner_margin(Margin::ZERO))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing   = Vec2::ZERO;
                ui.spacing_mut().button_padding = vec2(10.0, 7.0);

                self.ui_video(ui);
                self.ui_timeline(ui);
                CutvApp::hsep(ui);
                ui.add_space(7.0);
                self.ui_timecodes(ui);
                ui.add_space(7.0);
                CutvApp::hsep(ui);
                ui.add_space(7.0);
                self.ui_transport(ui, kb_hold_dir);
                ui.add_space(7.0);
                CutvApp::hsep(ui);
                ui.add_space(6.0);
                self.ui_edit_row(ui);
                CutvApp::hsep(ui);
                self.ui_progress(ui);
                self.ui_status(ui);
            });

        if !self.player.paused {
            trace!("update(): TOTAL {:?}", t_update_start.elapsed());
        }
    }
}

impl Drop for CutvApp {
    fn drop(&mut self) {
        if let Some(p) = &self.proxy_active {
            if let Err(e) = std::fs::remove_file(p) {
                debug!("failed to remove scrub proxy temp file {p:?}: {e}");
            }
        }
    }
}

// ── UI sections ───────────────────────────────────────────────────────────────

impl CutvApp {
    fn ui_video(&mut self, ui: &mut egui::Ui) {
        // Fill whatever space is left above the fixed-height chrome below,
        // letterboxing to preserve the source aspect ratio. Decoded frames
        // arrive as an egui texture (see update()'s poll()), drawn directly
        // here — no native child window, unlike the earlier mpv-based player.
        let avail_w = ui.available_width();
        let avail_h = (ui.available_height() - CHROME_H).max(50.0);
        let scale = (avail_w / self.src_w as f32).min(avail_h / self.src_h as f32);
        let disp_w = self.src_w as f32 * scale;
        let disp_h = self.src_h as f32 * scale;

        let (outer, resp) = ui.allocate_exact_size(vec2(avail_w, avail_h), Sense::click_and_drag());
        ui.painter().rect_filled(outer, 0.0, BG_DRK);
        let rect = Rect::from_center_size(outer.center(), vec2(disp_w, disp_h));

        if let Some(tex) = &self.video_tex {
            ui.painter().image(
                tex.id(), rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            let dims = tex.size();
            ui.painter().text(
                pos2(outer.left() + 8.0, outer.top() + 6.0), Align2::LEFT_TOP,
                format!("{:.0} fps  ·  decode {}x{}", self.measured_fps, dims[0], dims[1]),
                FontId::monospace(11.0), Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 160),
            );
        }

        if self.crop_mode {
            self.ui_crop_overlay(ui, rect, &resp);
        }
    }

    /// Draws the crop rectangle over `rect` (the video's on-screen display
    /// area) and handles dragging its handles/body — see crop.rs. `resp` is
    /// the same click-and-drag response `ui_video` allocated for the whole
    /// video area, reused here rather than allocating a second one.
    fn ui_crop_overlay(&mut self, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response) {
        const HANDLE_R: f32 = 10.0;
        let Some(crop_r) = self.crop_rect else { return };
        let canvas_crop = crop::to_canvas(rect, self.src_w, self.src_h, crop_r);

        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.crop_drag = crop::hit_test(canvas_crop, p, HANDLE_R);
            }
        }
        if resp.dragged() {
            if let Some(handle) = self.crop_drag {
                let delta = crop::canvas_to_source_delta(rect, self.src_w, self.src_h, resp.drag_delta());
                let mut r = crop_r;
                crop::apply_drag(&mut r, handle, delta, self.src_w as f32, self.src_h as f32);
                r.clamp_to(self.src_w as f32, self.src_h as f32);
                self.crop_rect = Some(r);
            }
        } else {
            self.crop_drag = None;
            // Hover feedback even before a drag starts.
            if let Some(p) = resp.hover_pos() {
                if let Some(h) = crop::hit_test(canvas_crop, p, HANDLE_R) {
                    ui.ctx().set_cursor_icon(crop::cursor_for(h));
                }
            }
        }
        if let Some(h) = self.crop_drag {
            ui.ctx().set_cursor_icon(crop::cursor_for(h));
        }

        let canvas_crop = crop::to_canvas(rect, self.src_w, self.src_h, self.crop_rect.unwrap_or(crop_r));
        crop::draw(ui.painter(), rect, canvas_crop, C_IN);
    }

    fn ui_timeline(&mut self, ui: &mut egui::Ui) {
        let avail_w = ui.available_width();
        let (tl_rect, resp) = ui.allocate_exact_size(
            vec2(avail_w, TL_H), Sense::click_and_drag(),
        );

        // Handle drag start: pick which handle to drag
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let x = p.x - tl_rect.left();
                self.tl_drag = Some(self.tl_hit(x, tl_rect.width()));
            }
        }

        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let x = (p.x - tl_rect.left()).clamp(0.0, tl_rect.width());
                let t = (x / tl_rect.width()) as f64 * self.duration;
                match self.tl_drag {
                    Some(Drag::In)  => { self.in_t  = t.clamp(0.0, self.out_t); }
                    Some(Drag::Out) => { self.out_t = t.clamp(self.in_t, self.duration); }
                    _ => {
                        // The playhead line follows the pointer immediately
                        // every dragged frame (cheap, always smooth), but
                        // the actual decoded frame only updates once the
                        // previous seek has landed — a fast mouse drag can
                        // generate far more pointer-move events per second
                        // than even a fast (short-GOP scrub proxy) exact
                        // seek can complete, and issuing one on every
                        // single frame just interrupts the previous one
                        // before it lands (measured: 11+ seeks/sec during a
                        // drag, only 1 ever actually completed). This
                        // naturally chases the latest position once the
                        // decoder is free, same idea as
                        // service_frame_hold's gate. Always exact — the
                        // scrub proxy's short GOP (proxy.rs) makes an exact
                        // seek about as cheap as a keyframe-snap one would
                        // have been, so there's no accuracy/speed tradeoff
                        // left to make here.
                        let t = t.clamp(0.0, self.duration);
                        self.cur_t = t;
                        if !self.player.is_seeking() {
                            self.player.seek(t);
                        }
                    }
                }
            }
        } else if resp.drag_stopped() && matches!(self.tl_drag, None | Some(Drag::Pos)) {
            // Land exactly on the intended frame once the drag ends (an
            // In/Out drag already carries its exact scrubbed timestamp —
            // nothing further to do there).
            self.seek(self.cur_t);
        }

        if !resp.dragged() { self.tl_drag = None; }

        // A plain click (press+release with no movement) never fires
        // drag_started()/dragged(), so without this the playhead only
        // ever "followed" once you started actually moving the mouse.
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let x = (p.x - tl_rect.left()).clamp(0.0, tl_rect.width());
                let t = (x / tl_rect.width()) as f64 * self.duration;
                match self.tl_hit(x, tl_rect.width()) {
                    Drag::In  => { self.in_t  = t.clamp(0.0, self.out_t); }
                    Drag::Out => { self.out_t = t.clamp(self.in_t, self.duration); }
                    Drag::Pos => { self.seek(t.clamp(0.0, self.duration)); }
                }
            }
        }

        draw_timeline(
            ui.painter(), tl_rect,
            self.duration, self.in_t, self.out_t, self.cur_t,
            &self.thumb_texs,
        );
    }

    fn ui_timecodes(&self, ui: &mut egui::Ui) {
        let avail = ui.available_width();
        let (row, _) = ui.allocate_exact_size(vec2(avail, 22.0), Sense::hover());
        let p = ui.painter();
        let mid = row.center().y;

        // IN (left)
        p.text(
            pos2(row.left() + 14.0, mid), Align2::LEFT_CENTER,
            format!("IN  {}", fmt_tc(self.in_t)),
            FontId::monospace(11.0), C_IN,
        );
        // Current position (center)
        p.text(
            row.center(), Align2::CENTER_CENTER,
            fmt_tc(self.cur_t),
            FontId::monospace(16.0), TXT,
        );
        // Duration (far right, dim)
        p.text(
            pos2(row.right() - 12.0, mid), Align2::RIGHT_CENTER,
            format!("/ {}", fmt_tc(self.duration)),
            FontId::monospace(11.0), TXT_DIM,
        );
        // OUT (just left of duration)
        p.text(
            pos2(row.right() - 106.0, mid), Align2::RIGHT_CENTER,
            format!("OUT  {}", fmt_tc(self.out_t)),
            FontId::monospace(11.0), C_OUT,
        );
    }

    fn ui_transport(&mut self, ui: &mut egui::Ui, kb_hold_dir: i32) {
        // Layout::top_down(Align::Center) doesn't center a nested
        // ui.horizontal() block reliably, so center it manually: pad by
        // half the (previous frame's) measured row width, then remeasure.
        // One-frame lag on resize, not visible in practice.
        let avail = ui.available_width();
        let pad   = ((avail - self.transport_w) * 0.5).max(0.0);

        let mut btn_dir = 0;
        let playing = !self.player.paused;
        let resp = ui.horizontal(|ui| {
            ui.add_space(pad);

            let cur = self.cur_t;
            let dur = self.duration;

            if tbtn(ui, "|◀") .clicked() { self.seek(0.0); }
            if tbtn(ui, "◀◀") .clicked() { self.seek(cur - 5.0); }
            if tbtn(ui, "◀ 1f").is_pointer_button_down_on() { btn_dir = -1; }

            let play_lbl = if playing { "⏸  Pause" } else { "▶  Play" };
            if ui.add(egui::Button::new(
                    RichText::new(play_lbl).color(TXT).size(13.0).strong())
                .fill(BTN)
                .stroke(Stroke::NONE)
                .min_size(vec2(88.0, 36.0)))
                .clicked()
            { self.toggle_play(); }

            if tbtn(ui, "1f ▶").is_pointer_button_down_on() { btn_dir = 1; }
            if tbtn(ui, "▶▶") .clicked() { self.seek(cur + 5.0); }
            if tbtn(ui, "▶|") .clicked() { self.seek(dur); }
        });
        self.transport_w = (resp.response.rect.width() - pad).max(0.0);

        let dir = if kb_hold_dir != 0 { kb_hold_dir } else { btn_dir };
        let now = ui.input(|i| i.time);
        self.service_frame_hold(dir, now, ui.ctx());
    }

    fn ui_edit_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            if cbtn(ui, "[ Set IN",       C_IN)     .clicked() { self.set_in(); }
            ui.add_space(2.0);
            if cbtn(ui, "Set OUT ]",      C_OUT)    .clicked() { self.set_out(); }
            ui.add_space(2.0);
            if cbtn(ui, "◀ From Start",   TXT_DIM)  .clicked() { self.cut_from_start(); }
            ui.add_space(2.0);
            if cbtn(ui, "To End ▶",       TXT_DIM)  .clicked() { self.cut_to_end(); }

            // Push mute + CUT to the right
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(10.0);
                // CUT button (light bg)
                if ui.add(egui::Button::new(
                        RichText::new("✂  CUT")
                            .color(Color32::from_rgb(0x11, 0x11, 0x11))
                            .size(13.0).strong())
                    .fill(Color32::from_rgb(0xee, 0xee, 0xee))
                    .stroke(Stroke::NONE)
                    .min_size(vec2(74.0, 34.0)))
                    .clicked()
                { self.do_cut(); }

                ui.add_space(4.0);
                let mute_lbl = if self.muted { "Muted" } else { "Sound" };
                let mute_fg  = if self.muted { C_OUT } else { TXT_DIM };
                if cbtn(ui, mute_lbl, mute_fg).clicked() { self.toggle_mute(); }

                ui.add_space(4.0);
                if cbtn(ui, "GIF", TXT_DIM).clicked() { self.do_gif(); }

                ui.add_space(4.0);
                let crop_fg = if self.crop_mode {
                    TXT
                } else if self.crop_rect.is_some() {
                    C_IN // crop is set and will apply to CUT, just not being edited right now
                } else {
                    TXT_DIM
                };
                let crop_resp = cbtn(ui, "⬚ CROP", crop_fg)
                    .on_hover_text("Click: toggle crop editing · Right-click: clear crop");
                if crop_resp.clicked() { self.toggle_crop_mode(); }
                if crop_resp.secondary_clicked() { self.clear_crop(); }
            });
        });
    }

    fn ui_progress(&self, ui: &mut egui::Ui) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
        ui.painter().rect_filled(r, 0.0, BG_DRK);
        if self.cut_progress > 0.0 {
            let w = r.width() * self.cut_progress.clamp(0.0, 1.0);
            let bar = Rect::from_min_size(r.min, vec2(w, r.height()));
            ui.painter().rect_filled(bar, 0.0, C_IN);
        }
    }

    fn ui_status(&self, ui: &mut egui::Ui) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        ui.painter().rect_filled(r, 0.0, BG_DRK);
        ui.painter().text(
            pos2(r.left() + 14.0, r.center().y),
            Align2::LEFT_CENTER,
            &self.status,
            FontId::proportional(11.0),
            Color32::from_rgb(0xaa, 0xaa, 0xaa),
        );
    }
}

// ── Widget helpers ────────────────────────────────────────────────────────────

fn tbtn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).color(TXT).size(11.0))
        .fill(BTN)
        .stroke(Stroke::NONE)
        .min_size(vec2(0.0, 36.0)))
}

fn cbtn(ui: &mut egui::Ui, label: &str, fg: Color32) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).color(fg).size(11.0))
        .fill(BTN)
        .stroke(Stroke::NONE)
        .min_size(vec2(0.0, 34.0)))
}

// ── Timeline drawing ──────────────────────────────────────────────────────────

fn draw_timeline(
    painter: &egui::Painter,
    rect:     Rect,
    duration: f64,
    in_t:     f64,
    out_t:    f64,
    pos:      f64,
    thumbs:   &[(f64, egui::TextureHandle)],
) {
    let w = rect.width();
    let tx = |t: f64| -> f32 { rect.left() + (t / duration) as f32 * w };

    // Background
    painter.rect_filled(rect, 0.0, BG_DRK);

    // Thumbnails
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    for (t, tex) in thumbs {
        let cx = tx(*t);
        let x0 = (cx - THUMB_W / 2.0).clamp(rect.left(), rect.right() - THUMB_W);
        let tr = Rect::from_min_size(pos2(x0, rect.top()), vec2(THUMB_W, THUMB_H));
        painter.image(tex.id(), tr, uv, Color32::WHITE);
    }

    // Dim regions outside IN / OUT
    let in_x  = tx(in_t);
    let out_x = tx(out_t);
    let dim   = Color32::from_black_alpha(120);
    painter.rect_filled(
        Rect::from_min_max(rect.min, pos2(in_x,  rect.max.y)), 0.0, dim,
    );
    painter.rect_filled(
        Rect::from_min_max(pos2(out_x, rect.min.y), rect.max), 0.0, dim,
    );

    // IN marker
    marker(painter, rect, in_x,  C_IN,  true,  "IN");
    // OUT marker
    marker(painter, rect, out_x, C_OUT, false, "OUT");

    // Playhead
    let px = tx(pos);
    painter.line_segment(
        [pos2(px, rect.top()), pos2(px, rect.bottom())],
        Stroke::new(1.0, Color32::WHITE),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(px - 5.0, rect.top()),
            pos2(px + 5.0, rect.top()),
            pos2(px,       rect.top() + 9.0),
        ],
        Color32::WHITE, Stroke::NONE,
    ));
}

fn marker(
    painter: &egui::Painter,
    rect:    Rect,
    x:       f32,
    color:   Color32,
    left:    bool,  // triangle + label face right (IN) or left (OUT)
    label:   &str,
) {
    painter.line_segment(
        [pos2(x, rect.top()), pos2(x, rect.bottom())],
        Stroke::new(2.0, color),
    );
    let (tri, anchor): (Vec<Pos2>, Align2) = if left {
        (
            vec![pos2(x, rect.top()), pos2(x + 8.0, rect.top()), pos2(x, rect.top() + 11.0)],
            Align2::LEFT_TOP,
        )
    } else {
        (
            vec![pos2(x, rect.top()), pos2(x - 8.0, rect.top()), pos2(x, rect.top() + 11.0)],
            Align2::RIGHT_TOP,
        )
    };
    painter.add(egui::Shape::convex_polygon(tri, color, Stroke::NONE));
    let lx = if left { x + 4.0 } else { x - 4.0 };
    painter.text(
        pos2(lx, rect.top() + 13.0), anchor, label,
        FontId::proportional(9.0), color,
    );
}

// ── Dark visuals ──────────────────────────────────────────────────────────────

pub fn set_dark_visuals(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill          = BG;
    v.window_fill         = BG;
    v.override_text_color = Some(TXT);

    v.widgets.noninteractive.weak_bg_fill = BTN;
    v.widgets.noninteractive.bg_fill      = BTN;
    v.widgets.inactive.weak_bg_fill       = BTN;
    v.widgets.inactive.bg_fill            = BTN;
    v.widgets.hovered.weak_bg_fill        = BTN_HV;
    v.widgets.hovered.bg_fill             = BTN_HV;
    v.widgets.active.weak_bg_fill         = BTN_HV;
    v.widgets.active.bg_fill              = BTN_HV;

    v.widgets.inactive.bg_stroke      = Stroke::NONE;
    v.widgets.hovered.bg_stroke       = Stroke::NONE;
    v.widgets.active.bg_stroke        = Stroke::NONE;
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;

    ctx.set_visuals(v);
}
