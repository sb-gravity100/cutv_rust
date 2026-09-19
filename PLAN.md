# PLAN.md

## Tech stack

- Rust 2024 edition.
- UI: `eframe`/`egui` 0.29 — immediate-mode GUI, custom-painted (no OS-native widgets).
- Playback: **GStreamer's `playbin` via `gstreamer-rs`** (`gstreamer`/`gstreamer-app`/`gstreamer-video` crates) — see "Architecture decision" below for the full path here (mpv → hand-rolled `ffmpeg-next` decoder → GStreamer). playbin owns demuxing, decode, A/V sync, buffering, and seeking as one unit; audio is its own default sink (`autoaudiosink`), no separate audio code in this app. Video frames are pulled from an `appsink` as raw RGB and rendered as an egui texture (`src/player.rs`).
- **Scrub proxy** (`src/proxy.rs`): on load, a short-GOP (every 8th frame a keyframe) background transcode of the source starts immediately; playback plays the original file until it's ready, then `Player` swaps over to it. This is what actually makes seeking both fast *and* frame-exact — see "The scrub proxy" section below. `do_cut`'s final export always uses the original file regardless.
- Export/encode/probe: shells out to `ffmpeg`/`ffprobe` on `PATH` (must be installed, not vendored) for `do_cut`'s final export and `probe.rs`. Entirely separate from playback decode (GStreamer, in-process).
- **Build-time dependency: GStreamer dev libs.** `gstreamer-rs`'s sys crates use `pkg-config`, which needs GStreamer's headers/libs + a `pkg-config`(`pkgconf`) binary — not just runtime DLLs. Set up on this machine via vcpkg, using this repo's own overlay port (`vcpkg-overlay/gstreamer/`, see below for why):
  ```
  C:\vcpkg\bootstrap-vcpkg.bat
  C:\vcpkg\vcpkg.exe install --overlay-ports="<repo>\vcpkg-overlay" "gstreamer[plugins-base,plugins-good,plugins-bad,libav]:x64-windows"
  ```
  (~6 min build.) `.cargo/config.toml` sets `PKG_CONFIG_PATH`/`PKG_CONFIG` to vcpkg's install + its bundled `pkgconf.exe`, so `cargo build` finds it with no other setup. **Runtime** also needs `GST_PLUGIN_PATH` (→ `C:\vcpkg\installed\x64-windows\plugins\gstreamer`) and vcpkg's `bin` dir on `PATH` — set as persistent *user* environment variables on this machine (a fresh machine/account needs that redone; see the `setx`-equivalent PowerShell in conversation history if reproducing).
  - The `nvcodec` vcpkg feature (NVENC/NVDEC) **fails to build** in this port — a missing D3D11 dependency wiring bug in the port itself (`GST_D3D11_DEVICE`/`GstD3D11BufferPool` undefined in `gstnvdecoder.cpp`), not our code. Not included. This turned out not to matter: `plugins-bad`'s separate `gstd3d11.dll` already provides `d3d11h264dec`/`d3d11h265dec` (hardware decode via DXVA/D3D11, auto-selected by decodebin's element ranking) and `d3d11convert` (GPU colorspace conversion) independent of the broken `nvcodec` feature — see below.
  - **Why the overlay port:** upstream vcpkg's `gstreamer` port hardcodes `-Dorc=disabled` ("gstreamer requires a specific version of orc which is not available in vcpkg" — true: vcpkg's own `orc` port is unrelated Apache ORC, a name collision with GStreamer's Oil Runtime Compiler SIMD codegen library). Without it, `videoconvert`/`audioconvert`/etc. fall back to slow scalar C — measured 8.85fps vs 655fps decode-only, a 74x hit (see "The low-FPS investigation"). The repo's overlay port (copy of the upstream port, one line changed to `-Dorc=enabled`) pre-vendors ORC 0.4.42 from its official tarball into `subprojects/orc` before configure, since `vcpkg_configure_meson` always passes `--wrap-mode nodownload` (blocking the `orc.wrap` file's git-fetch mechanism) but meson happily uses an already-present subprojects directory without needing the wrap at all. Fixed `videoconvert` throughput: **~175fps** (still below the D3D11 GPU path's ~640fps for this resolution, but comfortably above the 30fps we need — a good CPU fallback now, not just "won't hang").
- **`cargo build --release` is required to actually test playback**, not just for shipping. See "The low-FPS investigation" below — this isn't optional the way it usually is for iterating on other kinds of code.
- No test suite.

