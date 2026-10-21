//! YouTube / yt-dlp processor — resolve formats, download + FFmpeg merge, report progress.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

/// Folder ("onedir") build: one process, no Python unpacked to %TEMP% on every launch,
/// unlike the single-file yt-dlp.exe (which shows as two processes and burns CPU at start).
#[cfg(windows)]
const YT_DLP_WIN_ZIP: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_win.zip";
#[cfg(windows)]
const YT_DLP_ONEDIR: &str = "yt-dlp_win";
/// Static FFmpeg build recommended by yt-dlp (contains bin/ffmpeg.exe + ffprobe.exe).
#[cfg(windows)]
const FFMPEG_RELEASE_ZIP: &str =
    "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip";
/// Deno for yt-dlp's YouTube signature/"n" challenges (single deno.exe in the zip root).
#[cfg(windows)]
const DENO_RELEASE_ZIP: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
/// `/health` is polled by the UI, the extension and websites — remember the tool scan briefly.
const HEALTH_CACHE_TTL: Duration = Duration::from_secs(30);
/// True while a managed FFmpeg download is running (surfaced in `/health`).
static FFMPEG_FETCHING: AtomicBool = AtomicBool::new(false);

lazy_static::lazy_static! {
    static ref HEALTH_CACHE: parking_lot::Mutex<Option<(std::time::Instant, ToolsHealth)>> =
        parking_lot::Mutex::new(None);
    /// One FFmpeg download at a time, however many stream jobs start together.
    static ref FFMPEG_FETCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::new(());
    static ref DENO_FETCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::new(());
    /// Raw `yt-dlp -J` output per target URL. A YouTube extraction (player JS + challenge
    /// solving) takes tens of seconds; reusing it via `--load-info-json` avoids repeating it.
    static ref INFO_JSON_CACHE: parking_lot::Mutex<std::collections::HashMap<String, (std::time::Instant, Arc<Vec<u8>>)>> =
        parking_lot::Mutex::new(std::collections::HashMap::new());
    /// Binaries that already passed `verify_executable` this process.
    static ref VERIFIED_TOOLS: parking_lot::Mutex<std::collections::HashSet<PathBuf>> =
        parking_lot::Mutex::new(std::collections::HashSet::new());
}

/// Signed googlevideo URLs inside the info JSON stay valid for hours; stay well inside that.
const INFO_JSON_TTL: Duration = Duration::from_secs(20 * 60);
const INFO_JSON_CACHE_MAX: usize = 16;

fn cached_info_json(target: &str) -> Option<Arc<Vec<u8>>> {
    let mut cache = INFO_JSON_CACHE.lock();
    cache.retain(|_, (at, _)| at.elapsed() < INFO_JSON_TTL);
    cache.get(target).map(|(_, json)| Arc::clone(json))
}

fn store_info_json(target: &str, json: Vec<u8>) {
    let mut cache = INFO_JSON_CACHE.lock();
    cache.retain(|_, (at, _)| at.elapsed() < INFO_JSON_TTL);
    if cache.len() >= INFO_JSON_CACHE_MAX {
        if let Some(oldest) = cache.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone()) {
            cache.remove(&oldest);
        }
    }
    cache.insert(target.to_string(), (std::time::Instant::now(), Arc::new(json)));
}

fn forget_info_json(target: &str) {
    INFO_JSON_CACHE.lock().remove(target);
}

/// CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS: no console, and yt-dlp (plus the FFmpeg
/// it starts, which inherits the class) never competes with the foreground app for CPU.
#[cfg(windows)]
const YTDLP_SPAWN_FLAGS: u32 = 0x0800_0000 | 0x0000_4000;

/// Machine-readable progress line emitted by yt-dlp (`--progress-template`).
const DM_PROG_PREFIX: &str = "[DMPROG]";
const STDERR_TAIL_LINES: usize = 40;
/// Default interval between yt-dlp self-update checks.
pub const YTDLP_UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 3600);


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoutubeFormat {
    pub format_id: String,
    pub ext: String,
    pub resolution: Option<String>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    pub tbr: Option<f64>,
    pub note: Option<String>,
    pub is_video: bool,
    pub is_audio: bool,
    pub is_progressive: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoutubeInfo {
    pub id: String,
    pub title: String,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub webpage_url: String,
    pub formats: Vec<YoutubeFormat>,
}

#[derive(Debug, Clone, Default)]
pub struct YoutubeOptions {
    /// e.g. "1080", "720", "best", "audio"
    pub quality: Option<String>,
    /// Raw yt-dlp `-f` selector (overrides quality)
    pub format: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub referer: Option<String>,
    /// Parallel DASH/HLS fragment downloads (`yt-dlp -N`). Default 16.
    pub concurrent_fragments: Option<u32>,
}

/// Default parallel fragment count (Arroxy / yt-dlp multi-connection style).
pub const DEFAULT_CONCURRENT_FRAGMENTS: u32 = 16;

#[derive(Debug, Clone, Default)]
pub struct YoutubeProgress {
    pub percent: f64,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub speed_bps: f64,
    pub eta_seconds: Option<f64>,
    pub phase: String,
    pub filename: Option<String>,
    pub is_estimate: bool,
}

/// True for watch / shorts / embed / youtu.be page URLs (not googlevideo CDN).
pub fn is_youtube_page_url(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    let path = u.path().to_ascii_lowercase();

    if host == "youtu.be" {
        return path.trim_matches('/').len() >= 6;
    }
    if !(host == "youtube.com"
        || host == "www.youtube.com"
        || host == "m.youtube.com"
        || host == "music.youtube.com"
        || host.ends_with(".youtube.com"))
    {
        return false;
    }
    path.starts_with("/watch")
        || path.starts_with("/shorts/")
        || path.starts_with("/embed/")
        || path.starts_with("/live/")
        || u.query_pairs().any(|(k, _)| k == "v")
}

pub fn is_googlevideo_url(url: &str) -> bool {
    url.contains("googlevideo.com") && url.contains("videoplayback")
}

/// HLS / DASH manifests — yt-dlp downloads + remuxes these (IDM-style TS/MP4).
pub fn is_stream_manifest_url(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    let path = u.path().to_ascii_lowercase();
    if path.ends_with(".m3u8") || path.ends_with(".mpd") {
        return true;
    }
    let full = url.to_ascii_lowercase();
    full.contains("m3u8")
        || full.contains(".mpd")
        || full.contains("format=m3u8")
        || full.contains("type=m3u8")
        || full.contains("application/vnd.apple.mpegurl")
}

/// Prefer a watch-page URL for yt-dlp. Falls back to `referer` when the
/// capture URL is a googlevideo CDN stream.
pub fn resolve_youtube_target(url: &str, referer: Option<&str>) -> Option<String> {
    if is_youtube_page_url(url) {
        return Some(normalize_watch_url(url));
    }
    if is_stream_manifest_url(url) {
        return Some(url.to_string());
    }
    if let Some(r) = referer {
        if is_youtube_page_url(r) && (is_googlevideo_url(url) || url.contains("youtube.com")) {
            return Some(normalize_watch_url(r));
        }
    }
    None
}

pub fn normalize_watch_url(url: &str) -> String {
    let Ok(u) = url::Url::parse(url) else {
        return url.to_string();
    };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    let path = u.path();

    if host == "youtu.be" {
        let id = path.trim_matches('/');
        if !id.is_empty() {
            return format!("https://www.youtube.com/watch?v={id}");
        }
    }

    if path.starts_with("/shorts/") || path.starts_with("/embed/") || path.starts_with("/live/") {
        if let Some(id) = path.split('/').filter(|s| !s.is_empty()).nth(1) {
            return format!("https://www.youtube.com/watch?v={id}");
        }
    }

    if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "v") {
        return format!("https://www.youtube.com/watch?v={v}");
    }

    url.to_string()
}

/// yt-dlp `-f` for the exact stream a googlevideo URL points at (its `itag`), plus audio
/// for video-only streams. YouTube binds CDN URLs to the player session, so the app can't
/// fetch them directly; yt-dlp re-resolves the same format from the watch page.
pub fn cdn_itag_selector(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let mut itag = None;
    let mut mime = String::new();
    for (k, v) in u.query_pairs() {
        match k.as_ref() {
            "itag" => itag = v.parse::<u32>().ok(),
            "mime" => mime = v.to_ascii_lowercase(),
            _ => {}
        }
    }
    let itag = itag?;
    Some(if mime.starts_with("audio") {
        format!("{itag}/ba/b")
    } else {
        format!("{itag}+ba/{itag}/bv*+ba/b")
    })
}

pub fn format_selector(opts: &YoutubeOptions) -> String {
    if let Some(f) = explicit_format_id(opts) {
        return f;
    }
    let q = normalized_quality(opts);

    if q == "audio" || q == "audio-only" || q == "bestaudio" {
        return "ba/b".into();
    }

    // Prefer exact / near-exact height. Never put progressive `b` ahead of
    // adaptive webm/AV1 (4K is almost never muxed progressive mp4).
    if let Some(h) = parse_quality_height(&q) {
        let floor = ((h as f64) * 0.75) as u32; // e.g. 2160 → 1620 (keeps 1440+ path)
        return format!(
            "bv*[height={h}]+ba/bv*[height>={floor}][height<={h}]+ba/wv*[height={h}]+ba/wv*[height>={floor}][height<={h}]+ba/bv*[height<={h}]+ba/wv*[height<={h}]+ba/b[height>={floor}]/b"
        );
    }

    "bv*+ba/b".into()
}

/// Download an HLS/DASH manifest with [`crate::stream`] in the job's hidden work dir,
/// finish it (built-in muxer, FFmpeg only for MPEG-TS), and move it into `save_dir`.
async fn download_stream_native(
    job_id: uuid::Uuid,
    url: &str,
    save_dir: &Path,
    preferred_name: Option<&str>,
    opts: &YoutubeOptions,
    stop: &Arc<AtomicBool>,
    on_progress: &mut (impl FnMut(YoutubeProgress) + Send),
) -> std::result::Result<PathBuf, crate::stream::StreamError> {
    use crate::stream::{self, StreamError};

    std::fs::create_dir_all(save_dir).map_err(anyhow::Error::from)?;
    let mut yt_temp = HiddenYtTemp::create(save_dir, job_id)?;
    let work_dir = yt_temp.job_dir.join("native");
    std::fs::create_dir_all(&work_dir).map_err(anyhow::Error::from)?;

    let req = stream::Request {
        url: url.to_string(),
        quality: opts.quality.clone(),
        user_agent: opts.user_agent.clone(),
        cookies: opts.cookies.clone(),
        referer: opts.referer.clone(),
    };
    let started = std::time::Instant::now();
    let mut first_bytes: Option<u64> = None;
    let mut report = |p: stream::Progress| {
        // Speed over this session only (resumed bytes don't count).
        let base = *first_bytes.get_or_insert(p.downloaded);
        let secs = started.elapsed().as_secs_f64().max(0.5);
        let speed = (p.downloaded - base) as f64 / secs;
        let total = p.total_estimate.max(p.downloaded);
        on_progress(YoutubeProgress {
            percent: if total > 0 { p.downloaded as f64 * 100.0 / total as f64 } else { 0.0 },
            downloaded_bytes: p.downloaded,
            total_bytes: total,
            speed_bps: speed,
            eta_seconds: (speed > 1.0).then(|| total.saturating_sub(p.downloaded) as f64 / speed),
            phase: "downloading".into(),
            filename: None,
            is_estimate: true,
        });
    };
    let dl = stream::download(&req, &work_dir, Arc::clone(stop), &mut report).await?;

    on_progress(YoutubeProgress { phase: "merging".into(), ..Default::default() });
    let stem = preferred_name
        .and_then(|n| Path::new(n).file_stem().and_then(|s| s.to_str()))
        .map(sanitize_filename)
        .unwrap_or_else(|| "video".into());
    let finished = match stream::finish(&dl, &work_dir, &stem, None).await {
        Err(StreamError::NeedsFfmpeg) => {
            let ff = tokio::time::timeout(Duration::from_secs(600), find_or_fetch_ffmpeg())
                .await
                .map_err(|_| anyhow::anyhow!("FFmpeg download timed out"))??;
            stream::finish(&dl, &work_dir, &stem, Some(&ff)).await?
        }
        other => other?,
    };

    let visible = move_finished_to_save_dir(&finished, save_dir, preferred_name.unwrap_or(&stem))?;
    yt_temp.cleanup();
    let size = std::fs::metadata(&visible).map(|m| m.len()).unwrap_or(0);
    on_progress(YoutubeProgress {
        percent: 100.0,
        downloaded_bytes: size,
        total_bytes: size,
        eta_seconds: Some(0.0),
        phase: "done".into(),
        filename: Some(visible.display().to_string()),
        ..Default::default()
    });
    Ok(visible)
}

