<div align="center">

<img src="assets/icon-256.png" alt="CUTV logo" width="128" height="128">

<h1>CUTV</h1>

<p><strong>Trim videos in seconds: scrub, mark IN/OUT, cut.</strong></p>

<p>A lightweight, keyboard-driven video cutter for Windows.<br>
Frame-accurate scrubbing, near-lossless cuts, GIF export, and it updates itself.</p>

<p>
  <a href="https://github.com/sb-gravity100/cutv_rust/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/sb-gravity100/cutv_rust?style=for-the-badge&color=4caf50&labelColor=161616&label=release"></a>
  <a href="https://github.com/sb-gravity100/cutv_rust/releases"><img alt="Downloads" src="https://img.shields.io/github/downloads/sb-gravity100/cutv_rust/total?style=for-the-badge&color=4caf50&labelColor=161616"></a>
  <img alt="Windows x64" src="https://img.shields.io/badge/Windows-x64-4caf50?style=for-the-badge&labelColor=161616&logo=windows&logoColor=white">
</p>

<p>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-CE422B?style=flat-square&logo=rust&logoColor=white">
  <img alt="egui" src="https://img.shields.io/badge/egui-UI-3b3f5c?style=flat-square">
  <img alt="GStreamer" src="https://img.shields.io/badge/GStreamer-playback-D52E2E?style=flat-square&logo=gstreamer&logoColor=white">
  <img alt="FFmpeg" src="https://img.shields.io/badge/FFmpeg-export-007808?style=flat-square&logo=ffmpeg&logoColor=white">
  <img alt="gifski" src="https://img.shields.io/badge/gifski-GIF-6a4c93?style=flat-square">
</p>

<p>
  <a href="https://github.com/sb-gravity100/cutv_rust/releases/latest"><strong>Download</strong></a>
  &nbsp;·&nbsp;
  <a href="#features">Features</a>
  &nbsp;·&nbsp;
  <a href="#keyboard-shortcuts">Shortcuts</a>
  &nbsp;·&nbsp;
  <a href="#installing">Install</a>
  &nbsp;·&nbsp;
  <a href="#building-from-source">Build from source</a>
  &nbsp;·&nbsp;
  <a href="#releasing">Releasing</a>
</p>

</div>

---

## Features

| | |
|---|---|
| ✂️ **Frame-exact cuts** | Set IN/OUT anywhere, down to a single frame. Cuts keep the source's codec family at visually-lossless settings. |
| ⚡ **Instant scrubbing** | A short-GOP scrub proxy is built in the background, so dragging the timeline stays fast *and* exact. |
| 🎞️ **Thumbnail timeline** | A strip of thumbnails plus draggable IN/OUT markers. |
| 🟩 **NVENC acceleration** | Uses the GPU encoder when it's available and falls back to CPU when it isn't. |
| 🔲 **Crop** | Drag a crop box on the frame. It applies to both cuts and GIFs. |
| 🌀 **GIF export** | High-quality GIFs through [gifski](https://gif.ski)'s perceptual quantizer. |
| ⏩ **Variable speed** | Speed up or slow down the preview *and* the export, optionally keeping the pitch. |
| 🎛️ **Export overrides** | Optionally change the cut's frame rate, bitrate and resolution. By default they match the source. |
| 🔄 **Auto-update** | Checks for new releases in the background. Updates are signature-verified and install in one click. |
| 🖱️ **Explorer integration** | Optional *Cut with CUTV* right-click entry and a `cutv` terminal command. |

## Keyboard shortcuts

| Key | Action |
|---|---|
| <kbd>Space</kbd> | Play / pause |
| <kbd>←</kbd> <kbd>→</kbd> | Seek ±5 s |
| <kbd>,</kbd> <kbd>.</kbd> | Step ±1 frame (hold to repeat) |
| <kbd>I</kbd> / <kbd>O</kbd> | Set IN / OUT at the playhead |
| <kbd>M</kbd> | Mute |
| <kbd>Enter</kbd> | Export the cut |
| <kbd>G</kbd> | Export a GIF |

Cuts are saved next to the source as `<name>_cut_<timestamp>.<ext>`, and GIFs as `<name>_gif_<timestamp>.gif`.

## Installing

Grab **`cutv-setup-x.y.z.exe`** from the [latest release](https://github.com/sb-gravity100/cutv_rust/releases/latest). It installs per-user (no admin prompt) and offers:

- **Desktop shortcut**
- **Explorer context menu**: right-click a `.mp4` / `.mov` / `.avi` / `.mkv` / `.webm` → *Cut with CUTV*
- **Add to PATH**: run `cutv video.mp4` from any terminal

After that, CUTV keeps itself current: when a new release is out, an **Update & restart** button appears in the status bar.

> [!IMPORTANT]
> The installer ships the CUTV executable only. The **GStreamer and FFmpeg runtime** (vcpkg build, see below) must already be on the machine, or the app won't start.

## Usage

```bash
cutv path/to/video.mp4
```

Run `cutv` with no argument to open the newest video in the current folder, or use the **Open** button inside the app.

## Building from source

<details>
<summary><strong>Prerequisites</strong></summary>

- Rust (stable, edition 2024)
- [vcpkg](https://vcpkg.io) at `C:\vcpkg` with GStreamer and FFmpeg:
  ```bash
  C:\vcpkg\vcpkg.exe install "gstreamer[plugins-base,plugins-good,plugins-bad,libav]:x64-windows" --overlay-ports=vcpkg-overlay
  ```
- `GST_PLUGIN_PATH` and `PATH` pointed at vcpkg's install (see `PLAN.md` → *Architecture decision*). `.cargo/config.toml` already handles `pkg-config` for the build.

</details>

```bash
cargo run --release -- path/to/video.mp4
```

> [!NOTE]
> Always use `--release`. The debug build's per-frame conversion is about 50× slower, which caps playback at around 9 fps.

Logging defaults to `debug` and also goes to `cutv.log` next to the exe. Use `RUST_LOG=info` for quieter output or `RUST_LOG=trace` for per-frame timings. Set `CUTV_NO_UPDATE=1` to skip the update check.

## Releasing

Releases are built and published from a local PC (requires [Inno Setup](https://jrsoftware.org/isinfo.php), Node, `gh`, and the signing key at `~/.cutv/update-signing.pem`).

```bash
npm run bump -- patch
```

```bash
npm run release -- --notes-file notes.md
```

`bump` updates the version in `Cargo.toml` (its only source), commits and tags. `release` then builds the exe and the installer, Ed25519-signs the installer, writes `latest.json`, and publishes everything as a GitHub release, which installed copies pick up automatically. Add `--dry-run` to build without publishing.

## How it works

```
┌────────────┐   playbin + D3D11 convert   ┌──────────┐
│   video    │ ──────────────────────────▶ │  egui UI │
└────────────┘                             └──────────┘
      │  background: scrub proxy · thumbnails     │ IN/OUT, crop, speed
      ▼                                           ▼
  short-GOP proxy (fast exact seeks)      ffmpeg-next export (from the ORIGINAL source)
```

Playback goes through GStreamer, and exports always read the original file, never the proxy. The design history (three playback architectures and why two were dropped) is in [`PLAN.md`](PLAN.md).

<div align="center">
<sub>Built with Rust · egui · GStreamer · FFmpeg · gifski</sub>
</div>
