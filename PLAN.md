# PLAN.md

## Tech stack

- Rust 2024 edition.
- UI: `eframe`/`egui` 0.29 — immediate-mode GUI, custom-painted (no OS-native widgets).
- Playback: embedded **libmpv**, loaded at runtime (`LoadLibraryW`/`GetProcAddress` — not the `libmpv2`/`libmpv-rs` crates, which need an MSVC import lib that doesn't exist for the available DLL). mpv owns decode, GPU render, audio output, and clocking as one unit — no proxy, no manual A/V sync. See `src/player.rs`.
- Export/encode/probe: shells out to `ffmpeg`/`ffprobe` (must be on `PATH`, not vendored).
- No test suite.

## Architecture decision: libmpv migration

Decided (see conversation 2026-08-25): replace `video.rs` (`VideoDecoder`/`SeekWorker`/`spawn_proxy`) and `audio.rs` with a single `player.rs` wrapping libmpv. **Done 2026-09-19** — `video.rs`/`audio.rs` removed, `player.rs` + `thumbs.rs` in place, `app.rs`/`main.rs` wired up, builds clean. Not yet run against a real video (see `PHASES.md` Phase 1).

**Why this became urgent, not just planned:** the ffmpeg-pipe pipeline hit two complaints that directly trade off against each other and can't both be fixed by tuning it further — sharper decode resolution makes timeline scrubbing slower (each scrub position spawns a whole new `ffmpeg` process), and faster scrubbing needs lower resolution. There's no tuning path to "Premiere-level" scrub smoothness with a process-per-seek model; it requires a player that keeps a live decoder + demuxer cache open, which is what mpv is.

**Phase 0 spike (done 2026-08-25):** see `PHASES.md`. Confirmed mpv can render into a native Win32 child window embedded inside an app window, with correct resize behavior, using **runtime-loaded libmpv** (`LoadLibraryW`/`GetProcAddress`, not link-time linking — no MSVC import lib exists for the available `libmpv-2.dll`). `src/bin/mpv_spike.rs` has the proof.

**Phase 1 (done 2026-09-19, pending runtime verification):** `player.rs` resolves the real eframe window's HWND via `raw-window-handle`'s `frame.window_handle()` (the risk carried over from Phase 0 — the spike used a hand-rolled stand-in window) and embeds mpv's child window into it. Whether it actually coexists cleanly with eframe/glutin's GL surface/repaint loop at runtime is still unconfirmed — only `cargo check` has passed so far.

**Open question:** how to distribute `libmpv-2.dll` with the real app (bundle it, require a local mpv/libmpv install, or a configurable path). Still not decided — `player.rs`'s `dll_path()` currently falls back to the external reference project's copy (`C:\cli_tools\scripts\py\cutv\libmpv-2.dll`), overridable via `CUTV_LIBMPV_PATH`. Revisit before shipping.

Consequence for crop UI: mpv paints directly into its own native child window, so egui can't draw a crop rubber-band *on top of* the video texture (there is no texture anymore). Needs a separate topmost native overlay window positioned over mpv's child window (mirrors the Python reference's color-keyed Tk `Toplevel`, or on Windows a layered window with `SetLayeredWindowAttributes` for real per-pixel alpha instead of color-keying).

## Features to add (all approved, in priority order)

1. **Cut progress bar** — parse `ffmpeg -progress pipe:1` output during `do_cut`'s export (currently a blocking `.output()` call; needs to become `Stdio::piped()` + a stdout line reader in the worker thread, draining stderr in parallel to avoid pipe deadlock). Thin egui `Rect` fill in the status bar, same idea as reference's `_set_progress`.
2. **Crop tool** — rubber-band selection over the video (native overlay window per above), stored as `(x, y, w, h)` in source-pixel coordinates, baked into `do_cut`'s ffmpeg args as `-vf crop=...`. Also applies to GIF export.
3. **GIF export** — two-pass ffmpeg (palette generation via `palettegen`, then `paletteuse`) of the IN→OUT selection, mirrors reference's `_do_gif`.
4. **NVENC/GPU encode** — detect `h264_nvenc`/`hevc_nvenc` availability (`ffmpeg -hide_banner -encoders`, cached), branch `probe::encode_args` to prefer GPU encoders with `-hwaccel cuda` on the input side when available, fall back to libx264/libx265 otherwise.

## Explicitly deferred

- yt-dlp URL download support — lowest priority, separate concern (download + cache dir), not part of this phase.

## Module plan for remaining phases

- `probe.rs` — add `nvenc_available() -> bool` (cached), extend `encode_args` with the NVENC branch.
- `app.rs` — `do_cut` gains `-hwaccel cuda` + crop `-vf` injection + progress parsing; new `CROP`/`GIF` buttons already stubbed in the edit row (existing `cbtn`/`tbtn` helpers) — `toggle_crop_mode`/`do_gif` just need real implementations.
- `crop.rs` (new) — crop state, canvas↔source coordinate mapping, overlay window.
- `gif.rs` (new) — two-pass GIF export worker.