/// Rewrite a selector so every `video+audio` choice is a same-container pair
/// (mp4 + m4a, else webm + webm), which the built-in muxer can join without FFmpeg.
/// A plain "best" becomes a height ladder so a WebM-only top resolution still wins.
pub fn same_container_pairs(selector: &str) -> String {
    let ladder;
    let selector = if selector == "bv*+ba/b" {
        ladder = [4320, 2160, 1440, 1080, 720, 480, 360]
            .iter()
            .map(|h| format!("bv*[height>={h}]+ba"))
            .chain(["bv*+ba".to_string(), "b".to_string()])
            .collect::<Vec<_>>()
            .join("/");
        ladder.as_str()
    } else {
        selector
    };
    selector
        .split('/')
        .map(|alt| match alt.split_once('+') {
            Some((v, a)) => format!("{v}[ext=mp4]+{a}[ext=m4a]/{v}[ext=webm]+{a}[ext=webm]"),
            None => alt.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// yt-dlp without FFmpeg leaves `<stem>.f<id>.<ext>` files instead of merging. Join a
/// video/audio pair with the built-in muxer; `None` when there is nothing to join.
async fn mux_split_formats(work_dir: &Path, stem: &str) -> Result<Option<PathBuf>> {
    let prefix = format!("{stem}.f");
    let mut parts: Vec<PathBuf> = std::fs::read_dir(work_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            name.starts_with(&prefix) && matches!(ext.as_str(), "mp4" | "m4a" | "webm")
        })
        .collect();
    if parts.len() != 2 {
        return Ok(None);
    }
    parts.sort();
    let kind = crate::mux::detect(&parts[0])?;
    let ext = if kind == Some(crate::mux::Container::Webm) { "webm" } else { "mp4" };
    let out = work_dir.join(format!("{stem}.{ext}"));
    let (a, b, dest) = (parts[0].clone(), parts[1].clone(), out.clone());
    info!("Merging {} + {} with the built-in muxer", a.display(), b.display());
    tokio::task::spawn_blocking(move || crate::mux::mux_av(&a, &b, &dest, &mut |_, _| true))
        .await
        .context("muxer task")?
        .map_err(|e| anyhow::anyhow!("could not merge video and audio without FFmpeg: {e}"))?;
    for p in &parts {
        let _ = std::fs::remove_file(p);
    }
    Ok(Some(out))
}

/// Selector for direct HLS/DASH manifests — height filters often report 0p and fail.
pub fn stream_manifest_selector(opts: &YoutubeOptions) -> String {
    if let Some(f) = explicit_format_id(opts) {
        return f;
    }
    let q = normalized_quality(opts);
    if q == "audio" || q == "audio-only" || q == "bestaudio" {
        return "ba/b".into();
    }
    /* Single media playlist is already one quality; master → best ladder entry */
    "best/b".into()
}

/// yt-dlp `-S` sort: prefer requested resolution, then bitrate, then mp4-friendly.
pub fn format_sort(opts: &YoutubeOptions) -> Option<String> {
    if explicit_format_id(opts).is_some() {
        return None;
    }
    let q = normalized_quality(opts);
    if q == "audio" || q == "audio-only" || q == "bestaudio" {
        return Some("abr,ext:m4a:mp4".into());
    }
    if let Some(h) = parse_quality_height(&q) {
        return Some(format!("res:{h},br,ext:mp4:m4a"));
    }
    Some("res,br,ext:mp4:m4a".into())
}

fn normalized_quality(opts: &YoutubeOptions) -> String {
    opts.quality
        .as_deref()
        .unwrap_or("best")
        .trim()
        .to_ascii_lowercase()
}

/// Raw yt-dlp `-f` only when it looks like a real format id / expression —
/// not container labels like "mp4" / "ts" from the extension UI.
fn explicit_format_id(opts: &YoutubeOptions) -> Option<String> {
    let f = opts.format.as_ref()?.trim();
    if f.is_empty() {
        return None;
    }
    let fl = f.to_ascii_lowercase();
    if matches!(
        fl.as_str(),
        "mp4" | "m4a" | "webm" | "mkv" | "ts" | "hls" | "dash" | "m4v"
    ) {
        return None;
    }
    Some(f.to_string())
}

fn parse_quality_height(q: &str) -> Option<u32> {
    if q == "best" || q.is_empty() || q == "audio" || q == "audio-only" || q == "bestaudio" {
        return None;
    }
    // "1080", "1080p", "1080p hd", "2160p 4k", …
    if let Some(caps) = regex_height(q) {
        return Some(caps);
    }
    q.trim_end_matches('p').parse().ok()
}

fn regex_height(q: &str) -> Option<u32> {
    let bytes = q.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let n = std::str::from_utf8(&bytes[start..i])
                .ok()?
                .parse::<u32>()
                .ok()?;
            if (144..=4320).contains(&n) {
                return Some(n);
            }
        } else {
            i += 1;
        }
    }
    None
}

pub async fn ensure_tools() -> Result<(PathBuf, Option<PathBuf>)> {
    let ytdlp = find_or_fetch_ytdlp().await?;
    verify_executable(&ytdlp, "yt-dlp").await?;
    ensure_js_runtime().await;
    let ffmpeg = find_ffmpeg();
    if let Some(ref ff) = ffmpeg {
        if let Err(e) = verify_executable(ff, "ffmpeg").await {
            warn!("FFmpeg found but not usable ({e:#}) — DASH merge may fail");
        }
    } else {
        warn!("FFmpeg not found — DASH audio/video merge may fail");
    }
    Ok((ytdlp, ffmpeg))
}

/// Startup /health preflight: confirm binaries exist and can be executed.
pub async fn verify_tools_at_startup() -> ToolsHealth {
    let mut health = tools_health();
    if !tokio::task::spawn_blocking(has_js_runtime).await.unwrap_or(true) {
        info!("No JavaScript runtime (Deno/Node/Bun) found - a managed Deno will be downloaded on first YouTube use");
    }
    if health.yt_dlp.available {
        if let Some(ref path) = health.yt_dlp.path {
            if let Err(e) = verify_executable(Path::new(path), "yt-dlp").await {
                health.yt_dlp.available = false;
                health.yt_dlp.error = Some(e.to_string());
                health.ok = false;
            }
        }
    }
    if health.ffmpeg.available {
        if let Some(ref path) = health.ffmpeg.path {
            if let Err(e) = verify_executable(Path::new(path), "ffmpeg").await {
                health.ffmpeg.available = false;
                health.ffmpeg.error = Some(e.to_string());
                // ffmpeg missing is degraded, not fatal for non-YouTube downloads
                if health.status == "ok" {
                    health.status = "degraded".into();
                }
            }
        }
    } else if health.status == "ok" {
        health.status = "degraded".into();
    }
    health
}

async fn verify_executable(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() {
        VERIFIED_TOOLS.lock().remove(path);
        bail!("{label} binary missing at {}", path.display());
    }
    if VERIFIED_TOOLS.lock().contains(path) {
        return Ok(());
    }
    verify_executable_uncached(path, label).await?;
    VERIFIED_TOOLS.lock().insert(path.to_path_buf());
    Ok(())
}

