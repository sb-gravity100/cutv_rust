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

CUTV is a lightweight video trimming/cutting GUI. This repo (`src/`) is a Rust rewrite (`eframe`/`egui` + `rodio`) and is the actively developed version — it's a single-window app: load a video, scrub a timeline with thumbnails, set IN/OUT points, and export a lossless-quality cut via ffmpeg. External binaries `ffmpeg` and `ffprobe` must be on `PATH` — there is no bundled/vendored ffmpeg; all decoding, encoding, and probing shells out to them.

There is no Python version in this repo anymore (the original `cutv.py` prototype was removed). A **more advanced, actively maintained Python reference implementation** lives outside this repo at `C:\cli_tools\scripts\py\cutv\cutv.py` — consult it for feature parity / design ideas when extending the Rust app, but it is not part of this project's build and should not be assumed to exist here. Notable things it does that the Rust version does not yet:
- Playback via embedded **libmpv** (`python-mpv`) instead of piping raw frames through ffmpeg — mpv owns decode/GPU-render/audio/clock directly, embedded into the Tk window via `wid`.
- **NVENC/GPU encode path** (`h264_nvenc`/`hevc_nvenc`) for cuts and re-encodes when an NVIDIA GPU is detected, falling back to libx264/libx265 otherwise.
- **Crop tool**: right-drag rubber-band on a transparent color-keyed overlay `Toplevel` above the video, baked into cut/GIF exports as an ffmpeg `-vf crop=...` filter.
- **GIF export** (two-pass palette generation + paletteuse) of the IN/OUT selection.
- **yt-dlp URL support**: passing a URL downloads it first, then transcodes to H.264 if needed so later cuts stay on the GPU path.
- ffmpeg `-progress pipe:1` parsing for a live progress bar during cut/GIF export.
- ffprobe result caching keyed on `(path, mtime, size)`.

## Commands

- Build: `cargo build` (debug), `cargo build --release`
- Run: `cargo run -- <path/to/video.mp4>` (or `cargo run` with a video file in cwd — it auto-opens the most recently modified `.mp4/.mov/.avi/.mkv/.webm`)
- Check/lint: `cargo check`, `cargo clippy`
- No test suite exists in this repo.
- Logging defaults to `debug` level (see `main.rs`); override with `RUST_LOG=info cargo run -- ...` for quieter output.

## Architecture (Rust, `src/`)

- `main.rs` — entry point. Resolves the target video path (CLI arg or newest video file in cwd), probes it via `probe::probe_video`, computes the display size (fit to 640x360, never upscaled), builds the `eframe` window, and hands off to `app::CutvApp`.
- `probe.rs` — wraps `ffprobe` (JSON output) to extract duration/fps/width/height (`probe_video`), and separately builds ffmpeg encoder args for the final cut based on detected source codec (`encode_args`). Codec passthrough logic: video/audio codec of the source stream maps to a matching encoder (e.g. h264→libx264, hevc→libx265) at visually-lossless settings, so the cut preserves the original codec family. This codec map mirrors the (CPU-fallback branch of the) one in the external Python reference's `_encode_args` — keep them in sync if changed there.
- `video.rs` — all ffmpeg subprocess plumbing for the *display* path, distinct from `probe.rs`/final export:
  - `VideoDecoder` — spawns `ffmpeg` piping raw RGB24 frames (`pipe:1`) for sequential playback from a given start time, streaming decoded frames back over an `mpsc` channel to the UI thread.
  - `SeekWorker` — a persistent background thread that services single-frame seek requests (extracts one frame at a given timestamp) without blocking playback.
  - `spawn_thumbs` — generates a strip of N=24 timeline thumbnails asynchronously.
  - `spawn_proxy` — transcodes the source to a small ultrafast H.264 proxy at display resolution; **all playback, seeking, and thumbnails operate on this proxy, never the original file** — this is what keeps scrubbing responsive on large/high-bitrate sources. The proxy is a temp file cleaned up on `CutvApp` drop.
- `audio.rs` — `AudioPlayer` extracts the full audio track to a temp MP3 via `ffmpeg` in a background thread on construction, then plays it back through `rodio`, seeking via `Sink::try_seek`. Audio and video are two independently-driven pipelines (proxy video decode + separate audio decode/playback) kept in sync by passing the same start timestamp to both on play/seek — there is no shared clock. (The external mpv-based reference avoids this split entirely since mpv handles both natively.)
- `app.rs` — `CutvApp` (the `eframe::App` impl) owns all playback/UI state and ties the above pieces together each frame: initializes the proxy once ready, polls the seek/decoder/thumbnail channels, handles keyboard shortcuts (Space/←→/,./I/O/M/Enter), and draws the custom-painted UI (video frame, timeline with drag-able IN/OUT markers, transport controls, edit row, status bar). `do_cut()` shells out to `ffmpeg` using `probe::encode_args` against the **original source** (not the proxy) to produce the final trimmed file alongside the source, named `<stem>_cut_<timestamp>.<ext>`.
- `util.rs` — small formatting helpers: `fmt_tc` (timecode string) and `parse_fps` (parses ffprobe's `"num/den"` frame rate strings).

Key invariant to preserve when editing: the proxy/original split in `video.rs`/`app.rs` (proxy for all UI interaction, original source only touched by `probe::encode_args` + `do_cut`'s final ffmpeg invocation).
