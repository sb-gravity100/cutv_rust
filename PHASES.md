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

## Phase 1 — player.rs migration to libmpv (depends on Phase 0) (SUPERSEDED 2026-09-19 — see Phase 1b)

Completed and runtime-verified, but the libmpv approach itself was dropped after this phase — see Phase 1b below and `PLAN.md`'s "Architecture decision" for why. `src/player.rs`'s libmpv code stays in the tree as a reference until Phase 1b's rewrite replaces it.

- [x] Write `player.rs`: `Player::new(parent_hwnd, rect, path)`, `play()`, `pause()`, `seek(t)`, `step_frames(n)`, `set_mute(bool)`, `set_rect(...)`, `poll()` (polls `time-pos`/`duration`/`pause` each frame rather than an observer channel — simpler given egui's own per-frame `update()` tick already drives polling).
- [x] Wire `app.rs` to `player.rs` instead of `video.rs`/`audio.rs` — playback controls, timeline scrubbing all go through `Player`; thumbnails extracted to `thumbs.rs` (ffmpeg single-frame extraction, independent of the player). Real app window HWND resolved via `raw-window-handle`'s `frame.window_handle()` (the Phase 0 open item — spike used a hand-rolled stand-in window).
- [x] Remove `video.rs`, `audio.rs`.
- [x] Update `FILE_INDEX.md`, `PLAN.md`.
- [x] Runtime-verified 2026-09-19 against `sample.mp4` (2340x1080): mpv's embedded child window renders correctly on top of eframe/glutin's GL surface with no z-order/repaint conflicts, letterboxing/resizing is correct, thumbnails and audio work. Confirmed via screenshots — see below for a bug found during this pass.

**Bugs found during runtime verification, all fixed 2026-09-19:**

1. **Autoplay on load.** The video started playing immediately after `Player::new()` even though `Player.paused` was initialized `true` and `set_opt("pause", "yes")` was set before `mpv_initialize`. Root cause: that option only sets mpv's state at `mpv_initialize` time, and doesn't survive the subsequent `loadfile` command, which starts playing the newly loaded file regardless — `pause` only "sticks" once the file has actually finished loading, but `loadfile` is asynchronous. Fix: `Player` now tracks `pending_pause` and force-issues `set pause yes` in `poll()` the first time `duration` becomes known (i.e. once the file has actually loaded).
2. **Play/Pause button flicker during held frame-step.** `frame-step`/`frame-back-step` briefly unpause mpv internally to render the stepped frame, then re-pause. `poll()` used to read mpv's live `pause` property every frame (egui repaints far more often than the hold-repeat rate), catching that transient unpaused window and flickering the button. Fix: `paused` is now tracked authoritatively on the Rust side (`play()`/`pause()`/`step_frames()`), and `poll()` no longer reads back mpv's `pause` property at all — safe since mpv's OSC/input bindings are disabled, so nothing else can change it out from under us.
3. **Backward frame-step (`,` / "◀ 1f") was slow and built up a lag backlog when held.** mpv's `frame-back-step` is documented as much slower than `frame-step` (it seeks to *before* the target and steps forward frame-by-frame to land exactly right, rather than seeking straight there). Held-repeat issued a new step every ~83ms regardless of whether the previous one had finished — since `mpv_command_string` is fire-and-forget with no completion signal, repeats queued up faster than mpv could finish them, and stepping kept "catching up" well after the key was released. Fix: backward stepping now issues a precise `seek {t} absolute exact` to `position - n/fps` instead of `frame-back-step` (reaches the same frame without the redundant forward-stepping), and the hold-repeat loop gates on mpv's `seeking` property (`Player::is_seeking()`) — skipping a repeat until the previous one has actually landed — instead of assuming a fixed interval.
4. **Frame-step audio blip.** The same brief internal unpause from (2) let a blip of audio through during stepping even when not muted. Fix: `step_frames()` now force-mutes before stepping; `play()` restores the caller's real mute preference (tracked as `Player.want_mute`, set via `set_mute()`) when actual playback resumes.

Verified via debug logging (`step_frames n=... pos=...` / `frame hold: skip repeat, still seeking`, both now permanent `debug!` logs) rather than screenshots for the last two fixes — direction and cadence are correct in both directions near the start of the file, and the seeking-gate correctly suppresses a repeat mid-seek instead of queuing. Not yet re-verified visually/interactively after these four fixes (automated window-focus scripting hit Windows' `SetForegroundWindow` focus-stealing restriction mid-session) — worth a manual pass.

## Phase 1b — drop libmpv for a Rust-native persistent decoder (depends on Phase 1)

Goal: replace `player.rs`'s libmpv embedding with an in-process decoder via `ffmpeg-next`, keeping the demuxer/decoder warm across seeks (same scrub-smoothness win mpv gave us) but without mpv's async fire-and-forget command model, which was the root cause of every bug found in Phase 1 (see above). See `PLAN.md`'s "Architecture decision" section for full rationale.

- [x] Confirm build toolchain: vcpkg-built FFmpeg with NVENC/NVDEC/CUVID/D3D11VA/DXVA2 (`vcpkg install "ffmpeg[nvcodec,avcodec,avformat,avfilter,swscale,swresample]:x64-windows"`, ~12 min build), LLVM/clang for `bindgen`, `.cargo/config.toml` pointing `VCPKG_ROOT` at vcpkg. `cargo add ffmpeg-next` + `cargo build` confirmed working 2026-09-19.
- [ ] Write the new `player.rs` (or rename to `decoder.rs`): persistent background thread opens the file once via `ffmpeg-next`, decodes frames to RGB for an egui texture (replacing the native child window), handles play/pause/seek/step synchronously within that thread (no async command queue to guess completion of).
- [ ] Audio: decide whether to keep `rodio` for playback (decoding audio ourselves via `ffmpeg-next` too, feeding a custom `rodio::Source`) or find another path — needs to stay in sync with video via a shared clock/position, same problem the pre-mpv `audio.rs` had to solve manually.
- [ ] Hardware decode: use `ffmpeg-next`'s hardware device context APIs (NVDEC/D3D11VA, now built into the linked FFmpeg) so decode isn't CPU-bound.
- [ ] Wire `app.rs`'s `ui_video` back to drawing a texture (like pre-mpv `video.rs`) instead of positioning a native child window; `Player`'s public API (`play`/`pause`/`seek`/`step_frames`/`set_mute`/`poll`) should carry over mostly unchanged so the rest of `app.rs` needs minimal changes.
- [ ] Remove the libmpv code from `player.rs` (or delete it if renamed to `decoder.rs`) once the new decoder is proven; `src/bin/mpv_spike.rs` can stay as a reference for the win32-embedding technique even though it's no longer the playback direction.
- [ ] Update `FILE_INDEX.md`, `PLAN.md`, `commits.md`.

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

Note: `do_cut`'s final export still shells out to `ffmpeg.exe`/`ffprobe.exe` on `PATH` (unrelated to the vcpkg-built FFmpeg linked into the app for playback decode in Phase 1b) — this phase is about the `PATH` ffmpeg's NVENC support, not the linked library.

- [ ] `probe::nvenc_available()` (cached, greps `ffmpeg -hide_banner -encoders`).
- [ ] Extend `probe::encode_args` with GPU branch (`h264_nvenc`/`hevc_nvenc`) + `-hwaccel cuda` on the input side, falling back to CPU encoders when unavailable.

## Deferred (not scheduled)

- yt-dlp URL download support.