async fn verify_executable_uncached(path: &Path, label: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)?.permissions().mode();
        if mode & 0o111 == 0 {
            bail!("{label} at {} is not executable", path.display());
        }
    }
    let arg = if label == "ffmpeg" {
        "-version"
    } else {
        "--version"
    };
    
    let mut std_cmd = std::process::Command::new(path);
    std_cmd.arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std_cmd.creation_flags(0x08000000);
    }
    
    let mut cmd = Command::from(std_cmd);
    cmd.kill_on_drop(true);
    
    match tokio::time::timeout(Duration::from_secs(8), cmd.status()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => bail!("{label} exited with {status} during version check"),
        Ok(Err(e)) => bail!("{label} failed to run: {e}"),
        Err(_) => bail!("{label} version check timed out"),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub available: bool,
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsHealth {
    /// `ok` | `degraded` | `error`
    pub status: String,
    pub ok: bool,
    pub yt_dlp: ToolStatus,
    pub ffmpeg: ToolStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `tools_health()` with a short cache — the scan runs `where` several times (~1 s).
pub fn tools_health_cached() -> ToolsHealth {
    {
        let guard = HEALTH_CACHE.lock();
        if let Some((at, health)) = guard.as_ref() {
            if at.elapsed() < HEALTH_CACHE_TTL {
                return health.clone();
            }
        }
    }
    let fresh = tools_health();
    *HEALTH_CACHE.lock() = Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

fn invalidate_tools_health() {
    *HEALTH_CACHE.lock() = None;
}

/// Synchronous path presence check (no process spawn) for `/health`.
pub fn tools_health() -> ToolsHealth {
    let yt = find_ytdlp();
    let ff = find_ffmpeg();
    let yt_dlp = match yt {
        Some(p) => ToolStatus {
            available: true,
            path: Some(p.to_string_lossy().into()),
            error: None,
        },
        None => ToolStatus {
            available: false,
            path: None,
            error: Some(ytdlp_install_hint().into()),
        },
    };
    let ffmpeg = match ff {
        Some(p) => ToolStatus {
            available: true,
            path: Some(p.to_string_lossy().into()),
            error: None,
        },
        None => ToolStatus {
            available: false,
            path: None,
            error: Some(if FFMPEG_FETCHING.load(Ordering::Acquire) {
                "FFmpeg is downloading…".into()
            } else {
                "FFmpeg not found — install ffmpeg or place ffmpeg.exe in %LOCALAPPDATA%\\download-manager\\bin"
                    .into()
            }),
        },
    };

    let (status, ok, message) = if !yt_dlp.available {
        (
            "error".into(),
            false,
            Some(format!(
                "yt-dlp missing: {}",
                yt_dlp.error.clone().unwrap_or_default()
            )),
        )
    } else if !ffmpeg.available {
        (
            "degraded".into(),
            true,
            Some(
                "yt-dlp OK; FFmpeg missing — YouTube DASH merges may fail".into(),
            ),
        )
    } else {
        ("ok".into(), true, None)
    };

    ToolsHealth {
        status,
        ok,
        yt_dlp,
        ffmpeg,
        message,
    }
}

async fn find_or_fetch_ytdlp() -> Result<PathBuf> {
    if let Some(p) = find_ytdlp() {
        return Ok(p);
    }
    info!("yt-dlp not found — downloading a managed copy…");
    // ~18 MB: allow slow connections instead of failing the first YouTube job.
    match tokio::time::timeout(Duration::from_secs(600), fetch_ytdlp()).await {
        Ok(Ok(p)) => Ok(p),
        Ok(Err(e)) => Err(e).context(ytdlp_install_hint()),
        Err(_) => bail!("{}", ytdlp_install_hint()),
    }
}

fn ytdlp_install_hint() -> &'static str {
    "yt-dlp is required for YouTube downloads. Install it with: winget install yt-dlp.yt-dlp   OR place yt-dlp.exe in %LOCALAPPDATA%\\download-manager\\bin"
}

fn bin_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("download-manager")
        .join("bin")
}

/// The managed copy wins over PATH: it is the one the daily updater refreshes, and a stale
/// yt-dlp stops working when YouTube changes. A PATH copy is used only until the first update.
fn find_ytdlp() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let onedir = bin_dir().join(YT_DLP_ONEDIR).join("yt-dlp.exe");
        if onedir.is_file() {
            return Some(onedir);
        }
    }
    let names = ["yt-dlp.exe", "yt-dlp", "ytdlp.exe"];
    let bundled = bin_dir().join(if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    });
    if bundled.is_file() {
        return Some(bundled);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in names {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    for name in names {
        if let Ok(path) = which(name) {
            return Some(path);
        }
    }
    None
}

/// Public status helpers for `/health` (compat wrappers).
#[allow(dead_code)]
pub fn find_ytdlp_status() -> serde_json::Value {
    serde_json::to_value(&tools_health().yt_dlp).unwrap_or_default()
}

#[allow(dead_code)]
pub fn find_ffmpeg_status() -> serde_json::Value {
    serde_json::to_value(&tools_health().ffmpeg).unwrap_or_default()
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    for name in ["ffmpeg.exe", "ffmpeg"] {
        if let Ok(p) = which(name) {
            return Some(p);
        }
    }
    let bundled = bin_dir().join(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    });
    if bundled.is_file() {
        return Some(bundled);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join(if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            });
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn which(name: &str) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let mut std_cmd = std::process::Command::new("where");
        std_cmd.arg(name);
        use std::os::windows::process::CommandExt;
        std_cmd.creation_flags(0x08000000);
        
        let output = std_cmd.output()
            .context("where failed")?;
        if !output.status.success() {
            bail!("not found");
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().next().unwrap_or("").trim();
        if line.is_empty() {
            bail!("not found");
        }
        Ok(PathBuf::from(line))
    }
    #[cfg(not(windows))]
    {
        let output = std::process::Command::new("which")
            .arg(name)
            .output()
            .context("which failed")?;
        if !output.status.success() {
            bail!("not found");
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().next().unwrap_or("").trim();
        if line.is_empty() {
            bail!("not found");
        }
        Ok(PathBuf::from(line))
    }
}

#[cfg(windows)]
async fn fetch_ytdlp() -> Result<PathBuf> {
    let unpack = download_and_unpack(YT_DLP_WIN_ZIP, "yt-dlp", 5_000_000, &[]).await?;
    let result = install_ytdlp_onedir(&unpack);
    let _ = std::fs::remove_dir_all(&unpack);
    let dest = result?;
    // The single-file copy is no longer used once the folder build is in place.
    let _ = std::fs::remove_file(bin_dir().join("yt-dlp.exe"));
    VERIFIED_TOOLS.lock().clear();
    invalidate_tools_health();
    info!("Installed yt-dlp → {}", dest.display());
    Ok(dest)
}

/// Swap a freshly unpacked yt-dlp folder build into `bin/yt-dlp_win`.
#[cfg(windows)]
fn install_ytdlp_onedir(unpack: &Path) -> Result<PathBuf> {
    // The zip holds yt-dlp.exe + _internal\ either at its root or inside one folder.
    let src = if unpack.join("yt-dlp.exe").is_file() {
        unpack.to_path_buf()
    } else {
        std::fs::read_dir(unpack)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join("yt-dlp.exe").is_file())
            .context("yt-dlp.exe not found in yt-dlp_win.zip")?
    };
    let dir = bin_dir();
    let dest = dir.join(YT_DLP_ONEDIR);
    let old = dir.join(format!("{YT_DLP_ONEDIR}.old"));
    let _ = std::fs::remove_dir_all(&old);
    if dest.exists() {
        // Fails while a download is running yt-dlp; the next scheduled update retries.
        std::fs::rename(&dest, &old).context("replace yt-dlp (in use?)")?;
    }
    if let Err(e) = std::fs::rename(&src, &dest) {
        let _ = std::fs::rename(&old, &dest);
        return Err(e).context("install yt-dlp folder");
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(dest.join("yt-dlp.exe"))
}

#[cfg(not(windows))]
async fn fetch_ytdlp() -> Result<PathBuf> {
    let dir = bin_dir();
    std::fs::create_dir_all(&dir).context("create bin dir")?;
    let dest_name = "yt-dlp";
    let dest = dir.join(dest_name);
    let tmp = dir.join(format!("{dest_name}.partial"));
    let url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp";

    let client = reqwest::Client::builder()
        .user_agent("download-manager/0.1")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()?;
    let bytes = client
        .get(url)
        .send()
        .await
        .context("download yt-dlp")?
        .error_for_status()
        .context("yt-dlp HTTP error")?
        .bytes()
        .await
        .context("read yt-dlp body")?;

    if bytes.len() < 1_000_000 {
        bail!("yt-dlp download looks too small ({} bytes)", bytes.len());
    }
    // Atomic install: write temp in same dir, then rename over the live binary.
    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }
    std::fs::write(&tmp, &bytes).context("write yt-dlp partial")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms)?;
    }
    // On Windows, rename over a running yt-dlp.exe can fail — replace via delete+rename.
    if dest.exists() {
        let bak = dir.join(format!("{dest_name}.bak"));
        let _ = std::fs::remove_file(&bak);
        if std::fs::rename(&dest, &bak).is_err() {
            let _ = std::fs::remove_file(&dest);
        }
        if let Err(e) = std::fs::rename(&tmp, &dest) {
            let _ = std::fs::rename(&bak, &dest);
            return Err(e).context("finalize yt-dlp");
        }
        let _ = std::fs::remove_file(&bak);
    } else {
        std::fs::rename(&tmp, &dest).context("finalize yt-dlp")?;
    }
    info!("Installed yt-dlp → {}", dest.display());
    invalidate_tools_health();
    Ok(dest)
}

/// Stream manifests (HLS/DASH) need FFmpeg for the final MP4 remux. Returns the
/// binary already on this machine, or downloads a managed copy into the bin dir.
/// Only called for stream-manifest jobs — YouTube and plain HTTP jobs are unchanged.
async fn find_or_fetch_ffmpeg() -> Result<PathBuf> {
    if let Some(p) = find_ffmpeg() {
        return Ok(p);
    }
    let _guard = FFMPEG_FETCH_LOCK.lock().await;
    // Another job may have finished the download while we waited.
    if let Some(p) = find_ffmpeg() {
        return Ok(p);
    }
    FFMPEG_FETCHING.store(true, Ordering::Release);
    invalidate_tools_health();
    let result = fetch_ffmpeg().await;
    FFMPEG_FETCHING.store(false, Ordering::Release);
    invalidate_tools_health();
    result
}

#[cfg(not(windows))]
async fn fetch_ffmpeg() -> Result<PathBuf> {
    bail!("FFmpeg not found — install ffmpeg with your package manager")
}

/// Download a zip into the bin dir and unpack the given members with Windows' bsdtar
/// (Windows 10+ ships it, so no extra dependency). Returns the unpack directory.
#[cfg(windows)]
async fn download_and_unpack(url: &str, name: &str, min_bytes: u64, members: &[&str]) -> Result<PathBuf> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let dir = bin_dir();
    std::fs::create_dir_all(&dir).context("create bin dir")?;
    let zip = dir.join(format!("{name}.zip.partial"));
    let unpack = dir.join(format!("{name}-unpack"));
    let _ = std::fs::remove_file(&zip);
    let _ = std::fs::remove_dir_all(&unpack);

    let client = reqwest::Client::builder()
        .user_agent("download-manager/0.1")
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("download {name}"))?
        .error_for_status()
        .with_context(|| format!("{name} HTTP error"))?;

    // Tens of MB — stream to disk instead of holding it in memory.
    let mut total: u64 = 0;
    {
        let mut file = tokio::fs::File::create(&zip)
            .await
            .with_context(|| format!("create {name} zip"))?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("read {name} body"))?;
            total += chunk.len() as u64;
            file.write_all(&chunk).await.with_context(|| format!("write {name} zip"))?;
        }
        file.flush().await.with_context(|| format!("flush {name} zip"))?;
    }
    if total < min_bytes {
        let _ = std::fs::remove_file(&zip);
        bail!("{name} download looks too small ({total} bytes)");
    }

    std::fs::create_dir_all(&unpack).with_context(|| format!("create {name} unpack dir"))?;
    let mut std_cmd = std::process::Command::new("tar");
    std_cmd
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&unpack)
        .args(members)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    {
        use std::os::windows::process::CommandExt;
        std_cmd.creation_flags(0x08000000);
    }
    let mut cmd = Command::from(std_cmd);
    cmd.kill_on_drop(true);
    let unpacked = tokio::time::timeout(Duration::from_secs(300), cmd.status()).await;
    let _ = std::fs::remove_file(&zip);
    match unpacked {
        Ok(Ok(_)) => Ok(unpack),
        Ok(Err(e)) => {
            let _ = std::fs::remove_dir_all(&unpack);
            Err(e).with_context(|| format!("run tar to unpack {name}"))
        }
        Err(_) => {
            let _ = std::fs::remove_dir_all(&unpack);
            bail!("unpacking {name} timed out");
        }
    }
}

#[cfg(windows)]
async fn fetch_ffmpeg() -> Result<PathBuf> {
    let dir = bin_dir();
    info!("FFmpeg not found — downloading a managed copy…");
    let unpack = download_and_unpack(
        FFMPEG_RELEASE_ZIP,
        "ffmpeg",
        20_000_000,
        &["*/bin/ffmpeg.exe", "*/bin/ffprobe.exe"],
    )
    .await?;

    // The archive has one top-level folder: <build-name>/bin/ffmpeg.exe
    let mut found: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(&unpack) {
        for entry in entries.flatten() {
            let bin = entry.path().join("bin");
            if bin.join("ffmpeg.exe").is_file() {
                found = Some(bin);
                break;
            }
        }
    }
    let Some(bin) = found else {
        let _ = std::fs::remove_dir_all(&unpack);
        bail!("ffmpeg.exe was not found in the downloaded archive");
    };

    let dest = dir.join("ffmpeg.exe");
    for name in ["ffmpeg.exe", "ffprobe.exe"] {
        let from = bin.join(name);
        if !from.is_file() {
            continue;
        }
        let to = dir.join(name);
        let _ = std::fs::remove_file(&to);
        std::fs::rename(&from, &to).with_context(|| format!("install {name}"))?;
    }
    let _ = std::fs::remove_dir_all(&unpack);

    verify_executable(&dest, "ffmpeg").await?;
    info!("Installed FFmpeg → {}", dest.display());
    Ok(dest)
}

