//! Live YouTube check (needs network + yt-dlp + ffmpeg). Run explicitly:
//! `cargo test --test youtube_live -- --ignored --nocapture`

use std::sync::Arc;
use std::time::Duration;

use dm_core::engine::DownloadStatus;
use dm_core::DownloadManager;
use tokio::sync::RwLock;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "network: downloads a ~1 MB public YouTube video"]
async fn youtube_video_downloads_to_a_playable_file() {
    let dir = std::env::temp_dir().join(format!("dm-yt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("state")).unwrap();
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::env::set_var("DM_DATA_DIR", dir.join("state"));

    let mgr = Arc::new(RwLock::new(DownloadManager::new()));
    let job = mgr
        .write()
        .await
        .add_download(
            "https://www.youtube.com/watch?v=jNQXAC9IVRw".into(),
            dir.join("out").to_string_lossy().into(),
            None, None, None, None, None, None, None,
            Some("best".into()),
            None,
        )
        .unwrap();

    let (job, client) = {
        let m = mgr.read().await;
        (m.jobs.get(&job.id).unwrap().clone(), Arc::clone(&m.http_client))
    };
    let id = job.id;
    let res = tokio::time::timeout(
        Duration::from_secs(240),
        DownloadManager::run_download(mgr.clone(), job, client, None, None, None),
    )
    .await
    .expect("timed out");
    let done = mgr.read().await.jobs.get(&id).unwrap().clone();
    println!("status={:?} path={} bytes={} err={:?} res={:?}", done.status, done.save_path, done.downloaded_bytes, done.error, res.as_ref().err());
    res.expect("youtube download failed");
    assert_eq!(done.status, DownloadStatus::Completed);
    let len = std::fs::metadata(&done.save_path).expect("output file missing").len();
    assert!(len > 50_000, "output suspiciously small: {len}");
    println!("OK {} ({} bytes)", done.save_path, len);
    let _ = std::fs::remove_dir_all(&dir);
}
