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

    pub duration: f64,
    pub position: f64,
    pub paused:   bool,
}

impl Player {
    /// `parent`: the app's own top-level HWND (from
    /// `eframe::Frame::window_handle()`). `rect`: (x, y, w, h) in physical
    /// pixels, relative to the parent's client area — reposition every
    /// frame via `set_rect` as the video panel's on-screen rect changes.
    /// `path`: video file to load, starting paused.
    pub fn new(parent: HWND, rect: (i32, i32, i32, i32), path: &str) -> Result<Self, String> {
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

            debug!("player: mpv embedded, child hwnd={hwnd:?}");

            Ok(Player {
                mpv,
                hwnd,
                fn_command: mpv_command,
                fn_get_property: mpv_get_property,
                fn_terminate: mpv_terminate,
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
    }

    pub fn pause(&mut self) {
        self.command("set pause yes");
        self.paused = true;
    }

    pub fn seek(&self, t: f64) {
        self.command(&format!("seek {t:.3} absolute exact"));
    }

    /// mpv only steps cleanly while paused.
    pub fn step_frames(&mut self, n: i32) {
        if !self.paused {
            self.pause();
        }
        let cmd = if n >= 0 { "frame-step" } else { "frame-back-step" };
        for _ in 0..n.abs() {
            self.command(cmd);
        }
    }

    pub fn set_mute(&self, m: bool) {
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

    /// Refresh position/duration/pause state from mpv. Call once per frame.
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
                self.duration = dur;
            }

            let mut flag: c_int = 0;
            let name = CString::new("pause").unwrap();
            if (self.fn_get_property)(self.mpv, name.as_ptr(), MPV_FORMAT_FLAG, &mut flag as *mut _ as *mut c_void) >= 0 {
                self.paused = flag != 0;
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
