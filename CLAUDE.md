# _CLAUDE.md — General Session Rules

General-purpose session rules, independent of any specific project or tech stack. Copy this into any project's `CLAUDE.md` as a base.

---

## ⚡ START EVERY SESSION HERE

Before writing any code or reading any source file, always do this first:

1. **Ask the user to provide `PLAN.md`, `PHASES.md`, and `FILE_INDEX.md`** if they are not already in context. Do not proceed until at least `FILE_INDEX.md` is available.
2. Read `FILE_INDEX.md` (repo root) — it maps every file to its purpose and tags. Use it to find the exact file you need without scanning the tree.
3. Read `CLAUDE.md` in full.

**Do not scan directories or read source files until you have identified the target files via `FILE_INDEX.md`.**

---

## Token efficiency — CRITICAL

Every file read and every output costs money. Minimize both aggressively.

- **Read only what you need.** Use `FILE_INDEX.md` to identify the exact file before opening anything. Never open a file speculatively.
- **Read only the relevant section.** Use `view_range` when you need one function or block, not the whole file.
- **No unnecessary confirmations.** Don't summarize what you're about to do before doing it. Don't recap what you just did after doing it. Act, then move on.
- **No padding.** No filler phrases ("Great question!", "Sure, I'll help with that", "Here's what I did"). Responses should contain only information the user needs.
- **Prefer targeted edits.** Use `str_replace` on the specific lines that change. Never rewrite a whole file to change a few lines.
- **One read per file per task.** Read a file once, make all needed changes, move on. Don't re-read files you already have in context.

---

## Session saves — CRITICAL

**The user's PC crashes unpredictably, anywhere between 10 minutes and 1 hour into a session.**

- After every logical unit of work: finish the change → `git commit` → update `commits.md`.
- A "logical unit" is: one file created, one feature completed, one test passing, one doc updated.
- Never leave more than one uncommitted logical change in the working tree.
- Every commit gets its own entry prepended to `commits.md` (format below).

---

## Logging rules

**Every function, endpoint, event handler, service call, and error path must be logged.**

Log at:
- Entry point (params/body — never credentials)
- Service/handler entry
- DB or external API queries
- Branch decisions
- Exception raises
- Significant state transitions

**Never log passwords, PINs, or raw tokens of any kind.**

Use appropriate log levels:
- `debug` — fine-grained flow
- `info` — significant events
- `warning` — recoverable anomalies
- `error` — failures

---

## Key workflow rules

### commits.md rule
After every commit, **prepend** a new entry to `commits.md`:
```
### `<short-hash>` · <YYYY-MM-DD> · <commit subject>
- `affected/file1`
- `affected/file2`
```
`commits.md` itself gets its own commit: `docs: update commits.md`.

### Commit granularity
Every distinguishable change gets its own commit. Never bundle unrelated changes. If you need "and" in the commit message, it should be two commits.

### Straggler sweep
After any rename, removal, or refactor — grep for old references across all code and doc files before committing.

### Planning doc sync
When any change affects design, behaviour, schema, or architecture — update the relevant planning docs in the same task:
- `PLAN.md` — tech stack, modules, API endpoints, DB schema
- `PHASES.md` — per-phase task lists
- `FILE_INDEX.md` — file system index (update when files are added, moved, or removed)

All planning docs must stay consistent with each other at all times.

---

## Reference documents (adapt per project)
- `FILE_INDEX.md` — **file system index with tags and descriptions — read this first**
- `PLAN.md` — full tech stack, API endpoints, schema
- `PHASES.md` — detailed per-phase tasks and done criteria
- `commits.md` — commit log (prepend after every commit)

---

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

CUTV is a lightweight video trimming/cutting GUI. This repo (`src/`) is a Rust rewrite (`eframe`/`egui` + **GStreamer** via `gstreamer-rs`) and is the actively developed version — it's a single-window app: load a video, scrub a timeline with thumbnails, set IN/OUT points, and export a lossless-quality cut via ffmpeg. Final export (`do_cut`) shells out to `ffmpeg`/`ffprobe` on `PATH` — separate from playback, which goes through GStreamer's `playbin` (see below).

There is no Python version in this repo anymore (the original `cutv.py` prototype was removed). A **more advanced, actively maintained Python reference implementation** lives outside this repo at `C:\cli_tools\scripts\py\cutv\cutv.py` — consult it for feature parity / design ideas when extending the Rust app, but it is not part of this project's build. Notable things it does that the Rust version does not yet: NVENC/GPU encode path for cuts, crop tool, GIF export, yt-dlp URL support, cut progress bar. See `PLAN.md`/`PHASES.md` for the current roadmap on these.

## Commands

