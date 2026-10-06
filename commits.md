### `9b57ce9` · 2026-10-06 · docs: index README
- `FILE_INDEX.md`

### `e5d2428` · 2026-10-06 · docs: add README
- `README.md`

### `cddf8c6` · 2026-10-06 · docs: index PATH task and cutv.cmd
- `FILE_INDEX.md`

### `e7ca60a` · 2026-10-06 · build: optional 'Add CUTV to PATH' installer task with cutv.cmd shim
- `installer/cutv.cmd`
- `installer/cutv.iss`

### `687d1f5` · 2026-10-06 · docs: note installer-managed context menu
- `FILE_INDEX.md`
- `PHASES.md`

### `cc31657` · 2026-10-06 · build: optional Explorer context-menu entry in installer
- `installer/cutv.iss`

### `82bc535` · 2026-10-06 · docs: sync planning docs for release tooling and auto-updater
- `FILE_INDEX.md`
- `PLAN.md`

### `452a6bf` · 2026-10-06 · build: Cargo.toml-driven versioning, signed release script, per-user installer
- `installer/cutv.iss`
- `package.json`
- `scripts/bump-version.mjs`
- `scripts/release.mjs`

### `e8906c4` · 2026-10-06 · feat: signed in-app auto-updater
- `Cargo.lock`
- `Cargo.toml`
- `src/app.rs`
- `src/main.rs`
- `src/updater.rs`

### `509cf86` · 2026-10-06 · feat: export overrides for framerate, bitrate, resolution (default source)
- `src/export.rs`
- `src/app.rs`

### `e5dd8e6` · 2026-09-28 · build: add Inno Setup installer script (exe-only)
- `installer/cutv.iss`

# commits.md

Commit log — prepend a new entry after every commit.

### `a7a68a7` · 2026-09-27 · docs: sync planning docs for Phase 10 (probe/proxy/thumbs/gif via ffmpeg-next)
- `FILE_INDEX.md`
- `PHASES.md`
- `PLAN.md`

### `0b81821` · 2026-09-27 · feat: probe/proxy/thumbs/gif via ffmpeg-next, in-process (Phase 10)
- `Cargo.toml`
- `Cargo.lock`
- `src/probe.rs`
- `src/proxy.rs`
- `src/thumbs.rs`
- `src/gif.rs`
- `src/util.rs`

### `cce4789` · 2026-09-27 · docs: sync planning docs for Phase 8 (variable speed) and Phase 9 (ffmpeg-next export)
- `FILE_INDEX.md`
- `PHASES.md`
- `PLAN.md`

### `60e49b8` · 2026-09-27 · feat: in-process do_cut export via ffmpeg-next (Phase 9)
- `Cargo.toml`
- `Cargo.lock`
- `src/main.rs`
- `src/export.rs`
- `src/app.rs`
- `src/probe.rs`

### `fa65263` · 2026-09-27 · feat: variable playback/export speed (Phase 8)
- `src/app.rs`
- `src/gif.rs`
- `src/player.rs`
- `src/probe.rs`
- `src/util.rs`

### `74e2159` · 2026-09-27 · feat: append debug logs to cutv.log next to the exe
- `src/main.rs`

### `396745b` · 2026-09-19 · feat: content-hash proxy naming + exit-time proxy cleanup
- `src/proxy.rs`
- `src/app.rs`
- `FILE_INDEX.md`
- `PLAN.md`
- `PHASES.md`

### `b770f59` · 2026-09-19 · docs: record context-menu fix (per-extension) and console-suppression fix
- `FILE_INDEX.md`
- `PHASES.md`

### `fd88e5d` · 2026-09-19 · fix: suppress console window on standalone launch
- `src/main.rs`

### `1a8fd93` · 2026-09-19 · docs: sync planning docs for Phase 6 polish (icon, Open, drag-drop, OS integration)
- `FILE_INDEX.md`
- `PHASES.md`
- `PLAN.md`

### `4069584` · 2026-09-19 · feat: add Open-file button and drag-and-drop video loading
- `src/app.rs`

### `03a76df` · 2026-09-19 · feat: add app icon (runtime window/taskbar + embedded exe resource)
- `assets/icon.ico` (new)
- `assets/icon-256.png` (new)
- `build.rs` (new)
- `src/main.rs`
- `Cargo.toml`
- `Cargo.lock`

### `38e7238` · 2026-09-19 · fix: match do_cut export bitrate to source instead of pure CQ/CRF
- `src/probe.rs`
- `PHASES.md`

### `11d0c07` · 2026-09-19 · docs: sync planning docs for NVENC encode support (Phase 5)
- `FILE_INDEX.md`
- `PHASES.md`
- `PLAN.md`

### `4d44b72` · 2026-09-19 · feat: add NVENC/GPU encode branch to do_cut's export (Phase 5)
- `src/probe.rs`
- `.gitignore`

### `65e35c8` · 2026-09-19 · tune: bump GIF export width cap to 800px
- `src/gif.rs`
- `PLAN.md`

### `cedc5b7` · 2026-09-19 · tune: restore gifski quality to 90, keep 20fps
- `src/gif.rs`
- `PLAN.md`

### `6b74539` · 2026-09-19 · tune: bump GIF export to 20fps, lower gifski quality to 35
- `src/gif.rs`
- `PLAN.md`

### `f70a038` · 2026-09-19 · docs: sync planning docs for gifski-based GIF export (Phase 4)
- `FILE_INDEX.md`
- `PHASES.md`
- `PLAN.md`

### `5872a60` · 2026-09-19 · feat: add GIF export via gifski, wired to GIF button + G shortcut
- `.gitignore`
- `Cargo.lock`
- `Cargo.toml`
- `src/app.rs`
- `src/main.rs`
- `src/gif.rs` (new)

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
