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

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub state: String, // "up_to_date", "checking", "downloading", "ready_to_restart", "error"
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

/// Release asset matching the running executable (`dm-gui`, falling back to `dm-daemon`).
fn own_asset_name() -> String {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let stem = env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default();
    let base = if stem == "dm-gui" { "dm-gui" } else { "dm-daemon" };
    format!("{base}{ext}")
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

pub async fn check_and_apply_update() -> Result<bool> {
    set_status("checking", None, None);

    let client = reqwest::Client::builder()
        .user_agent("dm-daemon-updater/0.1")
        .timeout(Duration::from_secs(30))
        .build()?;

    let url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);
    let release: GithubRelease = match client.get(&url).send().await {
        Ok(res) if res.status().is_success() => res.json().await.context("Parse GitHub release")?,
        Ok(res) => bail!("GitHub API returned {}", res.status()),
        Err(e) => bail!("Failed to fetch release info: {}", e),
    };

    let latest_version = release.tag_name.trim_start_matches('v');
    if latest_version == APP_VERSION {
        set_status("up_to_date", Some(APP_VERSION.to_string()), Some("You are on the latest version".into()));
        return Ok(false);
    }

    // Update whichever binary is running (dm-gui or dm-daemon), never swap one for the other.
    let asset_name = own_asset_name();
    let asset = release.assets.iter().find(|a| a.name == asset_name);

    let download_url = match asset {
        Some(a) => &a.browser_download_url,
        None => {
            let msg = format!("Release {} does not contain asset {}", latest_version, asset_name);
            set_status("error", None, Some(msg.clone()));
            bail!(msg);
        }
    };

    set_status("downloading", Some(latest_version.to_string()), Some(format!("Downloading {}", latest_version)));

    let bytes = client.get(download_url).send().await?.error_for_status()?.bytes().await?;
    
    if bytes.len() < 1_000_000 {
        set_status("error", None, Some("Downloaded binary is too small".into()));
        bail!("Downloaded binary looks too small");
    }

    let current_exe = env::current_exe().context("Failed to get current executable path")?;
    let dir = current_exe.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp = dir.join(format!("{}.partial", asset_name));
    let bak = dir.join(format!("{}.bak", asset_name));

    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }

    std::fs::write(&tmp, &bytes).context("Failed to write partial binary")?;
    
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms)?;
    }

    if current_exe.exists() {
        let _ = std::fs::remove_file(&bak);
        std::fs::rename(&current_exe, &bak).context("Failed to rename current executable to .bak")?;
    }

    if let Err(e) = std::fs::rename(&tmp, &current_exe) {
        let _ = std::fs::rename(&bak, &current_exe);
        set_status("error", None, Some("Failed to replace binary".into()));
        bail!("Failed to replace binary: {}", e);
    }

    set_status("ready_to_restart", Some(latest_version.to_string()), Some("Update applied, please restart".into()));
    info!("Update successfully applied: version {}", latest_version);
    Ok(true)
}

pub fn spawn_auto_updater(interval: Duration) {
    tokio::spawn(async move {
        sleep(Duration::from_secs(60)).await; // Delay first check
        loop {
            if let Err(e) = check_and_apply_update().await {
                error!("Auto-updater error: {}", e);
                let current_state = get_status().state;
                if current_state != "ready_to_restart" {
                    set_status("up_to_date", None, Some(format!("Check failed: {}", e)));
                }
            }
            sleep(interval).await;
        }
    });
}

pub fn cleanup_bak() {
    if let Ok(current_exe) = env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let asset_name = own_asset_name();
            let bak = dir.join(format!("{}.bak", asset_name));
            if bak.exists() {
                let _ = std::fs::remove_file(&bak);
            }
        }
    }
}
