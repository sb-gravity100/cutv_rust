// Phase 0 spike (see PHASES.md): prove libmpv can render into a window we
// own before wiring it into eframe/egui. Throwaway — not part of the app,
// not wired into main.rs.
//
// libmpv-2.dll has no MSVC import lib available on this machine, so instead
// of link-time linking (what libmpv-rs/libmpv2 do) this loads the DLL at
// runtime via LoadLibraryW/GetProcAddress and calls the handful of client
// API functions we need through raw function pointers — the same approach
// python-mpv takes via ctypes.
//
// Usage: cargo run --bin mpv_spike -- [path-to-libmpv-2.dll] [path-to-video]

use std::ffi::{c_char, c_int, c_void, CString};

use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, EndPaint, FillRect, InvalidateRect, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::*;

type MpvCreate          = unsafe extern "C" fn() -> *mut c_void;
type MpvInitialize       = unsafe extern "C" fn(*mut c_void) -> c_int;
type MpvSetOptionString  = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type MpvCommandString    = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type MpvTerminateDestroy = unsafe extern "C" fn(*mut c_void);

// Bottom strip left unobscured by the mpv child window — stands in for
// where egui's own controls (timeline/transport/etc.) would paint, so this
// spike can confirm mpv only renders in its own region, not the whole window.
const CONTROLS_H: i32 = 120;

static mut CHILD_HWND: HWND = HWND(std::ptr::null_mut());

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
            WM_SIZE => {
                // Prove resizing the parent correctly repositions/resizes
                // the embedded mpv child, rather than leaving it stale.
                let width  = (lparam.0 as i32) & 0xFFFF;
                let height = ((lparam.0 as i32) >> 16) & 0xFFFF;
                if !CHILD_HWND.0.is_null() {
                    let _ = SetWindowPos(
                        CHILD_HWND, None, 0, 0, width, (height - CONTROLS_H).max(0),
                        SWP_NOZORDER,
                    );
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1), // we paint it ourselves in WM_PAINT below
            WM_PAINT => {
                // Fill just the bottom strip a distinct color — proves mpv's
                // child window is confined to its own region, not painting
                // over (or being painted over by) this "controls" area.
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                rc.top = rc.bottom - CONTROLS_H;
                let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00_2a_1e_1e));
                FillRect(hdc, &rc, brush);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dll_path   = args.next().unwrap_or_else(|| r"C:\cli_tools\scripts\py\cutv\libmpv-2.dll".into());
    let video_path = args.next().unwrap_or_else(|| "sample.mp4".into());

    println!("dll:   {dll_path}");
    println!("video: {video_path}");

    unsafe {
        // ── Load libmpv and resolve the client API functions we need ───────────
        let wide: Vec<u16> = dll_path.encode_utf16().chain(std::iter::once(0)).collect();
        let hmod = LoadLibraryW(PCWSTR(wide.as_ptr()))
            .unwrap_or_else(|e| panic!("LoadLibraryW({dll_path}) failed: {e}"));

        let mpv_create: MpvCreate = std::mem::transmute(
            GetProcAddress(hmod, s!("mpv_create")).expect("mpv_create not found"));
        let mpv_initialize: MpvInitialize = std::mem::transmute(
            GetProcAddress(hmod, s!("mpv_initialize")).expect("mpv_initialize not found"));
        let mpv_set_option_string: MpvSetOptionString = std::mem::transmute(
            GetProcAddress(hmod, s!("mpv_set_option_string")).expect("mpv_set_option_string not found"));
        let mpv_command_string: MpvCommandString = std::mem::transmute(
            GetProcAddress(hmod, s!("mpv_command_string")).expect("mpv_command_string not found"));
        let mpv_terminate_destroy: MpvTerminateDestroy = std::mem::transmute(
            GetProcAddress(hmod, s!("mpv_terminate_destroy")).expect("mpv_terminate_destroy not found"));

        println!("libmpv loaded, symbols resolved");

        // ── Plain top-level window to embed into ────────────────────────────────
        let hmodule: windows::Win32::Foundation::HMODULE = GetModuleHandleW(None).unwrap();
        let hinstance: windows::Win32::Foundation::HINSTANCE = hmodule.into();
        let class_name = w!("CutvMpvSpike");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("mpv spike — bottom strip is NOT mpv, simulates egui controls"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT, CW_USEDEFAULT, 960, 540,
            None, None, hinstance, None,
        ).expect("CreateWindowExW failed");

        println!("parent window created: {:?}", hwnd);

        // ── Child window: this is what mpv actually renders into. A real
        // integration hands mpv the HWND of a child positioned/sized to the
        // video rect inside the eframe window, exactly like this. ──────────
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let child = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WS_CHILD | WS_VISIBLE,
            0, 0, rc.right, (rc.bottom - CONTROLS_H).max(0),
            hwnd, None, hinstance, None,
        ).expect("child CreateWindowExW failed");
        CHILD_HWND = child;

        println!("child window created: {:?}", child);

        // ── Point mpv at the CHILD window (not the top-level one), then load ────
        let mpv = mpv_create();
        assert!(!mpv.is_null(), "mpv_create returned null");

        let opt_wid = CString::new("wid").unwrap();
        let wid_str = CString::new((child.0 as isize).to_string()).unwrap();
        let rc = mpv_set_option_string(mpv, opt_wid.as_ptr(), wid_str.as_ptr());
        println!("set wid -> {rc}");

        let rc = mpv_initialize(mpv);
        println!("mpv_initialize -> {rc}");

        let load_cmd = CString::new(format!("loadfile \"{video_path}\"")).unwrap();
        let rc = mpv_command_string(mpv, load_cmd.as_ptr());
        println!("loadfile -> {rc}");

        let _ = InvalidateRect(hwnd, None, true);

        // ── Pump messages until the window is closed ────────────────────────────
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        mpv_terminate_destroy(mpv);
        println!("mpv terminated cleanly");
    }
}
