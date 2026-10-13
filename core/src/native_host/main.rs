use anyhow::Result;
use serde::Serialize;
use std::io::{self, Read, Write};
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use dm_core::engine::native_payload::{NativeMessagePayload, NativeMessageEvent};

fn read_message() -> io::Result<Option<Vec<u8>>> {
    let mut len_bytes = [0u8; 4];
    let mut stdin = io::stdin();
    
    match stdin.read_exact(&mut len_bytes) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let len = u32::from_ne_bytes(len_bytes) as usize;
    let mut buf = vec![0u8; len];
    stdin.read_exact(&mut buf)?;
    Ok(Some(buf))
}

fn write_message(msg: &impl Serialize) -> io::Result<()> {
    let mut stdout = io::stdout();
    let json = serde_json::to_vec(msg)?;
    let len = (json.len() as u32).to_ne_bytes();
    stdout.write_all(&len)?;
    stdout.write_all(&json)?;
    stdout.flush()?;
    Ok(())
}

/// Download both streams, verify them, then mux into the final file.
async fn run_task(engine: std::sync::Arc<dm_core::engine::chunked::ChunkedEngine>, payload: NativeMessagePayload) {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let out = PathBuf::from(&payload.output_path);
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Keep the working files next to the destination so resume state and payload stay together.
    let video_tmp = PathBuf::from(format!("{}.video.part", payload.output_path));
    let audio_tmp = PathBuf::from(format!("{}.audio.part", payload.output_path));
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("mkv").to_string();
    let mux_tmp = out.with_extension(format!("muxing.{ext}"));

    let cancel = tokio_util::sync::CancellationToken::new();
    let video_done = Arc::new(AtomicU64::new(0));
    let audio_done = Arc::new(AtomicU64::new(0));
    let total = (payload.video_stream.content_length + payload.audio_stream.content_length).max(1);

    let reporter = {
        let (v, a, id) = (Arc::clone(&video_done), Arc::clone(&audio_done), payload.task_id.clone());
        tokio::spawn(async move {
            let mut last = 0u64;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let done = v.load(Ordering::Relaxed) + a.load(Ordering::Relaxed);
                let _ = write_message(&NativeMessageEvent::Downloading {
                    task_id: id.clone(),
                    progress_percent: (done as f64 / total as f64 * 100.0).min(100.0),
                    download_speed_bps: done.saturating_sub(last) * 2,
                });
                last = done;
            }
        })
    };

    let (vid_res, aud_res) = tokio::join!(
        engine.download_stream(&payload.task_id, "video", &payload.video_stream, &payload.session_headers, &video_tmp, 8, cancel.clone(), video_done),
        engine.download_stream(&payload.task_id, "audio", &payload.audio_stream, &payload.session_headers, &audio_tmp, 8, cancel.clone(), audio_done),
    );
    reporter.abort();

    if let Some(e) = vid_res.as_ref().err().or(aud_res.as_ref().err()) {
        let msg = format!("{e:#}");
        if msg.contains("EXPIRED_URL") {
            let _ = write_message(&NativeMessageEvent::NeedRefresh {
                event: "EXPIRED_URL".into(),
                task_id: payload.task_id.clone(),
                stream_type: if vid_res.is_err() { "video".into() } else { "audio".into() },
                last_known_offset: 0,
            });
        } else {
            let _ = write_message(&NativeMessageEvent::Error {
                task_id: payload.task_id.clone(),
                error_code: "DOWNLOAD_FAILED".into(),
                message: msg,
            });
        }
        return;
    }

    // Both inputs must exist and be complete before we spend time muxing.
    for (path, want) in [(&video_tmp, payload.video_stream.content_length), (&audio_tmp, payload.audio_stream.content_length)] {
        let ok = std::fs::metadata(path).map(|m| m.len() == want).unwrap_or(false);
        if !ok {
            let _ = write_message(&NativeMessageEvent::Error {
                task_id: payload.task_id.clone(),
                error_code: "STATE_MISSING".into(),
                message: format!("{} is missing or incomplete after download", path.display()),
            });
            return;
        }
    }

    info!("Multiplexing {}", payload.task_id);
    let ffmpeg = dm_core::youtube::find_ffmpeg().unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let output = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i").arg(&video_tmp)
        .arg("-i").arg(&audio_tmp)
        .args(["-map", "0:v:0", "-map", "1:a:0", "-c", "copy"])
        .arg(&mux_tmp)
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() && mux_tmp.exists() => {
            let _ = std::fs::remove_file(&out);
            if let Err(e) = std::fs::rename(&mux_tmp, &out) {
                let _ = write_message(&NativeMessageEvent::Error {
                    task_id: payload.task_id.clone(),
                    error_code: "FINALIZE_FAILED".into(),
                    message: format!("could not move muxed file into place: {e}"),
                });
                return;
            }
            let _ = std::fs::remove_file(&video_tmp);
            let _ = std::fs::remove_file(&audio_tmp);
            let _ = write_message(&NativeMessageEvent::Completed {
                task_id: payload.task_id.clone(),
                final_path: payload.output_path,
            });
        }
        Ok(o) => {
            let _ = std::fs::remove_file(&mux_tmp);
            let _ = write_message(&NativeMessageEvent::Error {
                task_id: payload.task_id.clone(),
                error_code: "MUX_FAILED".into(),
                message: String::from_utf8_lossy(&o.stderr).into_owned(),
            });
        }
        Err(e) => {
            let _ = write_message(&NativeMessageEvent::Error {
                task_id: payload.task_id.clone(),
                error_code: "FFMPEG_MISSING".into(),
                message: e.to_string(),
            });
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // We log to stderr to avoid polluting stdout (which is used for IPC)
    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(io::stderr))
        .with(EnvFilter::from_default_env().add_directive("dm_core=info".parse()?))
        .init();

    info!("dm-native-host started");

    let db_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("download-manager").join("native_db");
    std::fs::create_dir_all(&db_dir)?;

    let engine = std::sync::Arc::new(
        dm_core::engine::chunked::ChunkedEngine::new(&db_dir)
            .expect("Failed to initialize chunked engine")
    );

    loop {
        match read_message() {
            Ok(Some(buf)) => {
                if let Ok(payload) = serde_json::from_slice::<NativeMessagePayload>(&buf) {
                    info!("Received payload: {}", payload.task_id);
                    let engine = engine.clone();
                    
                    tokio::spawn(async move {
                        run_task(engine, payload).await;
                    });
                }
            }
            Ok(None) => {
                info!("Stdin closed, exiting.");
                break;
            }
            Err(e) => {
                error!("Error reading from stdin: {}", e);
                break;
            }
        }
    }

    Ok(())
}
