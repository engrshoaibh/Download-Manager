use anyhow::{bail, Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info};

const GITHUB_REPO: &str = "engrshoaibh/Download-Manager";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where macOS/Linux users get a new version (there is no in-place installer there).
pub const RELEASES_URL: &str = "https://github.com/engrshoaibh/Download-Manager/releases/latest";

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub state: String, // "up_to_date", "checking", "downloading", "ready_to_restart", "update_available", "error"
    pub version: Option<String>,
    pub message: Option<String>,
}

lazy_static::lazy_static! {
    pub static ref UPDATE_STATUS: Arc<RwLock<UpdateStatus>> = Arc::new(RwLock::new(UpdateStatus {
        state: "up_to_date".into(),
        version: Some(APP_VERSION.to_string()),
        message: None,
    }));
}

#[derive(Deserialize, Debug)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize, Debug)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub fn get_status() -> UpdateStatus {
    UPDATE_STATUS.read().clone()
}

fn set_status(state: &str, version: Option<String>, message: Option<String>) {
    let mut status = UPDATE_STATUS.write();
    status.state = state.to_string();
    if version.is_some() {
        status.version = version;
    }
    status.message = message;
}

/// When set (the desktop app), a downloaded update waits for the user to click "Restart";
/// otherwise (headless daemon) the installer is launched as soon as it is verified.
static USER_CONFIRMS_INSTALL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Path of a downloaded, checksum-verified installer waiting to be launched.
static STAGED_INSTALLER: parking_lot::Mutex<Option<std::path::PathBuf>> = parking_lot::Mutex::new(None);

/// Make updates wait for the user instead of restarting the app on their own.
pub fn require_user_confirmation() {
    USER_CONFIRMS_INSTALL.store(true, std::sync::atomic::Ordering::Relaxed);
}

const INSTALLER_PREFIX: &str = "DownloadManagerPro-Setup";

/// Find `<hash>  <name>` for `name` in a `sha256sum`-style listing.
fn checksum_for(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.trim().to_lowercase())
    })
}

