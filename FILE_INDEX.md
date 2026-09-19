# FILE_INDEX.md

File system index with tags and one-line descriptions. Update whenever files are added, moved, or removed.

## Root

- `Cargo.toml` — `[build]` crate manifest, dependencies (eframe/egui, gstreamer/gstreamer-app/gstreamer-video, chrono, anyhow, log, env_logger, serde_json).
- `Cargo.lock` — `[build]` pinned dependency versions.
- `.cargo/config.toml` — `[build]` sets `PKG_CONFIG_PATH`/`PKG_CONFIG` so `gstreamer-rs`'s sys crates find vcpkg's GStreamer automatically. See `PLAN.md` for the vcpkg setup this depends on (and the `GST_PLUGIN_PATH`/`PATH` runtime env vars, set separately as persistent user env vars, not via this file).
- `vcpkg-overlay/gstreamer/` — `[build]` vcpkg overlay port: copy of upstream `gstreamer` with `-Dorc=enabled` + pre-vendors ORC 0.4.42 into `subprojects/orc` (upstream disables ORC — GStreamer's SIMD codegen lib — because vcpkg has no port for it under that name; see PLAN.md's "low-FPS investigation"). Passed via `vcpkg install --overlay-ports=vcpkg-overlay ...`.
- `CLAUDE.md` — `[docs]` session rules + project architecture guidance for Claude Code.
- `.gitignore` — `[build]` ignores `/target`.

## `src/`

- `main.rs` — `[entry]` resolves target video path, probes it, sizes the window, launches `eframe` with `app::CutvApp`.
- `app.rs` — `[ui][state]` `CutvApp` (`eframe::App` impl) — owns all playback/UI state, egui drawing (video frame, timeline, transport, edit row, status bar, FPS counter), keyboard shortcuts, `do_cut()` export. Constructs `Player` directly (no native window handle needed — frames render as an egui texture).
- `player.rs` — `[playback]` `Player` — wraps a GStreamer `playbin` pipeline (`play`/`pause`/`seek`/`seek_fast`/`step_frames`/`set_mute`/`poll`). Video frames pulled from an `appsink` (via a GPU `d3d11upload!d3d11convert!...` colorspace-conversion bin, falling back to CPU `videoconvert`) as raw RGB for the caller to upload as a texture. Audio is playbin's own default sink — no separate audio code. Has detailed `trace!`/`debug!` timing instrumentation (see PLAN.md's "low-FPS investigation") — `RUST_LOG=trace` for per-frame numbers.
- `thumbs.rs` — `[playback][thumbnails]` `spawn_thumbs` — background-thread ffmpeg single-frame grabs for the timeline thumbnail strip (N=24), independent of `Player`.
- `probe.rs` — `[ffmpeg][metadata]` `probe_video` (ffprobe → duration/fps/width/height), `encode_args` (source-codec-aware ffmpeg encoder args for the final cut, CPU only today).
- `util.rs` — `[helpers]` `fmt_tc` (timecode formatting), `parse_fps` (ffprobe `"num/den"` parser).

- `bin/mpv_spike.rs` — `[spike][reference, no longer the playback direction]` Phase 0 proof that libmpv can render into a native Win32 child window embedded in an app window. Not wired into `main.rs`. Kept only as a reference for the win32-embedding/runtime-DLL-loading technique, in case it's ever needed again — playback is GStreamer now, not mpv.

## Planned / not yet created

- `crop.rs` — `[feature]` crop rubber-band state + coordinate mapping; draws directly over the video texture in egui (no overlay window needed — see PLAN.md).
- `gif.rs` — `[feature]` two-pass GIF export (palette generation + paletteuse).

## Removed (architecture history — see PLAN.md's "Architecture decision")

- `video.rs`, `audio.rs` (ffmpeg-pipe era) — removed when the mpv migration landed.
- `video.rs`, `audio.rs` (ffmpeg-next decoder era, different content, same filenames) — removed when the GStreamer migration landed 2026-09-19.

## External reference (not part of this repo/build)

- `C:\cli_tools\scripts\py\cutv\cutv.py` — Python/Tkinter+libmpv reference implementation with the full feature set (crop, GIF export, NVENC, yt-dlp URL support, progress bar). Source of design patterns for the ports listed above.
