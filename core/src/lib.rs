pub mod api;
pub mod engine;
pub mod io;
pub mod network;
pub mod persist;
pub mod settings;
pub mod updater;
pub mod youtube;

pub use engine::manager::DownloadManager;

type JobAddedHook = Box<dyn Fn(&str) + Send + Sync>;
static JOB_ADDED_HOOK: std::sync::OnceLock<JobAddedHook> = std::sync::OnceLock::new();

/// The desktop app registers this to react to a new download (its URL is passed),
/// e.g. to bring its window forward. Works even when the UI is hidden in the tray.
pub fn set_job_added_hook(hook: impl Fn(&str) + Send + Sync + 'static) {
    let _ = JOB_ADDED_HOOK.set(Box::new(hook));
}

pub(crate) fn notify_job_added(url: &str) {
    if let Some(hook) = JOB_ADDED_HOOK.get() {
        hook(url);
    }
}

type ShowWindowHook = Box<dyn Fn() + Send + Sync>;
static SHOW_WINDOW_HOOK: std::sync::OnceLock<ShowWindowHook> = std::sync::OnceLock::new();

/// The desktop app registers this so a second launch can bring the running window back
/// from the tray. Returns false when no window is attached (headless daemon).
pub fn set_show_window_hook(hook: impl Fn() + Send + Sync + 'static) {
    let _ = SHOW_WINDOW_HOOK.set(Box::new(hook));
}

pub(crate) fn notify_show_window() -> bool {
    SHOW_WINDOW_HOOK.get().map(|hook| hook()).is_some()
}

use anyhow::Result;
use tracing::{info, warn};

pub async fn run_daemon() -> Result<()> {
    info!("🚀 DM Daemon starting on 127.0.0.1:7373");
    info!("Job state: {}", persist::state_path().display());

    updater::cleanup_bak();

    let tools = youtube::verify_tools_at_startup().await;
    match tools.status.as_str() {
        "ok" => info!(
            "YouTube tools OK (yt-dlp={}, ffmpeg={})",
            tools.yt_dlp.path.as_deref().unwrap_or("?"),
            tools.ffmpeg.path.as_deref().unwrap_or("?")
        ),
        "degraded" => warn!(
            "YouTube tools degraded: {}",
            tools.message.as_deref().unwrap_or("ffmpeg missing")
        ),
        _ => warn!(
            "YouTube tools unavailable: {}",
            tools.message.as_deref().unwrap_or("yt-dlp missing")
        ),
    }

    youtube::spawn_ytdlp_update_scheduler(youtube::YTDLP_UPDATE_INTERVAL);
    updater::spawn_auto_updater(std::time::Duration::from_secs(24 * 3600));

    let manager = DownloadManager::load_or_new();
    let manager = std::sync::Arc::new(tokio::sync::RwLock::new(manager));

    api::serve(manager).await?;

    Ok(())
}
