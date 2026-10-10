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
                        let cancel = tokio_util::sync::CancellationToken::new();
                        let video_tmp = std::path::PathBuf::from(format!("{}_video.tmp", payload.task_id));
                        let audio_tmp = std::path::PathBuf::from(format!("{}_audio.tmp", payload.task_id));
                        
                        let video_fut = engine.download_stream(
                            &payload.task_id,
                            "video",
                            &payload.video_stream,
                            &payload.session_headers,
                            &video_tmp,
                            8,
                            cancel.clone(),
                        );
                        
                        let audio_fut = engine.download_stream(
                            &payload.task_id,
                            "audio",
                            &payload.audio_stream,
                            &payload.session_headers,
                            &audio_tmp,
                            8,
                            cancel.clone(),
                        );
                        
                        let (vid_res, aud_res) = tokio::join!(video_fut, audio_fut);
                        
                        if let Err(e) = vid_res.as_ref().or(aud_res.as_ref()) {
                            let msg = e.to_string();
                            if msg.contains("EXPIRED_URL") {
                                let _ = write_message(&NativeMessageEvent::NeedRefresh {
                                    event: "EXPIRED_URL".into(),
                                    task_id: payload.task_id.clone(),
                                    stream_type: if vid_res.is_err() { "video".into() } else { "audio".into() },
                                    last_known_offset: 0, // In full impl, this would come from the specific error
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
                        
                        // Multiplexing
                        info!("Multiplexing {}", payload.task_id);
                        let output = std::process::Command::new("ffmpeg")
                            .arg("-y")
                            .arg("-i").arg(&video_tmp)
                            .arg("-i").arg(&audio_tmp)
                            .arg("-c").arg("copy")
                            .arg(&payload.output_path)
                            .output();
                            
                        match output {
                            Ok(out) if out.status.success() => {
                                let _ = std::fs::remove_file(video_tmp);
                                let _ = std::fs::remove_file(audio_tmp);
                                
                                let _ = write_message(&NativeMessageEvent::Completed {
                                    task_id: payload.task_id.clone(),
                                    final_path: payload.output_path,
                                });
                            }
                            Ok(out) => {
                                let _ = write_message(&NativeMessageEvent::Error {
                                    task_id: payload.task_id.clone(),
                                    error_code: "MUX_FAILED".into(),
                                    message: String::from_utf8_lossy(&out.stderr).into_owned(),
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
