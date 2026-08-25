use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::Local;
use egui::{
    Align, Align2, Color32, FontId, Frame, Layout, Margin, Pos2, Rect, RichText,
    Sense, Stroke, Vec2, pos2, vec2,
};
use log::debug;

use crate::audio::AudioPlayer;
use crate::probe::encode_args;
use crate::util::fmt_tc;
use crate::video::{FrameData, SeekWorker, ThumbData, VideoDecoder, spawn_proxy, spawn_thumbs};

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
    path:     String,       // original source — always used for do_cut(); used for playback too when proxy is off
    proxy_path:      String,
    proxy_ready:     std::sync::Arc<std::sync::atomic::AtomicBool>,
    proxy_building:  bool,   // whether the background proxy transcode has been kicked off
    use_proxy:       bool,   // off by default — play/seek/thumbnail straight from source
    ready:           bool,   // can playback start right now (source: instant; proxy: waits for proxy_ready)
    duration: f64,
    fps:      f64,
    dw:       u32,       // decode/proxy resolution — fixed, unrelated to on-screen size
    dh:       u32,
    src_w:    u32,       // original source dimensions — used for display aspect ratio
    src_h:    u32,
    transport_w: f32,    // remembered width of the transport button row, for centering

    playing: bool,
    cur_t:   f64,
    in_t:    f64,
    out_t:   f64,
    muted:   bool,

    decoder:        Option<VideoDecoder>,
    seeker:         SeekWorker,
    frame_tex:      Option<egui::TextureHandle>,
    thumb_texs:     Vec<(f64, egui::TextureHandle)>,
    pending_thumbs: Arc<Mutex<Vec<ThumbData>>>,

    audio:  Option<AudioPlayer>,

    tl_drag: Option<Drag>,

    crop_mode: bool,

    frame_hold_dir: i32,   // active hold-to-repeat direction: -1, 0, or 1
    next_hold_tick: f64,   // egui input time the next repeat step fires at

    status:       String,
    cut_progress: f32,
    cut_result:   Arc<Mutex<Option<CutResult>>>,
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
        const MAX_W: u32 = 640;
        const MAX_H: u32 = 360;
        let scale = (MAX_W as f64 / src_w as f64)
            .min(MAX_H as f64 / src_h as f64)
            .min(1.0);
        let dw = (src_w as f64 * scale) as u32;
        let dh = (src_h as f64 * scale) as u32;
        debug!("display {dw}x{dh}  fps={fps:.3}  dur={duration:.3}s");

        let proxy_path = std::env::temp_dir()
            .join(format!("cutv_{}_proxy.mp4", std::process::id()))
            .to_string_lossy()
            .into_owned();

        // Proxy is off by default: play straight from source, no transcode
        // wait. Thumbs and initial seek fire on the first update() tick.
        let pending_thumbs = Arc::new(Mutex::new(Vec::new()));
        let seeker = SeekWorker::spawn(ctx.clone());

        let audio = AudioPlayer::new(&path);

        CutvApp {
            ctx,
            path,
            proxy_path,
            proxy_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            proxy_building: false,
            use_proxy: false,
            ready: false,
            duration,
            fps,
            dw,
            dh,
            src_w,
            src_h,
            transport_w: 280.0,
            playing: false,
            cur_t: 0.0,
            in_t: 0.0,
            out_t: duration,
            muted: false,
            decoder: None,
            seeker,
            frame_tex: None,
            thumb_texs: Vec::new(),
            pending_thumbs,
            audio,
            tl_drag: None,
            crop_mode: false,
            frame_hold_dir: 0,
            next_hold_tick: 0.0,
            status: "Loading...".into(),
            cut_progress: 0.0,
            cut_result: Arc::new(Mutex::new(None)),
        }
    }

    // ── Playback source ───────────────────────────────────────────────────────

    fn active_path(&self) -> String {
        if self.use_proxy { self.proxy_path.clone() } else { self.path.clone() }
    }

    /// Toggle proxy playback on/off. Off (default) plays/seeks straight from
    /// the source. On kicks off (or reuses) the background proxy transcode
    /// and switches to it once ready — useful for heavy/high-bitrate sources
    /// where decoding the original on every seek is too slow.
    fn toggle_use_proxy(&mut self) {
        self.use_proxy = !self.use_proxy;
        debug!("use_proxy={}", self.use_proxy);

        if self.use_proxy && !self.proxy_building {
            self.proxy_building = true;
            self.proxy_ready = spawn_proxy(self.path.clone(), self.proxy_path.clone(), self.dw, self.dh).ready;
        }

        let now_ready = !self.use_proxy || self.proxy_ready.load(Ordering::Relaxed);
        self.ready = now_ready;
        self.status = match (self.use_proxy, now_ready) {
            (true, true)   => "proxy on".into(),
            (true, false)  => "building proxy...".into(),
            (false, _)     => "proxy off — using source".into(),
        };

        if self.playing {
            self.playing = false;
            if let Some(d) = &mut self.decoder { d.stop(); }
            self.decoder = None;
            if let Some(a) = &self.audio { a.pause(); }
        }

        if now_ready {
            let path = self.active_path();
            self.thumb_texs.clear();
            self.pending_thumbs = spawn_thumbs(path.clone(), self.duration, self.ctx.clone());
            self.seeker.request(&path, self.cur_t, self.dw, self.dh);
        }
    }

    // ── Playback controls ─────────────────────────────────────────────────────

    fn toggle_play(&mut self) {
        if !self.ready { return; }
        self.playing = !self.playing;
        if self.playing {
            debug!("play from {:.3}s", self.cur_t);
            let path = self.active_path();
            self.decoder = Some(VideoDecoder::spawn(
                &path, self.cur_t, self.dw, self.dh, self.fps,
                self.ctx.clone(),
            ));
            if let Some(a) = &mut self.audio { a.play(self.cur_t); }
        } else {
            debug!("pause at {:.3}s", self.cur_t);
            if let Some(d) = &mut self.decoder { d.stop(); }
            self.decoder = None;
            if let Some(a) = &self.audio { a.pause(); }
        }
    }

    fn seek(&mut self, t: f64) {
        if !self.ready { return; }
        let t = t.clamp(0.0, self.duration);
        let was_playing = self.playing;
        if was_playing {
            self.playing = false;
            if let Some(d) = &mut self.decoder { d.stop(); }
            self.decoder = None;
            if let Some(a) = &self.audio { a.pause(); }
        }
        self.cur_t = t;
        let path = self.active_path();
        self.seeker.request(&path, t, self.dw, self.dh);
        if was_playing {
            self.playing = true;
            self.decoder = Some(VideoDecoder::spawn(
                &path, t, self.dw, self.dh, self.fps, self.ctx.clone(),
            ));
            if let Some(a) = &mut self.audio { a.play(t); }
        }
    }

    fn step_frames(&mut self, n: i32) {
        self.seek(self.cur_t + n as f64 / self.fps);
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
        if let Some(a) = &mut self.audio { a.set_mute(self.muted); }
    }

    // TODO(Phase 3): wire to a real crop overlay + rect state.
    fn toggle_crop_mode(&mut self) {
        self.crop_mode = !self.crop_mode;
        self.status = if self.crop_mode {
            "crop mode on — overlay not wired yet".into()
        } else {
            "crop mode off".into()
        };
    }

    // TODO(Phase 4): real two-pass palette GIF export.
    fn do_gif(&mut self) {
        self.status = "GIF export not implemented yet".into();
    }

    fn do_cut(&mut self) {
        if self.out_t - self.in_t < 0.1 {
            self.status = "IN/OUT too close (< 0.1 s)".into();
            return;
        }
        if self.playing { self.toggle_play(); }

        let p      = Path::new(&self.path);
        let stem   = p.file_stem().unwrap_or_default().to_string_lossy();
        let ext    = p.extension().unwrap_or_default().to_string_lossy();
        let dir    = p.parent().unwrap_or(Path::new(".")).to_string_lossy();
        let ts_str = Local::now().format("%Y%m%d-%H%M%S").to_string();
        let out_path = format!("{dir}/{stem}_cut_{ts_str}.{ext}");

        debug!("cut  [{:.3} → {:.3}]  -> {out_path}", self.in_t, self.out_t);
        self.status = format!("Cutting → {}", Path::new(&out_path).file_name()
            .unwrap_or_default().to_string_lossy());

        let path       = self.path.clone();
        let in_t       = self.in_t;
        let out_t      = self.out_t;
        let cut_result = self.cut_result.clone();
        let ctx        = self.ctx.clone();

        thread::spawn(move || {
            let enc = encode_args(&path);
            let mut args = vec![
                "-y".to_string(),
                "-ss".to_string(), format!("{in_t:.3}"),
                "-i".to_string(), path,
                "-t".to_string(), format!("{:.3}", out_t - in_t),
            ];
            args.extend(enc);
            args.push(out_path.clone());

            debug!("ffmpeg cut: {}", args.join(" "));
            let mut cmd = Command::new("ffmpeg");
            cmd.args(&args).stderr(std::process::Stdio::piped());
            #[cfg(target_os = "windows")]
            { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); }

            let result = match cmd.output() {
                Ok(o) if o.status.success() => {
                    let name = Path::new(&out_path)
                        .file_name().unwrap_or_default()
                        .to_string_lossy().to_string();
                    debug!("cut done: {name}");
                    CutResult::Done(name)
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr).to_string();
                    log::warn!("cut failed: {err}");
                    CutResult::Err(err)
                }
                Err(e) => { log::warn!("cut exec error: {e}"); CutResult::Err(e.to_string()) }
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

    fn update_frame_tex(&mut self, ctx: &egui::Context, f: &FrameData) {
        let img = egui::ColorImage::from_rgb([f.w as usize, f.h as usize], &f.rgb);
        match &mut self.frame_tex {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None    => self.frame_tex = Some(ctx.load_texture("frame", img, egui::TextureOptions::LINEAR)),
        }
    }

    // ── Separator helper ──────────────────────────────────────────────────────

    fn hsep(ui: &mut egui::Ui) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(r, 0.0, SEP_CLR);
    }
}