/// Download the latest yt-dlp into the managed bin dir (atomic swap).
/// Returns `true` if the binary was replaced.
pub async fn update_ytdlp() -> Result<bool> {
    let before = find_ytdlp().and_then(|p| std::fs::metadata(&p).ok().map(|m| m.len()));
    let path = fetch_ytdlp().await?;
    let after = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let changed = before != Some(after);
    if changed {
        info!("yt-dlp updated ({} → {} bytes)", before.unwrap_or(0), after);
    } else {
        info!("yt-dlp refresh complete (unchanged size {after})");
    }
    Ok(changed)
}

/// Background task: periodically refresh the managed yt-dlp binary.
pub fn spawn_ytdlp_update_scheduler(interval: Duration) {
    tokio::spawn(async move {
        // Let the API come up before the first network hit.
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            match update_ytdlp().await {
                Ok(true) => info!("Scheduled yt-dlp self-update applied"),
                Ok(false) => info!("Scheduled yt-dlp check: already current"),
                Err(e) => warn!("Scheduled yt-dlp update failed: {e:#}"),
            }
            tokio::time::sleep(interval).await;
        }
    });
}

/// True when `cookies` is already a full Netscape cookie file (as exported by the browser
/// extension) rather than a flat `Cookie:` header string.
fn is_cookie_jar(cookies: &str) -> bool {
    let t = cookies.trim_start();
    t.starts_with("# Netscape HTTP Cookie File") || t.starts_with("# HTTP Cookie File")
}

fn write_netscape_cookies(cookies: &str, path: &Path) -> Result<()> {
    // A real jar keeps each cookie's true domain, path, expiry and flags - the lossy
    // header-string conversion below is what made extension cookies unreliable.
    if is_cookie_jar(cookies) {
        let mut jar = cookies.trim().to_string();
        jar.push('\n');
        std::fs::write(path, jar).context("write cookie file")?;
        return Ok(());
    }
    // Netscape cookie file header + synthetic youtube.com cookies from Cookie header.
    let mut out = String::from("# Netscape HTTP Cookie File\n# Generated by download-manager\n");
    for part in cookies.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, value) = match part.split_once('=') {
            Some((n, v)) => (n.trim(), v.trim()),
            None => continue,
        };
        if name.is_empty() {
            continue;
        }
        // domain, flag, path, secure, expiry, name, value
        out.push_str(&format!(
            ".youtube.com\tTRUE\t/\tTRUE\t0\t{name}\t{value}\n"
        ));
    }
    std::fs::write(path, out).context("write cookie file")?;
    Ok(())
}

/// Kill yt-dlp and every child it spawned (ffmpeg, fragment workers).
pub fn kill_process_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

pub async fn probe(url: &str, opts: &YoutubeOptions) -> Result<YoutubeInfo> {
    probe_cancellable(url, opts, None, None).await
}

pub async fn probe_cancellable(
    url: &str,
    opts: &YoutubeOptions,
    stop: Option<Arc<AtomicBool>>,
    pid_slot: Option<Arc<AtomicU32>>,
) -> Result<YoutubeInfo> {
    let (ytdlp, _) = ensure_tools().await?;
    let target = resolve_youtube_target(url, opts.referer.as_deref())
        .unwrap_or_else(|| url.to_string());

    if let Some(json) = cached_info_json(&target) {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&json) {
            info!("yt-dlp probe: reusing cached extraction for {target}");
            return parse_info(value, &target);
        }
    }

    // Prefer cookie-less first — extension Cookie headers commonly trigger
    // "The page needs to be reloaded". Fall back to cookies for age-gated media.
    // A real cookie jar from the browser avoids the bot check, so use it from the start.
    let has_jar = opts.cookies.as_deref().is_some_and(is_cookie_jar);
    let attempts: &[(bool, &str)] = if has_jar {
        &[(true, "with_cookies"), (false, "no_cookies")]
    } else {
        &[(false, "no_cookies"), (true, "with_cookies")]
    };
    let mut last_err = String::new();

    for (use_cookies, _label) in attempts {
        if *use_cookies && opts.cookies.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
            continue;
        }
        let cookie_path = if *use_cookies {
            tempfile_cookie(opts.cookies.as_deref())?
        } else {
            None
        };

        let mut std_cmd = std::process::Command::new(&ytdlp);
        std_cmd.arg("-J")
            .arg("--no-playlist")
            .arg("--no-warnings")
            .arg(&target)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
            
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            std_cmd.creation_flags(YTDLP_SPAWN_FLAGS);
        }
        
        let mut cmd = Command::from(std_cmd);
        add_js_runtime(&mut cmd);

        if let Some(ref ua) = opts.user_agent {
            if !ua.trim().is_empty() {
                cmd.arg("--user-agent").arg(ua);
            }
        }
        if let Some(ref path) = cookie_path {
            cmd.arg("--cookies").arg(path);
            if let Some(ref r) = opts.referer {
                cmd.arg("--referer").arg(r);
            }
        } else {
            apply_account_auth(&mut cmd);
        }

        let mut child = cmd.spawn().context("run yt-dlp -J")?;
        let child_pid = child.id().unwrap_or(0);
        if let Some(slot) = &pid_slot {
            slot.store(child_pid, Ordering::Release);
        }
        let mut stdout = child.stdout.take().context("yt-dlp stdout")?;
        let mut stderr = child.stderr.take().context("yt-dlp stderr")?;
        
        let out_handle = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf).await;
            buf
        });
        let err_handle = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        });

        let output = tokio::select! {
            status = child.wait() => {
                if let Some(slot) = &pid_slot {
                    slot.store(0, Ordering::Release);
                }
                let status = status.context("wait yt-dlp")?;
                let stdout_buf = out_handle.await.unwrap_or_default();
                let stderr_buf = err_handle.await.unwrap_or_default();
                std::process::Output {
                    status,
                    stdout: stdout_buf,
                    stderr: stderr_buf,
                }
            }
            _ = async {
                loop {
                    if stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed)) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            } => {
                kill_process_tree(child_pid);
                let _ = child.start_kill();
                if let Some(slot) = &pid_slot {
                    slot.store(0, Ordering::Release);
                }
                if let Some(path) = &cookie_path {
                    let _ = std::fs::remove_file(path);
                }
                bail!("stopped");
            }
        };
        if let Some(path) = cookie_path {
            let _ = std::fs::remove_file(path);
        }

        if output.status.success() {
            let json: serde_json::Value =
                serde_json::from_slice(&output.stdout).context("parse yt-dlp JSON")?;
            let info = parse_info(json, &target)?;
            store_info_json(&target, output.stdout);
            return Ok(info);
        }

        last_err = String::from_utf8_lossy(&output.stderr).trim().to_string();
    }

    bail!("yt-dlp probe failed: {}", explain_ytdlp_error(&last_err));
}

/// Account cookies from Options (cookies.txt or a browser profile), used when the job itself
/// carries none. YouTube increasingly answers anonymous yt-dlp requests with a bot check.
fn apply_account_auth(cmd: &mut Command) {
    let settings = crate::settings::load_settings();
    if let Some(file) = settings.youtube_cookies_file.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        if Path::new(file).is_file() {
            cmd.arg("--cookies").arg(file);
            return;
        }
        warn!("Configured YouTube cookies file not found: {file}");
    }
    if let Some(browser) = settings.youtube_cookies_browser.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        cmd.arg("--cookies-from-browser").arg(browser);
    }
}

fn managed_deno() -> PathBuf {
    bin_dir().join(if cfg!(windows) { "deno.exe" } else { "deno" })
}

/// First JavaScript runtime on PATH that yt-dlp supports (checked once per process).
fn path_js_runtime() -> Option<&'static str> {
    use std::sync::OnceLock;
    static FOUND: OnceLock<Option<&'static str>> = OnceLock::new();
    *FOUND.get_or_init(|| {
        ["deno", "node", "bun"].into_iter().find(|bin| {
            let mut cmd = std::process::Command::new(bin);
            cmd.arg("--version").stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            cmd.status().map(|s| s.success()).unwrap_or(false)
        })
    })
}

/// Value for yt-dlp's `--js-runtimes`. yt-dlp only enables Deno by default, so Node/Bun
/// and our managed Deno (not on PATH) must be named explicitly.
pub fn js_runtime_arg() -> Option<String> {
    let managed = managed_deno();
    if managed.is_file() {
        return Some(format!("deno:{}", managed.display()));
    }
    path_js_runtime().map(str::to_string)
}

/// Whether yt-dlp has a JavaScript runtime. Without one, recent yt-dlp cannot solve
/// YouTube's signature challenges and some or all formats go missing.
pub fn has_js_runtime() -> bool {
    js_runtime_arg().is_some()
}

fn add_js_runtime(cmd: &mut Command) {
    if let Some(rt) = js_runtime_arg() {
        cmd.arg("--js-runtimes").arg(rt);
    }
}

/// Download a managed Deno when no runtime is available, so YouTube works on a fresh
/// install. Failures are logged; yt-dlp then runs without one, as before.
async fn ensure_js_runtime() {
    if tokio::task::spawn_blocking(has_js_runtime).await.unwrap_or(true) {
        return;
    }
    let _guard = DENO_FETCH_LOCK.lock().await;
    if managed_deno().is_file() {
        return;
    }
    match tokio::time::timeout(Duration::from_secs(600), fetch_deno()).await {
        Ok(Ok(p)) => info!("Installed Deno → {}", p.display()),
        Ok(Err(e)) => warn!("Deno auto-download failed ({e:#}). {JS_RUNTIME_HINT}"),
        Err(_) => warn!("Deno auto-download timed out. {JS_RUNTIME_HINT}"),
    }
}

#[cfg(not(windows))]
async fn fetch_deno() -> Result<PathBuf> {
    bail!("no JavaScript runtime — install Deno with your package manager")
}

#[cfg(windows)]
async fn fetch_deno() -> Result<PathBuf> {
    info!("No JavaScript runtime found — downloading a managed Deno…");
    let unpack = download_and_unpack(DENO_RELEASE_ZIP, "deno", 10_000_000, &["deno.exe"]).await?;
    let from = unpack.join("deno.exe");
    let dest = managed_deno();
    let moved = if from.is_file() {
        let _ = std::fs::remove_file(&dest);
        std::fs::rename(&from, &dest).context("install deno.exe")
    } else {
        Err(anyhow::anyhow!("deno.exe was not found in the downloaded archive"))
    };
    let _ = std::fs::remove_dir_all(&unpack);
    moved?;
    if let Err(e) = verify_executable(&dest, "deno").await {
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }
    Ok(dest)
}

const JS_RUNTIME_HINT: &str = "yt-dlp needs a JavaScript runtime for YouTube, and none was found. Install Deno (winget install DenoLand.Deno) or Node.js, restart the app, and try again.";

