use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use uuid::Uuid;

use super::error::{self, ErrorCode};
use super::ws;
use super::SharedManager;
use crate::engine::manager::DownloadManager;
use crate::engine::{DownloadStatus, DmEvent};
use crate::network::client::HttpClient;
use crate::settings::DaemonSettings;

pub async fn health() -> impl IntoResponse {
    let tools = crate::youtube::tools_health_cached();
    let status = tools.status.clone();
    let http = match status.as_str() {
        "error" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::OK,
    };
    (
        http,
        Json(serde_json::json!({
            "status": status,
            "version": "0.1.0",
            "message": tools.message,
            "youtube": {
                "yt_dlp": tools.yt_dlp,
                "ffmpeg": tools.ffmpeg,
            }
        })),
    )
}

pub async fn show_window() -> impl IntoResponse {
    Json(serde_json::json!({ "shown": crate::notify_show_window() }))
}

pub async fn get_settings(State(mgr): State<SharedManager>) -> impl IntoResponse {
    let mgr = mgr.read().await;
    Json(mgr.settings.clone())
}

pub async fn put_settings(
    State(mgr): State<SharedManager>,
    Json(settings): Json<DaemonSettings>,
) -> impl IntoResponse {
    let mut m = mgr.write().await;
    match m.update_settings(settings.clone()) {
        Ok(()) => (StatusCode::OK, Json(settings)).into_response(),
        Err(e) => error::api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::SettingsSaveFailed,
            e.to_string(),
        ),
    }
}

pub async fn list_downloads(State(mgr): State<SharedManager>) -> impl IntoResponse {
    let mgr = mgr.read().await;
    let jobs: Vec<_> = mgr.list_jobs().into_iter().cloned().collect();
    Json(jobs)
}

pub async fn select_folder() -> impl IntoResponse {
    let path = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new().pick_folder()
    }).await.unwrap_or(None);

    if let Some(p) = path {
        Json(serde_json::json!({ "path": p.to_string_lossy() }))
    } else {
        Json(serde_json::json!({ "path": None::<String> }))
    }
}

#[derive(Deserialize)]
pub struct AddDownloadRequest {
    pub url: String,
    pub save_dir: Option<String>,
    pub segments: Option<usize>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub referer: Option<String>,
    pub filename: Option<String>,
    /// Display title from the browser (used to build a real filename immediately).
    pub title: Option<String>,
    /// Known size from the extension (Content-Length / player JSON) — skips "Unknown".
    pub total_bytes: Option<u64>,
    /// Alias for total_bytes (extension convenience).
    pub size: Option<u64>,
    /// YouTube quality: best | 2160 | 1440 | 1080 | 720 | 480 | 360 | audio
    pub quality: Option<String>,
    /// Raw yt-dlp `-f` selector
    pub format: Option<String>,
}

#[derive(Deserialize)]
pub struct YoutubeProbeRequest {
    pub url: String,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub referer: Option<String>,
}

pub async fn youtube_probe(Json(req): Json<YoutubeProbeRequest>) -> impl IntoResponse {
    let opts = crate::youtube::YoutubeOptions {
        quality: None,
        format: None,
        user_agent: req.user_agent,
        cookies: req.cookies,
        referer: req.referer,
        concurrent_fragments: None,
    };
    match crate::youtube::probe(&req.url, &opts).await {
        Ok(info) => (StatusCode::OK, Json(serde_json::to_value(info).unwrap_or_default()))
            .into_response(),
        Err(e) => error::api_error(
            StatusCode::BAD_REQUEST,
            ErrorCode::ProbeFailed,
            e.to_string(),
        ),
    }
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

    let known_bytes = req.total_bytes.or(req.size).filter(|&n| n > 0);

    let job_res = {
        let mut m = mgr.write().await;
        m.add_download(
            req.url,
            save_dir,
            req.segments,
            req.user_agent.clone(),
            req.cookies.clone(),
            req.referer.clone(),
            req.filename.clone(),
            req.title.clone(),
            known_bytes,
            req.quality.clone(),
            req.format.clone(),
        )
    };

    match job_res {
        Ok(job) => {
            let id = job.id.to_string();
            crate::notify_job_added(&job.url);
            let client = {
                let m = mgr.read().await;
                std::sync::Arc::clone(&m.http_client)
            };
            spawn_job(
                mgr,
                job,
                client,
                req.user_agent,
                req.cookies,
                req.referer,
            );
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => error::api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::DownloadFailed,
            e.to_string(),
        ),
    }
}

