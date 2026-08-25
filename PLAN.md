# PLAN.md

## Tech stack

- Rust 2024 edition.
- UI: `eframe`/`egui` 0.29 — immediate-mode GUI, custom-painted (no OS-native widgets).
- Playback (current): raw-RGB24 frames piped from `ffmpeg` subprocess + `rodio` for audio, kept in sync manually via shared timestamps. Requires an H.264 proxy transcode of the source for responsive scrubbing.
- Playback (target): embedded **libmpv** via the `libmpv2` crate (maintained fork of the abandoned `libmpv-rs`). mpv owns decode, GPU render, audio output, and clocking as one unit — no proxy, no manual A/V sync.
- Export/encode/probe: shells out to `ffmpeg`/`ffprobe` (must be on `PATH`, not vendored).
- No test suite.

## Architecture decision: libmpv migration

Decided (see conversation 2026-08-25): replace `video.rs` (`VideoDecoder`/`SeekWorker`/`spawn_proxy`) and `audio.rs` with a single `player.rs` wrapping libmpv.

Key technical risk: eframe/winit has no native child-HWND API like Tk's `wid`. mpv needs a raw window handle to render into. Plan: get the raw HWND of the eframe window via `raw-window-handle`, create a native Win32 child window sized/positioned to the video rect each frame, hand that HWND to mpv as `wid`. **Spike this in isolation before ripping out `video.rs`/`audio.rs`** — if it doesn't work cleanly, fall back to keeping the current ffmpeg-pipe pipeline and layer new features on top of it instead.

Consequence for crop UI: mpv paints directly into its own native child window, so egui can't draw a crop rubber-band *on top of* the video texture (there is no texture anymore). Needs a separate topmost native overlay window positioned over mpv's child window (mirrors the Python reference's color-keyed Tk `Toplevel`, or on Windows a layered window with `SetLayeredWindowAttributes` for real per-pixel alpha instead of color-keying).

## Features to add (all approved, in priority order)

1. **Cut progress bar** — parse `ffmpeg -progress pipe:1` output during `do_cut`'s export (currently a blocking `.output()` call; needs to become `Stdio::piped()` + a stdout line reader in the worker thread, draining stderr in parallel to avoid pipe deadlock). Thin egui `Rect` fill in the status bar, same idea as reference's `_set_progress`.
2. **Crop tool** — rubber-band selection over the video (native overlay window per above), stored as `(x, y, w, h)` in source-pixel coordinates, baked into `do_cut`'s ffmpeg args as `-vf crop=...`. Also applies to GIF export.
3. **GIF export** — two-pass ffmpeg (palette generation via `palettegen`, then `paletteuse`) of the IN→OUT selection, mirrors reference's `_do_gif`.
4. **NVENC/GPU encode** — detect `h264_nvenc`/`hevc_nvenc` availability (`ffmpeg -hide_banner -encoders`, cached), branch `probe::encode_args` to prefer GPU encoders with `-hwaccel cuda` on the input side when available, fall back to libx264/libx265 otherwise.

## Explicitly deferred

- yt-dlp URL download support — lowest priority, separate concern (download + cache dir), not part of this phase.

## Module plan post-migration

- `player.rs` (new) — `Player::new(path)`, `play()`, `pause()`, `seek(t)`, `step_frames(n)`, `set_mute(bool)`; property-observer callbacks pushed into a channel drained each `update()` tick (same pattern as the current decoder/seek channels, one source instead of two).
- `probe.rs` — add `nvenc_available() -> bool` (cached), extend `encode_args` with the NVENC branch.
- `app.rs` — `do_cut` gains `-hwaccel cuda` + crop `-vf` injection + progress parsing; new `CROP`/`GIF` buttons in the edit row (existing `cbtn`/`tbtn` helpers).
- `crop.rs` (new) — crop state, canvas↔source coordinate mapping, overlay window.
- `gif.rs` (new) — two-pass GIF export worker.
- `video.rs`, `audio.rs` — removed once `player.rs` is proven.
