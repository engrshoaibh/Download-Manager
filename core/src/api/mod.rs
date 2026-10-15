pub mod error;
pub mod events;
pub mod routes;
pub mod ws;

use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;
use axum::{Router, routing::{get, post, delete}};
use tower_http::cors::{CorsLayer, Any};
use tracing::info;

use crate::engine::manager::DownloadManager;

pub type SharedManager = Arc<RwLock<DownloadManager>>;

/// Start the axum HTTP + WebSocket server on 127.0.0.1:7373
pub async fn serve(manager: SharedManager) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(routes::health))
        .route("/settings", get(routes::get_settings))
        .route("/settings", axum::routing::put(routes::put_settings))
        .route("/downloads", get(routes::list_downloads))
        .route("/downloads", post(routes::add_download))
        .route("/downloads/:id", delete(routes::cancel_download))
        .route("/downloads/:id/pause", post(routes::pause_download))
        .route("/downloads/:id/resume", post(routes::resume_download))
        .route("/youtube/probe", post(routes::youtube_probe))
        .route("/dialog/folder", get(routes::select_folder))
        .route("/update", post(routes::trigger_update))
        .route("/update/status", get(routes::get_update_status))
        .route("/window/show", post(routes::show_window))
        .route("/ws", get(routes::ws_handler))
        .layer(cors)
        .with_state(manager);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:7373").await?;
    info!("🌐 API listening on http://127.0.0.1:7373");

    axum::serve(listener, app).await?;
    Ok(())
}