impl Drop for CutvApp {
    fn drop(&mut self) {
        let p = std::path::Path::new(&self.proxy_path);
        if p.exists() {
            let _ = std::fs::remove_file(p);
            debug!("proxy removed: {}", self.proxy_path);
        }
    }
}

// ── egui App trait ────────────────────────────────────────────────────────────

impl eframe::App for CutvApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {

        // ── Become ready once the active source can be played ──────────────────
        // Proxy off (default): ready on the very first tick. Proxy on: wait
        // for the background transcode.
        if !self.ready && (!self.use_proxy || self.proxy_ready.load(Ordering::Relaxed)) {
            self.ready = true;
            debug!("ready, starting thumbs + initial seek");
            let path = self.active_path();
            self.pending_thumbs = spawn_thumbs(path.clone(), self.duration, ctx.clone());
            self.seeker.request(&path, 0.0, self.dw, self.dh);
            let audio_ready = self.audio.as_ref()
                .map_or(false, |a| a.ready.load(Ordering::Relaxed));
            self.status = if audio_ready {
                "Space play  ←→ ±5s  ,. ±1f  I/O in/out  M mute  Enter cut".into()
            } else {
                "Loading audio...".into()
            };
        }

        // ── Poll seek preview ─────────────────────────────────────────────────
        // Only update the displayed texture here, not cur_t: seek() already
        // sets cur_t synchronously the moment the user clicks/drags, and
        // this result can arrive well after later requests superseded it
        // (rapid dragging queues many requests; only the latest survives).
        // Overwriting cur_t with this frame's now-stale timestamp is what
        // made the timecode/playhead jerk backward while scrubbing.
        if let Some(f) = self.seeker.poll() {
            self.update_frame_tex(ctx, &f);
        }

