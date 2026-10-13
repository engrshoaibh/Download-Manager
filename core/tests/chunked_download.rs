//! The chunked (extension stream) engine must produce a byte-identical file, and must not
//! trust saved chunk state once the payload file it describes has disappeared.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use dm_core::engine::chunked::ChunkedEngine;
use dm_core::engine::native_payload::{ExtensionStream, SessionHeaders};
use tokio_util::sync::CancellationToken;

const SIZE: usize = 3 * 1024 * 1024 + 17;

fn payload() -> Vec<u8> {
    (0..SIZE).map(|i| ((i * 7 + i / 13) % 256) as u8).collect()
}

async fn serve(headers: HeaderMap) -> Response {
    let data = payload();
    let (start, end) = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?.min(SIZE - 1)))
        })
        .unwrap_or((0, SIZE - 1));
    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{SIZE}"))
        .body(Body::from(data[start..=end].to_vec()))
        .unwrap()
}

async fn server() -> String {
    let app = Router::new().route("/s", get(serve));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}/s")
}

fn headers() -> SessionHeaders {
    SessionHeaders { user_agent: "t".into(), referer: "http://x/".into(), origin: "http://x".into(), cookie: "a=b".into() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chunked_download_and_payload_removed_between_runs() {
    let dir = std::env::temp_dir().join(format!("dm-chunk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let engine = ChunkedEngine::new(&dir.join("db")).unwrap();
    let stream = ExtensionStream { url: server().await, mime_type: "video/mp4".into(), content_length: SIZE as u64 };
    let out = dir.join("v.part");

    engine
        .download_stream("t1", "video", &stream, &headers(), &out, 4, CancellationToken::new(), Arc::new(AtomicU64::new(0)))
        .await
        .expect("first download");
    assert!(std::fs::read(&out).unwrap() == payload(), "first download differs");

    // Payload deleted between runs: the engine must re-download, not trust leftover state.
    std::fs::remove_file(&out).unwrap();
    let progress = Arc::new(AtomicU64::new(0));
    engine
        .download_stream("t1", "video", &stream, &headers(), &out, 4, CancellationToken::new(), Arc::clone(&progress))
        .await
        .expect("second download");
    assert!(std::fs::read(&out).unwrap() == payload(), "second download differs");
    assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), SIZE as u64);
    let _ = std::fs::remove_dir_all(&dir);
}