/// Turn yt-dlp's raw stderr into something the user can act on.
fn explain_ytdlp_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("not a bot") || lower.contains("sign in to confirm") {
        return format!(
            "YouTube is asking to confirm you are not a bot. Open Options and set a YouTube cookies.txt file (or a browser such as Firefox), or send the video from the browser extension so your session cookies are used. Details: {}",
            truncate_for_log(raw, 200)
        );
    }
    let missing_formats = lower.contains("no video formats found")
        || lower.contains("requested format is not available")
        || lower.contains("n challenge")
        || lower.contains("signature solving failed")
        || lower.contains("javascript runtime");
    if missing_formats && !has_js_runtime() {
        return format!("{JS_RUNTIME_HINT} Details: {}", truncate_for_log(raw, 200));
    }
    if lower.contains("429") || lower.contains("too many requests") {
        return format!("YouTube is rate limiting this connection (HTTP 429). Wait a few minutes and retry. Details: {}", truncate_for_log(raw, 200));
    }
    raw.to_string()
}

fn parse_info(json: serde_json::Value, fallback_url: &str) -> Result<YoutubeInfo> {
    let id = json
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let title = json
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("YouTube video")
        .to_string();
    let duration = json.get("duration").and_then(|v| v.as_f64());
    let thumbnail = json
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let webpage_url = json
        .get("webpage_url")
        .and_then(|v| v.as_str())
        .unwrap_or(fallback_url)
        .to_string();

    let mut formats = Vec::new();
    if let Some(arr) = json.get("formats").and_then(|v| v.as_array()) {
        for f in arr {
            let format_id = f
                .get("format_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if format_id.is_empty() {
                continue;
            }
            let vcodec = f
                .get("vcodec")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| s != "none");
            let acodec = f
                .get("acodec")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| s != "none");
            let is_video = vcodec.is_some();
            let is_audio = acodec.is_some();
            if !is_video && !is_audio {
                continue;
            }
            formats.push(YoutubeFormat {
                format_id,
                ext: f
                    .get("ext")
                    .and_then(|v| v.as_str())
                    .unwrap_or("mp4")
                    .to_string(),
                resolution: f
                    .get("resolution")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        f.get("height")
                            .and_then(|v| v.as_u64())
                            .map(|h| format!("{h}p"))
                    }),
                height: f.get("height").and_then(|v| v.as_u64()).map(|h| h as u32),
                fps: f.get("fps").and_then(|v| v.as_f64()),
                vcodec,
                acodec,
                filesize: f.get("filesize").and_then(|v| v.as_u64()),
                filesize_approx: f.get("filesize_approx").and_then(|v| v.as_u64()),
                tbr: f.get("tbr").and_then(|v| v.as_f64()),
                note: f
                    .get("format_note")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                is_video,
                is_audio,
                is_progressive: is_video && is_audio,
                url: f.get("url").and_then(|v| v.as_str()).map(|s| s.to_string()),
            });
        }
    }

    // Prefer higher resolutions first for UI lists.
    formats.sort_by(|a, b| {
        b.height
            .unwrap_or(0)
            .cmp(&a.height.unwrap_or(0))
            .then_with(|| {
                let asz = a.filesize.or(a.filesize_approx).unwrap_or(0);
                let bsz = b.filesize.or(b.filesize_approx).unwrap_or(0);
                bsz.cmp(&asz)
            })
    });

    Ok(YoutubeInfo {
        id,
        title,
        duration,
        thumbnail,
        webpage_url,
        formats,
    })
}

fn tempfile_cookie(cookies: Option<&str>) -> Result<Option<PathBuf>> {
    let Some(c) = cookies.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let dir = std::env::temp_dir().join("download-manager");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("cookies-{}.txt", uuid::Uuid::new_v4()));
    write_netscape_cookies(c, &path)?;
    Ok(Some(path))
}

fn sanitize_filename(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    s = s.trim().trim_matches('.').to_string();
    if s.len() > 180 {
        s.truncate(180);
    }
    if s.is_empty() {
        "youtube_video".into()
    } else {
        s
    }
}

/// Hidden work folder under the save directory for yt-dlp fragments / .part files.
struct HiddenYtTemp {
    job_dir: PathBuf,
}

impl HiddenYtTemp {
    fn create(save_dir: &Path, job_id: uuid::Uuid) -> Result<Self> {
        let root = save_dir.join(".dm-yt");
        std::fs::create_dir_all(&root).context("create .dm-yt")?;
        set_hidden_attr(&root);
        let job_dir = root.join(job_id.to_string());
        std::fs::create_dir_all(&job_dir).context("create yt work dir")?;
        set_hidden_attr(&job_dir);
        Ok(Self { job_dir })
    }

    pub fn cleanup(&mut self) {
        let _ = std::fs::remove_dir_all(&self.job_dir);
    }
}

pub fn cleanup_yt_temp(save_dir: &std::path::Path, job_id: uuid::Uuid) {
    let root = save_dir.join(".dm-yt");
    let job_dir = root.join(job_id.to_string());
    let _ = std::fs::remove_dir_all(&job_dir);
}

