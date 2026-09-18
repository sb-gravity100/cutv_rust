# PHASES.md

Per-phase task lists and done criteria. Check off as completed; keep in sync with `PLAN.md`/`FILE_INDEX.md`.

## Phase 0 — libmpv embedding spike (DONE 2026-08-25)

Goal: prove mpv can render into a child window inside the eframe window before committing to ripping out `video.rs`/`audio.rs`.

- [x] Confirm a libmpv build is available on this machine: `C:\cli_tools\scripts\py\cutv\libmpv-2.dll` (no MSVC import lib ships with it).
- [x] Load libmpv at **runtime** instead of link-time: `libmpv2`/`libmpv-rs` require an MSVC import lib that doesn't exist for this DLL, so `src/bin/mpv_spike.rs` uses `LoadLibraryW`/`GetProcAddress` on `mpv_create`/`mpv_initialize`/`mpv_set_option_string`/`mpv_command_string`/`mpv_terminate_destroy` — the same approach `python-mpv` takes via `ctypes`. This is the pattern `player.rs` will use in Phase 1.
- [x] Create a native Win32 **child** window (not just a top-level one) and hand its HWND to mpv via the `wid` option — proven with a real WS_CHILD window, not simulated.
- [x] Confirm playback renders correctly inside the child region only (a bottom "controls" strip painted separately stayed untouched by mpv), and that resizing the parent (`WM_SIZE` → `SetWindowPos` on the child) correctly repositions/resizes the embedded video. Confirmed working by the user.
- [ ] Still open for Phase 1: get the raw HWND of the *actual* eframe/winit window (this spike used a hand-rolled Win32 window standing in for it) via `raw-window-handle`, and confirm mpv's child window doesn't fight eframe/glutin's own GL surface/repaint loop.
- **Result:** spike succeeded — proceeding to Phase 1. `src/bin/mpv_spike.rs` stays in the tree as a reference/regression check for the embedding technique; not part of the shipped app (not wired into `main.rs`).
- **Open question carried to Phase 1:** how to distribute `libmpv-2.dll` with the real app (bundle vs. require a local mpv/libmpv install vs. read a configurable path) — not decided yet, spike currently points at the external reference project's copy.

## Phase 1 — player.rs migration (depends on Phase 0) (DONE 2026-09-19, pending runtime verification)

- [x] Write `player.rs`: `Player::new(parent_hwnd, rect, path)`, `play()`, `pause()`, `seek(t)`, `step_frames(n)`, `set_mute(bool)`, `set_rect(...)`, `poll()` (polls `time-pos`/`duration`/`pause` each frame rather than an observer channel — simpler given egui's own per-frame `update()` tick already drives polling).
- [x] Wire `app.rs` to `player.rs` instead of `video.rs`/`audio.rs` — playback controls, timeline scrubbing all go through `Player`; thumbnails extracted to `thumbs.rs` (ffmpeg single-frame extraction, independent of the player). Real app window HWND resolved via `raw-window-handle`'s `frame.window_handle()` (the Phase 0 open item — spike used a hand-rolled stand-in window).
- [x] Remove `video.rs`, `audio.rs`.
- [x] Update `FILE_INDEX.md`, `PLAN.md`.
- [ ] **Not yet verified**: actually running the app against a real video to confirm mpv's embedded child window coexists cleanly with eframe/glutin's GL surface/repaint loop and that playback/seek/mute work end-to-end. Only `cargo check` has passed so far.

## Phase 2 — cut progress bar

- [ ] Convert `do_cut`'s ffmpeg export call from blocking `.output()` to `Stdio::piped()` with a stdout line reader parsing `out_time_ms=`, draining stderr in parallel.
- [ ] Add progress state to `CutvApp`, thin `Rect` fill in the status bar.

## Phase 3 — crop tool

- [ ] Native topmost overlay window positioned over the video region (layered window w/ per-pixel alpha).
- [ ] Right-drag rubber-band → crop rect in canvas coords → map to source-pixel coords.
- [ ] `CROP` toggle button in the edit row.
- [ ] Bake `-vf crop=...` into `do_cut` (and GIF export once Phase 4 lands) when a crop is set.

## Phase 4 — GIF export

- [ ] `gif.rs`: two-pass `palettegen`/`paletteuse` worker, respecting IN/OUT selection and active crop.
- [ ] `GIF` button + `g` keyboard shortcut in the edit row.

## Phase 5 — NVENC/GPU encode

- [ ] `probe::nvenc_available()` (cached, greps `ffmpeg -hide_banner -encoders`).
- [ ] Extend `probe::encode_args` with GPU branch (`h264_nvenc`/`hevc_nvenc`) + `-hwaccel cuda` on the input side, falling back to CPU encoders when unavailable.

## Deferred (not scheduled)

- yt-dlp URL download support.
