// GUI subsystem: no console window on a standalone launch (double-click,
// context-menu "Cut with CUTV", drag-a-file-onto-the-exe). Only affects
// launches with no existing console to attach to — running via `cargo run`
// or `cutv.bat` from an already-open terminal still inherits that
// terminal's stdout/stderr, so `debug!`/`RUST_LOG` logging is unaffected
// there. Either way, logs are also always appended to `cutv.log` next to
// the .exe, so a standalone launch (no console to read) can still be
// debugged after the fact.
#![windows_subsystem = "windows"]

mod app;
mod crop;
mod export;
mod gif;
mod player;
mod probe;
mod proxy;
mod thumbs;
mod updater;
mod util;

use anyhow::{bail, ensure, Result};
use log::debug;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Writes every log line to both stderr (if a console is attached) and the
/// log file; a failed stderr write (no console) is ignored.
struct Tee(std::fs::File);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        self.0.flush()
    }
}

/// Sets up env_logger and appends to `cutv.log` next to the .exe.
/// Returns the log path if the file could be opened; always initializes
/// stderr-only logging as a fallback if it couldn't.
fn init_logging() -> Option<PathBuf> {
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("cutv.log")));

    let file = log_path
        .as_ref()
        .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());

    // Default to DEBUG so all log::debug! calls are visible.
    // Override with RUST_LOG=info for quieter output.
    let mut builder = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("debug"),
    );

    match file {
        Some(mut f) => {
            let _ = writeln!(
                f,
                "\n===== session start {} =====",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            );
            builder.target(env_logger::Target::Pipe(Box::new(Tee(f))));
            builder.init();
            log_path
        }
        None => {
            builder.init();
            None
        }
    }
}

fn main() -> Result<()> {
    let log_path = init_logging();

    let path = resolve_path()?;
    let info = probe::probe_video(&path)?;

    const MAX_W: u32 = 640;
    const MAX_H: u32 = 360;
    let scale = (MAX_W as f64 / info.width as f64)
        .min(MAX_H as f64 / info.height as f64)
        .min(1.0);
    let dw = (info.width  as f64 * scale) as u32;
    let dh = (info.height as f64 * scale) as u32;

    let filename = Path::new(&path)
        .file_name().unwrap_or_default()
        .to_string_lossy().to_string();

    debug!("opening: {filename}  src={}x{}  display={dw}x{dh}",
        info.width, info.height);
    if let Some(p) = &log_path {
        debug!("logging to {}", p.display());
    }

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png"))
        .expect("bundled icon-256.png should always decode");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("CUTV v{}  —  {filename}", updater::VERSION))
            .with_inner_size([dw as f32, dh as f32 + 255.0])
            .with_min_inner_size([360.0, 280.0])
            .with_resizable(true)
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "CUTV",
        options,
        Box::new(move |cc| {
            app::set_dark_visuals(&cc.egui_ctx);
            // Requesting maximize via the viewport builder races with
            // with_inner_size/with_min_inner_size on some backends and only
            // partially maximizes; asking the real window after it exists
            // is reliable.
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            let ctx = cc.egui_ctx.clone();
            Ok(Box::new(app::CutvApp::new(
                path,
                info.duration,
                info.fps,
                info.width,
                info.height,
                ctx,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e:?}"))
}

fn resolve_path() -> Result<String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let p = &args[1];
        ensure!(Path::new(p).exists(), "file not found: {p}");
        return Ok(p.clone());
    }

    // Auto-detect most recently modified video in cwd
    let exts = ["mp4", "mov", "avi", "mkv", "webm"];
    let mut files: Vec<_> = std::fs::read_dir(".")?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path();
            p.is_file()
                && p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| exts.contains(&x.to_ascii_lowercase().as_str()))
                    .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    files.reverse();

    match files.first() {
        Some(f) => {
            let path = f.path().to_string_lossy().to_string();
            eprintln!("Opening: {}", f.file_name().to_string_lossy());
            Ok(path)
        }
        None => bail!("no video files found — pass a path as argument"),
    }
}
