# commits.md

Commit log — prepend a new entry after every commit.

### `740fc4a` · 2026-09-19 · feat: add resizable crop overlay, applied to CUT export
- `.gitignore`
- `src/app.rs`
- `src/main.rs`
- `src/crop.rs` (new)

### `23c63c7` · 2026-09-19 · feat: parse ffmpeg -progress output for a live cut progress bar
- `src/app.rs`

### `07928bf` · 2026-09-19 · fix: cut output saved to filesystem root instead of source's directory
- `src/app.rs`

### `06f12ea` · 2026-09-19 · feat: add short-GOP scrub proxy for fast, frame-exact seeking
- `src/app.rs`
- `src/main.rs`
- `src/player.rs`
- `src/proxy.rs` (new)

### `10efb66` · 2026-09-19 · fix: enable ORC in vcpkg's gstreamer build via overlay port
- `.gitattributes` (new)
- `src/player.rs`
- `vcpkg-overlay/gstreamer/` (new)

### `9f98014` · 2026-09-19 · feat: switch playback from hand-rolled ffmpeg-next decoder to GStreamer playbin
- `.cargo/config.toml`
- `Cargo.lock`
- `Cargo.toml`
- `src/app.rs`
- `src/player.rs`

### `09c5ea1` · 2026-09-19 · build: add ffmpeg-next dependency and vcpkg FFmpeg toolchain
- `Cargo.lock`
- `Cargo.toml`
- `.cargo/config.toml`

### `e914273` · 2026-09-19 · fix: resolve mpv autoplay, pause-flicker, backward-step lag, and audio-blip bugs
- `src/player.rs`
- `src/app.rs`

### `e1d1482` · 2026-09-19 · feat: Phase 1 libmpv migration — replace video.rs/audio.rs with player.rs
- `Cargo.lock`
- `Cargo.toml`
- `src/app.rs`
- `src/audio.rs` (removed)
- `src/main.rs`
- `src/player.rs` (new)
- `src/thumbs.rs` (new)
- `src/video.rs` (removed)

### `3f6fb9e` · 2026-08-25 · docs: record Phase 0 libmpv spike results and open questions
- `FILE_INDEX.md`
- `PHASES.md`
- `PLAN.md`

### `72e0528` · 2026-08-25 · feat: import CUTV Rust source (baseline + session UI/playback work)
- `.github/copilot-instructions.md`
- `.gitignore`
- `Cargo.lock`
- `Cargo.toml`
- `src/app.rs`
- `src/audio.rs`
- `src/bin/mpv_spike.rs`
- `src/main.rs`
- `src/probe.rs`
- `src/util.rs`
- `src/video.rs`

### `77ed411` · 2026-08-25 · docs: bootstrap planning docs for libmpv migration + feature roadmap
- `CLAUDE.md`
- `FILE_INDEX.md`
- `PLAN.md`
- `PHASES.md`
- `commits.md`
