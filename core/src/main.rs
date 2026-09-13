use anyhow::Result;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use dm_core::{api, engine, io, network, persist, settings, youtube, DownloadManager};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive("dm_core=debug".parse()?))
        .init();

    info!("🚀 DM Daemon starting on 127.0.0.1:7373");
    info!("Job state: {}", persist::state_path().display());

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

    let manager = DownloadManager::load_or_new();
    let manager = std::sync::Arc::new(tokio::sync::RwLock::new(manager));

    api::serve(manager).await?;

    Ok(())
}