## Architecture decision: three approaches to playback, and why the first two were dropped

### Approach 1 (original): ffmpeg-pipe proxy transcode — replaced 2026-09-19

`video.rs`/`audio.rs` (removed) spawned a new `ffmpeg` subprocess per seek against a pre-transcoded low-res proxy. Scrubbing smoothness and decode resolution directly traded off against each other with no way to get both — no tuning path to "Premiere-level" scrub smoothness with a process-per-seek model. Motivated the move to a persistent decoder (something that keeps a live decoder + demuxer cache warm across seeks), which is what both of the next two approaches are.

### Approach 2: embedded libmpv — superseded 2026-09-19

`Player` (old `src/player.rs`) embedded libmpv into a native Win32 child window via `LoadLibraryW`/`GetProcAddress` runtime loading (no MSVC import lib existed for the available `libmpv-2.dll`; `src/bin/mpv_spike.rs` has the embedding-technique proof and stays in the tree as a reference even though it's no longer the playback direction). Phase 0/1 verified mpv coexists fine with eframe/glutin's GL surface, and it worked — but every bug fix was whack-a-mole with mpv's **async, fire-and-forget command interface**: the app is constantly guessing whether a command (pause, seek, mute) has actually "landed" yet, with no real completion signal short of building out mpv's full event loop. Four real bugs got fixed this way (autoplay-on-load, Play/Pause button flicker, slow/backlogged backward frame-stepping, audio blips during stepping — see `PHASES.md` Phase 1) before deciding the pattern itself wasn't worth continuing to fight.

### Approach 3: hand-rolled `ffmpeg-next` decoder thread — dropped 2026-09-19, same day

Wrote a persistent decode thread directly via `ffmpeg-next` (libavcodec/libavformat bindings), decoding synchronously so there was no async command queue to guess about, rendering to an egui texture instead of mpv's native child window. This actually worked correctly (verified: correct direction/cadence for frame stepping, working precise-seek-via-keyframe technique, a real `is_seeking`-style backlog fix) — but **reimplementing a video player's decode pacing, A/V sync, hardware-decode integration, and seek strategy by hand is a lot of surface area to get right**, and after several rounds of real regressions (black screen on load, wrong-assumption resolution capping, a hard freeze on seek when NVDEC was wired in via raw FFI) the user called it: "make a regular video player" rather than keep debugging a from-scratch one. `ffmpeg-next` was removed as a dependency; the vcpkg FFmpeg build from this approach isn't needed either, though it's harmless left installed.

### Approach 4 (current): GStreamer `playbin` — landed 2026-09-19

Chosen specifically because it's a mature, safe-Rust-API alternative that still owns decode/sync/seek as a unit (same category of win as mpv) but isn't mpv's async command-string interface and isn't a from-scratch reimplementation. `gstreamer-rs` gives proper typed Rust bindings (`Result`-returning calls, no guessing) over the same class of mature C pipeline machinery. Landed cleanly: build + link worked first try once vcpkg's GStreamer was up, `playbin` + `appsink` handles the whole play/pause/seek/step/mute/poll surface with far less code than either prior approach, and — after the low-FPS/D3D11 fix described below — performs excellently.

## The low-FPS investigation (2026-09-19) — read this before touching playback perf again

After landing GStreamer, playback measured a hard ~8.5-9fps ceiling (should be 30fps for the test source). Investigation, in order:

1. **Hypothesis: our polling/appsink integration.** Ruled out — added detailed `trace!`/`debug!` timing to `Player::poll()`/`sample_to_frame` (see `src/player.rs`; kept in the tree, gated behind `RUST_LOG=trace` so normal runs aren't spammed). `appsink.try_pull_sample` and the RGB row-copy were consistently sub-2ms.
2. **Hypothesis: software decode / no hardware accel.** Tested directly with `gst-launch-1.0 filesrc ! decodebin ! fakesink` (`sync=false`, i.e. max throughput, no real-time pacing) — **655fps**. Decode was never the bottleneck.
3. **Hypothesis: `videoconvert` (colorspace conversion).** Same test with `videoconvert` added before the sink: **8.85fps** — a 74x drop, isolated to this one element. Root cause: this vcpkg GStreamer build has **ORC disabled** (`Dependency orc-0.4 skipped: feature orc disabled` in the vcpkg build log) — ORC is GStreamer's SIMD codegen library; without it, `videoconvert` and friends fall back to slow scalar reference C. **Fixed properly afterward** via the `vcpkg-overlay/gstreamer` overlay port (see "Tech stack" above) — `videoconvert` alone now measures ~175fps, no longer the CPU-fallback liability it was when step 4 below was written.
4. **Fix found: GPU colorspace conversion instead.** `gst-plugins-bad`'s `gstd3d11.dll` (already built, not gated behind the broken `nvcodec` feature) provides `d3d11convert`/`d3d11colorconvert`. Swapped the video-sink bin from `videoconvert ! appsink` to `d3d11upload ! d3d11convert ! video/x-raw(memory:D3D11Memory),format=RGB ! d3d11download ! appsink` (falls back to plain `videoconvert` if D3D11 is unavailable — see `Player::new` in `src/player.rs`). Re-tested via `gst-launch`: **~640fps**. Bonus: this also revealed decodebin was *already* auto-selecting `d3d11h264dec` (hardware decode) for the isolated decode-only test in step 2 — that's why 655fps was plausible for software-looking numbers; it wasn't software.
5. **Still ~9fps in the actual app despite the isolated pipeline being ~640fps.** Re-added the D3D11 sink to `player.rs`, re-tested in-app: still slow. More `trace!` instrumentation pinned it exactly: `egui::ColorImage::from_rgb` (a simple `chunks_exact(3).map(...).collect()` over ~2.5M pixels) was taking **~96-100ms/frame**. Everything else (GStreamer pull, row-copy, `tex.set`) was sub-5ms combined.
6. **Actual root cause: we were testing debug builds.** `cargo build --release` dropped `ColorImage::from_rgb` from ~96ms to **~1.7ms**, and measured playback hit a steady **30.0fps**. The D3D11 GPU-convert fix (step 4) is still correct and worth keeping — it removed a genuine 74x CPU bottleneck that would otherwise still cap release-build performance — but the dramatic "9fps in the app" symptom specifically was a debug-vs-release artifact layered on top of it. **Always test playback perf with `cargo build --release` / `cargo run --release`** — a debug build will look broken for this per-frame pixel-copy workload no matter how good the underlying pipeline is.

Consequence for crop UI: rendering frames as an egui texture (rather than mpv's native child window) means the crop rubber-band can be drawn directly over the video texture in egui — no separate native overlay window needed. Simplifies Phase 3 versus the original libmpv-era plan.

## The scrub proxy (2026-09-19) — why seeking needed more than gating

Once playback itself was fast (steady 30/60fps, per above), the remaining complaint was seeking: 500ms+ per seek, and held frame-stepping/timeline-dragging looking completely frozen. Two separate problems, found by testing against `sample_60fps.mp4` (1920x1080@60fps, chosen specifically because it has a much longer GOP than the original test file):

1. **The GOP is long.** `ffprobe`'s packet flags showed keyframes roughly every 4.2s / 250 frames on this source. `SeekFlags::ACCURATE` always decodes forward from the keyframe before the target to land exactly on it — so a seek's cost is bounded by GOP length, not by decode/convert speed (which were already fast per the low-FPS investigation). No amount of pipeline optimization fixes this; it's inherent to the encoding.
2. **Rapid requests thrash each other.** Holding a frame-step button, or dragging the timeline, issues a new seek far faster than even a *fast* (`SeekFlags::KEY_UNIT`, keyframe-snap) seek can complete — each new `FLUSH` seek interrupts the previous one before it lands, so on a fast drag nothing ever actually completes (measured: 11+ seeks/sec during one drag, only 1 ever finished). Gating repeats on `Player::is_seeking()` (a flag set on `seek()`, cleared the next time `poll()` actually receives a frame) fixed the thrashing, but the user's read on this was correct: **gating just stops the cost from compounding, it doesn't remove it.**

The real fix, and what regular video editors (Premiere, Resolve, etc.) actually do: transcode a **short-GOP scrub proxy** in the background and play that instead of the original for everything except final export. `proxy.rs`'s `spawn_proxy()` kicks this off on load (`-g 8 -keyint_min 8`, NVENC first with a libx264 `ultrafast` fallback); `app.rs`'s `update()` swaps `Player` over to it once ready, preserving position/play state. With a keyframe every 8 frames instead of every 250, an *exact* (`ACCURATE`) seek is cheap enough that there's no reason to trade accuracy for speed at all — **`seek_fast`/`KEY_UNIT` was removed entirely** per explicit direction ("do not use fast seek at all"); every seek in the app is now frame-exact.

Two more bugs found and fixed in the same pass, both root-caused by testing against real interaction rather than synthetic scripted input:

- **Frame 0 never rendered on load**, and the picture looked frozen after *any* seek while paused (which is most of scrubbing/stepping): `appsink` delivers a frame two different ways depending on pipeline state — a **preroll** buffer while `PAUSED` (initial load, and every seek issued while paused), vs a regular **sample** while `PLAYING`. `Player::poll()` only ever called `try_pull_sample()`, which never catches preroll buffers — so essentially all paused-state frames were silently dropped. Fixed by trying `try_pull_preroll()` first, falling back to `try_pull_sample()`.
- **Seeking right after the proxy swap failed** ("Failed to seek"): a freshly-constructed `Player`'s pipeline hasn't finished prerolling by the time `set_state(Paused)` returns (it's an async state change). Fixed by blocking on `pipeline.state(timeout)` in `Player::new()` until the state change actually completes before returning.

Proxy transcode time is proportional to source length/resolution (measured: ~10s for a 1.5min 1080p60 source, ~29s for a ~5min 2340x1080 source, both via NVENC) — noticeable but not blocking, since playback works on the original file the whole time.

## Features to add (all approved, in priority order)

1. ~~**Cut progress bar**~~ — done, see `PHASES.md` Phase 2.
2. ~~**Crop tool**~~ — done, see `PHASES.md` Phase 3. `crop.rs`; applies to `do_cut` and (as of Phase 4) `do_gif`.
3. ~~**GIF export**~~ — done, see `PHASES.md` Phase 4. Built on the `gifski` crate (https://github.com/imageoptim/gifski) instead of the originally-planned ffmpeg `palettegen`/`paletteuse` two-pass — gifski's perceptual quantizer/dithering gives noticeably better quality per byte than ffmpeg's palette filters, at the cost of pulling in a Rust dependency instead of shelling out twice.
4. **NVENC/GPU encode** — detect `h264_nvenc`/`hevc_nvenc` availability (`ffmpeg -hide_banner -encoders`, cached), branch `probe::encode_args` to prefer GPU encoders with `-hwaccel cuda` on the input side when available, fall back to libx264/libx265 otherwise. Note: this is about the `PATH` ffmpeg's NVENC support for the final cut export, unrelated to GStreamer's (currently broken) `nvcodec` feature.

## Explicitly deferred

- yt-dlp URL download support — lowest priority, separate concern (download + cache dir), not part of this phase.

## GIF export design (`gif.rs`)

ffmpeg still does frame *extraction* (it's already the project's only video I/O dependency, and its `-vf` chain already handles crop/scale/fps in one pass) but no longer does palette generation or GIF muxing — that's all `gifski` now:

- ffmpeg decodes the IN→OUT range from the **original source** (not the scrub proxy, for quality) straight to `pipe:1` as raw RGBA frames (`-f rawvideo -pix_fmt rgba`), with `-vf` applying `crop=...` (if `self.crop_rect` is set, same string `do_cut` uses), then `scale=W:H:flags=lanczos` (capped to 640px wide, aspect-preserved, never upscaled), then `fps=10` — same defaults (640px/10fps) as the external Python reference's `_gif_w`/`_gif_fps`.
- Two threads run concurrently, per `gifski::new()`'s documented contract (the `Collector` blocks once its queue fills until the `Writer` is actively draining it): one reads ffmpeg's stdout in exact `w*h*4`-byte frame chunks and calls `Collector::add_frame_rgba`; the other calls `Writer::write()` against the output `.gif` file with a `ProgressReporter` impl that updates the same `Arc<Mutex<f32>>` progress-bar pattern `do_cut` uses (reused directly — CUT and GIF export are mutually exclusive single actions, so sharing the field needed no new UI code).
- `gifski::Settings.width`/`.height` are pinned to the *exact* dimensions ffmpeg already scaled to, so gifski's own internal auto-resize heuristic (which otherwise triggers based on total pixel count) never second-guesses the size.
- Verified via a throwaway `src/bin/gif_test.rs` (not committed) that called `gif::spawn_gif_export` directly, once with no crop (640×360 output) and once with an 800×600 crop (confirmed output scaled to 640×480, aspect preserved) — both produced valid `GIF89a` files.

## Module plan for remaining phases

- `probe.rs` — add `nvenc_available() -> bool` (cached), extend `encode_args` with the NVENC branch.
- `app.rs` — `do_cut` gains `-hwaccel cuda` (remaining); progress parsing and crop `-vf` injection already done for both `do_cut` and `do_gif`.
