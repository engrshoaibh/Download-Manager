//! Native messaging registration, so the browser extension can start the app when it is
//! closed. On Windows the installer writes the registry keys; on macOS and Linux the app
//! writes the host manifest into each installed Chromium browser's config folder itself.

pub const HOST_NAME: &str = "com.dmpro.native_host";
/// Pinned by the `key` in `extension/manifest.json`.
pub const EXTENSION_ID: &str = "ekgkljpblmhpfjfnkmedhhjegphncedk";

/// The host manifest pointing at `host_path`.
pub fn manifest_json(host_path: &std::path::Path) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": HOST_NAME,
        "description": "Download Manager Pro native messaging host",
        "path": host_path.display().to_string(),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    }))
    .unwrap_or_default()
}

/// Browser profile roots (relative to the user's config dir) that read
/// `NativeMessagingHosts/<name>.json`.
#[cfg(unix)]
fn browser_dirs() -> Vec<std::path::PathBuf> {
    let Some(config) = dirs::config_dir() else { return Vec::new() };
    let names: &[&str] = if cfg!(target_os = "macos") {
        // ~/Library/Application Support/…
        &["Google/Chrome", "Google/Chrome Beta", "Chromium", "Microsoft Edge", "BraveSoftware/Brave-Browser", "Vivaldi"]
    } else {
        // ~/.config/…
        &["google-chrome", "google-chrome-beta", "chromium", "microsoft-edge", "BraveSoftware/Brave-Browser", "vivaldi"]
    };
    names.iter().map(|n| config.join(n)).collect()
}

/// Register `dm-native-host` (next to the running executable) with every installed
/// Chromium browser. Browsers that aren't installed are skipped. Returns how many
/// manifests were written or already up to date.
#[cfg(unix)]
pub fn register_for_current_exe() -> std::io::Result<usize> {
    let host = std::env::current_exe()?.with_file_name("dm-native-host");
    if !host.is_file() {
        return Ok(0);
    }
    let json = manifest_json(&host);
    let mut count = 0;
    for dir in browser_dirs().into_iter().filter(|d| d.is_dir()) {
        let target = dir.join("NativeMessagingHosts");
        std::fs::create_dir_all(&target)?;
        let file = target.join(format!("{HOST_NAME}.json"));
        if std::fs::read_to_string(&file).ok().as_deref() != Some(json.as_str()) {
            std::fs::write(&file, &json)?;
        }
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_matches_the_installer_copy() {
        let ours: serde_json::Value = serde_json::from_str(&manifest_json(std::path::Path::new("dm-native-host.exe"))).unwrap();
        let shipped: serde_json::Value = serde_json::from_str(include_str!("../../installer/com.dmpro.native_host.json")).unwrap();
        assert_eq!(ours, shipped, "keep the Windows manifest and this one in sync");
    }
}
