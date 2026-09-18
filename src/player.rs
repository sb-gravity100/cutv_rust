// Embeds libmpv into a native Win32 child window for real video playback —
// replaces the old ffmpeg-pipe pipeline (formerly video.rs/audio.rs).
// mpv owns decode, GPU render, audio output, and clocking as one unit, so
// there's no proxy transcode step and no manual audio/video sync.
//
// libmpv is loaded at RUNTIME (LoadLibraryW/GetProcAddress), not link time:
// there's no MSVC import lib for the available libmpv-2.dll. See
// src/bin/mpv_spike.rs for the proof-of-concept this is built from, and
// PLAN.md for the still-open question of how to distribute the DLL.

use std::ffi::{c_char, c_double, c_int, c_void, CString};

use log::{debug, warn};
use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
    WINDOW_EX_STYLE, WS_CHILD, WS_VISIBLE,
};

type MpvCreate          = unsafe extern "C" fn() -> *mut c_void;
type MpvInitialize       = unsafe extern "C" fn(*mut c_void) -> c_int;
type MpvSetOptionString  = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type MpvCommandString    = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type MpvGetProperty      = unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int;
type MpvTerminateDestroy = unsafe extern "C" fn(*mut c_void);

const MPV_FORMAT_FLAG:   c_int = 3;
const MPV_FORMAT_DOUBLE: c_int = 5;

fn dll_path() -> String {
    // TODO: distribution decision still open (PLAN.md) — bundle vs require
    // a local install vs configurable path. For now: env override, else
    // the copy that ships next to the external Python reference project.
    std::env::var("CUTV_LIBMPV_PATH")
        .unwrap_or_else(|_| r"C:\cli_tools\scripts\py\cutv\libmpv-2.dll".to_string())
}

pub struct Player {
    mpv:  *mut c_void,
    hwnd: HWND,

    fn_command:      MpvCommandString,
    fn_get_property: MpvGetProperty,
    fn_terminate:    MpvTerminateDestroy,

    fps: f64,
    // The "pause" option/command race described in `new()`: set once the
    // file has actually finished loading (first time `duration` is known),
    // to force the real starting-paused state mpv otherwise drops.
    pending_pause: bool,
    // Caller's actual mute preference, independent of the transient mute
    // step_frames() applies for the duration of frame stepping.
    want_mute: bool,

    pub duration: f64,
    pub position: f64,
    pub paused:   bool,
}