fn set_hidden_attr(path: &Path) {
    #[cfg(windows)]
    {
        // `attrib +H` is enough — Explorer hides the folder without needing winapi crates.
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("attrib")
            .arg("+H")
            .arg(path)
            .creation_flags(0x08000000)
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// Sum all file bytes under the yt-dlp work dir (fragments grow continuously).
fn sum_workdir_bytes(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_file() {
            total = total.saturating_add(meta.len());
        } else if meta.is_dir() {
            total = total.saturating_add(sum_workdir_bytes(&entry.path()));
        }
    }
    total
}

/// Move the merged file out of the hidden work dir into Downloads.
fn move_finished_to_save_dir(src: &Path, save_dir: &Path, preferred_name: &str) -> Result<PathBuf> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4");
    let stem = Path::new(preferred_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("youtube_video");
    let stem = sanitize_filename(stem);
    let mut dest = save_dir.join(format!("{stem}.{ext}"));
    if dest.exists() {
        for i in 1..1000 {
            let candidate = save_dir.join(format!("{stem} ({i}).{ext}"));
            if !candidate.exists() {
                dest = candidate;
                break;
            }
        }
    }
    if src == dest {
        return Ok(dest);
    }
    match std::fs::rename(src, &dest) {
        Ok(()) => Ok(dest),
        Err(_) => {
            std::fs::copy(src, &dest).context("copy finished youtube file")?;
            let _ = std::fs::remove_file(src);
            Ok(dest)
        }
    }
}

/// Confirm yt-dlp's chosen format is close to the requested height before spending bandwidth.
async fn assert_selected_height(
    ytdlp: &Path,
    source: &[String],
    selector: &str,
    sort: Option<&str>,
    want_height: u32,
    opts: &YoutubeOptions,
) -> Result<()> {
    let mut cmd = Command::new(ytdlp);
    add_js_runtime(&mut cmd);
    cmd.arg("--no-playlist")
        .arg("--no-warnings")
        .arg("-f")
        .arg(selector)
        .arg("--print")
        .arg("%(height)s")
        .arg("--skip-download")
        .args(source)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(s) = sort {
        cmd.arg("-S").arg(s);
    }
    if let Some(ref ua) = opts.user_agent {
        if !ua.trim().is_empty() {
            cmd.arg("--user-agent").arg(ua);
        }
    }
    // Prefer cookie-less preflight — matches the download path.
    apply_account_auth(&mut cmd);

    #[cfg(windows)]
    cmd.creation_flags(YTDLP_SPAWN_FLAGS);
    let output = cmd.output().await.context("yt-dlp format preflight")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!(
            "yt-dlp could not resolve a {}p format: {}",
            want_height,
            explain_ytdlp_error(&truncate_for_log(err.trim(), 400))
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let got = stdout
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            if t.is_empty() || t.eq_ignore_ascii_case("na") || t.eq_ignore_ascii_case("none") {
                return None;
            }
            t.parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0);

    let min_ok = ((want_height as f64) * 0.75) as u32;
    if got < min_ok {
        bail!(
            "Requested {want_height}p but yt-dlp selected {got}p (need at least {min_ok}p). \
             The site may be blocking adaptive formats — update yt-dlp or try again."
        );
    }
    info!("yt-dlp preflight OK: requested {want_height}p → selected {got}p");
    Ok(())
}

/// Run yt-dlp download with live progress callbacks. Honors `stop` for pause/cancel.
///
/// On FFmpeg/merge failure, retries the whole download once, then fails with the
/// stderr tail attached for diagnostics.
pub async fn download(
    job_id: uuid::Uuid,
    url: &str,
    save_dir: &Path,
    preferred_name: Option<&str>,
    opts: &YoutubeOptions,
    known_total: Option<u64>,
    stop: Arc<AtomicBool>,
    pid_slot: Arc<AtomicU32>,
    mut on_progress: impl FnMut(YoutubeProgress) + Send,
) -> Result<PathBuf> {
    // HLS/DASH: our own segment downloader first; yt-dlp only for what it can't handle.
    if is_stream_manifest_url(url) {
        match download_stream_native(job_id, url, save_dir, preferred_name, opts, &stop, &mut on_progress).await {
            Ok(path) => return Ok(path),
            Err(crate::stream::StreamError::Unsupported(why)) => {
                info!("Native HLS/DASH downloader skipped ({why}); using yt-dlp");
            }
            Err(crate::stream::StreamError::Stopped) => bail!("stopped"),
            Err(e) => return Err(anyhow::anyhow!("{e:#}")),
        }
    }
    let target = resolve_youtube_target(url, opts.referer.as_deref())
        .unwrap_or_else(|| url.to_string());
    let mut used_cache = cached_info_json(&target).is_some();
    let mut last_err = String::new();
    for attempt in 1..=2u32 {
        match download_once(
            job_id,
            url,
            save_dir,
            preferred_name,
            opts,
            known_total,
            Arc::clone(&stop),
            Arc::clone(&pid_slot),
            &mut on_progress,
            attempt,
        )
        .await
        {
            Ok(path) => return Ok(path),
            Err(e) if stop.load(Ordering::Relaxed) || e.to_string().contains("stopped") => {
                return Err(e);
            }
            Err(e) => {
                last_err = e.to_string();
                if attempt == 1 && used_cache {
                    warn!(
                        "yt-dlp failed with cached extraction — retrying with a fresh one: {}",
                        truncate_for_log(&last_err, 240)
                    );
                    forget_info_json(&target);
                    used_cache = false;
                    continue;
                }
                let merge = is_merge_or_ffmpeg_error(&last_err);
                if attempt == 1 && merge {
                    warn!(
                        "yt-dlp merge/ffmpeg failed (attempt 1) — retrying once: {}",
                        truncate_for_log(&last_err, 240)
                    );
                    on_progress(YoutubeProgress {
                        phase: "retrying".into(),
                        ..Default::default()
                    });
                    continue;
                }
                return Err(e);
            }
        }
    }
    bail!("{last_err}")
}

#[allow(clippy::too_many_arguments)]
async fn download_once(
    job_id: uuid::Uuid,
    url: &str,
    save_dir: &Path,
    preferred_name: Option<&str>,
    opts: &YoutubeOptions,
    known_total: Option<u64>,
    stop: Arc<AtomicBool>,
    pid_slot: Arc<AtomicU32>,
    on_progress: &mut impl FnMut(YoutubeProgress),
    attempt: u32,
) -> Result<PathBuf> {
    let (ytdlp, mut ffmpeg) = ensure_tools().await?;
    let target = resolve_youtube_target(url, opts.referer.as_deref())
        .unwrap_or_else(|| url.to_string());
    let is_hls = is_stream_manifest_url(&target);
    // FFmpeg merges YouTube's separate video/audio streams (1080p and up) and remuxes
    // stream manifests to MP4. Fetch a managed copy on first use so a fresh install works.
    if ffmpeg.is_none() {
        match tokio::time::timeout(Duration::from_secs(600), find_or_fetch_ffmpeg()).await {
            Ok(Ok(p)) => ffmpeg = Some(p),
            Ok(Err(e)) => warn!("FFmpeg auto-download failed ({e:#}) — continuing without it"),
            Err(_) => warn!("FFmpeg auto-download timed out — continuing without it"),
        }
    }
    let selector = if is_hls {
        stream_manifest_selector(opts)
    } else {
        format_selector(opts)
    };
    // Without FFmpeg, yt-dlp saves video and audio as two files; ask only for pairs in one
    // container so the built-in muxer can join them (see `mux_split_formats`).
    let builtin_merge = ffmpeg.is_none() && !is_hls;
    let selector = if builtin_merge { same_container_pairs(&selector) } else { selector };
    /* Container hint from extension (mp4/ts) — not a yt-dlp -f id */
    let container_hint = opts
        .format
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| matches!(s.as_str(), "mp4" | "m4v" | "ts" | "mpegts" | "mkv"));

    std::fs::create_dir_all(save_dir).context("create save dir")?;

    // Keep DASH fragments / .part files out of Downloads — hidden work dir only.
    let mut yt_temp = HiddenYtTemp::create(save_dir, job_id)?;
    let work_dir = yt_temp.job_dir.clone();

    let stem = if let Some(name) = preferred_name.map(str::trim).filter(|s| !s.is_empty()) {
        let raw = Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(name);
        sanitize_filename(raw)
    } else {
        "youtube_video".into()
    };
    let out_tmpl = work_dir
        .join(format!("{}.%(ext)s", stem))
        .to_string_lossy()
        .to_string();
    let final_ext = match container_hint.as_deref() {
        Some("ts") | Some("mpegts") => "ts",
        Some("mkv") => "mkv",
        _ => "mp4",
    };
    let final_hint = preferred_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let p = Path::new(s);
            if p.extension().is_some() {
                sanitize_filename(s)
            } else {
                format!("{}.{}", sanitize_filename(s), final_ext)
            }
        })
        .unwrap_or_else(|| format!("{stem}.{final_ext}"));

    // Extension Cookie headers often break yt-dlp ("page needs to be reloaded").
    // Default clients already expose the full adaptive ladder without cookies.
    let has_jar = opts.cookies.as_deref().is_some_and(is_cookie_jar);
    let cookie_path = if attempt > 1 || has_jar {
        tempfile_cookie(opts.cookies.as_deref())?
    } else {
        None
    };

    // Single progress template — dual templates caused empty stderr on some builds.
    let progress_template = format!(
        "{DM_PROG_PREFIX}%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(progress._percent_str)s"
    );

    // Reuse the probe's extraction instead of making yt-dlp re-extract the page (twice).
    let source: Vec<String> = match (!is_hls).then(|| cached_info_json(&target)).flatten() {
        Some(json) => {
            let path = work_dir.join("info.json");
            match std::fs::write(&path, json.as_slice()) {
                Ok(()) => {
                    info!("yt-dlp: using cached extraction ({} bytes)", json.len());
                    vec!["--load-info-json".into(), path.to_string_lossy().into_owned()]
                }
                Err(e) => {
                    warn!("could not write cached info JSON ({e}) — re-extracting");
                    vec![target.clone()]
                }
            }
        }
        None => vec![target.clone()],
    };

    let fragments = if is_hls {
        /* Native HLS fragment workers. 32 saturates a typical link without a second merge pass. */
        opts.concurrent_fragments.unwrap_or(32).clamp(8, 32)
    } else {
        /* YouTube DASH formats are single files: more workers only add Python CPU load. */
        opts.concurrent_fragments
            .unwrap_or(DEFAULT_CONCURRENT_FRAGMENTS)
            .clamp(1, 8)
    };

    // Fail closed for YouTube only — HLS manifests often report height=0/NA.
    if !is_hls {
        if let Some(want) = parse_quality_height(&normalized_quality(opts)) {
            if let Err(e) = assert_selected_height(
                &ytdlp,
                &source,
                &selector,
                format_sort(opts).as_deref(),
                want,
                opts,
            )
            .await
            {
                if let Some(path) = &cookie_path {
                    let _ = std::fs::remove_file(path);
                }
                return Err(e);
            }
        }
    }

    let mut cmd = Command::new(&ytdlp);
    add_js_runtime(&mut cmd);
    cmd.arg("--no-playlist")
        .arg("--no-warnings")
        .arg("--newline")
        .arg("--progress")
        .arg("--progress-template")
        .arg(&progress_template)
        // Parallel fragment fetches — same idea as Arroxy/aria2 multi-connection speed.
        .arg("-N")
        .arg(fragments.to_string())
        .arg("--concurrent-fragments")
        .arg(fragments.to_string())
        .arg("-f")
        .arg(&selector);
    if is_hls {
        /* Parallel native fragment fetch. ffmpeg-as-downloader is one connection. */
        cmd.arg("--hls-prefer-native");
        /* yt-dlp skips fragments it cannot fetch and still reports success, which
           saved shortened videos. Retry harder, then fail the job so it can be
           resumed (-c keeps the fragments already downloaded). */
        cmd.arg("--abort-on-unavailable-fragments")
            .arg("--fragment-retries")
            .arg("30")
            .arg("--retry-sleep")
            .arg("fragment:exp=1:20")
            .arg("--socket-timeout")
            .arg("30");
    }
    if !is_hls {
        if let Some(sort) = format_sort(opts) {
            cmd.arg("-S").arg(sort);
        }
    }
    match container_hint.as_deref() {
        Some("ts") | Some("mpegts") => {
            cmd.arg("--hls-use-mpegts");
        }
        Some("mkv") => {
            cmd.arg("--merge-output-format").arg("mkv");
        }
        _ if is_hls => {
            /* One ffmpeg stream-copy remux of the fragments into mp4. */
            cmd.arg("--remux-video").arg("mp4");
        }
        Some("mp4") | Some("m4v") => {
            cmd.arg("--merge-output-format").arg("mp4");
        }
        _ => {}
    }
    // Temp intermediates stay in the hidden work dir; final file is moved out after merge.
    cmd.arg("--paths")
        .arg(format!("temp:{}", work_dir.display()))
        .arg("--paths")
        .arg(format!("home:{}", work_dir.display()));
    /* Default remux to mp4 unless caller asked for TS/MKV (already set above) */
    if !matches!(container_hint.as_deref(), Some("ts") | Some("mpegts") | Some("mkv"))
        && !is_hls
    {
        cmd.arg("--merge-output-format").arg("mp4");
    }
    cmd.arg("-o")
        .arg(&out_tmpl)
        .arg("--print")
        .arg("before_dl:format_id=%(format_id)s height=%(height)s ext=%(ext)s")
        .arg("--print")
        .arg("after_move:filepath")
        .arg("--print")
        .arg("after_move:filename")
        .args(&source)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8");

    if let Some(ff) = ffmpeg.as_ref() {
        if let Some(dir) = ff.parent() {
            cmd.arg("--ffmpeg-location").arg(dir);
        }
    }
    if let Some(ref ua) = opts.user_agent {
        if !ua.trim().is_empty() {
            cmd.arg("--user-agent").arg(ua);
        }
    }
    if let Some(ref path) = cookie_path {
        cmd.arg("--cookies").arg(path);
    } else {
        apply_account_auth(&mut cmd);
    }
    if let Some(ref r) = opts.referer {
        cmd.arg("--referer").arg(r);
    }
    cmd.arg("-c");

    info!(
        "yt-dlp attempt {attempt}: {} | -f {} | cookies={} | tmp={}",
        target,
        selector,
        cookie_path.is_some(),
        work_dir.display()
    );

    // The app is a GUI process: without this Windows opens a console for yt-dlp.
    #[cfg(windows)]
    cmd.creation_flags(YTDLP_SPAWN_FLAGS);

    let mut child = cmd.spawn().context("spawn yt-dlp")?;
    let child_pid = child.id().unwrap_or(0);
    pid_slot.store(child_pid, Ordering::Release);
    let stdout = child.stdout.take().context("yt-dlp stdout")?;
    let stderr = child.stderr.take().context("yt-dlp stderr")?;

    // Dedicated readers so progress lines aren't starved by select! fairness.
    let (err_tx, mut err_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if err_tx.send(line).is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if out_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut final_path: Option<PathBuf> = None;
    let mut last_progress = YoutubeProgress {
        phase: "downloading".into(),
        total_bytes: known_total.unwrap_or(0),
        ..Default::default()
    };
    let mut stream_acc = DashProgressAccum {
        estimate_total: known_total.unwrap_or(0),
        ..Default::default()
    };
    let mut stderr_done = false;
    let mut stdout_done = false;
    let mut stderr_tail: VecDeque<String> = VecDeque::with_capacity(STDERR_TAIL_LINES);
    let stderr_lines_seen = Arc::new(AtomicU64::new(0));
    let progress_parsed = Arc::new(AtomicU64::new(0));

    let mut last_part_bytes: u64 = 0;
    let mut last_part_instant = std::time::Instant::now();
    let mut poll_ticks: u64 = 0;
    // stdout DMPROG lines carry live progress; the work-dir scan is only a fallback.
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let status = loop {
        if stop.load(Ordering::Relaxed) {
            kill_process_tree(child_pid);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            pid_slot.store(0, Ordering::Release);
            if let Some(path) = &cookie_path {
                let _ = std::fs::remove_file(path);
            }
            // Keep the work dir: Resume continues from these fragments (-c).
            // Cancel/Delete remove it in the manager (cleanup_yt_temp).
            bail!("stopped");
        }

        tokio::select! {
            _ = ticker.tick() => {
                poll_ticks += 1;
                // Sum everything in the hidden work dir (fragments + .part + partials).
                let part_bytes = sum_workdir_bytes(&work_dir);
                if part_bytes > 0 {
                    let elapsed = last_part_instant.elapsed().as_secs_f64().max(0.001);
                    let delta = part_bytes.saturating_sub(last_part_bytes);
                    let speed = if last_part_bytes == 0 {
                        0.0
                    } else if delta > 0 && elapsed >= 0.15 {
                        delta as f64 / elapsed
                    } else if last_progress.speed_bps > 1.0 {
                        last_progress.speed_bps
                    } else {
                        0.0
                    };
                    if part_bytes != last_part_bytes || poll_ticks % 2 == 0 {
                        if part_bytes != last_part_bytes {
                            last_part_bytes = part_bytes;
                            last_part_instant = std::time::Instant::now();
                        }
                        
                        // NOTE: workdir bytes (sum of all files in the hidden job dir)
                        // are intentionally NOT used to update downloaded_bytes here.
                        // For playlists, completed episode .mp4 files remain in the
                        // workdir and their cumulative size inflates the counter to
                        // 100 % long before the download is done.
                        // DashProgressAccum (via apply_dash_progress) already handles
                        // stream-to-stream transitions (video → audio, episode N → N+1)
                        // through its finished_bytes accumulator, so it is the sole
                        // authoritative source for downloaded_bytes.

                        last_progress.total_bytes = reconcile_yt_total(
                            &stream_acc,
                            last_progress.total_bytes,
                            last_progress.downloaded_bytes,
                            last_progress.is_estimate,
                        );
                        normalize_progress_fields(&mut last_progress, speed);
                        if last_progress.phase != "merging" {
                            last_progress.phase = "downloading".into();
                        }
                        on_progress(last_progress.clone());
                    }
                }
            }
            line = err_rx.recv(), if !stderr_done => {
                match line {
                    Some(line) => {
                        push_stderr_tail(&mut stderr_tail, &line);
                        let n = stderr_lines_seen.fetch_add(1, Ordering::Relaxed) + 1;
                        let parsed = parse_progress_line(&line);
                        if n <= 8 || parsed.is_some() || line.contains("ERROR") || line.contains("[Merger]") {
                        }
                        if parsed.is_some() {
                            progress_parsed.fetch_add(1, Ordering::Relaxed);
                        }
                        if let Some(p) = parsed {
                            let mut merged = apply_dash_progress(&mut stream_acc, p);

                            if merged.speed_bps <= 1.0 && last_progress.speed_bps > 1.0 {
                                merged.speed_bps = last_progress.speed_bps;
                            }
                            if merged.eta_seconds.is_none() {
                                merged.eta_seconds = last_progress.eta_seconds;
                            }
                            let speed = merged.speed_bps;
                            normalize_progress_fields(&mut merged, speed);
                            last_progress = merged;
                            on_progress(last_progress.clone());
                        } else if line.contains("[Merger]") || line.contains("[ExtractAudio]") {
                            last_progress.phase = "merging".into();
                            last_progress.speed_bps = 0.0;
                            last_progress.eta_seconds = None;
                            on_progress(last_progress.clone());
                        } else if line.contains("[download] Destination:") {
                            if let Some(path) = line.split("Destination:").nth(1) {
                                last_progress.filename = Some(path.trim().to_string());
                            }
                        }
                    }
                    None => stderr_done = true,
                }
            }
            line = out_rx.recv(), if !stdout_done => {
                match line {
                    Some(line) => {
                        let t = line.trim();
                        if t.is_empty() { continue; }
                        if let Some(p) = parse_progress_line(t) {
                            let mut merged = apply_dash_progress(&mut stream_acc, p);

                            let speed = merged.speed_bps;
                            normalize_progress_fields(&mut merged, speed);
                            last_progress = merged;
                            on_progress(last_progress.clone());
                            continue;
                        }
                        if t.starts_with("format_id=") || t.contains(" height=") {
                            info!("yt-dlp selected: {t}");
                            continue;
                        }
                        let path = PathBuf::from(t);
                        if path.exists() || (t.contains('.') && !t.contains('=')) {
                            final_path = Some(path);
                        }
                    }
                    None => stdout_done = true,
                }
            }
            status = child.wait() => {
                break status.context("wait yt-dlp")?;
            }
        }
    };
    pid_slot.store(0, Ordering::Release);

    // Drain any remaining lines after process exit.
    while let Ok(line) = err_rx.try_recv() {
        push_stderr_tail(&mut stderr_tail, &line);
        if let Some(p) = parse_progress_line(&line) {
            let mut merged = apply_dash_progress(&mut stream_acc, p);

            let speed = merged.speed_bps;
            normalize_progress_fields(&mut merged, speed);
            last_progress = merged;
            on_progress(last_progress.clone());
        }
    }
    while let Ok(line) = out_rx.try_recv() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("format_id=") || t.contains(" height=") {
            continue;
        }
        let path = PathBuf::from(t);
        if path.exists() || (t.contains('.') && !t.contains('=')) {
            final_path = Some(path);
        }
    }
    if let Some(path) = cookie_path {
        let _ = std::fs::remove_file(path);
    }
    if stop.load(Ordering::Relaxed) {
        // Work dir kept for Resume (see the stop branch above).
        bail!("stopped");
    }
    if !status.success() {
        let tail = stderr_tail.iter().cloned().collect::<Vec<_>>().join("\n");
        let merge = is_merge_or_ffmpeg_error(&tail);
        if merge {
            // The retry after a merge failure starts fresh on purpose.
            yt_temp.cleanup();
            bail!(
                "yt-dlp/ffmpeg merge failed (exit {status}). stderr tail:\n{}",
                truncate_for_log(&tail, 2000)
            );
        }
        bail!(
            "yt-dlp exited with {status}. stderr tail:\n{}",
            truncate_for_log(&tail, 2000)
        );
    }

    if builtin_merge {
        last_progress.phase = "merging".into();
        on_progress(last_progress.clone());
        if let Some(merged) = mux_split_formats(&work_dir, &stem).await? {
            final_path = Some(merged);
        }
    }

    let path = if let Some(p) = final_path.filter(|p| p.exists()) {
        p
    } else if let Some(name) = last_progress.filename.clone() {
        let p = PathBuf::from(name);
        if p.exists() {
            p
        } else {
            find_newest_media(&work_dir).context("could not locate yt-dlp output")?
        }
    } else {
        find_newest_media(&work_dir).context("could not locate yt-dlp output")?
    };

    // Promote the finished file into Downloads (visible); wipe the hidden work dir.
    let visible = move_finished_to_save_dir(&path, save_dir, &final_hint)?;
    yt_temp.cleanup();

    last_progress.percent = 100.0;
    last_progress.phase = "done".into();
    last_progress.filename = Some(visible.display().to_string());
    last_progress.speed_bps = 0.0;
    last_progress.eta_seconds = Some(0.0);
    if let Ok(meta) = std::fs::metadata(&visible) {
        last_progress.total_bytes = meta.len();
        last_progress.downloaded_bytes = meta.len();
    } else if last_progress.total_bytes == 0 {
        last_progress.downloaded_bytes = last_progress.downloaded_bytes.max(last_part_bytes);
    } else {
        last_progress.downloaded_bytes = last_progress.total_bytes;
    }
    on_progress(last_progress);

    Ok(visible)
}