- Build: `cargo build` (debug), `cargo build --release`
- **Run: `cargo run --release -- <path/to/video.mp4>` — the `--release` is not optional for actually testing playback.** A debug build runs the same code roughly 50x slower on the per-frame pixel-format conversion in `app.rs` (`ColorImage::from_rgb`: ~100ms/frame unoptimized vs ~1.7ms optimized), which alone caps playback at ~9fps regardless of how fast decode/GPU-convert are. This isn't a `cargo run` performance quirk to route around — a debug build will always feel broken for this workload. (`cargo run` with a video file in cwd, no arg, auto-opens the most recently modified `.mp4/.mov/.avi/.mkv/.webm`.)
- Check/lint: `cargo check`, `cargo clippy`
- No test suite exists in this repo.
- Logging defaults to `debug` level (see `main.rs`); override with `RUST_LOG=info cargo run --release -- ...` for quieter output, or `RUST_LOG=trace` for detailed per-frame timing breakdowns in `player.rs`/`app.rs` (gated behind `trace!` specifically so normal `debug` runs aren't spammed — see PHASES.md's "low FPS" investigation for what these numbers look like).
- **Environment setup (GStreamer, required to build at all):** see `PLAN.md`'s "Architecture decision" section. Summary: `C:\vcpkg\vcpkg.exe install "gstreamer[plugins-base,plugins-good,plugins-bad,libav]:x64-windows"`, plus `GST_PLUGIN_PATH`/`PATH` pointed at vcpkg's install (already set as persistent user env vars on this machine; a fresh machine needs that setup redone). `.cargo/config.toml` handles `PKG_CONFIG`/`PKG_CONFIG_PATH` for the build itself.

## Architecture (Rust, `src/`)

- `main.rs` — entry point. Resolves the target video path (CLI arg or newest video file in cwd), probes it via `probe::probe_video`, computes the display size (fit to 640x360, never upscaled), builds the `eframe` window, and hands off to `app::CutvApp`.
- `probe.rs` — wraps `ffprobe` (JSON output) to extract duration/fps/width/height (`probe_video`), and separately builds ffmpeg encoder args for the final cut based on detected source codec (`encode_args`). Codec passthrough logic: video/audio codec of the source stream maps to a matching encoder (e.g. h264→libx264, hevc→libx265) at visually-lossless settings, so the cut preserves the original codec family. This codec map mirrors the (CPU-fallback branch of the) one in the external Python reference's `_encode_args` — keep them in sync if changed there.
- `player.rs` — `Player` wraps a GStreamer `playbin` pipeline: play/pause/seek (frame-exact, `SeekFlags::ACCURATE`)/seek_fast (keyframe-snap, `SeekFlags::KEY_UNIT`, used while scrubbing)/step_frames (a ±1-frame exact seek)/set_mute/poll. Video frames come from an appsink pulling raw RGB (via a `d3d11upload ! d3d11convert ! ... ! d3d11download` GPU colorspace-conversion bin, falling back to CPU `videoconvert` if no D3D11 device — see PLAN.md for why the GPU path matters) for the caller to upload as an egui texture. Audio is playbin's own default sink (`autoaudiosink`) — no separate audio code in this app at all; GStreamer owns decode, A/V sync, buffering, and seeking as one unit.
- `spawn_thumbs` (in `thumbs.rs`) — generates a strip of N=24 timeline thumbnails asynchronously via `ffmpeg` single-frame extraction, independent of the player.
- `app.rs` — `CutvApp` (the `eframe::App` impl) owns all playback/UI state: polls `Player` each frame (position/duration/new video texture), handles keyboard shortcuts (Space/←→/,./I/O/M/Enter), and draws the custom-painted UI (video frame, timeline with drag-able IN/OUT markers, transport controls, edit row, status bar, FPS counter). Timeline drag uses `seek_fast` continuously and one `seek` (exact) on release, so scrubbing stays responsive without sacrificing the exact frame you land on. `do_cut()` shells out to `ffmpeg` using `probe::encode_args` against the **original source** to produce the final trimmed file, named `<stem>_cut_<timestamp>.<ext>`.
- `util.rs` — small formatting helpers: `fmt_tc` (timecode string) and `parse_fps` (parses ffprobe's `"num/den"` frame rate strings).

**Architecture history:** this went through two prior approaches before landing on GStreamer — an ffmpeg-pipe proxy-transcode player (original), then embedded libmpv, then a hand-rolled `ffmpeg-next`-based decoder. See `PLAN.md`'s "Architecture decision" section for the full why-we-moved-on story on each; it's worth reading before touching `player.rs` so you don't re-propose an already-tried-and-rejected approach.
