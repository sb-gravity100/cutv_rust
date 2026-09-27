// Playback via GStreamer's `playbin` element, replacing both the earlier
// libmpv child-window approach and the hand-rolled ffmpeg-next decode
// thread + rodio audio player — see PLAN.md's "Architecture decision".
//
// playbin is a mature, battle-tested pipeline that owns demuxing, decoding,
// audio/video clock sync, buffering, and seeking as one unit (the same
// category of thing mpv gave us), but with a properly safe, idiomatic Rust
// API via gstreamer-rs instead of mpv's async fire-and-forget command
// string interface (the root cause of every mpv-era bug) or reimplementing
// a video player's decode/pacing/sync logic by hand (the root cause of the
// custom decoder's performance problems). Audio is playbin's own default
// sink (autoaudiosink) — no separate AudioPlayer needed at all.
//
// Video frames are pulled from an appsink (via a small
// `videoconvert ! appsink` bin set as playbin's video-sink) as raw RGB
// buffers and handed to the caller to upload as an egui texture, same shape
// as the previous decoders so app.rs's rendering code didn't need to change.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use log::{debug, trace, warn};

pub struct DecodedFrame {
    pub rgb: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

pub struct Player {
    pipeline: gst::Element, // playbin
    appsink: gst_app::AppSink,

    pub duration: f64,
    pub position: f64,
    pub paused: bool,
    fps: f64,

