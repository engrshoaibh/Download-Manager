//! Native HLS / DASH downloader (IDM's `IDMVMPrs` equivalent): parse the manifest, pick a
//! variant for the requested quality, download its segments in parallel, then join a
//! separate audio track with the built-in muxer.
//!
//! Anything this engine does not handle (live streams, DRM, SAMPLE-AES, multi-period
//! DASH, …) returns [`StreamError::Unsupported`] so the caller can fall back to yt-dlp.

pub mod dash;
pub mod fetch;
pub mod hls;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("unsupported stream: {0}")]
    Unsupported(String),
    /// Finishing needs FFmpeg (MPEG-TS remux, or tracks the built-in muxer can't join).
    #[error("FFmpeg is needed to finish this stream")]
    NeedsFfmpeg,
    #[error("stopped")]
    Stopped,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, StreamError>;

pub(crate) fn unsupported(msg: impl Into<String>) -> StreamError {
    StreamError::Unsupported(msg.into())
}

/// One downloadable piece of a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub url: String,
    /// Inclusive byte range within `url`.
    pub range: Option<(u64, u64)>,
    /// AES-128-CBC key location and IV.
    pub key: Option<Key>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub url: String,
    pub iv: [u8; 16],
}

/// What the concatenated segments of a track form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Init segment + fragments: a fragmented MP4.
    Fmp4,
    WebM,
    /// MPEG transport stream.
    Ts,
    /// Raw ADTS AAC (HLS packed audio).
    Aac,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub init: Option<Seg>,
    pub segments: Vec<Seg>,
    pub kind: Kind,
    /// Seconds, when the manifest says.
    pub duration: f64,
}

impl Track {
    fn items(&self) -> Vec<Seg> {
        self.init.iter().cloned().chain(self.segments.iter().cloned()).collect()
    }
}

/// Pick the index of the best candidate for a quality ("best", "1080", "720p", …):
/// the tallest at or below the requested height (highest bitrate on ties), else the shortest.
pub fn pick_variant(quality: Option<&str>, candidates: &[(Option<u32>, u64)]) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    let want: Option<u32> = quality
        .map(|q| q.chars().filter(char::is_ascii_digit).collect::<String>())
        .and_then(|d| d.parse().ok());
    let key = |i: &usize| (candidates[*i].0.unwrap_or(0), candidates[*i].1);
    let all = 0..candidates.len();
    match want {
        Some(h) => all
            .clone()
            .filter(|&i| candidates[i].0.map(|ch| ch <= h).unwrap_or(true))
            .max_by_key(key)
            .or_else(|| all.min_by_key(key)),
        None => all.max_by_key(key),
    }
}

/// What to fetch: the request as the job knows it.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub url: String,
    pub quality: Option<String>,
    pub user_agent: Option<String>,
    /// `Cookie` header value or a Netscape cookie jar.
    pub cookies: Option<String>,
    pub referer: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    pub downloaded: u64,
    /// Running estimate from bytes per segment so far; 0 until the first segment lands.
    pub total_estimate: u64,
}

#[derive(Debug)]
pub struct Downloaded {
    pub main: PathBuf,
    pub main_kind: Kind,
    pub audio: Option<(PathBuf, Kind)>,
}

fn audio_only(quality: Option<&str>) -> bool {
    quality.map(|q| q.trim().to_ascii_lowercase().starts_with("audio")).unwrap_or(false)
}

