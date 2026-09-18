# PLAN.md

## Tech stack

- Rust 2024 edition.
- UI: `eframe`/`egui` 0.29 — immediate-mode GUI, custom-painted (no OS-native widgets).
- Playback: **in-process FFmpeg decode via the `ffmpeg-next` crate** (bindings to libavcodec/libavformat/etc.), replacing the earlier libmpv approach — see "Architecture decision" below. Frames render as an egui texture (like the pre-mpv `video.rs` did), driven by a persistent decoder thread instead of a process spawned per seek.
- Export/encode/probe: shells out to `ffmpeg`/`ffprobe` (must be on `PATH`, not vendored) for `do_cut`'s final export and `probe.rs`. Playback decode is separate (in-process via `ffmpeg-next`, not a subprocess).
- **Build-time dependency (new):** `ffmpeg-next`/`ffmpeg-sys-next` link against FFmpeg's C libraries via `bindgen`, which needs FFmpeg dev libs/headers + clang/LLVM on the machine — not just `ffmpeg.exe`/`ffprobe.exe` on `PATH`. Set up on this machine 2026-09-19 via vcpkg:
  ```
  C:\vcpkg\bootstrap-vcpkg.bat
  C:\vcpkg\vcpkg.exe install "ffmpeg[nvcodec,avcodec,avformat,avfilter,swscale,swresample]:x64-windows"
  ```
  built with `--enable-cuda --enable-nvenc --enable-nvdec --enable-cuvid --enable-ffnvcodec --enable-d3d11va --enable-dxva2 --enable-mediafoundation` (hardware decode/encode support, ~12 min build). Also needs LLVM/clang (https://github.com/llvm/llvm-project/releases) for `bindgen`. `.cargo/config.toml` sets `VCPKG_ROOT=C:/vcpkg` so `cargo build` finds it automatically via the `vcpkg` crate — no manual env vars needed once vcpkg + clang are installed. This is a real jump in dev-machine setup versus the project's original "just needs ffmpeg on PATH" story; document for anyone else setting up this repo.
- No test suite.

## Architecture decision: away from libmpv, to a Rust-native persistent decoder

**Superseded 2026-09-19** (see conversation) — dropped the libmpv approach after runtime testing surfaced a cluster of bugs (autoplay-on-load, Play/Pause button flicker, slow/backlogged backward frame-stepping, audio blips during stepping) that all traced back to the same root cause: mpv's command interface is async and fire-and-forget, so the app is constantly guessing whether a command has "landed" yet, with no real completion signal short of setting up mpv's full event loop. Rather than keep fighting that, decided to write our own persistent decoder in Rust via `ffmpeg-next`, decoding synchronously in a background thread we fully control (no async command queue to guess about), rendering to an egui texture instead of an embedded native child window (avoids the win32 child-window/z-order complexity entirely, and lets egui draw crop UI directly over the video again — see below).

Old libmpv work (kept in `src/player.rs` until replaced, see `PHASES.md`): `Player` embedded libmpv into a native Win32 child window via `LoadLibraryW`/`GetProcAddress` runtime loading (no MSVC import lib existed for the available `libmpv-2.dll`). It worked — Phase 0/1 verified mpv coexists fine with eframe/glutin's GL surface — but every subsequent bug fix was a game of whack-a-mole with mpv's async command model. `src/bin/mpv_spike.rs` remains as a reference for the win32-child-window-embedding technique in case it's ever needed again, but is no longer the direction for playback.

**Why the original ffmpeg-pipe approach (`video.rs`/`audio.rs`, removed in the mpv migration) still isn't right either:** it spawned a brand new `ffmpeg` subprocess per seek, so scrubbing smoothness and decode resolution directly traded off against each other with no way to get both — no tuning path to "Premiere-level" scrub smoothness with a process-per-seek model. The new approach solves this the same way mpv did (a persistent decoder + demuxer cache kept warm across seeks), just via in-process `ffmpeg-next` calls instead of both an external always-running process (mpv) or a per-seek process spawn.

**Status:** build toolchain confirmed working 2026-09-19 (`cargo build` links `ffmpeg-next` successfully against vcpkg's FFmpeg). Actual persistent-decoder `Player`/`Decoder` implementation not yet written — next step.

**Hardware decode/encode:** requested alongside this pivot. The vcpkg FFmpeg build above already has NVDEC/NVENC/CUVID/D3D11VA/DXVA2 enabled, so the new decoder can use `ffmpeg-next`'s hardware device context APIs for decode (avoiding CPU decode entirely where available), and Phase 5's planned NVENC export work (see "Features to add" below) has everything it needs already built in.

Consequence for crop UI: rendering frames as an egui texture again (rather than mpv's native child window) means the crop rubber-band can go back to being drawn directly over the video texture in egui, same as the pre-mpv approach — no separate native overlay window needed. This simplifies Phase 3 (crop tool) versus what the libmpv-era `PLAN.md` had originally planned.

## Features to add (all approved, in priority order)

1. **Cut progress bar** — parse `ffmpeg -progress pipe:1` output during `do_cut`'s export (currently a blocking `.output()` call; needs to become `Stdio::piped()` + a stdout line reader in the worker thread, draining stderr in parallel to avoid pipe deadlock). Thin egui `Rect` fill in the status bar, same idea as reference's `_set_progress`.
2. **Crop tool** — rubber-band selection over the video (native overlay window per above), stored as `(x, y, w, h)` in source-pixel coordinates, baked into `do_cut`'s ffmpeg args as `-vf crop=...`. Also applies to GIF export.
3. **GIF export** — two-pass ffmpeg (palette generation via `palettegen`, then `paletteuse`) of the IN→OUT selection, mirrors reference's `_do_gif`.
4. **NVENC/GPU encode** — detect `h264_nvenc`/`hevc_nvenc` availability (`ffmpeg -hide_banner -encoders`, cached), branch `probe::encode_args` to prefer GPU encoders with `-hwaccel cuda` on the input side when available, fall back to libx264/libx265 otherwise.

## Explicitly deferred

- yt-dlp URL download support — lowest priority, separate concern (download + cache dir), not part of this phase.

## Module plan for remaining phases

- `player.rs` — being rewritten: persistent decode thread via `ffmpeg-next` (open file once, keep decoder/demuxer state warm across seeks, decode frames to an egui texture) replacing the libmpv child-window approach. Same public shape as before (`play`/`pause`/`seek`/`step_frames`/`set_mute`/`poll`) so `app.rs`'s call sites mostly carry over; `set_rect`/native-window plumbing goes away since there's no child window anymore.
- `probe.rs` — add `nvenc_available() -> bool` (cached), extend `encode_args` with the NVENC branch.
- `app.rs` — `do_cut` gains `-hwaccel cuda` + crop `-vf` injection + progress parsing; new `CROP`/`GIF` buttons already stubbed in the edit row (existing `cbtn`/`tbtn` helpers) — `toggle_crop_mode`/`do_gif` just need real implementations. `ui_video` goes back to drawing a texture (like pre-mpv `video.rs`) instead of positioning a native child window.
- `crop.rs` (new) — crop state, canvas↔source coordinate mapping; draws directly over the video texture in egui (no overlay window needed now — see architecture decision above).
- `gif.rs` (new) — two-pass GIF export worker.