fn push_stderr_tail(tail: &mut VecDeque<String>, line: &str) {
    if tail.len() >= STDERR_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line.to_string());
}

fn is_merge_or_ffmpeg_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("[merger]")
        || lower.contains("error merging")
        || lower.contains("ffmpeg exited")
        || lower.contains("ffmpeg not found")
        || lower.contains("postprocessing: error")
        || (lower.contains("ffmpeg") && lower.contains("error"))
}

fn truncate_for_log(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Keep percent / ETA consistent with HTTP job fields for the UI.
fn normalize_progress_fields(p: &mut YoutubeProgress, speed_hint: f64) {
    if p.total_bytes > 0 {
        if p.downloaded_bytes > p.total_bytes {
            p.downloaded_bytes = p.total_bytes;
        }
        if p.percent <= 0.0 || !p.percent.is_finite() {
            p.percent = (p.downloaded_bytes as f64 / p.total_bytes as f64 * 100.0).clamp(0.0, 100.0);
        }
    }
    if speed_hint > 1.0 {
        p.speed_bps = speed_hint;
    }
    if p.eta_seconds.is_none() && p.speed_bps > 1.0 && p.total_bytes > p.downloaded_bytes {
        p.eta_seconds = Some((p.total_bytes - p.downloaded_bytes) as f64 / p.speed_bps);
    }
}

/// Tracks DASH video+audio as two sequential yt-dlp downloads so the UI sees
/// one continuous progress bar instead of resetting at the audio stream.
#[derive(Debug, Default, Clone)]
struct DashProgressAccum {
    finished_bytes: u64,
    last_stream_downloaded: u64,
    current_stream_total: u64,
    estimate_total: u64,
}

fn apply_dash_progress(acc: &mut DashProgressAccum, mut p: YoutubeProgress) -> YoutubeProgress {
    // New stream: yt-dlp resets downloaded near zero after finishing video/audio.
    // yt-dlp concurrent fragments (-N) can cause downloaded_bytes to briefly fluctuate
    // backwards by several megabytes. We must only trigger a stream transition when
    // downloaded_bytes drops massively (e.g. back to near zero).
    if acc.last_stream_downloaded > 256 * 1024
        && p.downloaded_bytes < acc.last_stream_downloaded / 2
    {
        acc.finished_bytes = acc
            .finished_bytes
            .saturating_add(acc.last_stream_downloaded);
        acc.current_stream_total = 0;
    }
    if p.total_bytes > 0 {
        acc.current_stream_total = p.total_bytes;
    }
    // Only update last_stream_downloaded if it didn't fluctuate backwards, 
    // or if we just reset it for a new stream.
    if p.downloaded_bytes > acc.last_stream_downloaded || p.downloaded_bytes < acc.last_stream_downloaded / 2 {
        acc.last_stream_downloaded = p.downloaded_bytes;
    }

    let combined_dl = acc.finished_bytes.saturating_add(p.downloaded_bytes);
    let live_total = if acc.current_stream_total > 0 {
        acc.finished_bytes.saturating_add(acc.current_stream_total)
    } else {
        combined_dl
    };
    p.downloaded_bytes = combined_dl;
    p.total_bytes = reconcile_yt_total(acc, live_total, combined_dl, p.is_estimate);
    p
}

/// Prefer live yt-dlp stream totals over inflated extension/probe estimates
/// (e.g. 933 MB announced vs ~40 MB actually selected).
fn reconcile_yt_total(acc: &DashProgressAccum, live_total: u64, downloaded: u64, is_estimate: bool) -> u64 {
    let live = live_total.max(downloaded);
    if live == 0 {
        return acc.estimate_total;
    }
    if acc.estimate_total == 0 {
        return live;
    }
    // If we have a firm estimate from the extension but yt-dlp only has a volatile estimate,
    // don't let yt-dlp inflate the total wildly. Cap it at the initial firm estimate.
    if is_estimate && acc.estimate_total > 0 && live > acc.estimate_total {
        return acc.estimate_total.max(downloaded);
    }
    // Once yt-dlp reports a real stream size, drop a wildly larger estimate.
    if acc.current_stream_total > 0
        && acc.estimate_total > live.saturating_mul(2).max(live + 8 * 1024 * 1024)
    {
        return live;
    }
    // The sum of separate DASH component streams (video + audio) can slightly exceed
    // the final merged container size due to muxing overhead differences. On
    // pause/resume the estimate_total comes from the actual merged file size, so when
    // `live` only marginally exceeds it (< 32 MiB) the estimate is more accurate.
    // Genuine large underestimates (> 32 MiB over) are still allowed to grow.
    if acc.estimate_total > 0
        && live > acc.estimate_total
        && live.saturating_sub(acc.estimate_total) < 32 * 1024 * 1024
    {
        return acc.estimate_total.max(downloaded);
    }
    acc.estimate_total.max(live)
}

fn find_newest_media(dir: &Path) -> Result<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).context("read save dir")? {
        let entry = entry?;
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(
            ext.as_str(),
            "mp4" | "mkv" | "webm" | "m4a" | "mp3" | "opus" | "ogg"
        ) {
            continue;
        }
        let modified = entry.metadata()?.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        match &best {
            None => best = Some((modified, path)),
            Some((t, _)) if modified > *t => best = Some((modified, path)),
            _ => {}
        }
    }
    best.map(|(_, p)| p)
        .ok_or_else(|| anyhow::anyhow!("no media file found in {}", dir.display()))
}

