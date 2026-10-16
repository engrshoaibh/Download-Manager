//! Native HLS/DASH downloader end to end: fixtures served by a tiny local HTTP server
//! (with byte ranges), downloaded, finished, and checked with FFmpeg when it is installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dm_core::stream::{self, Request, StreamError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stream")
}

/// Serve `fixtures()` on 127.0.0.1; `/live.m3u8` is a playlist without ENDLIST.
async fn serve() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap().to_string();
                let range = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_string))
                    .and_then(|r| {
                        let (a, b) = r.trim().split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                    });
                let body = if path == "/live.m3u8" {
                    Some(b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nhls_ts_aes/seg_0.ts\n".to_vec())
                } else {
                    std::fs::read(fixtures().join(path.trim_start_matches('/'))).ok()
                };
                let resp = match (body, range) {
                    (None, _) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    (Some(b), Some((s, e))) => {
                        let part = &b[s..=e.min(b.len() - 1)];
                        let mut r = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {s}-{e}/{}\r\nConnection: close\r\n\r\n", part.len(), b.len()).into_bytes();
                        r.extend_from_slice(part);
                        r
                    }
                    (Some(b), None) => {
                        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len()).into_bytes();
                        r.extend_from_slice(&b);
                        r
                    }
                };
                let _ = sock.write_all(&resp).await;
            });
        }
    });
    format!("http://{addr}")
}

fn work_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("dm-stream-test").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ffmpeg() -> Option<PathBuf> {
    Command::new("ffmpeg").arg("-version").output().ok().filter(|o| o.status.success()).map(|_| PathBuf::from("ffmpeg"))
}

/// (codecs, duration) of a finished file, and a full decode with no errors.
fn check_media(path: &Path, codecs: &[&str]) {
    if ffmpeg().is_none() {
        return;
    }
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_name:format=duration", "-of", "json"])
        .arg(path)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let got: Vec<_> = v["streams"].as_array().unwrap().iter().map(|s| s["codec_name"].as_str().unwrap().to_string()).collect();
    assert_eq!(got, codecs, "{}", path.display());
    let dur: f64 = v["format"]["duration"].as_str().unwrap().parse().unwrap();
    assert!((dur - 4.0).abs() < 0.3, "duration {dur}");
    let dec = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path).args(["-f", "null", "-"]).output().unwrap();
    let err = String::from_utf8_lossy(&dec.stderr);
    assert!(dec.status.success() && err.trim().is_empty(), "decode errors: {err}");
}

async fn fetch(url: String, dir: &Path) -> stream::Result<stream::Downloaded> {
    let req = Request { url, ..Default::default() };
    stream::download(&req, dir, Arc::new(AtomicBool::new(false)), &mut |_| {}).await
}

#[tokio::test]
async fn hls_fmp4_master_with_separate_audio() {
    let base = serve().await;
    let dir = work_dir("hls_fmp4");
    let dl = fetch(format!("{base}/hls_fmp4/master.m3u8"), &dir).await.unwrap();
    assert!(dl.audio.is_some(), "audio rendition should be fetched");
    // Joined by the built-in muxer: no FFmpeg passed.
    let out = stream::finish(&dl, &dir, "clip", None).await.unwrap();
    assert_eq!(out.extension().unwrap(), "mp4");
    check_media(&out, &["h264", "aac"]);
}

#[tokio::test]
async fn dash_template_timeline() {
    let base = serve().await;
    let dir = work_dir("dash");
    let dl = fetch(format!("{base}/dash/manifest.mpd"), &dir).await.unwrap();
    let out = stream::finish(&dl, &dir, "clip", None).await.unwrap();
    check_media(&out, &["h264", "aac"]);
}

#[tokio::test]
async fn hls_ts_aes128_is_decrypted() {
    let base = serve().await;
    let dir = work_dir("hls_ts_aes");
    let dl = fetch(format!("{base}/hls_ts_aes/index.m3u8"), &dir).await.unwrap();
    // Decrypted MPEG-TS: 0x47 sync byte every 188 bytes.
    let ts = std::fs::read(&dl.main).unwrap();
    assert!(ts.len() > 188 * 10 && ts.len() % 188 == 0);
    assert!(ts.chunks(188).all(|p| p[0] == 0x47), "not a clean transport stream");
    // TS → MP4 needs FFmpeg; without it the caller is told to fetch one.
    assert!(matches!(stream::finish(&dl, &dir, "clip", None).await, Err(StreamError::NeedsFfmpeg)));
    if let Some(ff) = ffmpeg() {
        let out = stream::finish(&dl, &dir, "clip", Some(&ff)).await.unwrap();
        check_media(&out, &["h264", "aac"]);
    }
}

#[tokio::test]
async fn pause_and_resume_gives_identical_output() {
    let base = serve().await;
    let url = format!("{base}/dash/manifest.mpd");
    let fresh_dir = work_dir("fresh");
    let fresh = fetch(url.clone(), &fresh_dir).await.unwrap();

    let dir = work_dir("resumed");
    let req = Request { url, ..Default::default() };
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = Arc::clone(&stop);
    let mut calls = 0;
    let first = stream::download(&req, &dir, Arc::clone(&stop), &mut |p| {
        calls += 1;
        if p.downloaded > 0 && calls >= 3 {
            s2.store(true, Ordering::Relaxed);
        }
    })
    .await;
    assert!(matches!(first, Err(StreamError::Stopped)), "{first:?}");
    stop.store(false, Ordering::Relaxed);
    let resumed = stream::download(&req, &dir, stop, &mut |_| {}).await.unwrap();

    assert_eq!(std::fs::read(&fresh.main).unwrap(), std::fs::read(&resumed.main).unwrap());
    let (fa, ra) = (fresh.audio.unwrap().0, resumed.audio.unwrap().0);
    assert_eq!(std::fs::read(fa).unwrap(), std::fs::read(ra).unwrap());
}

#[tokio::test]
async fn live_playlists_fall_back() {
    let base = serve().await;
    let res = fetch(format!("{base}/live.m3u8"), &work_dir("live")).await;
    assert!(matches!(res, Err(StreamError::Unsupported(_))), "{res:?}");
}