    // True from the moment seek() is called until the next frame actually
    // arrives via poll(). seek() (SeekFlags::ACCURATE) always decodes
    // forward from the keyframe before the target to land exactly on it —
    // cheap on the short-GOP scrub proxy (proxy.rs) this app plays back
    // through, but never instantaneous. Without this gate, a rapid burst
    // of seeks (held frame-stepping, or a fast timeline drag) each
    // interrupt the previous one before it can land, so nothing ever
    // completes. See app.rs's service_frame_hold and ui_timeline, both of
    // which check this before issuing another seek.
    seeking: bool,
    seek_issued_at: Option<std::time::Instant>, // for logging real seek->frame latency
}

impl Player {
    pub fn new(path: &str, duration: f64, fps: f64) -> Self {
        if let Err(e) = gst::init() {
            warn!("gstreamer init failed: {e}");
        }

        // filename_to_uri requires an absolute path (fails outright on a
        // relative one like "sample.mp4", which main.rs's cwd auto-detect
        // hands us) — canonicalize first.
        let abs_path = std::fs::canonicalize(path)
            .map(|p| p.to_string_lossy().trim_start_matches(r"\\?\").to_string())
            .unwrap_or_else(|e| {
                warn!("canonicalize({path}) failed: {e}, using the path as-is");
                path.to_string()
            });
        let uri = match gst::glib::filename_to_uri(&abs_path, None) {
            Ok(u) => u.to_string(),
            Err(e) => {
                warn!("filename_to_uri({abs_path}) failed: {e}, falling back to a manual file:// URI");
                format!("file:///{}", abs_path.replace('\\', "/"))
            }
        };

        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", &uri)
            .build()
            .expect("no 'playbin' element — is GStreamer's plugins-base installed?");

        // Colorspace conversion (planar YUV from the decoder -> interleaved
        // RGB for egui) prefers the GPU (D3D11) path: it's faster still
        // (~640fps vs ~175fps measured for a 2340x1080 source, now that
        // vcpkg-overlay/gstreamer's ORC fix makes CPU videoconvert fast
        // too — see PLAN.md's "low-FPS investigation") and offloads work
        // from the CPU entirely. d3d11upload is a no-op if the decoder
        // already produced a D3D11 surface (it does, when d3d11h264dec
        // auto-wins decodebin's element-ranking). Falls back to plain
        // `videoconvert` on a machine with no D3D11 device — a solid
        // fallback now, not just "won't hang", now that ORC is enabled.
        //
        // max-buffers=1 drop=true caps the appsink's internal queue at one
        // frame and discards overflow instead of queuing it (default is
        // unlimited) — poll()'s doc comment already assumed this ("appsink
        // drops old ones for us"), but nothing actually enforced it. At 1x
        // playback, push and poll() cadence stayed close enough that the
        // gap never showed; Player::set_speed() (rate > 1) pushes frames
        // faster than poll() (once per UI frame) can necessarily drain, and
        // on a large-resolution source each queued raw RGB frame is tens of
        // MB — without this cap that backlog grows unbounded, which is
        // what a high playback rate on a big file was observed to do.
        let video_sink_bin = gst::parse::bin_from_description(
            "d3d11upload ! d3d11convert ! video/x-raw(memory:D3D11Memory),format=RGB ! \
             d3d11download ! appsink name=cutv_sink caps=video/x-raw,format=RGB sync=true \
             max-buffers=1 drop=true",
            true,
        )
        .or_else(|e| {
            warn!("D3D11 video sink unavailable ({e}), falling back to CPU videoconvert");
            gst::parse::bin_from_description(
                "videoconvert ! appsink name=cutv_sink caps=video/x-raw,format=RGB sync=true \
                 max-buffers=1 drop=true",
                true,
            )
        })
        .expect("failed to build a video-sink bin (neither D3D11 nor videoconvert worked)");
        let appsink = video_sink_bin
            .by_name("cutv_sink")
            .expect("appsink not found in bin")
            .downcast::<gst_app::AppSink>()
            .expect("cutv_sink is not an AppSink");
        pipeline.set_property("video-sink", &video_sink_bin);

        if let Err(e) = pipeline.set_state(gst::State::Paused) {
            warn!("playbin set_state(Paused) failed: {e}");
        }
        // Block until the pipeline actually finishes prerolling (reaches
        // PAUSED) — set_state() alone returns as soon as the change is
        // accepted, often well before preroll completes, and seeking before
        // that point reliably fails ("Failed to seek"). Matters most when
        // swapping Player over to the scrub proxy mid-playback (app.rs),
        // where we need to seek to the resume position immediately after
        // construction.
        let (result, state, _pending) = pipeline.state(gst::ClockTime::from_seconds(5));
        debug!("player: preroll wait -> {result:?}, state={state:?}");

        debug!("player ready (playbin)");
        Player {
            pipeline, appsink, duration, position: 0.0, paused: true,
            fps: fps.max(1.0), seeking: false, seek_issued_at: None,
        }
    }

    pub fn play(&mut self) {
        debug!("play() called, current state={:?}", self.pipeline.current_state());
        let t0 = std::time::Instant::now();
        if let Err(e) = self.pipeline.set_state(gst::State::Playing) {
            warn!("play() failed: {e}");
            return;
        }
        debug!("play(): set_state(Playing) returned in {:?}", t0.elapsed());
        self.paused = false;
    }

    pub fn pause(&mut self) {
        debug!("pause() called, current state={:?}", self.pipeline.current_state());
        let t0 = std::time::Instant::now();
        if let Err(e) = self.pipeline.set_state(gst::State::Paused) {
            warn!("pause() failed: {e}");
            return;
        }
        debug!("pause(): set_state(Paused) returned in {:?}", t0.elapsed());
        self.paused = true;
    }

    /// Frame-exact seek — correct for edit points (IN/OUT, frame stepping),
    /// costs real decode time proportional to GOP length.
    pub fn seek(&mut self, t: f64) {
        let t = t.clamp(0.0, self.duration);
        let pos = gst::ClockTime::from_nseconds((t * 1_000_000_000.0) as u64);
        debug!("seek({t:.3}) -> {pos}");
        let t0 = std::time::Instant::now();
        if let Err(e) = self.pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            pos,
        ) {
            warn!("seek({t:.3}) failed: {e}");
        } else {
            self.seeking = true;
            self.seek_issued_at = Some(std::time::Instant::now());
        }
        debug!("seek({t:.3}): seek_simple returned in {:?}", t0.elapsed());
        self.position = t;
    }

    /// True while a seek issued via `seek()` hasn't yet produced a new
    /// frame. See the `seeking` field doc for why this matters for held
    /// frame-stepping and dragging the timeline.
    pub fn is_seeking(&self) -> bool {
        self.seeking
    }

    /// Only steps cleanly while paused. Implemented as a frame-exact seek
    /// (position ± n/fps) rather than GStreamer's native step event: step
    /// support/behavior varies across decoder elements, while accurate
    /// seeking is universally supported and just as fast for a 1-frame nudge.
    pub fn step_frames(&mut self, n: i32) {
        if !self.paused {
            self.pause();
        }
        let target = (self.position + n as f64 / self.fps).clamp(0.0, self.duration);
        self.seek(target);
    }

    pub fn set_mute(&mut self, m: bool) {
        self.pipeline.set_property("mute", m);
    }

    /// Changes playback rate in place, keeping the current position.
    /// playbin has no settable "rate" property — GStreamer only exposes
    /// speed changes through a rate-seek (same mechanism `seek()` uses,
    /// just with `rate` != 1.0 and `SeekType::Set`/`SeekType::End` instead
    /// of an explicit stop position), so this costs the same
    /// keyframe-forward-decode as a normal seek and goes through the same
    /// `seeking` gate.
    pub fn set_speed(&mut self, rate: f64) {
        let rate = rate.clamp(0.1, 10.0);
        let pos = gst::ClockTime::from_nseconds((self.position * 1_000_000_000.0) as u64);
        debug!("set_speed({rate}) at pos={pos}");
        if let Err(e) = self.pipeline.seek(
            rate,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            pos,
            gst::SeekType::End,
            gst::ClockTime::ZERO,
        ) {
            warn!("set_speed({rate}) failed: {e}");
            return;
        }
        self.seeking = true;
        self.seek_issued_at = Some(std::time::Instant::now());
    }

    /// Toggles pitch-preserving time-stretch for preview audio. A rate-seek
    /// alone (`set_speed`) resamples audio along with video — pitch shifts
    /// with speed, tape/vinyl-style. playbin has no property for this
    /// either; the fix is inserting `scaletempo` (WSOLA time-stretch, no
    /// pitch shift) into the audio path via playbin's `audio-filter` slot,
    /// vs. `identity` (no-op passthrough) when pitch should shift with speed.
    pub fn set_keep_pitch(&mut self, keep: bool) {
        let factory = if keep { "scaletempo" } else { "identity" };
        let filter = match gst::ElementFactory::make(factory).build() {
            Ok(f) => f,
            Err(e) => {
                warn!("set_keep_pitch({keep}): failed to build '{factory}': {e}");
                return;
            }
        };
        // playbin only wires `audio-filter` into the pipeline when it builds
        // its audio sink bin, which happens on the NULL/READY -> PAUSED
        // transition — setting the property while already PAUSED/PLAYING
        // (i.e. every call after the very first) is a silent no-op with no
        // error. Drop to READY (tears down and re-links the sink bins, but
        // keeps the loaded `uri`) to force a rebuild, then restore position
        // and play state.
        let was_playing = !self.paused;
        let pos = self.position;
        if let Err(e) = self.pipeline.set_state(gst::State::Ready) {
            warn!("set_keep_pitch({keep}): set_state(Ready) failed: {e}");
            return;
        }
        self.pipeline.set_property("audio-filter", &filter);
        if let Err(e) = self.pipeline.set_state(gst::State::Paused) {
            warn!("set_keep_pitch({keep}): set_state(Paused) failed: {e}");
            return;
        }
        let (result, state, _) = self.pipeline.state(gst::ClockTime::from_seconds(5));
        debug!("set_keep_pitch({keep}) -> audio-filter={factory}, reconfigure -> {result:?}, state={state:?}");
        self.seek(pos);
        if was_playing {
            self.play();
        }
    }

    /// Refresh position/duration and return the most recent decoded frame
    /// (if any arrived since the last poll — appsink drops old ones for us
    /// since we only pull the latest). Call once per UI frame.
    pub fn poll(&mut self) -> Option<DecodedFrame> {
        let t_poll = std::time::Instant::now();

        let t_pos = std::time::Instant::now();
        let pos_q = self.pipeline.query_position::<gst::ClockTime>();
        if let Some(pos) = pos_q {
            self.position = pos.nseconds() as f64 / 1_000_000_000.0;
        }
        let dt_pos = t_pos.elapsed();

        let t_dur = std::time::Instant::now();
        let dur_q = self.pipeline.query_duration::<gst::ClockTime>();
        if let Some(dur) = dur_q {
            let secs = dur.nseconds() as f64 / 1_000_000_000.0;
            if secs > 0.0 {
                self.duration = secs;
            }
        }
        let dt_dur = t_dur.elapsed();

        let state = self.pipeline.current_state();
        trace!(
            "poll: state={state:?} pos_query={:?}({dt_pos:?}) dur_query={:?}({dt_dur:?})",
            pos_q.map(|c| c.to_string()), dur_q.map(|c| c.to_string()),
        );

        let t_pull = std::time::Instant::now();
        let mut latest = None;
        let mut n = 0;
        loop {
            let t_one = std::time::Instant::now();
            // appsink delivers a frame two different ways depending on
            // pipeline state: a PREROLL buffer while PAUSED (initial load,
            // and every seek issued while paused — i.e. essentially all of
            // scrubbing/stepping), or a regular SAMPLE while PLAYING.
            // try_pull_sample() alone only ever catches the latter — we
            // were missing every paused-state frame entirely (frame 0 never
            // rendering on load, and the display looking frozen after any
            // seek while paused, were both this).
            let pulled = self.appsink.try_pull_preroll(gst::ClockTime::ZERO)
                .or_else(|| self.appsink.try_pull_sample(gst::ClockTime::ZERO));
            match pulled {
                Some(sample) => {
                    trace!("poll: pulled #{n} took {:?}", t_one.elapsed());
                    let t_conv = std::time::Instant::now();
                    latest = sample_to_frame(&sample);
                    trace!("poll: sample_to_frame #{n} took {:?}", t_conv.elapsed());
                    n += 1;
                }
                None => break,
            }
        }
        let dt_pull = t_pull.elapsed();

        if latest.is_some() {
            self.seeking = false;
            if let Some(issued) = self.seek_issued_at.take() {
                debug!("seek->frame latency: {:?}", issued.elapsed());
            }
            trace!(
                "poll: TOTAL {:?}  (pos={dt_pos:?} dur={dt_dur:?} pull_loop={dt_pull:?} n_samples={n})",
                t_poll.elapsed(),
            );
        } else {
            trace!("poll: no new sample  (total {:?})", t_poll.elapsed());
        }
        latest
    }
}

fn sample_to_frame(sample: &gst::Sample) -> Option<DecodedFrame> {
    let t0 = std::time::Instant::now();
    let buffer = match sample.buffer() {
        Some(b) => b,
        None => {
            warn!("sample_to_frame: sample had no buffer");
            return None;
        }
    };
    let caps = match sample.caps() {
        Some(c) => c,
        None => {
            warn!("sample_to_frame: sample had no caps");
            return None;
        }
    };
    let info = match gstreamer_video::VideoInfo::from_caps(caps) {
        Ok(i) => i,
        Err(e) => {
            warn!("sample_to_frame: VideoInfo::from_caps failed: {e}");
            return None;
        }
    };
    let t_caps = t0.elapsed();

    let t_map_start = std::time::Instant::now();
    let map = match buffer.map_readable() {
        Ok(m) => m,
        Err(e) => {
            warn!("sample_to_frame: buffer.map_readable failed: {e}");
            return None;
        }
    };
    let t_map = t_map_start.elapsed();

    let w = info.width();
    let h = info.height();
    let stride = info.stride()[0] as usize;
    let src = map.as_slice();

    let t_copy_start = std::time::Instant::now();
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for row in 0..h as usize {
        let start = row * stride;
        rgb.extend_from_slice(&src[start..start + w as usize * 3]);
    }
    let t_copy = t_copy_start.elapsed();

    trace!(
        "sample_to_frame: {w}x{h} stride={stride} format={:?} caps_parse={t_caps:?} map={t_map:?} row_copy={t_copy:?} buf_size={}",
        info.format(), map.size(),
    );

    Some(DecodedFrame { rgb, w, h })
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        debug!("player (playbin) stopped");
    }
}
