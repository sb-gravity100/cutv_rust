# FILE_INDEX.md

File system index with tags and one-line descriptions. Update whenever files are added, moved, or removed.

## Root

- `Cargo.toml` — `[build]` crate manifest, dependencies (eframe/egui, gstreamer/gstreamer-app/gstreamer-video, gifski, chrono, anyhow, log, env_logger, serde_json).
- `Cargo.lock` — `[build]` pinned dependency versions.
- `.cargo/config.toml` — `[build]` sets `PKG_CONFIG_PATH`/`PKG_CONFIG` so `gstreamer-rs`'s sys crates find vcpkg's GStreamer automatically. See `PLAN.md` for the vcpkg setup this depends on (and the `GST_PLUGIN_PATH`/`PATH` runtime env vars, set separately as persistent user env vars, not via this file).
- `vcpkg-overlay/gstreamer/` — `[build]` vcpkg overlay port: copy of upstream `gstreamer` with `-Dorc=enabled` + pre-vendors ORC 0.4.42 into `subprojects/orc` (upstream disables ORC — GStreamer's SIMD codegen lib — because vcpkg has no port for it under that name; see PLAN.md's "low-FPS investigation"). Passed via `vcpkg install --overlay-ports=vcpkg-overlay ...`.
- `CLAUDE.md` — `[docs]` session rules + project architecture guidance for Claude Code.
- `.gitignore` — `[build]` ignores `/target`.

## `src/`

- `main.rs` — `[entry]` resolves target video path, probes it, sizes the window, launches `eframe` with `app::CutvApp`.
- `app.rs` — `[ui][state]` `CutvApp` (`eframe::App` impl) — owns all playback/UI state, egui drawing (video frame, timeline, transport, edit row, status bar, FPS counter, crop overlay), keyboard shortcuts, `do_cut()`/`do_gif()` export (both bake in `-vf crop=...` whenever a crop is set, and share the `cut_progress_shared` progress-bar field since only one export runs at a time). Constructs `Player` directly (no native window handle needed — frames render as an egui texture).
- `crop.rs` — `[feature]` resizable/movable crop rectangle (`CropRect`, source-pixel coords) drawn directly over the video texture in egui; `to_canvas`/`canvas_to_source_delta` for coordinate mapping, `hit_test`/`apply_drag` for the 8 resize handles + move-by-drag-inside, `to_vf()` for the ffmpeg `-vf crop=...` string. `app.rs`'s `ui_crop_overlay` drives it.
- `gif.rs` — `[feature]` `spawn_gif_export` — GIF export via `gifski`: ffmpeg extracts the IN→OUT range from the **original source** as raw RGBA frames (`-vf crop=...,scale=...,fps=10`, 640px-wide cap), fed to a `gifski::Collector`/`Writer` pair on two threads (required by gifski's own blocking-queue contract). Respects `crop_rect` and reports progress through the same `Arc<Mutex<f32>>` `do_cut` uses. See `PLAN.md`'s "GIF export design".
- `player.rs` — `[playback]` `Player` — wraps a GStreamer `playbin` pipeline (`play`/`pause`/`seek`/`step_frames`/`set_mute`/`poll`/`is_seeking`). Every seek is frame-exact (`SeekFlags::ACCURATE`) — no `seek_fast`/keyframe-snap variant; see PLAN.md's "The scrub proxy" for why that tradeoff isn't needed. Video frames pulled from an `appsink` (via a GPU `d3d11upload!d3d11convert!...` colorspace-conversion bin, falling back to CPU `videoconvert`) as raw RGB for the caller to upload as a texture — `poll()` tries both `try_pull_preroll()` and `try_pull_sample()` since appsink delivers frames differently depending on pipeline state (paused vs playing). Audio is playbin's own default sink — no separate audio code. Has detailed `trace!`/`debug!` timing instrumentation (see PLAN.md's "low-FPS investigation") — `RUST_LOG=trace` for per-frame numbers.
- `proxy.rs` — `[playback]` `spawn_proxy` — background-thread ffmpeg transcode of the source into a short-GOP (every 8th frame a keyframe) scrub proxy, NVENC first with a libx264 `ultrafast` fallback. `app.rs` swaps `Player` over to it once ready, preserving position/play state; `do_cut()` always uses the original file regardless. See PLAN.md's "The scrub proxy".
- `thumbs.rs` — `[playback][thumbnails]` `spawn_thumbs` — background-thread ffmpeg single-frame grabs for the timeline thumbnail strip (N=24), independent of `Player`.
- `probe.rs` — `[ffmpeg][metadata]` `probe_video` (ffprobe → duration/fps/width/height), `encode_args` (source-codec-aware ffmpeg encoder args for the final cut — NVENC `h264_nvenc`/`hevc_nvenc` when `nvenc_available()` (cached) says so, CPU libx264/libx265/etc. otherwise).
- `util.rs` — `[helpers]` `fmt_tc` (timecode formatting), `parse_fps` (ffprobe `"num/den"` parser).

- `bin/mpv_spike.rs` — `[spike][reference, no longer the playback direction]` Phase 0 proof that libmpv can render into a native Win32 child window embedded in an app window. Not wired into `main.rs`. Kept only as a reference for the win32-embedding/runtime-DLL-loading technique, in case it's ever needed again — playback is GStreamer now, not mpv.

## Removed (architecture history — see PLAN.md's "Architecture decision")

- `video.rs`, `audio.rs` (ffmpeg-pipe era) — removed when the mpv migration landed.
- `video.rs`, `audio.rs` (ffmpeg-next decoder era, different content, same filenames) — removed when the GStreamer migration landed 2026-09-19.

## External reference (not part of this repo/build)

- `C:\cli_tools\scripts\py\cutv\cutv.py` — Python/Tkinter+libmpv reference implementation with the full feature set (crop, GIF export, NVENC, yt-dlp URL support, progress bar). Source of design patterns for the ports listed above.