/// `1.2.3` (optionally `v`-prefixed, with a `-pre`/`+build` suffix ignored) → numbers.
fn version_parts(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// True only when `latest` is a strictly higher version than `current`, so a dev build
/// ahead of the last release is never "updated" back to it.
fn is_newer(latest: &str, current: &str) -> bool {
    let (mut a, mut b) = (version_parts(latest), version_parts(current));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Check GitHub for a newer release and download its installer. Returns `true` when a
/// verified installer has been staged (and launched, if no user confirmation is required).
pub async fn check_and_apply_update() -> Result<bool> {
    set_status("checking", None, None);

    let client = reqwest::Client::builder()
        .user_agent("download-manager-updater/0.2")
        .timeout(Duration::from_secs(120))
        .build()?;

    let url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);
    let release: GithubRelease = match client.get(&url).send().await {
        Ok(res) if res.status().is_success() => res.json().await.context("Parse GitHub release")?,
        Ok(res) => bail!("GitHub API returned {}", res.status()),
        Err(e) => bail!("Failed to fetch release info: {}", e),
    };

    let latest_version = release.tag_name.trim_start_matches('v').to_string();
    if !is_newer(&latest_version, APP_VERSION) {
        set_status("up_to_date", Some(APP_VERSION.to_string()), Some("You are on the latest version".into()));
        return Ok(false);
    }
    if !cfg!(windows) {
        // macOS/Linux builds are archives, not installers: point the user at the release.
        set_status(
            "update_available",
            Some(latest_version.clone()),
            Some(format!("Version {latest_version} is available at {RELEASES_URL}")),
        );
        return Ok(false);
    }
    if STAGED_INSTALLER.lock().is_some() {
        return Ok(true);
    }

    let installer = release
        .assets
        .iter()
        .find(|a| a.name.starts_with(INSTALLER_PREFIX) && a.name.ends_with(".exe"));
    let installer = match installer {
        Some(a) if cfg!(windows) => a,
        _ => {
            let msg = format!("Release {latest_version} has no installer for this platform");
            set_status("error", None, Some(msg.clone()));
            bail!(msg);
        }
    };
    let sums = release.assets.iter().find(|a| a.name == "SHA256SUMS.txt");
    let Some(sums) = sums else {
        set_status("error", None, Some("Release has no SHA256SUMS.txt".into()));
        bail!("Release has no SHA256SUMS.txt - refusing to run an unverified installer");
    };

    set_status("downloading", Some(latest_version.clone()), Some(format!("Downloading {latest_version}")));

    let listing = client.get(&sums.browser_download_url).send().await?.error_for_status()?.text().await?;
    let expected = checksum_for(&listing, &installer.name)
        .with_context(|| format!("{} is not listed in SHA256SUMS.txt", installer.name))?;

    let bytes = client.get(&installer.browser_download_url).send().await?.error_for_status()?.bytes().await?;
    if bytes.len() < 1_000_000 {
        set_status("error", None, Some("Downloaded installer is too small".into()));
        bail!("Downloaded installer looks too small");
    }
    let actual = sha256_hex(&bytes);
    if actual != expected {
        set_status("error", None, Some("Installer checksum mismatch".into()));
        bail!("Installer checksum mismatch (expected {expected}, got {actual})");
    }

    let dir = env::temp_dir().join("download-manager-update");
    std::fs::create_dir_all(&dir).context("create update dir")?;
    let path = dir.join(&installer.name);
    std::fs::write(&path, &bytes).context("write installer")?;
    *STAGED_INSTALLER.lock() = Some(path);

    set_status(
        "ready_to_restart",
        Some(latest_version.clone()),
        Some(format!("Version {latest_version} is ready to install")),
    );
    info!("Update {latest_version} downloaded and verified");

    if !USER_CONFIRMS_INSTALL.load(std::sync::atomic::Ordering::Relaxed) {
        launch_staged_installer()?;
    }
    Ok(true)
}

/// Run the staged installer silently; it closes this app and upgrades in place. For the
/// desktop app it also starts the new version afterwards (`/RELAUNCH=1`, see the installer).
/// The caller should exit right after.
pub fn launch_staged_installer() -> Result<()> {
    let path = STAGED_INSTALLER
        .lock()
        .clone()
        .context("no update has been downloaded")?;
    let mut cmd = std::process::Command::new(&path);
    cmd.args(["/SILENT", "/NORESTART", "/CLOSEAPPLICATIONS"]);
    if USER_CONFIRMS_INSTALL.load(std::sync::atomic::Ordering::Relaxed) {
        cmd.arg("/RELAUNCH=1");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP so the installer outlives us.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    cmd.spawn().context("start installer")?;
    set_status("installing", None, Some("Installing update...".into()));
    Ok(())
}

pub fn spawn_auto_updater(interval: Duration) {
    tokio::spawn(async move {
        sleep(Duration::from_secs(60)).await; // Delay first check
        loop {
            if let Err(e) = check_and_apply_update().await {
                error!("Auto-updater error: {}", e);
                let current_state = get_status().state;
                if current_state != "ready_to_restart" && current_state != "update_available" {
                    set_status("up_to_date", None, Some(format!("Check failed: {}", e)));
                }
            }
            sleep(interval).await;
        }
    });
}

/// Remove leftovers from the old in-place updater and from a finished installer run.
pub fn cleanup_bak() {
    if let Ok(current_exe) = env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            for name in ["dm-gui.exe.bak", "dm-daemon.exe.bak", "dm-gui.exe.partial", "dm-daemon.exe.partial"] {
                let _ = std::fs::remove_file(dir.join(name));
            }
        }
    }
    let _ = std::fs::remove_dir_all(env::temp_dir().join("download-manager-update"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_checksum_by_file_name() {
        let listing = "aaa111  dm-extension.zip\nBBB222 *DownloadManagerPro-Setup-0.1.16.exe\n";
        assert_eq!(checksum_for(listing, "DownloadManagerPro-Setup-0.1.16.exe").as_deref(), Some("bbb222"));
        assert_eq!(checksum_for(listing, "missing.exe"), None);
    }

    #[test]
    fn only_newer_versions_count() {
        assert!(is_newer("0.1.16", "0.1.15"));
        assert!(is_newer("v0.2.0", "0.1.99"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.15", "0.1.15"));
        assert!(!is_newer("0.1.15", "0.1.16"), "never downgrade a newer build");
        assert!(!is_newer("0.1.15-beta", "0.1.15"));
        assert!(!is_newer("0.1.15", "0.1.15.0"));
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
