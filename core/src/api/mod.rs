pub mod routes;

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
        .route("/downloads", get(routes::list_downloads))
        .route("/downloads", post(routes::add_download))
        .route("/downloads/:id", delete(routes::cancel_download))
        .route("/ws", get(routes::ws_handler))
        .layer(cors)
        .with_state(manager);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:7373").await?;
    info!("🌐 API listening on http://127.0.0.1:7373");

    axum::serve(listener, app).await?;
    Ok(())
}
