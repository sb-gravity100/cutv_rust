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

## Phase 1b — drop libmpv for a Rust-native persistent decoder (SUPERSEDED 2026-09-19, same day — see Phase 1c)

Goal was a hand-rolled `ffmpeg-next` decode thread. Built and largely worked (correct direction/cadence for frame stepping, a real precise-seek-via-keyframe implementation, backlog-avoidance gating) but after real regressions (black screen on load, an under-thought resolution cap, a hard freeze on seek once NVDEC was wired in via raw FFI) the call was made to stop reimplementing a video player by hand and use a mature pipeline library instead — see `PLAN.md`'s "Architecture decision" for the full reasoning. `ffmpeg-next` was removed as a dependency.

## Phase 1c — GStreamer `playbin` (depends on Phase 1b) (DONE 2026-09-19)

- [x] Build toolchain: vcpkg GStreamer (`vcpkg install "gstreamer[plugins-base,plugins-good,plugins-bad,libav]:x64-windows"`, ~5 min — the `nvcodec` feature fails to build, a vcpkg port bug, not included), `.cargo/config.toml` pointing `PKG_CONFIG_PATH`/`PKG_CONFIG` at it. `GST_PLUGIN_PATH` + vcpkg `bin` on `PATH` set as persistent user env vars for runtime DLL/plugin discovery.
- [x] Rewrote `player.rs`: `Player` wraps a `playbin` pipeline + `appsink` (via a GPU `d3d11upload!d3d11convert!...!d3d11download` colorspace-conversion bin — see below for why not plain `videoconvert`). Same public API shape as before (`play`/`pause`/`seek`/`step_frames`/`set_mute`/`poll`), so `app.rs` needed minimal changes. Audio is playbin's own default sink — `audio.rs` deleted, no replacement needed.
- [x] `seek` uses `SeekFlags::ACCURATE` (frame-exact). Originally paired with a `seek_fast`/`SeekFlags::KEY_UNIT` variant for cheap continuous timeline-drag scrubbing — **removed in Phase 1d** once the scrub proxy made exact seeks cheap enough that the accuracy/speed tradeoff wasn't needed at all.
- [x] Removed `video.rs`/`audio.rs` (the `ffmpeg-next` versions) entirely; `mod video`/`mod audio` dropped from `main.rs`.
- [x] **The low-FPS investigation** (~9fps → 30fps): see `PLAN.md`'s dedicated section. Two real, separate fixes: (1) swap CPU `videoconvert` for GPU `d3d11convert` as the primary path (with `videoconvert` as fallback), and (2) **use `cargo build --release`** — the debug build's unoptimized `ColorImage::from_rgb` alone cost ~96ms/frame, capping playback at ~9fps regardless of how fast the pipeline was.
- [x] **ORC fix (same day, follow-up):** root-caused why `videoconvert` needed the D3D11 workaround at all — this vcpkg GStreamer build has ORC (its SIMD codegen library) disabled (a real upstream vcpkg port limitation: vcpkg's own `orc` port is unrelated Apache ORC, a name collision). Added `vcpkg-overlay/gstreamer/` — a local overlay port (copy of upstream + one line changed to `-Dorc=enabled`, pre-vendoring ORC 0.4.42's official tarball into `subprojects/orc` since `vcpkg_configure_meson` always blocks the wrap file's git-fetch). `videoconvert` alone now measures ~175fps (was 8.85fps) — the D3D11 path stays primary (still faster, ~640fps, and offloads the CPU) but the fallback is now genuinely solid instead of just "won't hang."
- [x] Detailed `trace!`/`debug!` timing instrumentation left in `player.rs`/`app.rs` (gated behind `RUST_LOG=trace` for per-frame numbers, `RUST_LOG=debug` for sparse events like `measured_fps`) — reusable if a future perf regression needs the same kind of investigation.
- [x] Update `FILE_INDEX.md`, `PLAN.md`, `CLAUDE.md`, `commits.md`.
- [x] Re-verified end-to-end interactively by the user after the D3D11 + release-build + ORC fixes — confirmed steady 30/60fps playback.

## Phase 1d — scrub proxy for fast + accurate seeking (depends on Phase 1c) (DONE 2026-09-19)

Playback was fast; seeking still wasn't ("takes maybe 500ms+", "+1/-1 frame doesn't work", "dragging doesn't really update playback"). Root cause and fix: see `PLAN.md`'s "The scrub proxy" section for the full investigation. Summary:

- [x] `proxy.rs` (new): `spawn_proxy()` background-transcodes the source into a short-GOP (`-g 8 -keyint_min 8`) proxy on load, NVENC first with libx264 `ultrafast` fallback. `app.rs`'s `update()` swaps `Player` over to it once ready (preserving position/play state); `do_cut()` keeps using the original file.
- [x] Fixed the actual seek-thrashing bug (not just held-repeat, but *every* fresh press too — `service_frame_hold`'s initial-press branch bypassed the `is_seeking()` gate entirely) and applied the same gate to timeline-drag seeking (`ui_timeline`), which weren't gated at all before and could fire 11+ seeks/sec with only 1 ever landing.
- [x] Fixed frame 0 never rendering on load (and the picture looking frozen after any paused-state seek): `appsink` delivers a **preroll** buffer while `PAUSED`, not a regular **sample** — `Player::poll()` only ever called `try_pull_sample()`, silently missing every paused-state frame. Now tries `try_pull_preroll()` first.
- [x] Fixed `seek()` failing right after the proxy swap ("Failed to seek"): a freshly-constructed `Player`'s pipeline hasn't finished prerolling when `set_state(Paused)` returns (async). `Player::new()` now blocks on `pipeline.state(timeout)` until preroll actually completes.
- [x] **Removed `seek_fast`/`KEY_UNIT` entirely** per explicit direction ("do not use fast seek at all") — with the proxy's short GOP, an exact seek is cheap enough that there's no reason to trade accuracy for speed. Every seek in the app is now frame-exact.
- [x] Confirmed working end-to-end by the user against both `sample_60fps.mp4` (long-GOP test case) and the usual `sample.mp4`.
- [x] Update `FILE_INDEX.md`, `PLAN.md`, `commits.md`.

## Phase 2 — cut progress bar (DONE 2026-09-19)

- [x] Convert `do_cut`'s ffmpeg export call from blocking `.output()` to `Stdio::piped()` with a stdout line reader parsing `out_time_us=`/`out_time_ms=`, draining stderr in parallel on its own thread to avoid a pipe deadlock.
- [x] Progress state (`cut_progress_shared: Arc<Mutex<f32>>`, synced into `CutvApp::cut_progress` each frame) drives the existing thin `Rect` fill in the status bar (`ui_progress`, already wired — it just never had anything but 0.0 writing to it before).
- [x] **Bug found and fixed along the way:** `do_cut`'s output path was landing at the filesystem root instead of the source's directory for a bare relative filename (`Path::parent()` returns `Some("")`, not `None`, for a path with no directory component — the old `unwrap_or(".")` fallback never triggered). Fixed via `PathBuf::join` instead of manual string formatting.
- [x] Confirmed working by the user.

## Phase 3 — crop tool (DONE 2026-09-19)

- [x] `crop.rs`: resizable/movable crop rect drawn directly over the video texture in egui (no native overlay window needed — GStreamer's frames render as an egui texture, not a native child window). Stored in source-pixel coords, converted to/from canvas coords via `to_canvas`/`canvas_to_source_delta`. 8 drag handles (N/S/E/W + 4 corners) via `hit_test`/`apply_drag`, plus drag-to-move from inside the rect.
- [x] `CROP` toggle button in the edit row (existing `cbtn` helper) — click toggles the editing overlay, right-click clears the crop entirely. A default centered 80% rect appears the first time crop mode is turned on.
- [x] Baked `-vf crop=w:h:x:y` into `do_cut` whenever a crop is set (independent of whether the overlay is currently shown — only clearing it removes the filter). GIF export (Phase 4) still needs the same treatment once that lands.
- [x] Confirmed working by the user (handles resize/move correctly, right-click clears, cropped CUT output dimensions match).

## Phase 4 — GIF export (DONE 2026-09-19)

Original plan was ffmpeg's two-pass `palettegen`/`paletteuse`; redirected mid-phase to the `gifski` crate (https://github.com/imageoptim/gifski) per explicit direction — better quality/byte via its perceptual quantizer, and it's a Rust library rather than a second ffmpeg shell-out. See `PLAN.md`'s "GIF export design" for the full pipeline.

- [x] `gif.rs` (new): `spawn_gif_export()` — ffmpeg extracts the IN→OUT range as raw RGBA frames (`-vf crop=...,scale=W:H:flags=lanczos,fps=10`, crop/scale/fps all in one pass, 640px-wide cap, from the **original source**, not the scrub proxy), fed to a `gifski::Collector` on one thread while a `gifski::Writer` writes the `.gif` on another (required — `gifski::new()`'s collector blocks once its queue fills until the writer is actively draining it).
- [x] Respects `self.crop_rect` (same `to_vf()` used by `do_cut`) and the current IN/OUT selection.
- [x] Progress reporting reuses `do_cut`'s existing `cut_progress_shared: Arc<Mutex<f32>>` bar (CUT and GIF export are mutually exclusive single actions, so no new UI/progress-bar code was needed) via a small `ProgressReporter` impl.
- [x] `GIF` button (already stubbed in the edit row) + `G` keyboard shortcut, both wired to `do_gif()`.
- [x] `cargo build --release` clean, `cargo clippy --release` clean (only pre-existing style lints elsewhere in the tree).
- [x] Verified via a throwaway `src/bin/gif_test.rs` (written, run, deleted — not committed) calling `spawn_gif_export` directly: no-crop export produced a valid 640×360 `GIF89a` file; an 800×600 crop produced a valid 640×480 file (aspect correctly preserved, confirming the crop→scale dimension math).
- [x] Update `FILE_INDEX.md`, `PLAN.md`.

## Phase 5 — NVENC/GPU encode (DONE 2026-09-19)

Note: `do_cut`'s final export shells out to `ffmpeg.exe`/`ffprobe.exe` on `PATH` — entirely separate from GStreamer (used only for playback decode). This phase is about the `PATH` ffmpeg's NVENC support.

- [x] `probe::nvenc_available()` — cached (`OnceLock<bool>`, shells out at most once per process), checks `ffmpeg -hide_banner -encoders` for `h264_nvenc`.
- [x] `probe::encode_args` picks `h264_nvenc`/`hevc_nvenc` (via `nvenc_extra`, a shared rate-control preset — VBR, `-cq 18`, spatial/temporal AQ, 32-frame lookahead) when NVENC is available, falling through to CPU `libx264`/`libx265` (`cpu_h26x_extra`) otherwise; vp9/vp8/av1/mpeg4/mpeg2video/prores are unaffected by NVENC either way (NVENC only covers h264/hevc). Mirrors the external Python reference's `_encode_args`/`_NVENC_OPTS`.
- [x] **Follow-up (same day): output bitrate now matches the source.** Originally both NVENC (`-cq 18 -b:v 0`) and CPU (`-crf 18`) paths were pure constant-quality — output size/bitrate could drift arbitrarily far from the source depending on content complexity. `source_video_bitrate_kbps()` reads the video stream's (or, failing that, the container's overall) `bit_rate` from ffprobe; `bitrate_args()` turns it into `-b:v <src>k -maxrate <src*1.5>k -bufsize <src*2>k` (VBV headroom around the target, not a hard cap) fed into both `nvenc_extra` (alongside `-cq 18`, NVENC's "capped quality" hybrid mode) and `cpu_h26x_extra` (replacing `-crf` outright — libx264/libx265 don't mix `-crf` with an explicit `-b:v` the way NVENC's CQ mode does). vp9/vp8/av1 keep their `-crf` but get the source bitrate as an `-b:v` soft cap too (libvpx's "constrained quality" mode) instead of the previous unconstrained `-b:v 0`. When the source bitrate can't be determined at all (rare — very old/exotic containers), all paths fall back to their previous quality-only behavior.
- [x] Verified via a throwaway `src/bin/probe_test.rs` (written, run, deleted — not committed): `sample.mp4`'s video stream reports `bit_rate=1893018` (1893kbps); `encode_args("sample.mp4")` correctly produced `-b:v 1893k -maxrate 2839k -bufsize 3786k` alongside `h264_nvenc`/`-cq 18`.
- [x] `cargo build --release` and `cargo clippy --release` both clean.
- [x] Update `FILE_INDEX.md`, `PLAN.md`.

## Phase 6 — polish: app icon, Open button, OS integration (DONE 2026-09-19)

- [x] `assets/icon.ico`/`assets/icon-256.png` (new, generated via a throwaway Pillow script, not part of the build): a dark rounded-square badge with a white play triangle and green/red IN/OUT brackets either side, echoing the app's own timeline marker colors (`C_IN`/`C_OUT` in `app.rs`).
- [x] Runtime window/taskbar icon: `main.rs` loads `assets/icon-256.png` via `eframe::icon_data::from_png_bytes` (already an eframe helper — no new dependency) and sets it via `ViewportBuilder::with_icon`.
- [x] `.exe`'s own icon (Explorer/taskbar-before-launch/context-menu default icon): `build.rs` (new) + `winres` build-dependency embed `assets/icon.ico` as a Windows PE resource.
- [x] `Open` button (`📂 Open`, edit row, next to `CROP`): native file picker via the `rfd` crate (new dependency — plain Win32 common dialog on Windows, no extra runtime deps), `open_video()` → `load_video()`. `load_video()` re-initializes everything `CutvApp::new()` does (probe, `Player`, scrub proxy, thumbnails, IN/OUT/crop/status reset, window title) in place, and explicitly deletes the *previous* proxy's temp file (only the proxy active at `Drop` time gets cleaned up automatically otherwise).
- [x] Drag & drop: dropping a video file anywhere on the window calls the same `load_video()` path as `Open` — no new dependency, egui/winit deliver dropped-file paths natively (`ctx.input(|i| i.raw.dropped_files)`, enabled by default on Windows). A full-window dark overlay with a green border and "Drop video to open" (`ctx.input(|i| i.raw.hovered_files)`, drawn on egui's `Order::Foreground` layer) gives live feedback while a file is being dragged over the window, before it's dropped.
- [x] `Cut with CUTV` Explorer context-menu entry: registered under `HKCU\Software\Classes\SystemFileAssociations\video\shell\...` first (PerceivedType-based, meant to cover every video extension in one place) — **didn't show up in the actual right-click menu** on this Windows 10 (build 19045) machine despite the registry being correctly merged into `HKCR` (confirmed via `reg query`/`Get-ChildItem`); the PerceivedType-level verb location is a known-unreliable one on some Windows 10 configurations (surfaces in "Open with" but not the main menu). Fixed by registering the same verb directly per-extension instead (`HKCU\...\SystemFileAssociations\.mp4\shell\CutWithCUTV`, `.mov`, `.avi`, `.mkv`, `.webm`) — confirmed visually by the user after an Explorer restart (`Stop-Process -Name explorer -Force` + relaunch, needed to pick up the new registry keys). Outside the repo, not tracked by git.
- [x] **Follow-up: suppress the console window on a standalone launch.** Once the context-menu entry worked, running it that way (or double-clicking, or via drag-a-file-onto-the-exe) popped up a console window alongside the app — the default for a Rust binary (`console` subsystem). Fixed with `#![windows_subsystem = "windows"]` in `main.rs`. This only changes behavior when there's no console to attach to; launching via `cargo run`/`cutv.bat` from an already-open terminal still inherits that terminal's stdout/stderr, so `RUST_LOG`/`debug!` logging during development is unaffected — verified by re-running via `cargo run --release` after the change and confirming log output still appeared.
- [x] **Explorer integration (outside the repo, not tracked by git):** `HKCU\Software\Classes\SystemFileAssociations\video\shell\CutWithCUTV` registered (no admin needed, user-scoped, reversible) — adds "Cut with CUTV" to the right-click context menu for any file Windows already classifies as a video (covers mp4/mov/avi/mkv/webm by default). `C:\cli_tools\scripts\cutv.bat` (the `cutv` command on `PATH`, outside the repo) repointed from the Python reference (`cutv.py`) to this Rust build's release exe — the Rust version now has full feature parity-plus (GIF export, crop, NVENC, bitrate matching) and is the one meant to be used day to day.
- [x] `cargo build --release` and `cargo clippy --release` both clean; app launched and verified (both via `cargo run --release` and via the repointed `cutv.bat`) to load `sample.mp4` correctly with the new icon/button in place.
- [x] Update `FILE_INDEX.md`, `PLAN.md`.

## Phase 7 — scrub proxy: content-hash caching + exit-time cleanup (DONE 2026-09-19)

- [x] `proxy.rs`: `video_cache_key()` hashes file length + up to 1MiB sampled from each end (bounded I/O, not a full-file hash) to name proxies `cutv_proxy_<key>.mp4` instead of `cutv_proxy_<pid>.mp4` — same video content reuses an existing proxy regardless of filename/path, including one left by a crashed previous session.
- [x] `spawn_proxy()` checks for an existing proxy file before transcoding (cache hit → reused instantly, no ffmpeg run at all); transcodes to a `.mp4.tmp` sibling and `rename()`s into place only on success, so an interrupted transcode never leaves a corrupt file mistaken for a valid cache entry. Needed an explicit `-f mp4` on the ffmpeg command — the `.tmp` extension broke ffmpeg's container-format auto-detection (found and fixed during manual verification below).
- [x] `load_video()` no longer deletes the outgoing video's proxy on switch — left on disk for reuse. `cleanup_all_proxies()` (new) sweeps every `cutv_proxy_*` file in the temp dir; wired into `CutvApp`'s `Drop`, replacing the old single-file removal there.
- [x] Manually verified end-to-end: transcoded a fresh proxy for `sample.mp4`, `taskkill`'d the process (simulating a crash — skips `Drop`) and confirmed the proxy file survived; relaunched against the same file and confirmed an instant cache hit (`reusing cached proxy for ...`, no transcode) instead of a re-transcode.
- [x] `cargo build --release` and `cargo clippy --release` clean (no new lints in `proxy.rs`).
- [x] Update `FILE_INDEX.md`, `PLAN.md`.

## Deferred (not scheduled)

- yt-dlp URL download support.