impl Player {
    /// `parent`: the app's own top-level HWND (from
    /// `eframe::Frame::window_handle()`). `rect`: (x, y, w, h) in physical
    /// pixels, relative to the parent's client area — reposition every
    /// frame via `set_rect` as the video panel's on-screen rect changes.
    /// `path`: video file to load, starting paused. `fps`: source frame
    /// rate, used to convert 1-frame steps into precise seek offsets.
    pub fn new(parent: HWND, rect: (i32, i32, i32, i32), path: &str, fps: f64) -> Result<Self, String> {
        let dll = dll_path();
        unsafe {
            let wide: Vec<u16> = dll.encode_utf16().chain(std::iter::once(0)).collect();
            let hmod = LoadLibraryW(PCWSTR(wide.as_ptr()))
                .map_err(|e| format!("LoadLibraryW({dll}) failed: {e}"))?;

            macro_rules! sym {
                ($name:literal, $ty:ty) => {{
                    let addr = GetProcAddress(hmod, s!($name))
                        .ok_or_else(|| concat!($name, " not found in libmpv").to_string())?;
                    std::mem::transmute::<_, $ty>(addr)
                }};
            }

            let mpv_create:       MpvCreate          = sym!("mpv_create", MpvCreate);
            let mpv_initialize:   MpvInitialize       = sym!("mpv_initialize", MpvInitialize);
            let mpv_set_option:   MpvSetOptionString  = sym!("mpv_set_option_string", MpvSetOptionString);
            let mpv_command:      MpvCommandString    = sym!("mpv_command_string", MpvCommandString);
            let mpv_get_property: MpvGetProperty      = sym!("mpv_get_property", MpvGetProperty);
            let mpv_terminate:    MpvTerminateDestroy = sym!("mpv_terminate_destroy", MpvTerminateDestroy);

            let hinstance: HINSTANCE = GetModuleHandleW(None)
                .map_err(|e| e.to_string())?
                .into();

            let (x, y, rw, rh) = rect;
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WS_CHILD | WS_VISIBLE,
                x, y, rw.max(1), rh.max(1),
                parent, None, hinstance, None,
            ).map_err(|e| format!("child CreateWindowExW failed: {e}"))?;

            let mpv = mpv_create();
            if mpv.is_null() {
                return Err("mpv_create returned null".into());
            }

            let set_opt = |name: &str, val: &str| {
                let n = CString::new(name).unwrap();
                let v = CString::new(val).unwrap();
                mpv_set_option(mpv, n.as_ptr(), v.as_ptr())
            };
            set_opt("wid", &(hwnd.0 as isize).to_string());
            set_opt("keep-open", "always");
            set_opt("pause", "yes");
            set_opt("osc", "no");
            set_opt("osd-level", "0");
            set_opt("hwdec", "auto-copy");
            // Don't let mpv steal keyboard/mouse input meant for our egui
            // shortcuts and timeline interaction.
            set_opt("input-default-bindings", "no");
            set_opt("input-vo-keyboard", "no");
            set_opt("input-cursor", "no");
            // We size the child window to the aspect-correct rect ourselves
            // (see app.rs's ui_video); don't let mpv add its own margins on
            // top of that.
            set_opt("keepaspect-window", "no");
            set_opt("video-margin-ratio-left", "0");
            set_opt("video-margin-ratio-right", "0");
            set_opt("video-margin-ratio-top", "0");
            set_opt("video-margin-ratio-bottom", "0");

            let rc = mpv_initialize(mpv);
            if rc < 0 {
                return Err(format!("mpv_initialize failed: {rc}"));
            }

            let cmd = CString::new(format!("loadfile \"{path}\"")).unwrap();
            mpv_command(mpv, cmd.as_ptr());
            // The "pause" option only sets mpv's *initial* state, and does
            // NOT reliably stick through the loadfile command above — mpv
            // starts playing the newly loaded file regardless, because
            // "pause" only applies once the file has actually finished
            // loading and loadfile is asynchronous. `pending_pause` below
            // re-forces it once we observe the file is actually ready
            // (first time `duration` is known, in `poll()`).

            debug!("player: mpv embedded, child hwnd={hwnd:?}");

            Ok(Player {
                mpv,
                hwnd,
                fn_command: mpv_command,
                fn_get_property: mpv_get_property,
                fn_terminate: mpv_terminate,
                fps: fps.max(1.0),
                pending_pause: true,
                want_mute: false,
                duration: 0.0,
                position: 0.0,
                paused: true,
            })
        }
    }

    fn command(&self, cmd: &str) {
        let c = CString::new(cmd).unwrap();
        let rc = unsafe { (self.fn_command)(self.mpv, c.as_ptr()) };
        if rc < 0 {
            warn!("mpv command '{cmd}' failed: {rc}");
        }
    }

    pub fn play(&mut self) {
        self.command("set pause no");
        self.paused = false;
        // step_frames() force-mutes for the duration of frame stepping (see
        // below); resuming real playback restores whatever mute state the
        // caller actually wants.
        self.set_mute(self.want_mute);
    }

    pub fn pause(&mut self) {
        self.command("set pause yes");
        self.paused = true;
    }

    pub fn seek(&self, t: f64) {
        self.command(&format!("seek {t:.3} absolute exact"));
    }

    /// True while mpv is still processing a seek issued via `seek()` /
    /// the backward branch of `step_frames()`. Used to avoid queuing
    /// another step/seek command before the previous one has actually
    /// landed — mpv_command_string is fire-and-forget with no completion
    /// signal, and issuing commands faster than mpv can finish them just
    /// builds a backlog that keeps "catching up" well after input stops.
    pub fn is_seeking(&self) -> bool {
        unsafe {
            let mut flag: c_int = 0;
            let name = CString::new("seeking").unwrap();
            if (self.fn_get_property)(self.mpv, name.as_ptr(), MPV_FORMAT_FLAG, &mut flag as *mut _ as *mut c_void) >= 0 {
                flag != 0
            } else {
                false
            }
        }
    }

    /// mpv only steps cleanly while paused.
    pub fn step_frames(&mut self, n: i32) {
        debug!("step_frames n={n}  pos={:.3}", self.position);
        if !self.paused {
            self.pause();
        }
        // frame-step's implementation briefly unpauses mpv to actually
        // render the next frame, which lets a blip of audio through even
        // though we're conceptually still "paused" for stepping. Mute for
        // the duration; play() restores the real mute preference.
        self.command("set mute yes");
        if n >= 0 {
            for _ in 0..n {
                self.command("frame-step");
            }
        } else {
            // `frame-back-step` is dramatically slower than `frame-step`:
            // per mpv's own docs it seeks to *before* the target and then
            // steps forward frame-by-frame to land exactly 1 frame back,
            // rather than just seeking straight to the target. A precise
            // seek to the target timestamp reaches the same frame without
            // that redundant forward-stepping, and lands in roughly the
            // same time as a forward frame-step.
            let target = (self.position - (n.unsigned_abs() as f64) / self.fps).max(0.0);
            self.seek(target);
            self.position = target; // optimistic; corrected by the next poll()
        }
    }

    pub fn set_mute(&mut self, m: bool) {
        self.want_mute = m;
        self.command(if m { "set mute yes" } else { "set mute no" });
    }

    /// Reposition/resize the embedded child window. Call every frame the
    /// video panel's on-screen rect changes (`rect` in physical pixels,
    /// relative to the parent's client area).
    pub fn set_rect(&self, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd, None, x, y, w.max(1), h.max(1),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// Refresh position/duration from mpv. Call once per frame.
    ///
    /// Deliberately does NOT read back the "pause" property: mpv's
    /// `frame-step`/`frame-back-step` commands internally unpause for one
    /// frame's presentation then re-pause, and polling faster than that
    /// cycle (egui repaints far more often than the frame-hold repeat rate)
    /// catches the transient unpaused state, making the Play/Pause button
    /// flicker during a held frame-step. `paused` is instead tracked
    /// authoritatively on the Rust side by play()/pause()/step_frames() —
    /// safe since mpv's OSC/input bindings are disabled, so nothing else
    /// can change pause state out from under us.
    pub fn poll(&mut self) {
        unsafe {
            let mut pos: c_double = 0.0;
            let name = CString::new("time-pos").unwrap();
            if (self.fn_get_property)(self.mpv, name.as_ptr(), MPV_FORMAT_DOUBLE, &mut pos as *mut _ as *mut c_void) >= 0 {
                self.position = pos;
            }

            let mut dur: c_double = 0.0;
            let name = CString::new("duration").unwrap();
            if (self.fn_get_property)(self.mpv, name.as_ptr(), MPV_FORMAT_DOUBLE, &mut dur as *mut _ as *mut c_void) >= 0 && dur > 0.0 {
                let was_loading = self.duration <= 0.0;
                self.duration = dur;
                if was_loading && self.pending_pause {
                    self.pending_pause = false;
                    self.command("set pause yes");
                }
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        unsafe {
            (self.fn_terminate)(self.mpv);
            let _ = DestroyWindow(self.hwnd);
        }
        debug!("player dropped");
    }
}