/// Resolve the manifest to the track(s) to download.
async fn plan(f: &fetch::Fetcher, req: &Request) -> Result<(Track, Option<Track>)> {
    let text = f.text(&req.url).await?;
    let quality = req.quality.as_deref();
    if text.trim_start().starts_with("#EXTM3U") {
        match hls::parse(&text, &req.url)? {
            hls::Playlist::Media(track) => Ok((track, None)),
            hls::Playlist::Master(master) => {
                let audio_url = |group: Option<&str>| {
                    master
                        .audio
                        .iter()
                        .filter(|r| group.map(|g| r.group == g).unwrap_or(true) && r.url.is_some())
                        .max_by_key(|r| r.default)
                        .and_then(|r| r.url.clone())
                };
                if audio_only(quality) {
                    let url = audio_url(None).ok_or_else(|| unsupported("no separate audio rendition"))?;
                    return Ok((media_track(f, &url).await?, None));
                }
                let candidates: Vec<_> = master.variants.iter().map(|v| (v.height, v.bandwidth)).collect();
                let v = &master.variants[pick_variant(quality, &candidates).ok_or_else(|| unsupported("no variants"))?];
                let video = media_track(f, &v.url).await?;
                let audio = match v.audio_group.as_deref().and_then(|g| audio_url(Some(g))) {
                    Some(url) => Some(media_track(f, &url).await?),
                    None => None,
                };
                Ok((video, audio))
            }
        }
    } else if text.contains("<MPD") {
        let reps = dash::parse(&text, &req.url)?;
        let audios: Vec<_> = reps.iter().filter(|r| r.content == dash::Content::Audio).collect();
        let best_audio = |prefer: Option<Kind>| {
            audios
                .iter()
                .max_by_key(|r| (Some(r.track.kind) == prefer, r.bandwidth))
                .map(|r| r.track.clone())
        };
        if audio_only(quality) {
            return Ok((best_audio(None).ok_or_else(|| unsupported("no audio track"))?, None));
        }
        let videos: Vec<_> = reps.iter().filter(|r| r.content == dash::Content::Video).collect();
        if videos.is_empty() {
            return Ok((best_audio(None).ok_or_else(|| unsupported("empty manifest"))?, None));
        }
        let candidates: Vec<_> = videos.iter().map(|r| (r.height, r.bandwidth)).collect();
        let video = videos[pick_variant(quality, &candidates).unwrap()].track.clone();
        let audio = best_audio(Some(video.kind));
        Ok((video, audio))
    } else {
        Err(unsupported("not an HLS or DASH manifest"))
    }
}

async fn media_track(f: &fetch::Fetcher, url: &str) -> Result<Track> {
    match hls::parse(&f.text(url).await?, url)? {
        hls::Playlist::Media(t) => Ok(t),
        hls::Playlist::Master(_) => Err(unsupported("nested master playlist")),
    }
}

/// Download the chosen track(s) into `work_dir`. Progress survives Pause: a later call
/// with the same `work_dir` continues from the last finished segment.
pub async fn download(
    req: &Request,
    work_dir: &Path,
    stop: Arc<AtomicBool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Downloaded> {
    let f = fetch::Fetcher::new(req)?;
    let (main, audio) = plan(&f, req).await?;
    info!(
        "Native stream: {} segment(s) {:?}{}",
        main.segments.len(),
        main.kind,
        audio.as_ref().map(|a| format!(" + audio {} segment(s) {:?}", a.segments.len(), a.kind)).unwrap_or_default()
    );

    let total_items = main.items().len() + audio.as_ref().map(|a| a.items().len()).unwrap_or(0);
    let mut done_items = 0usize;
    let mut done_bytes = 0u64;
    let mut report = |items: usize, bytes: u64| {
        let total_estimate = if items > 0 { bytes * total_items as u64 / items as u64 } else { 0 };
        on_progress(Progress { downloaded: bytes, total_estimate });
    };

    let main_path = work_dir.join(format!("main.{}", ext(main.kind)));
    let (items, bytes) = f
        .track_to_file(&main, &main_path, &work_dir.join("main.state"), &stop, &mut |i, b| report(i, b))
        .await?;
    done_items += items;
    done_bytes += bytes;

    let audio = match audio {
        Some(track) => {
            let path = work_dir.join(format!("audio.{}", ext(track.kind)));
            let (base_i, base_b) = (done_items, done_bytes);
            f.track_to_file(&track, &path, &work_dir.join("audio.state"), &stop, &mut |i, b| {
                report(base_i + i, base_b + b)
            })
            .await?;
            Some((path, track.kind))
        }
        None => None,
    };
    Ok(Downloaded { main: main_path, main_kind: main.kind, audio })
}

fn ext(kind: Kind) -> &'static str {
    match kind {
        Kind::Fmp4 => "mp4",
        Kind::WebM => "webm",
        Kind::Ts => "ts",
        Kind::Aac => "aac",
    }
}