        // ── Poll decoder frames ───────────────────────────────────────────────
        if self.playing {
            let mut eos = false;
            if let Some(decoder) = &self.decoder {
                match decoder.rx.try_recv() {
                    Ok(Some(f)) => { self.cur_t = f.ts; self.update_frame_tex(ctx, &f); }
                    Ok(None)    => { eos = true; }
                    Err(_)      => {}
                }
            }
            if eos {
                debug!("decoder EOF — stopping playback");
                self.playing = false;
                self.decoder = None;
                if let Some(a) = &self.audio { a.pause(); }
            }
            if self.playing { ctx.request_repaint(); }
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

        // ── Check audio ready ─────────────────────────────────────────────────
        if self.status == "Loading audio..." {
            if self.audio.as_ref().map_or(false, |a| a.ready.load(Ordering::Relaxed)) {
                self.status = "Space play  ←→ ±5s  ,. ±1f  I/O in/out  M mute  Enter cut".into();
            }
        }

        // ── Check cut result ──────────────────────────────────────────────────
        if let Ok(mut g) = self.cut_result.lock() {
            if let Some(r) = g.take() {
                self.status = match r {
                    CutResult::Done(n)   => format!("Saved: {n}"),
                    CutResult::Err(msg)  => format!("Error: {}", &msg[..msg.len().min(80)]),
                };
            }
        }

        // ── Keyboard input ────────────────────────────────────────────────────
        // Comma/period use key_down (held, sampled every frame) instead of
        // key_pressed (fires once on the down-edge) so they can drive the
        // same hold-to-repeat cadence as the on-screen 1f buttons below.
        let (kspace, kleft, kright, kcomma_down, kperiod_down, ki, ko, km, kenter) =
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
            ));
        if kspace  { self.toggle_play(); }
        if kleft   { let t = self.cur_t - 5.0; self.seek(t); }
        if kright  { let t = self.cur_t + 5.0; self.seek(t); }
        if ki      { self.set_in(); }
        if ko      { self.set_out(); }
        if km      { self.toggle_mute(); }
        if kenter  { self.do_cut(); }

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
    }
}

