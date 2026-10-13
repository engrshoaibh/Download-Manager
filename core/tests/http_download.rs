//! End-to-end check of the segmented HTTP engine against a local range-capable server:
//! a plain download must be byte-identical, and a resume whose `.part` payload has gone
//! missing must restart cleanly instead of finishing with a zero-filled file.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use dm_core::engine::{DownloadStatus, Segment};
use dm_core::DownloadManager;
use tokio::sync::RwLock;

static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SIZE: usize = 6 * 1024 * 1024 + 123;

fn payload() -> Vec<u8> {
    (0..SIZE).map(|i| ((i * 31 + i / 251) % 256) as u8).collect()
}

async fn serve_file(headers: HeaderMap) -> Response {
    let data = payload();
    let total = data.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            let start: usize = a.parse().ok()?;
            let end: usize = if b.is_empty() { total - 1 } else { b.parse().ok()? };
            Some((start, end.min(total - 1)))
        });
    match range {
        Some((start, end)) if start <= end => Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
            .header(header::CONTENT_LENGTH, end - start + 1)
            .body(Body::from(data[start..=end].to_vec()))
            .unwrap(),
        _ => Response::builder()
            .status(StatusCode::OK)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, total)
            .body(Body::from(data))
            .unwrap(),
    }
}

async fn start_server() -> String {
    let app = Router::new().route("/file.bin", get(serve_file));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/file.bin")
}

fn setup_dirs(name: &str) -> std::path::PathBuf {
    // DM_DATA_DIR is process-global, so tests take TEST_LOCK and each gets its own state dir.
    let state = std::env::temp_dir().join(format!("dm-it-state-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    std::env::set_var("DM_DATA_DIR", &state);
    let dir = std::env::temp_dir().join(format!("dm-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("out")).unwrap();
    dir
}

async fn run_job(mgr: &Arc<RwLock<DownloadManager>>, id: dm_core::engine::JobId) -> anyhow::Result<()> {
    let (job, client) = {
        let m = mgr.read().await;
        (m.jobs.get(&id).unwrap().clone(), Arc::clone(&m.http_client))
    };
    let (ua, ck, rf) = (job.user_agent.clone(), job.cookies.clone(), job.referer.clone());
    tokio::time::timeout(
        Duration::from_secs(60),
        DownloadManager::run_download(mgr.clone(), job, client, ua, ck, rf),
    )
    .await
    .expect("download timed out")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_download_is_byte_identical() {
    let _guard = TEST_LOCK.lock().await;
    let dir = setup_dirs("plain");
    let url = start_server().await;
    let mgr = Arc::new(RwLock::new(DownloadManager::new()));
    let job = mgr
        .write()
        .await
        .add_download(
            url,
            dir.join("out").to_string_lossy().into(),
            Some(6),
            None, None, None, None, None, None, None, None,
        )
        .unwrap();

    run_job(&mgr, job.id).await.expect("download failed");

    let done = mgr.read().await.jobs.get(&job.id).unwrap().clone();
    assert_eq!(done.status, DownloadStatus::Completed);
    let saved = std::fs::read(&done.save_path).expect("final file missing");
    assert_eq!(saved.len(), SIZE);
    assert!(saved == payload(), "downloaded bytes differ from source");
    assert!(!std::path::Path::new(&format!("{}.part", done.save_path)).exists());
    assert!(!std::path::Path::new(&format!("{}.part.meta", done.save_path)).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_with_missing_part_file_restarts_instead_of_corrupting() {
    let _guard = TEST_LOCK.lock().await;
    let dir = setup_dirs("resume");
    let url = start_server().await;
    let mgr = Arc::new(RwLock::new(DownloadManager::new()));
    let job = mgr
        .write()
        .await
        .add_download(
            url,
            dir.join("out").to_string_lossy().into(),
            Some(4),
            None, None, None, None, None, None, None, None,
        )
        .unwrap();

    // Simulate "state says half done, payload file is gone".
    {
        let mut m = mgr.write().await;
        let j = m.jobs.get_mut(&job.id).unwrap();
        j.total_bytes = SIZE as u64;
        j.supports_ranges = true;
        let half = SIZE as u64 / 2;
        j.segments = vec![
            Segment { id: 0, start: 0, end: half - 1, downloaded: half, speed_bps: 0.0 },
            Segment { id: 1, start: half, end: SIZE as u64 - 1, downloaded: 0, speed_bps: 0.0 },
        ];
        j.downloaded_bytes = half;
        j.status = DownloadStatus::Paused;
    }

    run_job(&mgr, job.id).await.expect("resume failed");

    let done = mgr.read().await.jobs.get(&job.id).unwrap().clone();
    assert_eq!(done.status, DownloadStatus::Completed);
    let saved = std::fs::read(&done.save_path).expect("final file missing");
    assert!(saved == payload(), "resume produced a corrupt file");
    let _ = std::fs::remove_dir_all(&dir);
}
