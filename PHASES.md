# PHASES.md

Per-phase task lists and done criteria. Check off as completed; keep in sync with `PLAN.md`/`FILE_INDEX.md`.

## Phase 0 — libmpv embedding spike (blocking, do first)

Goal: prove mpv can render into a child window inside the eframe window before committing to ripping out `video.rs`/`audio.rs`.

- [ ] Add `libmpv2` dependency, confirm it builds/links against a local libmpv on this machine.
- [ ] Get the raw HWND of the eframe window via `raw-window-handle`.
- [ ] Create a native Win32 child window sized/positioned to the video rect, hand its HWND to mpv as `wid`.
- [ ] Confirm playback renders correctly inside the eframe window, resizes/repositions correctly as the child window moves, and doesn't fight egui's own repaint loop.
- **Done when:** a throwaway build shows a video playing embedded in the app window. If this doesn't work cleanly, stop and fall back to keeping the current ffmpeg-pipe pipeline (`video.rs`/`audio.rs`) and layer the Phase 2+ features on top of it instead.

## Phase 1 — player.rs migration (depends on Phase 0)

- [ ] Write `player.rs`: `Player::new(path)`, `play()`, `pause()`, `seek(t)`, `step_frames(n)`, `set_mute(bool)`, property-observer channel for position/duration/pause state.
- [ ] Wire `app.rs` to `player.rs` instead of `video.rs`/`audio.rs` (playback controls, timeline scrubbing, thumbnails — thumbnails can stay ffmpeg-based single-frame extraction, independent of the player).
- [ ] Remove `video.rs`, `audio.rs`.
- [ ] Update `FILE_INDEX.md`, `PLAN.md`.

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
