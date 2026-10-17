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
    let muxed = match builtin_mux(&video_tmp, &audio_tmp, &mux_tmp, &ext).await {
        Ok(()) => Ok(()),
        Err(why) => {
            info!("Built-in muxer not used ({why}); falling back to FFmpeg");
            ffmpeg_mux(&video_tmp, &audio_tmp, &mux_tmp).await
        }
    };

    match muxed {
        Ok(()) => {
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
        Err((code, message)) => {
            let _ = std::fs::remove_file(&mux_tmp);
            let _ = write_message(&NativeMessageEvent::Error {
                task_id: payload.task_id.clone(),
                error_code: code.into(),
                message,
            });
        }
    }
}

/// Join the streams with the built-in muxer when the inputs and the requested
/// extension allow it (MP4 into .mp4, WebM into .webm/.mkv). No FFmpeg needed.
async fn builtin_mux(video: &std::path::Path, audio: &std::path::Path, out: &std::path::Path, ext: &str) -> Result<(), String> {
    use dm_core::mux::{self, Container};
    let (video, audio, out, ext) = (video.to_path_buf(), audio.to_path_buf(), out.to_path_buf(), ext.to_ascii_lowercase());
    tokio::task::spawn_blocking(move || {
        let wanted = match ext.as_str() {
            "mp4" | "m4v" => Container::Mp4,
            "webm" | "mkv" => Container::Webm,
            other => return Err(format!(".{other} output")),
        };
        let found = mux::detect(&video).map_err(|e| e.to_string())?;
        if found != Some(wanted) {
            return Err(format!("{found:?} input for .{ext} output"));
        }
        mux::mux_av(&video, &audio, &out, &mut |_, _| true).map(|_| ()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn ffmpeg_mux(video: &std::path::Path, audio: &std::path::Path, out: &std::path::Path) -> Result<(), (&'static str, String)> {
    let ffmpeg = dm_core::youtube::find_ffmpeg().unwrap_or_else(|| std::path::PathBuf::from("ffmpeg"));
    let mut cmd = tokio::process::Command::new(&ffmpeg);
    cmd.arg("-y")
        .arg("-i").arg(video)
        .arg("-i").arg(audio)
        .args(["-map", "0:v:0", "-map", "1:a:0", "-c", "copy"])
        .arg(out);
    // CREATE_NO_WINDOW: never flash a console window while muxing.
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    match cmd.output().await {
        Ok(o) if o.status.success() && out.exists() => Ok(()),
        Ok(o) => Err(("MUX_FAILED", String::from_utf8_lossy(&o.stderr).into_owned())),
        Err(e) => Err(("FFMPEG_MISSING", e.to_string())),
    }
}

/// Start the desktop app hidden in the tray unless it already answers on the API port,
/// then wait until it does. Lets the browser extension wake the app when it is closed.
async fn ensure_app_running() -> NativeLaunchReply {
    use std::time::Duration;

    async fn api_up() -> bool {
        tokio::time::timeout(Duration::from_millis(500), tokio::net::TcpStream::connect(("127.0.0.1", 7373)))
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false)
    }

    if api_up().await {
        return NativeLaunchReply { status: "RUNNING", message: None };
    }
    let gui = match std::env::current_exe() {
        Ok(exe) => exe.with_file_name(if cfg!(windows) { "dm-gui.exe" } else { "dm-gui" }),
        Err(e) => return NativeLaunchReply { status: "ERROR", message: Some(e.to_string()) },
    };
    if let Err(e) = spawn_detached(&gui) {
        error!("Could not start {}: {e}", gui.display());
        return NativeLaunchReply { status: "ERROR", message: Some(format!("could not start {}: {e}", gui.display())) };
    }
    info!("Started {}", gui.display());
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if api_up().await {
            return NativeLaunchReply { status: "LAUNCHED", message: None };
        }
    }
    NativeLaunchReply { status: "ERROR", message: Some("the app did not start listening within 10 seconds".into()) }
}

/// The browser kills the native host when the extension disconnects, so the app must not
/// stay in the host's process tree or job object.
fn spawn_detached(gui: &std::path::Path) -> io::Result<()> {
    let mut cmd = std::process::Command::new(gui);
    cmd.arg("--background")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
        // The browser's job may forbid breakaway; fall back to a plain detached start.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    // Own process group, so the browser killing the host's group leaves the app running.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ())
}

#[derive(Serialize)]
struct NativeLaunchReply {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // We log to stderr to avoid polluting stdout (which is used for IPC)
    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(io::stderr))
        .with(EnvFilter::from_default_env().add_directive("dm_core=info".parse()?))
        .init();

    info!("dm-native-host started");

    // Opened on the first download task only: a launch request must not need (or lock) the DB.
    let mut engine: Option<std::sync::Arc<dm_core::engine::chunked::ChunkedEngine>> = None;

    loop {
        match read_message() {
            Ok(Some(buf)) => {
                let msg_type = serde_json::from_slice::<serde_json::Value>(&buf)
                    .ok()
                    .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned));
                if msg_type.as_deref() == Some("launch") {
                    let reply = ensure_app_running().await;
                    let _ = write_message(&reply);
                    continue;
                }
                if let Ok(payload) = serde_json::from_slice::<NativeMessagePayload>(&buf) {
                    info!("Received payload: {}", payload.task_id);
                    if engine.is_none() {
                        let db_dir = dirs::data_local_dir()
                            .unwrap_or_else(|| std::path::PathBuf::from("."))
                            .join("download-manager").join("native_db");
                        std::fs::create_dir_all(&db_dir)?;
                        engine = Some(std::sync::Arc::new(
                            dm_core::engine::chunked::ChunkedEngine::new(&db_dir)
                                .expect("Failed to initialize chunked engine"),
                        ));
                    }
                    let engine = engine.clone().expect("engine initialized above");

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
