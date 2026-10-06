//! In-app auto-updater. On startup a background thread reads `latest.json`
//! from the newest GitHub release, and if it names a newer version, downloads
//! the installer and checks its Ed25519 signature against `PUBKEY_B64`. Once
//! verified, `app.rs` shows an "Update & restart" button. `install_and_restart`
//! runs the installer silently, relaunches the app on the same video and exits.
//!
//! `scripts/release.mjs` publishes these files and signs them with the private
//! key at `~/.cutv/update-signing.pem`. Set `CUTV_NO_UPDATE=1` to skip the check.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{bail, Context, Result};
use base64::Engine;
use log::{debug, error, info, warn};
use serde::Deserialize;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_URL: &str = "https://github.com/sb-gravity100/cutv_rust/releases/latest/download/latest.json";
/// Raw 32-byte Ed25519 public key, base64 (pairs with ~/.cutv/update-signing.pem).
const PUBKEY_B64: &str = "pK92zoWV1Z1bIuy3M9nu4P/5Sy13gasVqo1d/9jO2A8=";

#[derive(Deserialize)]
struct Latest {
    version:   String,
    url:       String,
    signature: String,
}

/// A downloaded, signature-verified installer ready to run.
#[derive(Clone)]
pub struct ReadyUpdate {
    pub version:   String,
    pub installer: PathBuf,
}

pub type PendingUpdate = Arc<Mutex<Option<ReadyUpdate>>>;

/// Starts the background check; the slot fills only if a verified update is ready.
pub fn spawn_check(ctx: egui::Context) -> PendingUpdate {
    let slot: PendingUpdate = Arc::new(Mutex::new(None));
    if std::env::var_os("CUTV_NO_UPDATE").is_some() {
        info!("updater: disabled via CUTV_NO_UPDATE");
        return slot;
    }
    let out = slot.clone();
    thread::spawn(move || match check_and_download() {
        Ok(Some(ready)) => {
            info!("updater: v{} ready at {}", ready.version, ready.installer.display());
            *out.lock().unwrap() = Some(ready);
            ctx.request_repaint();
        }
        Ok(None) => {}
        Err(e) => warn!("updater: check failed: {e:#}"),
    });
    slot
}

fn check_and_download() -> Result<Option<ReadyUpdate>> {
    debug!("updater: checking {LATEST_URL} (current v{VERSION})");
    let latest: Latest = ureq::get(LATEST_URL)
        .call().context("fetching latest.json")?
        .into_json().context("parsing latest.json")?;

    if !is_newer(&latest.version, VERSION) {
        debug!("updater: up to date (latest v{})", latest.version);
        return Ok(None);
    }
    info!("updater: v{} available, downloading {}", latest.version, latest.url);

    let mut bytes = Vec::new();
    ureq::get(&latest.url).call().context("downloading installer")?
        .into_reader().read_to_end(&mut bytes).context("reading installer")?;
    debug!("updater: downloaded {} bytes", bytes.len());

    verify(&bytes, &latest.signature)?;
    debug!("updater: signature ok");

    let dir = std::env::temp_dir().join("cutv-update");
    std::fs::create_dir_all(&dir)?;
    let installer = dir.join(format!("cutv-setup-{}.exe", latest.version));
    std::fs::write(&installer, &bytes).context("saving installer")?;
    Ok(Some(ReadyUpdate { version: latest.version, installer }))
}

fn verify(data: &[u8], sig_b64: &str) -> Result<()> {
    use ed25519_dalek::{Signature, VerifyingKey};
    let b64 = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = b64.decode(PUBKEY_B64)?.try_into()
        .map_err(|_| anyhow::anyhow!("bad public key length"))?;
    let sig: [u8; 64] = b64.decode(sig_b64.trim()).context("decoding signature")?.try_into()
        .map_err(|_| anyhow::anyhow!("bad signature length"))?;
    let key = VerifyingKey::from_bytes(&key)?;
    if key.verify_strict(data, &Signature::from_bytes(&sig)).is_err() {
        error!("updater: installer signature INVALID — refusing to install");
        bail!("installer signature invalid");
    }
    Ok(())
}

fn parse_ver(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim_start_matches('v').split('.').map(|p| p.parse().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_ver(latest), parse_ver(current)), (Some(l), Some(c)) if l > c)
}

/// Runs the installer silently, relaunches on `video`, then exits this process.
pub fn install_and_restart(update: &ReadyUpdate, video: &str) -> ! {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cutv_rust.exe"));
    info!("updater: installing v{} and relaunching {}", update.version, exe.display());
    // `start /wait` blocks until the installer finishes; `&&` relaunches only on success.
    let cmd = format!(
        r#"/C start "" /wait "{}" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS && start "" "{}" "{}""#,
        update.installer.display(), exe.display(), Path::new(video).display(),
    );
    if let Err(e) = std::process::Command::new("cmd")
        .raw_arg(cmd)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        error!("updater: failed to launch installer: {e}");
        std::process::exit(1);
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::is_newer;
    #[test]
    fn version_compare() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}
