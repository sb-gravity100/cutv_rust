# FILE_INDEX.md

File system index with tags and one-line descriptions. Update whenever files are added, moved, or removed.

## Root

- `Cargo.toml` — `[build]` crate manifest, dependencies (eframe/egui, rodio, chrono, anyhow, log, env_logger, serde_json).
- `Cargo.lock` — `[build]` pinned dependency versions.
- `CLAUDE.md` — `[docs]` session rules + project architecture guidance for Claude Code.
- `.gitignore` — `[build]` ignores `/target`.

## `src/`

- `main.rs` — `[entry]` resolves target video path, probes it, sizes the window, launches `eframe` with `app::CutvApp`.
- `app.rs` — `[ui][state]` `CutvApp` (`eframe::App` impl) — owns all playback/UI state, egui drawing (video frame, timeline, transport, edit row, status bar), keyboard shortcuts, `do_cut()` export.
- `video.rs` — `[playback][decode]` **being replaced** — `VideoDecoder` (ffmpeg raw-RGB24 pipe playback), `SeekWorker` (single-frame seek), `spawn_thumbs` (thumbnail strip), `spawn_proxy` (display-resolution H.264 proxy transcode). Slated for removal once `player.rs` (libmpv) lands.
- `audio.rs` — `[playback][audio]` **being replaced** — `AudioPlayer`: extracts full audio track to temp MP3 via ffmpeg, plays via `rodio`, manually kept in sync with video by shared start-timestamp. Slated for removal once `player.rs` (libmpv) lands — mpv owns audio natively.
- `probe.rs` — `[ffmpeg][metadata]` `probe_video` (ffprobe → duration/fps/width/height), `encode_args` (source-codec-aware ffmpeg encoder args for the final cut, CPU only today).
- `util.rs` — `[helpers]` `fmt_tc` (timecode formatting), `parse_fps` (ffprobe `"num/den"` parser).

## Planned / not yet created

- `player.rs` — `[playback]` libmpv wrapper replacing `video.rs` + `audio.rs`; embeds mpv into a native child window inside the eframe window.
- `crop.rs` — `[feature]` crop rubber-band state + coordinate mapping + overlay window.
- `gif.rs` — `[feature]` two-pass GIF export (palette generation + paletteuse).

## External reference (not part of this repo/build)

- `C:\cli_tools\scripts\py\cutv\cutv.py` — Python/Tkinter+libmpv reference implementation with the full feature set (crop, GIF export, NVENC, yt-dlp URL support, progress bar). Source of design patterns for the ports listed above.
