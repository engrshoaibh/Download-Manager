use anyhow::Result;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod api;
mod engine;
mod io;
mod network;

pub use engine::manager::DownloadManager;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive("dm_core=debug".parse()?))
        .init();

    info!("🚀 DM Daemon starting on 127.0.0.1:7373");

    // Initialize the download manager (shared state)
    let manager = DownloadManager::new();
    let manager = std::sync::Arc::new(tokio::sync::RwLock::new(manager));

    // Start the API server (REST + WebSocket)
    api::serve(manager).await?;

    Ok(())
}