fn parse_progress_line(line: &str) -> Option<YoutubeProgress> {
    if let Some(p) = parse_dmprog_line(line) {
        return Some(p);
    }
    // Legacy human-readable: [download]  45.2% of  120.50MiB at    3.20MiB/s ETA 00:21
    if !line.contains("[download]") || !line.contains('%') {
        return None;
    }
    if line.contains("Destination:") || line.contains("Downloading item") {
        return None;
    }
    let percent = {
        let idx = line.find('%')?;
        let start = line[..idx].rfind(|c: char| c.is_ascii_whitespace())?;
        line[start..idx].trim().parse::<f64>().ok()?
    };

    let total_bytes = extract_size_after(line, " of ").unwrap_or(0);
    let downloaded_bytes = if total_bytes > 0 {
        ((percent / 100.0) * total_bytes as f64) as u64
    } else {
        0
    };
    let speed_bps = extract_speed(line).unwrap_or(0.0);
    let eta_seconds = extract_eta(line);

    let mut p = YoutubeProgress {
        percent,
        downloaded_bytes,
        total_bytes,
        speed_bps,
        eta_seconds,
        phase: "downloading".into(),
        filename: None,
        is_estimate: line.contains("~"),
    };
    normalize_progress_fields(&mut p, speed_bps);
    Some(p)
}

/// Parse `--progress-template` lines:
/// `[DMPROG]downloaded|total|estimate|speed|eta|percent`
fn parse_dmprog_line(line: &str) -> Option<YoutubeProgress> {
    let idx = line.find(DM_PROG_PREFIX)?;
    let rest = line[idx + DM_PROG_PREFIX.len()..].trim();
    let parts: Vec<&str> = rest.split('|').collect();
    if parts.len() < 6 {
        return None;
    }
    let downloaded = parse_numeric_bytes(parts[0]).unwrap_or(0);
    let total = parse_numeric_bytes(parts[1]).unwrap_or(0);
    let estimate = parse_numeric_bytes(parts[2]).unwrap_or(0);
    let is_estimate = total == 0 && estimate > 0;
    let total_bytes = if total > 0 { total } else { estimate };
    let speed_bps = parse_numeric_bytes(parts[3]).map(|b| b as f64).unwrap_or(0.0);
    let eta_seconds = parse_eta_token(parts[4]);
    let percent = {
        let tok = parts[5].trim().trim_end_matches('%').trim();
        tok.parse::<f64>().unwrap_or_else(|_| {
            if total_bytes > 0 {
                downloaded as f64 / total_bytes as f64 * 100.0
            } else {
                0.0
            }
        })
    };

    let mut p = YoutubeProgress {
        percent,
        downloaded_bytes: downloaded,
        total_bytes,
        speed_bps,
        eta_seconds,
        phase: "downloading".into(),
        filename: None,
        is_estimate,
    };
    normalize_progress_fields(&mut p, speed_bps);
    Some(p)
}

fn parse_numeric_bytes(token: &str) -> Option<u64> {
    let t = token.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") || t.eq_ignore_ascii_case("na") {
        return None;
    }
    if let Ok(n) = t.parse::<u64>() {
        return Some(n);
    }
    if let Ok(n) = t.parse::<f64>() {
        return Some(n as u64);
    }
    parse_size_token(t)
}

fn parse_eta_token(token: &str) -> Option<f64> {
    let t = token.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("unknown") || t.eq_ignore_ascii_case("none") {
        return None;
    }
    if let Ok(secs) = t.parse::<f64>() {
        return Some(secs);
    }
    // HH:MM:SS or MM:SS
    let parts: Vec<&str> = t.split(':').collect();
    match parts.len() {
        2 => {
            let m: f64 = parts[0].parse().ok()?;
            let s: f64 = parts[1].parse().ok()?;
            Some(m * 60.0 + s)
        }
        3 => {
            let h: f64 = parts[0].parse().ok()?;
            let m: f64 = parts[1].parse().ok()?;
            let s: f64 = parts[2].parse().ok()?;
            Some(h * 3600.0 + m * 60.0 + s)
        }
        _ => None,
    }
}

fn extract_size_after(line: &str, marker: &str) -> Option<u64> {
    let idx = line.find(marker)?;
    let rest = line[idx + marker.len()..].trim_start();
    let rest = rest.trim_start_matches('~').trim_start();
    let token = rest.split_whitespace().next()?;
    parse_size_token(token)
}

fn extract_speed(line: &str) -> Option<f64> {
    let idx = line.find(" at ")?;
    let rest = line[idx + 4..].trim_start();
    let token = rest.split_whitespace().next()?;
    if token.eq_ignore_ascii_case("unknown") {
        return None;
    }
    parse_size_token(token.trim_end_matches("/s")).map(|b| b as f64)
}

fn extract_eta(line: &str) -> Option<f64> {
    let idx = line.find(" ETA ")?;
    let rest = line[idx + 5..].trim_start();
    let token = rest.split_whitespace().next()?;
    parse_eta_token(token)
}

fn parse_size_token(token: &str) -> Option<u64> {
    let token = token.trim();
    if token.is_empty() || token.eq_ignore_ascii_case("unknown") {
        return None;
    }
    let mut num = String::new();
    let mut unit = String::new();
    for c in token.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            unit.push(c);
        }
    }
    let n: f64 = num.parse().ok()?;
    let mult = match unit.to_ascii_lowercase().as_str() {
        "b" | "" => 1.0,
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    Some((n * mult) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdn_itag_selector_pins_stream() {
        let video = "https://rr2---sn-q4f.googlevideo.com/videoplayback?expire=1&itag=137&mime=video%2Fmp4&clen=407699258";
        assert_eq!(cdn_itag_selector(video).as_deref(), Some("137+ba/137/bv*+ba/b"));
        let audio = "https://rr2---sn-q4f.googlevideo.com/videoplayback?itag=140&mime=audio%2Fmp4";
        assert_eq!(cdn_itag_selector(audio).as_deref(), Some("140/ba/b"));
        assert_eq!(cdn_itag_selector("https://rr2.googlevideo.com/videoplayback?mime=video%2Fmp4"), None);
    }

    #[test]
    fn selector_prefers_adaptive_not_mp4_only() {
        let opts = YoutubeOptions {
            quality: Some("1080".into()),
            ..Default::default()
        };
        let sel = format_selector(&opts);
        assert!(sel.contains("height=1080"), "{sel}");
        assert!(sel.contains("height>=810"), "{sel}");
        assert!(!sel.contains("[ext=mp4]"), "{sel}");
        assert!(sel.starts_with("bv*"), "{sel}");
        // Progressive `b` must not win before adaptive webm/AV1.
        let b_pos = sel.find("/b[").or_else(|| sel.rfind("/b")).unwrap_or(usize::MAX);
        let wv_pos = sel.find("wv*").unwrap_or(usize::MAX);
        assert!(wv_pos < b_pos, "adaptive webm should precede progressive: {sel}");
        let sort = format_sort(&opts).unwrap();
        assert!(sort.contains("res:1080"), "{sort}");
    }

    #[test]
    fn container_labels_are_not_raw_format_ids() {
        let opts = YoutubeOptions {
            quality: Some("720".into()),
            format: Some("mp4".into()),
            ..Default::default()
        };
        let sel = format_selector(&opts);
        assert!(sel.contains("height<=720"), "{sel}");
        assert_ne!(sel, "mp4");
    }

    #[test]
    fn parses_quality_with_suffix() {
        let opts = YoutubeOptions {
            quality: Some("1080p HD".into()),
            ..Default::default()
        };
        assert_eq!(parse_quality_height(&normalized_quality(&opts)), Some(1080));
    }

    #[test]
    fn parses_dmprog_template_line() {
        let line = "[DMPROG]1048576|10485760|NA|524288|15| 10.0%";
        let p = parse_progress_line(line).unwrap();
        assert_eq!(p.downloaded_bytes, 1_048_576);
        assert_eq!(p.total_bytes, 10_485_760);
        assert!((p.speed_bps - 524_288.0).abs() < 1.0);
        assert_eq!(p.eta_seconds, Some(15.0));
        assert!((p.percent - 10.0).abs() < 0.01);
    }

    #[test]
    fn dash_progress_accumulates_video_then_audio() {
        let mut acc = DashProgressAccum {
            estimate_total: 900_000_000,
            ..Default::default()
        };
        let v1 = apply_dash_progress(
            &mut acc,
            YoutubeProgress {
                downloaded_bytes: 20_000_000,
                total_bytes: 30_000_000,
                speed_bps: 5_000_000.0,
                percent: 66.0,
                phase: "downloading".into(),
                ..Default::default()
            },
        );
        assert_eq!(v1.downloaded_bytes, 20_000_000);
        // Inflated estimate is replaced by live stream total.
        assert_eq!(v1.total_bytes, 30_000_000);

        let v2 = apply_dash_progress(
            &mut acc,
            YoutubeProgress {
                downloaded_bytes: 30_000_000,
                total_bytes: 30_000_000,
                speed_bps: 5_000_000.0,
                percent: 100.0,
                phase: "downloading".into(),
                ..Default::default()
            },
        );
        assert_eq!(v2.downloaded_bytes, 30_000_000);

        // Audio stream starts — downloaded resets.
        let a1 = apply_dash_progress(
            &mut acc,
            YoutubeProgress {
                downloaded_bytes: 1_000_000,
                total_bytes: 10_000_000,
                speed_bps: 2_000_000.0,
                percent: 10.0,
                phase: "downloading".into(),
                ..Default::default()
            },
        );
        assert_eq!(a1.downloaded_bytes, 31_000_000);
        assert_eq!(a1.total_bytes, 40_000_000);
    }

    #[test]
    fn detects_merge_errors() {
        assert!(is_merge_or_ffmpeg_error(
            "[Merger] ERROR: ffmpeg exited with code 1"
        ));
        assert!(!is_merge_or_ffmpeg_error("ERROR: Video unavailable"));
    }
}

#[cfg(test)]
mod cookie_tests {
    use super::*;

    #[test]
    fn jar_is_detected_and_written_verbatim() {
        let jar = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t1893456000\tSID\tabc\n#HttpOnly_.google.com\tTRUE\t/\tTRUE\t0\t__Secure-1PSID\txyz";
        assert!(is_cookie_jar(jar));
        assert!(!is_cookie_jar("SID=abc; HSID=def"));
        let path = std::env::temp_dir().join(format!("dm-cookie-test-{}.txt", std::process::id()));
        write_netscape_cookies(jar, &path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("#HttpOnly_.google.com\tTRUE\t/\tTRUE\t0\t__Secure-1PSID\txyz"));
        assert!(out.contains("1893456000"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn header_string_still_converts() {
        let path = std::env::temp_dir().join(format!("dm-cookie-hdr-{}.txt", std::process::id()));
        write_netscape_cookies("SID=abc; HSID=def", &path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains(".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc"));
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod same_container_tests {
    use super::*;

    #[test]
    fn pairs_stay_in_one_container() {
        assert_eq!(
            same_container_pairs("313+bestaudio/ba"),
            "313[ext=mp4]+bestaudio[ext=m4a]/313[ext=webm]+bestaudio[ext=webm]/ba"
        );
        assert_eq!(same_container_pairs("ba/b"), "ba/b");
        let best = same_container_pairs("bv*+ba/b");
        assert!(best.starts_with("bv*[height>=4320][ext=mp4]+ba[ext=m4a]/bv*[height>=4320][ext=webm]+ba[ext=webm]/"));
        assert!(best.ends_with("/bv*[ext=webm]+ba[ext=webm]/b"));
    }

    #[test]
    #[ignore = "prints selectors for manual yt-dlp checks"]
    fn print_selectors() {
        for q in ["best", "1080", "2160", "720"] {
            let opts = YoutubeOptions { quality: Some(q.into()), ..Default::default() };
            println!("SEL {}", same_container_pairs(&format_selector(&opts)));
        }
    }
}