/// Turn the downloaded track(s) into one playable file `<work_dir>/<stem>.<ext>`.
/// Uses the built-in muxer; `ffmpeg` is only needed for MPEG-TS remuxes or
/// track pairs the muxer can't join ([`StreamError::NeedsFfmpeg`] when missing).
pub async fn finish(dl: &Downloaded, work_dir: &Path, stem: &str, ffmpeg: Option<&Path>) -> Result<PathBuf> {
    match &dl.audio {
        None => match dl.main_kind {
            Kind::Fmp4 | Kind::WebM | Kind::Aac => {
                let out = work_dir.join(format!("{stem}.{}", ext(dl.main_kind)));
                std::fs::rename(&dl.main, &out).map_err(anyhow::Error::from)?;
                Ok(out)
            }
            Kind::Ts => {
                let ff = ffmpeg.ok_or(StreamError::NeedsFfmpeg)?;
                let out = work_dir.join(format!("{stem}.mp4"));
                run_ffmpeg(ff, &[&dl.main], &out, true).await?;
                Ok(out)
            }
        },
        Some((audio, audio_kind)) => {
            let both_webm = dl.main_kind == Kind::WebM && *audio_kind == Kind::WebM;
            let out = work_dir.join(format!("{stem}.{}", if both_webm { "webm" } else { "mp4" }));
            let (v, a, o) = (dl.main.clone(), audio.clone(), out.clone());
            let built_in = tokio::task::spawn_blocking(move || crate::mux::mux_av(&v, &a, &o, &mut |_, _| true))
                .await
                .map_err(anyhow::Error::from)?;
            match built_in {
                Ok(_) => Ok(out),
                Err(e) => {
                    info!("Built-in muxer not used ({e}); trying FFmpeg");
                    let ff = ffmpeg.ok_or(StreamError::NeedsFfmpeg)?;
                    let any_webm = dl.main_kind == Kind::WebM || *audio_kind == Kind::WebM;
                    let out = work_dir.join(format!("{stem}.{}", if any_webm { "mkv" } else { "mp4" }));
                    run_ffmpeg(ff, &[&dl.main, audio], &out, *audio_kind == Kind::Ts || *audio_kind == Kind::Aac || dl.main_kind == Kind::Ts).await?;
                    Ok(out)
                }
            }
        }
    }
}

async fn run_ffmpeg(ffmpeg: &Path, inputs: &[&Path], out: &Path, adts_fix: bool) -> Result<()> {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.arg("-y").arg("-v").arg("error");
    for i in inputs {
        cmd.arg("-i").arg(i);
    }
    if inputs.len() == 2 {
        cmd.args(["-map", "0:v:0?", "-map", "0:a:0?", "-map", "1:a:0"]);
    }
    cmd.args(["-c", "copy"]);
    if adts_fix && out.extension().map(|e| e == "mp4").unwrap_or(false) {
        cmd.args(["-bsf:a", "aac_adtstoasc"]);
    }
    cmd.arg(out);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let o = cmd.output().await.map_err(anyhow::Error::from)?;
    if !o.status.success() {
        let _ = std::fs::remove_file(out);
        return Err(anyhow::anyhow!("ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr).trim()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_by_height() {
        let c = [(Some(360), 500), (Some(720), 1500), (Some(1080), 3000), (Some(1080), 4500)];
        assert_eq!(pick_variant(None, &c), Some(3));
        assert_eq!(pick_variant(Some("best"), &c), Some(3));
        assert_eq!(pick_variant(Some("720p"), &c), Some(1));
        assert_eq!(pick_variant(Some("1440"), &c), Some(3));
        assert_eq!(pick_variant(Some("240"), &c), Some(0));
    }
}