// ── UI sections ───────────────────────────────────────────────────────────────

impl CutvApp {
    fn ui_video(&mut self, ui: &mut egui::Ui) {
        // Fill whatever space is left above the fixed-height chrome below,
        // letterboxing to preserve the source aspect ratio. The texture
        // itself stays at the fixed decode resolution (self.dw/self.dh) —
        // only its on-screen rect grows/shrinks with the window.
        let avail_w = ui.available_width();
        let avail_h = (ui.available_height() - CHROME_H).max(50.0);
        let scale = (avail_w / self.src_w as f32).min(avail_h / self.src_h as f32);
        let disp_w = self.src_w as f32 * scale;
        let disp_h = self.src_h as f32 * scale;

        let (outer, _) = ui.allocate_exact_size(vec2(avail_w, avail_h), Sense::hover());
        ui.painter().rect_filled(outer, 0.0, BG_DRK);
        let rect = Rect::from_center_size(outer.center(), vec2(disp_w, disp_h));
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        if let Some(tex) = &self.frame_tex {
            ui.painter().image(
                tex.id(), rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
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
        if !resp.dragged() { self.tl_drag = None; }

        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let x = (p.x - tl_rect.left()).clamp(0.0, tl_rect.width());
                let t = (x / tl_rect.width()) as f64 * self.duration;
                match self.tl_drag {
                    Some(Drag::In)  => { self.in_t  = t.clamp(0.0, self.out_t); }
                    Some(Drag::Out) => { self.out_t = t.clamp(self.in_t, self.duration); }
                    _               => { self.seek(t.clamp(0.0, self.duration)); }
                }
            }
        }

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
        let resp = ui.horizontal(|ui| {
            ui.add_space(pad);

            let cur = self.cur_t;
            let dur = self.duration;
            let playing = self.playing;

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
                let proxy_fg = if self.use_proxy { TXT } else { TXT_DIM };
                if cbtn(ui, "PROXY", proxy_fg).clicked() { self.toggle_use_proxy(); }

                ui.add_space(4.0);
                if cbtn(ui, "GIF", TXT_DIM).clicked() { self.do_gif(); }

                ui.add_space(4.0);
                let crop_fg = if self.crop_mode { TXT } else { TXT_DIM };
                if cbtn(ui, "⬚ CROP", crop_fg).clicked() { self.toggle_crop_mode(); }
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
