use axum::{
    extract::{Path, State, WebSocketUpgrade},
    extract::ws::{Message, WebSocket},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::SharedManager;

// ─── Health ──────────────────────────────────────────────────────────────────

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "version": "0.1.0" }))
}

// ─── List Downloads ───────────────────────────────────────────────────────────

pub async fn list_downloads(State(mgr): State<SharedManager>) -> impl IntoResponse {
    let mgr = mgr.read().await;
    let jobs: Vec<_> = mgr.list_jobs().into_iter().cloned().collect();
    Json(jobs)
}

// ─── Add Download ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddDownloadRequest {
    pub url: String,
    pub save_dir: Option<String>,
    pub segments: Option<usize>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
}

#[derive(Serialize)]
pub struct AddDownloadResponse {
    pub id: Uuid,
}

pub async fn add_download(
    State(mgr): State<SharedManager>,
    Json(req): Json<AddDownloadRequest>,
) -> impl IntoResponse {
    let save_dir = req.save_dir.unwrap_or_else(|| {
        dirs::download_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .to_string_lossy()
            .to_string()
    });

    let job_res = {
        let mut m = mgr.write().await;
        m.add_download(req.url, save_dir, req.segments)
    };

    match job_res {
        Ok(job) => {
            let id = job.id.to_string();
            // Spawn the core execution task
            let mgr_clone = mgr.clone();
            let ua_clone = req.user_agent;
            let cookies_clone = req.cookies;
            
            // We need an HttpClient to pass to the task. 
            // We can construct a fresh one here or borrow it from the manager? 
            // Manager no longer gives public access to http_client, so let's instantiate one
            // or just make http_client public.
            // Wait, we can get it from the manager.
            let client = {
                let m = mgr.read().await;
                std::sync::Arc::clone(&m.http_client) // Need to make http_client pub in manager.rs
            };

            tokio::spawn(async move {
                if let Err(e) = crate::engine::manager::DownloadManager::run_download(
                    mgr_clone.clone(),
                    job.clone(),
                    client,
                    ua_clone,
                    cookies_clone,
                ).await {
                    tracing::error!("Download {} failed: {}", job.id, e);
                    let m = mgr_clone.read().await;
                    let _ = m.event_tx.send(crate::engine::DmEvent::Error {
                        id: Some(job.id),
                        message: e.to_string(),
                    });
                }
            });

            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ─── Cancel Download ──────────────────────────────────────────────────────────
// NOTE: Path extractor must come before State in axum 0.7 for DELETE routes.

pub async fn cancel_download(
    Path(id): Path<Uuid>,
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
    let mut mgr = mgr.write().await;
    if mgr.cancel(&id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "Job not found" }))).into_response()
    }
}

// ─── WebSocket Handler ────────────────────────────────────────────────────────

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, mgr))
}

async fn handle_ws(socket: WebSocket, mgr: SharedManager) {
    let mut rx = {
        let mgr = mgr.read().await;
        mgr.subscribe()
    };

    let (mut sender, mut receiver) = socket.split();

    // Send current jobs snapshot immediately on connect
    {
        let mgr = mgr.read().await;
        let jobs: Vec<_> = mgr.list_jobs().into_iter().cloned().collect();
        let snapshot = serde_json::json!({ "type": "snapshot", "jobs": jobs });
        let _ = sender.send(Message::Text(snapshot.to_string().into())).await;
    }

    // Stream live events
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(ev) => {
                        if let Ok(json) = serde_json::to_string(&ev) {
                            if sender.send(Message::Text(json.into())).await.is_err() {
                                break; // Client disconnected
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = receiver.next() => {
                match msg {
                    None | Some(Err(_)) => break, // Client disconnected
                    Some(Ok(Message::Close(_))) => break,
                    _ => {} // Ignore other messages
                }
            }
        }
    }
}
