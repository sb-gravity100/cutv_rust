# FILE_INDEX.md

File system index with tags and one-line descriptions. Update whenever files are added, moved, or removed.

## Root

- `Cargo.toml` — `[build]` crate manifest, dependencies (eframe/egui, rodio, chrono, anyhow, log, env_logger, serde_json, ffmpeg-next).
- `Cargo.lock` — `[build]` pinned dependency versions.
- `.cargo/config.toml` — `[build]` sets `VCPKG_ROOT` so `ffmpeg-sys-next`'s build script finds vcpkg's FFmpeg automatically. See `PLAN.md` for the vcpkg/LLVM setup this depends on.
- `CLAUDE.md` — `[docs]` session rules + project architecture guidance for Claude Code.
- `.gitignore` — `[build]` ignores `/target`.

## `src/`

- `main.rs` — `[entry]` resolves target video path, probes it, sizes the window, launches `eframe` with `app::CutvApp`.
- `app.rs` — `[ui][state]` `CutvApp` (`eframe::App` impl) — owns all playback/UI state, egui drawing (video frame, timeline, transport, edit row, status bar), keyboard shortcuts, `do_cut()` export. Lazily creates `Player` once the real window HWND is resolvable via `raw-window-handle`.
- `player.rs` — `[playback]` **being replaced** — currently `Player` embeds libmpv into a native Win32 child window (runtime-loaded via `LoadLibraryW`/`GetProcAddress`, no MSVC import lib for the available DLL). Superseded 2026-09-19 (see `PLAN.md`'s "Architecture decision") — being rewritten as a persistent in-process decoder via `ffmpeg-next` instead, to drop mpv's async-command bug class. See `PHASES.md` Phase 1b.
- `thumbs.rs` — `[playback][thumbnails]` `spawn_thumbs` — background-thread ffmpeg single-frame grabs for the timeline thumbnail strip (N=24), independent of `Player`.
- `probe.rs` — `[ffmpeg][metadata]` `probe_video` (ffprobe → duration/fps/width/height), `encode_args` (source-codec-aware ffmpeg encoder args for the final cut, CPU only today).
- `util.rs` — `[helpers]` `fmt_tc` (timecode formatting), `parse_fps` (ffprobe `"num/den"` parser).

- `bin/mpv_spike.rs` — `[spike][reference]` Phase 0 proof that libmpv can render into a native Win32 child window embedded in an app window. Not wired into `main.rs`; run directly via `cargo run --bin mpv_spike -- <libmpv-2.dll path> <video path>`. Kept as a reference for the runtime-loading technique `player.rs` uses.

## Planned / not yet created

- `crop.rs` — `[feature]` crop rubber-band state + coordinate mapping + overlay window.
- `gif.rs` — `[feature]` two-pass GIF export (palette generation + paletteuse).

## External reference (not part of this repo/build)

- `C:\cli_tools\scripts\py\cutv\cutv.py` — Python/Tkinter+libmpv reference implementation with the full feature set (crop, GIF export, NVENC, yt-dlp URL support, progress bar). Source of design patterns for the ports listed above.