#[derive(Deserialize)]
pub struct CancelQuery {
    #[serde(default)]
    pub delete_file: bool,
}

pub async fn cancel_download(
    Path(id): Path<Uuid>,
    Query(query): Query<CancelQuery>,
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
    let mut mgr = mgr.write().await;
    if mgr.cancel(&id, query.delete_file) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error::not_found("Job not found")
    }
}

pub async fn pause_download(
    Path(id): Path<Uuid>,
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
    let mut mgr = mgr.write().await;
    if mgr.pause(&id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error::bad_request("Download is not running")
    }
}

pub async fn resume_download(
    Path(id): Path<Uuid>,
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
    // Pause kills yt-dlp, but its run task needs a moment to wind down. Starting the
    // new run before that made the old task flip the resumed job to "failed".
    for _ in 0..150 {
        if !mgr.read().await.previous_run_active(&id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    if mgr.read().await.previous_run_active(&id) {
        return error::bad_request("Download is still stopping — try again in a moment");
    }

    let (job, client) = {
        let mut m = mgr.write().await;
        match m.begin_resume(&id) {
            Ok(job) => (job, std::sync::Arc::clone(&m.http_client)),
            Err(e) => {
                return error::bad_request(e.to_string());
            }
        }
    };

    spawn_job(
        mgr,
        job.clone(),
        client,
        job.user_agent.clone(),
        job.cookies.clone(),
        job.referer.clone(),
    );
    StatusCode::NO_CONTENT.into_response()
}

pub async fn ws_handler(
    ws_upgrade: WebSocketUpgrade,
    State(mgr): State<SharedManager>,
) -> impl IntoResponse {
    ws_upgrade.on_upgrade(move |socket| ws::handle_ws(socket, mgr))
}

fn spawn_job(
    mgr: SharedManager,
    job: crate::engine::DownloadJob,
    client: std::sync::Arc<HttpClient>,
    user_agent: Option<String>,
    cookies: Option<String>,
    referer: Option<String>,
) {
    tokio::spawn(async move {
        if let Err(e) = DownloadManager::run_download(
            mgr.clone(),
            job.clone(),
            client,
            user_agent,
            cookies,
            referer,
        )
        .await
        {
            tracing::error!("Download {} failed: {}", job.id, e);
            let mut m = mgr.write().await;
            let failed_job = if let Some(j) = m.jobs.get_mut(&job.id) {
                if j.status != DownloadStatus::Cancelled && j.status != DownloadStatus::Paused {
                    j.status = DownloadStatus::Failed;
                    j.speed_bps = 0.0;
                    // Keep the reason short enough for a list row / tooltip.
                    j.error = Some(format!("{e:#}").chars().take(400).collect());
                    Some(j.clone())
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(updated) = failed_job {
                m.events.send(DmEvent::JobUpdated { job: updated });
                m.persist();
            }
            m.events.send(DmEvent::Error {
                id: Some(job.id),
                message: e.to_string(),
            });
        }
    });
}

pub async fn get_update_status() -> impl IntoResponse {
    Json(crate::updater::get_status())
}

pub async fn trigger_update() -> impl IntoResponse {
    // Fire and forget since the update takes a while and we don't want to block the HTTP response
    tokio::spawn(async move {
        if let Err(e) = crate::updater::check_and_apply_update().await {
            tracing::error!("Manual update trigger failed: {}", e);
        }
    });
    StatusCode::ACCEPTED.into_response()
}
