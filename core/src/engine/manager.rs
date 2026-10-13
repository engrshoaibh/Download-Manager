use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tracing::{info, warn};

use super::scheduler::{StealState, MIN_STEAL_BYTES};
use super::{DownloadJob, DownloadStatus, DmEvent, JobId, Segment};
use crate::api::events::EventPublisher;
use crate::io::sparse::{
    part_data_path, remove_part_artifacts, resolve_partial_path, write_part_meta_atomic,
    PartMetadata, SharedMmap, SparseFile,
};
use crate::network::client::HttpClient;
use crate::persist;
use crate::settings::{BandwidthLimiter, DaemonSettings};

/// Adaptive scaler starts here and grows while throughput keeps rising.
/// Start with 4 connections to avoid triggering strict server-side connection limits.
const INITIAL_SEGMENT_COUNT: usize = 4;
const DEFAULT_SEGMENT_COUNT: usize = 32;
const MAX_SEGMENT_COUNT: usize = 32;
const MIN_SEGMENT_BYTES: u64 = 256 * 1024;
/// UI / speed sample cadence. Meta on disk is written less often (see META_EVERY_N).
const UI_TICK: Duration = Duration::from_millis(500);
/// Write `.part.meta` every N UI ticks (4s) — resume safety without constant disk I/O.
const META_EVERY_N: u32 = 8;
/// Persist job list every N UI ticks (8s).
const PERSIST_EVERY_N: u32 = 16;
/// Keep adding connections while marginal throughput gain exceeds this ratio.
const ADAPTIVE_GAIN_THRESHOLD: f64 = 0.10;

pub struct JobSignal {
    pub stop: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
    pub delete_file: Arc<AtomicBool>,
    /// Live yt-dlp / ffmpeg pid so Stop can kill the process tree immediately.
    pub pid: Arc<AtomicU32>,
    /// Run tasks currently alive for this job. Resume waits for 0 so a paused run
    /// that is still shutting down cannot mark the resumed job as failed.
    pub active_runs: Arc<AtomicU32>,
}

/// Counts a run task in `JobSignal::active_runs` for as long as it lives.
struct RunGuard(Arc<AtomicU32>);

impl RunGuard {
    fn enter(signal: &JobSignal) -> Self {
        signal.active_runs.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(&signal.active_runs))
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl JobSignal {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            delete_file: Arc::new(AtomicBool::new(false)),
            pid: Arc::new(AtomicU32::new(0)),
            active_runs: Arc::new(AtomicU32::new(0)),
        })
    }
}

fn kill_job_process(sig: &JobSignal) {
    let pid = sig.pid.load(Ordering::Acquire);
    if pid != 0 {
        crate::youtube::kill_process_tree(pid);
    }
}

pub struct DownloadManager {
    pub jobs: HashMap<JobId, DownloadJob>,
    pub events: EventPublisher,
    pub http_client: Arc<HttpClient>,
    pub settings: DaemonSettings,
    pub bandwidth: Arc<BandwidthLimiter>,
    signals: HashMap<JobId, Arc<JobSignal>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        let settings = crate::settings::load_settings();
        let bandwidth = BandwidthLimiter::new(&settings);
        Self {
            jobs: HashMap::new(),
            events: EventPublisher::new(),
            http_client: Arc::new(HttpClient::new()),
            settings,
            bandwidth,
            signals: HashMap::new(),
        }
    }

    pub fn update_settings(&mut self, settings: DaemonSettings) -> Result<()> {
        crate::settings::save_settings(&settings)?;
        self.bandwidth.apply(&settings);
        self.settings = settings;
        Ok(())
    }

    /// Load persisted jobs (paused unfinished / completed history) or start empty.
    pub fn load_or_new() -> Self {
        let mut mgr = Self::new();
        match persist::load_jobs() {
            Ok(mut jobs) => {
                if let Err(e) = persist::apply_orphan_sweep(&mut jobs, &mgr.settings) {
                    warn!("Orphan .part sweep failed: {e:#}");
                }
                for job in jobs {
                    let id = job.id;
                    mgr.signals.insert(id, JobSignal::new());
                    mgr.jobs.insert(id, job);
                }
            }
            Err(e) => warn!("Could not restore download list: {e:#}"),
        }
        mgr
    }

    pub(crate) fn persist(&mut self) {
        let jobs: Vec<DownloadJob> = self.jobs.values().cloned().collect();
        match persist::save_jobs(&jobs, &self.settings) {
            Ok(saved) => {
                let keep: std::collections::HashSet<_> =
                    saved.iter().map(|j| j.id).collect();
                self.jobs.retain(|id, _| keep.contains(id));
            }
            Err(e) => warn!("Failed to save download list: {e:#}"),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::api::events::WsMessage> {
        self.events.subscribe()
    }

    pub fn add_download(
        &mut self,
        url: String,
        save_dir: String,
        segment_count: Option<usize>,
        user_agent: Option<String>,
        cookies: Option<String>,
        referer: Option<String>,
        filename_override: Option<String>,
        title: Option<String>,
        total_bytes_hint: Option<u64>,
        quality: Option<String>,
        format: Option<String>,
    ) -> Result<DownloadJob> {
        let url = url.trim().to_string();
        let segs = segment_count
            .unwrap_or(DEFAULT_SEGMENT_COUNT)
            .min(MAX_SEGMENT_COUNT);

        let is_direct_cdn = crate::network::client::is_googlevideo_media_url(&url);
        let is_hls = crate::youtube::is_stream_manifest_url(&url);
        let force_yt = !is_direct_cdn
            && (quality
                .as_ref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
                || format
                    .as_ref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false));
        let yt_target = if is_direct_cdn {
            /* Signed CDN stream — IDM-style segmented HTTP download */
            None
        } else if crate::youtube::is_youtube_page_url(&url) {
            Some(crate::youtube::normalize_watch_url(&url))
        } else if is_hls {
            /* HLS/DASH manifests — yt-dlp fetches segments + remuxes */
            Some(url.clone())
        } else if force_yt {
            crate::youtube::resolve_youtube_target(&url, referer.as_deref())
        } else {
            None
        };
        let engine = if yt_target.is_some() {
            "youtube"
        } else {
            "http"
        };

        let ext = if quality
            .as_deref()
            .map(|q| q.eq_ignore_ascii_case("audio"))
            .unwrap_or(false)
        {
            "m4a"
        } else if engine == "youtube" || is_direct_cdn {
            "mp4"
        } else {
            ""
        };

        let filename = filename_override
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let t = title
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(sanitize_yt_name)
                    .filter(|s| !s.is_empty())?;
                let q = quality
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("best"));
                let name = if let Some(q) = q {
                    format!("{t} [{q}].{ext}")
                } else if !ext.is_empty() {
                    format!("{t}.{ext}")
                } else {
                    t
                };
                Some(name)
            })
            .or_else(|| {
                if engine == "youtube" {
                    Some("YouTube video.mp4".into())
                } else {
                    crate::network::client::filename_hint_from_url(&url)
                }
            })
            .unwrap_or_else(|| {
                url.split('/')
                    .last()
                    .and_then(|s| s.split('?').next())
                    .filter(|s| !s.is_empty() && *s != "videoplayback")
                    .unwrap_or("download")
                    .to_string()
            });

        let save_dir = PathBuf::from(&save_dir);
        let (filename, save_path) = unique_save_path(&save_dir, &filename);
        let mut job = DownloadJob::new(url, filename, save_path, segs);
        job.user_agent = user_agent;
        job.cookies = cookies;
        job.referer = referer;
        job.quality = quality;
        job.format = format;
        job.engine = engine.into();
        if let Some(n) = total_bytes_hint.filter(|&n| n > 0) {
            job.total_bytes = n;
        } else {
            let clen = crate::network::client::content_length_from_url(&job.url);
            if clen > 0 {
                job.total_bytes = clen;
            }
        }
        // Prefer the canonical watch URL so resume/reprobe stay stable.
        if let Some(target) = yt_target {
            job.url = target;
        }
        let job_id = job.id;
        self.signals.insert(job_id, JobSignal::new());

        self.events.send(DmEvent::JobAdded { job: job.clone() });
        self.jobs.insert(job_id, job.clone());
        self.persist();

        Ok(job)
    }

    pub fn pause(&mut self, id: &JobId) -> bool {
        if let Some(sig) = self.signals.get(id) {
            sig.stop.store(true, Ordering::Release);
            kill_job_process(sig);
        }
        if let Some(job) = self.jobs.get_mut(id) {
            if !matches!(
                job.status,
                DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued
            ) {
                return false;
            }
            job.status = DownloadStatus::Paused;
            job.speed_bps = 0.0;
            self.events.send(DmEvent::JobUpdated { job: job.clone() });
            self.persist();
            true
        } else {
            false
        }
    }

    pub fn cancel(&mut self, id: &JobId, delete_file: bool) -> bool {
        let Some(job) = self.jobs.get(id).cloned() else {
            return false;
        };

        if let Some(sig) = self.signals.get(id) {
            sig.stop.store(true, Ordering::Release);
            sig.cancel.store(true, Ordering::Release);
            if delete_file {
                sig.delete_file.store(true, Ordering::Release);
            }
            kill_job_process(sig);
        }

        let active = matches!(
            job.status,
            DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued
        );

        // For active downloads, workers still hold the mmap — delete .part in finalize_stop.
        if !active && job.status != DownloadStatus::Completed {
            crate::io::sparse::remove_part_artifacts(&job.save_path);
            if job.engine == "youtube" {
                let save_dir = std::path::PathBuf::from(&job.save_path).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
                crate::youtube::cleanup_yt_temp(&save_dir, job.id);
            }
        }

        if delete_file {
            let path = std::path::PathBuf::from(&job.save_path);
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
            // Also clean up any partial artifacts
            crate::io::sparse::remove_part_artifacts(&job.save_path);
            if job.engine == "youtube" {
                let save_dir = std::path::PathBuf::from(&job.save_path).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
                crate::youtube::cleanup_yt_temp(&save_dir, job.id);
            }
        }

        self.jobs.remove(id);
        self.signals.remove(id);
        self.events.send(DmEvent::JobRemoved { id: *id });
        self.persist();
        true
    }

    pub fn remove_completed(&mut self, id: &JobId) -> bool {
        self.cancel(id, false)
    }

    /// True while a paused/failed job's previous run task has not finished yet.
    pub fn previous_run_active(&self, id: &JobId) -> bool {
        let stopped = self
            .jobs
            .get(id)
            .map(|j| matches!(j.status, DownloadStatus::Paused | DownloadStatus::Failed))
            .unwrap_or(false);
        stopped
            && self
                .signals
                .get(id)
                .map(|s| s.active_runs.load(Ordering::Acquire) > 0)
                .unwrap_or(false)
    }

    /// Mark a job as Failed and broadcast update
    pub fn mark_failed(&mut self, id: &JobId, error_msg: String) {
        if let Some(j) = self.jobs.get_mut(id) {
            if j.status != DownloadStatus::Cancelled && j.status != DownloadStatus::Paused {
                j.status = DownloadStatus::Failed;
                j.speed_bps = 0.0;
                j.error = Some(error_msg.chars().take(400).collect());
                self.events.send(DmEvent::JobUpdated { job: j.clone() });
                self.persist();
            }
        }
    }

    pub fn begin_resume(&mut self, id: &JobId) -> Result<DownloadJob> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("Job not found"))?;
        if !matches!(job.status, DownloadStatus::Paused | DownloadStatus::Failed) {
            bail!("Download is not paused");
        }
        let sig = self.signals.entry(*id).or_insert_with(JobSignal::new);
        sig.stop.store(false, Ordering::Release);
        sig.cancel.store(false, Ordering::Release);
        job.status = DownloadStatus::Queued;
        job.speed_bps = 0.0;
        job.error = None;
        let snapshot = job.clone();
        self.events
            .send(DmEvent::JobUpdated { job: snapshot.clone() });
        self.persist();
        Ok(snapshot)
    }

    pub async fn run_download(
        mgr: Arc<RwLock<Self>>,
        mut job: DownloadJob,
        client: Arc<HttpClient>,
        user_agent: Option<String>,
        cookies: Option<String>,
        referer: Option<String>,
    ) -> Result<()> {
        let job_id = job.id;
        let tx = { mgr.read().await.events.clone() };
        let signal = {
            let m = mgr.read().await;
            m.signals
                .get(&job_id)
                .cloned()
                .unwrap_or_else(JobSignal::new)
        };
        let _run_guard = RunGuard::enter(&signal);

        // Prefer headers stored on the job (needed after restart).
        let user_agent = user_agent.or_else(|| job.user_agent.clone());
        let cookies = cookies.or_else(|| job.cookies.clone());
        let referer = referer.or_else(|| job.referer.clone());
        if job.user_agent.is_none() && user_agent.is_some() {
            job.user_agent = user_agent.clone();
        }
        if job.cookies.is_none() && cookies.is_some() {
            job.cookies = cookies.clone();
        }
        if job.referer.is_none() && referer.is_some() {
            job.referer = referer.clone();
        }

        // Resume only when the saved segment state and the `.part` payload agree.
        // If either half is missing/corrupt, resuming would yield a zero-filled file.
        let mut restarted = false;
        let resuming = if job.segments.is_empty() {
            false
        } else {
            let partial = crate::io::sparse::resolve_partial_path(&job.save_path)
                .unwrap_or_else(|| part_data_path(&job.save_path));
            match crate::engine::integrity::check_resume(&job.segments, job.total_bytes, &partial) {
                crate::engine::integrity::ResumeCheck::Resume => true,
                crate::engine::integrity::ResumeCheck::Restart(reason) => {
                    warn!("Cannot resume {}: {reason} - restarting from the beginning", job.filename);
                    job.segments.clear();
                    job.downloaded_bytes = 0;
                    restarted = true;
                    false
                }
            }
        };

        let update_state = |mgr: Arc<RwLock<Self>>, updated_job: DownloadJob| async move {
            let mut m = mgr.write().await;
            if let Some(j) = m.jobs.get_mut(&updated_job.id) {
                if j.status == DownloadStatus::Cancelled {
                    return;
                }
                if j.status == DownloadStatus::Paused
                    && !matches!(
                        updated_job.status,
                        DownloadStatus::Paused | DownloadStatus::Completed | DownloadStatus::Failed
                    )
                {
                    return;
                }
                *j = updated_job.clone();
                m.events.send(DmEvent::JobUpdated { job: updated_job });
                m.persist();
            }
        };

        // ── YouTube / yt-dlp path ──────────────────────────────────────
        if job.engine == "youtube" {
            return Self::run_youtube_download(
                mgr,
                job,
                signal,
                user_agent,
                cookies,
                referer,
            )
            .await;
        }

        if !resuming {
            if job.total_bytes > 0 {
                job.status = DownloadStatus::Downloading;
                if !restarted {
                    job.supports_ranges = true; // Assume true if size is provided
                }
                update_state(mgr.clone(), job.clone()).await;
            } else {
                job.status = DownloadStatus::Probing;
                update_state(mgr.clone(), job.clone()).await;

                let info = client
                    .probe(
                        &job.url,
                        user_agent.as_deref(),
                        cookies.as_deref(),
                        referer.as_deref(),
                    )
                    .await
                    .context("Failed to probe download URL")?;

                if signal.stop.load(Ordering::Relaxed) {
                    return Self::finalize_stop(mgr, job, None, &signal, "").await;
                }

                job.total_bytes = info.content_length;
                job.supports_ranges = info.supports_ranges;
                job.use_http3 = info.supports_http3;
                // Prefer the post-redirect URL so workers skip re-following with Range.
                if let Some(final_url) = info.final_url {
                    if !final_url.is_empty() && final_url != job.url {
                        job.url = final_url;
                    }
                }

                if let Some(hint) = info.filename_hint {
                    if !hint.is_empty()
                        && (job.filename == "download"
                            || job.filename == "videoplayback"
                            || !job.filename.contains('.'))
                    {
                        let parent = PathBuf::from(&job.save_path)
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| PathBuf::from("."));
                        let (name, path) = unique_save_path(&parent, &hint);
                        job.filename = name;
                        job.save_path = path;
                    }
                }

                // Prefer clen embedded in googlevideo URLs when probe length is missing.
                if job.total_bytes == 0 {
                    let clen = crate::network::client::content_length_from_url(&job.url);
                    if clen > 0 {
                        job.total_bytes = clen;
                        if is_googlevideo_url(&job.url) {
                            job.supports_ranges = true;
                        }
                    }
                }
            }
        }

        let tmp_path = resolve_partial_path(&job.save_path)
            .unwrap_or_else(|| part_data_path(&job.save_path))
            .to_string_lossy()
            .to_string();

        // Migrate legacy `.tmp` → `.part` so meta and sweep use one naming scheme.
        let canonical_part = part_data_path(&job.save_path);
        if PathBuf::from(&tmp_path) != canonical_part && PathBuf::from(&tmp_path).exists() {
            if let Err(e) = std::fs::rename(&tmp_path, &canonical_part) {
                warn!("Could not migrate {} → {}: {e}", tmp_path, canonical_part.display());
            }
        }
        let tmp_path = canonical_part.to_string_lossy().to_string();

        let mmap = if job.total_bytes > 0 {
            let sparse = if resuming && std::path::Path::new(&tmp_path).exists() {
                SparseFile::open(&tmp_path, job.total_bytes)
                    .context("Failed to reopen paused download")?
            } else {
                SparseFile::create(&tmp_path, job.total_bytes)
                    .context("Failed to pre-allocate disk space")?
            };
            Some(sparse.into_mmap()?)
        } else {
            None
        };

        if !resuming {
            // Adaptive: start small; the 2s scaler grows toward job.segment_count / 32.
            let max_segs = if job.supports_ranges && job.total_bytes > 0 {
                let max_by_size = (job.total_bytes / MIN_SEGMENT_BYTES).max(1) as usize;
                job.segment_count
                    .min(MAX_SEGMENT_COUNT)
                    .min(max_by_size)
                    .max(1)
            } else {
                1
            };
            let initial = if max_segs > 1 {
                INITIAL_SEGMENT_COUNT.min(max_segs)
            } else {
                1
            };
            job.segment_count = max_segs;
            job.segments = divide_into_segments(job.total_bytes, initial);
        }

        {
            let meta = PartMetadata::from_job(&job, job.segments.clone());
            if let Err(e) = write_part_meta_atomic(&meta) {
                warn!("Failed to write initial .part.meta: {e:#}");
            }
        }

        job.status = DownloadStatus::Downloading;
        update_state(mgr.clone(), job.clone()).await;

        info!(
            "{} download: {} | {} initial segments (max {}) | HTTP3={}",
            if resuming { "Resuming" } else { "Starting" },
            job.filename,
            job.segments.len(),
            job.segment_count,
            job.use_http3
        );

        // Always track live segment state so we can checkpoint to disk.
        let steal = Arc::new(parking_lot::Mutex::new(StealState::from_segments(
            &job.segments,
        )));
        let allow_steal = job.supports_ranges && job.total_bytes > 0 && job.segment_count > 1;
        let max_connections = job.segment_count.min(MAX_SEGMENT_COUNT);

        let url = job.url.clone();
        let meta_job = job.clone();
        let bandwidth = {
            let m = mgr.read().await;
            Arc::clone(&m.bandwidth)
        };

        let mut workers = JoinSet::new();
        let mut active_connections = 0usize;

        for seg in &job.segments {
            if job.total_bytes > 0 && seg.remaining() == 0 {
                continue;
            }
            spawn_segment_worker(
                &mut workers,
                job_id,
                seg.id,
                seg.start,
                seg.end,
                seg.downloaded,
                Arc::clone(&steal),
                url.clone(),
                Arc::clone(&client),
                mmap.clone(),
                tmp_path.clone(),
                meta_job.clone(),
                tx.clone(),
                user_agent.clone(),
                cookies.clone(),
                referer.clone(),
                Arc::clone(&signal.stop),
                allow_steal,
                Arc::clone(&bandwidth),
            );
            steal.lock().set_active(seg.id);
            active_connections += 1;
        }

        let mut checkpoint_n: u32 = 0;
        let mut last_sample_bytes = {
            steal.lock().total_downloaded()
        };
        let mut last_sample_speed = 0.0f64;
        let mut scaling_enabled = allow_steal && active_connections < max_connections;
        let mut worker_error = None;
        let mut ticker = tokio::time::interval(UI_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Skip the immediate first tick so the first sample covers a full UI window.
        ticker.tick().await;

        loop {
            tokio::select! {
                biased;

                joined = workers.join_next(), if !workers.is_empty() => {
                    match joined {
                        Some(Ok(Ok(()))) => {
                            active_connections = active_connections.saturating_sub(1);
                        }
                        Some(Ok(Err(e))) => {
                            active_connections = active_connections.saturating_sub(1);
                            if !signal.stop.load(Ordering::Relaxed) {
                                // Local I/O / unrecoverable — remember but do not abort siblings.
                                warn!("Worker error (siblings continue): {e:#}");
                                worker_error = Some(e);
                            }
                        }
                        Some(Err(e)) => {
                            warn!("Worker join error: {e}");
                        }
                        None => {}
                    }

                    // If workers drained but unfinished bytes remain, spawn a recovery worker.
                    if workers.is_empty()
                        && !signal.stop.load(Ordering::Relaxed)
                        && allow_steal
                        && steal.lock().has_remaining()
                    {
                        if let Some((new_id, new_start, new_end, already, new_cap)) =
                            steal.lock().try_steal(MIN_STEAL_BYTES.min(256 * 1024))
                        {
                            info!(
                                "Recovery worker for leftover range {}-{} (seg {})",
                                new_start, new_end, new_id
                            );
                            let end_cap = new_cap;
                            spawn_segment_worker_with_cap(
                                &mut workers,
                                job_id,
                                new_id,
                                new_start,
                                new_end,
                                already,
                                end_cap,
                                Arc::clone(&steal),
                                url.clone(),
                                Arc::clone(&client),
                                mmap.clone(),
                                tmp_path.clone(),
                                meta_job.clone(),
                                tx.clone(),
                                user_agent.clone(),
                                cookies.clone(),
                                referer.clone(),
                                Arc::clone(&signal.stop),
                                allow_steal,
                                Arc::clone(&bandwidth),
                            );
                            active_connections += 1;
                            continue;
                        }
                        // Remaining but too small to steal — one worker per unfinished seg.
                        let orphans: Vec<_> = {
                            let state = steal.lock();
                            state
                                .segments
                                .iter()
                                .filter(|s| {
                                    let end = s.end.load(Ordering::Relaxed);
                                    let dl = s.downloaded.load(Ordering::Relaxed);
                                    end.saturating_sub(s.start).saturating_add(1) > dl
                                })
                                .map(|s| {
                                    (
                                        s.id,
                                        s.start,
                                        s.end.load(Ordering::Relaxed),
                                        s.downloaded.load(Ordering::Relaxed),
                                        Arc::clone(&s.end),
                                    )
                                })
                                .collect()
                        };
                        for (id, start, end, already, end_cap) in orphans {
                            spawn_segment_worker_with_cap(
                                &mut workers,
                                job_id,
                                id,
                                start,
                                end,
                                already,
                                end_cap,
                                Arc::clone(&steal),
                                url.clone(),
                                Arc::clone(&client),
                                mmap.clone(),
                                tmp_path.clone(),
                                meta_job.clone(),
                                tx.clone(),
                                user_agent.clone(),
                                cookies.clone(),
                                referer.clone(),
                                Arc::clone(&signal.stop),
                                false, // no steal — finish orphans
                                Arc::clone(&bandwidth),
                            );
                        }
                        if workers.is_empty() {
                            break;
                        }
                        continue;
                    }

                    if workers.is_empty() {
                        break;
                    }
                }

                _ = ticker.tick() => {
                    checkpoint_n = checkpoint_n.wrapping_add(1);
                    if signal.stop.load(Ordering::Relaxed) {
                        // Wake join loop by waiting for workers after stop.
                        if workers.is_empty() {
                            break;
                        }
                        continue;
                    }



                    let downloaded = steal.lock().total_downloaded();
                    let snapshot = steal.lock().snapshot();
                    // Resume metadata is less frequent than UI ticks — avoid disk thrash.
                    if checkpoint_n % META_EVERY_N == 0 {
                        let meta = PartMetadata::from_job(&meta_job, snapshot.clone());
                        if let Err(e) = write_part_meta_atomic(&meta) {
                            warn!("Checkpoint .part.meta write failed: {e:#}");
                        }
                    }
                    {
                        let mut m = mgr.write().await;
                        let updated = if let Some(j) = m.jobs.get_mut(&job_id) {
                            if j.status == DownloadStatus::Downloading {
                                let elapsed = UI_TICK.as_secs_f64().max(0.001);
                                let sample_speed =
                                    (downloaded.saturating_sub(last_sample_bytes)) as f64 / elapsed;
                                // Smooth so a quiet window doesn't drop the UI to ~0.
                                j.speed_bps = if j.speed_bps > 1.0 && sample_speed > 0.0 {
                                    j.speed_bps * 0.4 + sample_speed * 0.6
                                } else if sample_speed > 0.0 {
                                    sample_speed
                                } else {
                                    j.speed_bps * 0.5
                                };
                                let active = snapshot
                                    .iter()
                                    .filter(|s| s.remaining() > 0)
                                    .count()
                                    .max(1) as f64;
                                let per_seg = j.speed_bps / active;
                                j.segments = snapshot
                                    .into_iter()
                                    .map(|mut s| {
                                        s.speed_bps = if s.remaining() > 0 { per_seg } else { 0.0 };
                                        s
                                    })
                                    .collect();
                                j.downloaded_bytes = downloaded;
                                if j.speed_bps > 10.0 && j.total_bytes > downloaded {
                                    let eta = (j.total_bytes - downloaded) as f64 / j.speed_bps;
                                    if eta.is_finite() && eta > 0.0 && eta < 86400.0 * 30.0 {
                                        j.eta_seconds = Some(eta);
                                    } else {
                                        j.eta_seconds = None;
                                    }
                                } else {
                                    j.eta_seconds = None;
                                }
                                Some(j.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        if updated.is_some() && checkpoint_n % PERSIST_EVERY_N == 0 {
                            m.persist();
                        }
                        if let Some(job) = updated {
                            m.events.send(DmEvent::JobUpdated { job });
                        }
                    }

                    let elapsed = UI_TICK.as_secs_f64().max(0.001);
                    let sample_speed =
                        (downloaded.saturating_sub(last_sample_bytes)) as f64 / elapsed;

                    // Adaptive connection scaling: add while marginal gain > ~10%.
                    if scaling_enabled && active_connections < max_connections {
                        let gain_ok = last_sample_speed <= 0.0
                            || (sample_speed - last_sample_speed) / last_sample_speed
                                > ADAPTIVE_GAIN_THRESHOLD;
                        if gain_ok {
                            if let Some((new_id, new_start, new_end, already, new_cap)) =
                                steal.lock().try_steal(MIN_STEAL_BYTES)
                            {
                                info!(
                                    "Adaptive scale-up → {} connections (seg {} {}-{}, speed {:.0}→{:.0} B/s)",
                                    active_connections + 1,
                                    new_id,
                                    new_start,
                                    new_end,
                                    last_sample_speed,
                                    sample_speed
                                );
                                spawn_segment_worker_with_cap(
                                    &mut workers,
                                    job_id,
                                    new_id,
                                    new_start,
                                    new_end,
                                    already,
                                    new_cap,
                                    Arc::clone(&steal),
                                    url.clone(),
                                    Arc::clone(&client),
                                    mmap.clone(),
                                    tmp_path.clone(),
                                    meta_job.clone(),
                                    tx.clone(),
                                    user_agent.clone(),
                                    cookies.clone(),
                                    referer.clone(),
                                    Arc::clone(&signal.stop),
                                    allow_steal,
                                    Arc::clone(&bandwidth),
                                );

                                active_connections += 1;
                                if active_connections >= max_connections {
                                    scaling_enabled = false;
                                }
                            } else {
                                scaling_enabled = false;
                            }
                        } else if last_sample_speed > 0.0 {
                            info!(
                                "Adaptive scaling plateau at {} connections ({:.0}→{:.0} B/s, gain ≤ {:.0}%)",
                                active_connections,
                                last_sample_speed,
                                sample_speed,
                                ADAPTIVE_GAIN_THRESHOLD * 100.0
                            );
                            scaling_enabled = false;
                        }
                    }

                    last_sample_speed = sample_speed;
                    last_sample_bytes = downloaded;

                    if workers.is_empty() {
                        if !signal.stop.load(Ordering::Relaxed)
                            && allow_steal
                            && steal.lock().has_remaining()
                        {
                            if let Some((new_id, new_start, new_end, already, new_cap)) =
                                steal.lock().try_steal(MIN_STEAL_BYTES.min(256 * 1024))
                            {
                                spawn_segment_worker_with_cap(
                                    &mut workers,
                                    job_id,
                                    new_id,
                                    new_start,
                                    new_end,
                                    already,
                                    new_cap,
                                    Arc::clone(&steal),
                                    url.clone(),
                                    Arc::clone(&client),
                                    mmap.clone(),
                                    tmp_path.clone(),
                                    meta_job.clone(),
                                    tx.clone(),
                                    user_agent.clone(),
                                    cookies.clone(),
                                    referer.clone(),
                                    Arc::clone(&signal.stop),
                                    allow_steal,
                                    Arc::clone(&bandwidth),
                                );

                                active_connections += 1;
                            } else {
                                let orphans: Vec<_> = {
                                    let state = steal.lock();
                                    state
                                        .segments
                                        .iter()
                                        .filter(|s| {
                                            let end = s.end.load(Ordering::Relaxed);
                                            let dl = s.downloaded.load(Ordering::Relaxed);
                                            end.saturating_sub(s.start).saturating_add(1) > dl
                                        })
                                        .map(|s| {
                                            (
                                                s.id,
                                                s.start,
                                                s.end.load(Ordering::Relaxed),
                                                s.downloaded.load(Ordering::Relaxed),
                                                Arc::clone(&s.end),
                                            )
                                        })
                                        .collect()
                                };
                                for (id, start, end, already, end_cap) in orphans {
                                    spawn_segment_worker_with_cap(
                                        &mut workers,
                                        job_id,
                                        id,
                                        start,
                                        end,
                                        already,
                                        end_cap,
                                        Arc::clone(&steal),
                                        url.clone(),
                                        Arc::clone(&client),
                                        mmap.clone(),
                                        tmp_path.clone(),
                                        meta_job.clone(),
                                        tx.clone(),
                                        user_agent.clone(),
                                        cookies.clone(),
                                        referer.clone(),
                                        Arc::clone(&signal.stop),
                                        false,
                                        Arc::clone(&bandwidth),
                                    );
                                }
                                if workers.is_empty() {
                                    break;
                                }
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        }

        // Drain any stragglers after stop.
        while let Some(joined) = workers.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if !signal.stop.load(Ordering::Relaxed) && worker_error.is_none() {
                        worker_error = Some(e);
                    }
                }
                Err(_) => {}
            }
        }

        if signal.stop.load(Ordering::Relaxed) {
            job.segments = steal.lock().snapshot();
            job.downloaded_bytes = job.segments.iter().map(|s| s.downloaded).sum();
            job.speed_bps = 0.0;
            return Self::finalize_stop(mgr, job, mmap, &signal, &tmp_path).await;
        }

        if let Some(err) = worker_error {
            job.segments = steal.lock().snapshot();
            job.downloaded_bytes = job.segments.iter().map(|s| s.downloaded).sum();
            if let Some(map) = &mmap {
                let _ = map.flush();
            }
            drop(mmap);
            job.status = DownloadStatus::Failed;
            job.speed_bps = 0.0;
            update_state(mgr.clone(), job.clone()).await;
            return Err(err);
        }

        // Verify written bytes match Content-Length before marking complete.
        job.segments = steal.lock().snapshot();
        let downloaded: u64 = job.segments.iter().map(|s| s.downloaded).sum();
        job.downloaded_bytes = downloaded;

        if let Some(map) = &mmap {
            map.flush()?;
        }
        drop(mmap);

        if job.total_bytes > 0 {
            let verified = crate::engine::integrity::verify_complete(&job.segments, job.total_bytes);
            if let Err(e) = verified {
                job.status = DownloadStatus::Failed;
                job.speed_bps = 0.0;
                update_state(mgr.clone(), job.clone()).await;
                bail!("Download incomplete ({downloaded} of {} bytes): {e:#}", job.total_bytes);
            }
        }

        let expected_len = (job.total_bytes > 0).then_some(job.total_bytes);
        if let Err(e) = crate::engine::integrity::finalize_file(
            std::path::Path::new(&tmp_path),
            std::path::Path::new(&job.save_path),
            expected_len,
        )
        .await
        {
            // Keep the .part payload + meta so Resume can finish the move.
            job.status = DownloadStatus::Failed;
            job.speed_bps = 0.0;
            update_state(mgr.clone(), job.clone()).await;
            return Err(e.context("Could not finalize downloaded file"));
        }
        // Payload is in place; drop resume sidecar.
        let _ = std::fs::remove_file(crate::io::sparse::part_meta_path(&job.save_path));

        job.status = DownloadStatus::Completed;
        job.completed_at = Some(chrono::Utc::now());
        job.downloaded_bytes = if job.total_bytes > 0 {
            job.total_bytes
        } else {
            downloaded
        };
        job.speed_bps = 0.0;
        job.eta_seconds = Some(0.0);
        update_state(mgr.clone(), job.clone()).await;

        info!("Download complete: {}", job.filename);
        Ok(())
    }

    async fn run_youtube_download(
        mgr: Arc<RwLock<Self>>,
        mut job: DownloadJob,
        signal: Arc<JobSignal>,
        user_agent: Option<String>,
        cookies: Option<String>,
        referer: Option<String>,
    ) -> Result<()> {
        let update_state = |mgr: Arc<RwLock<Self>>, updated_job: DownloadJob| async move {
            let mut m = mgr.write().await;
            if let Some(j) = m.jobs.get_mut(&updated_job.id) {
                if j.status == DownloadStatus::Cancelled {
                    return;
                }
                if j.status == DownloadStatus::Paused
                    && !matches!(
                        updated_job.status,
                        DownloadStatus::Paused | DownloadStatus::Completed | DownloadStatus::Failed
                    )
                {
                    return;
                }
                *j = updated_job.clone();
                m.events.send(DmEvent::JobUpdated { job: updated_job });
                m.persist();
            }
        };

        job.engine = "youtube".into();
        job.status = DownloadStatus::Probing;
        job.supports_ranges = false;
        let yt_fragments = crate::youtube::DEFAULT_CONCURRENT_FRAGMENTS as usize;
        job.segment_count = yt_fragments;
        update_state(mgr.clone(), job.clone()).await;

        let opts = crate::youtube::YoutubeOptions {
            quality: job.quality.clone(),
            format: job.format.clone(),
            user_agent: user_agent.clone(),
            cookies: cookies.clone(),
            referer: referer.clone(),
            concurrent_fragments: Some(crate::youtube::DEFAULT_CONCURRENT_FRAGMENTS),
        };

        // Resolve title / duration up front when possible.
        // Ensure tools exist before spending time on probe/download.
        if let Err(e) = crate::youtube::ensure_tools().await {
            job.status = DownloadStatus::Failed;
            job.speed_bps = 0.0;
            update_state(mgr.clone(), job.clone()).await;
            return Err(e);
        }

        /* HLS/DASH URLs are already the media. Probing them runs a second yt-dlp
           pass before any bytes are saved and often renames the file to "index". */
        let client_meta_ready = crate::youtube::is_stream_manifest_url(&job.url);

        if !client_meta_ready {
        match crate::youtube::probe_cancellable(
            &job.url,
            &opts,
            Some(Arc::clone(&signal.stop)),
            Some(Arc::clone(&signal.pid)),
        )
        .await
        {
            Ok(info) => {
                if signal.stop.load(Ordering::Relaxed) {
                    return Self::finalize_stop(mgr, job, None, &signal, "").await;
                }
                let safe = sanitize_yt_name(&info.title);
                let ext = if job
                    .quality
                    .as_deref()
                    .map(|q| q.eq_ignore_ascii_case("audio"))
                    .unwrap_or(false)
                {
                    "m4a"
                } else {
                    "mp4"
                };
                let hint = if let Some(q) = job.quality.as_deref().filter(|s| !s.is_empty()) {
                    format!("{safe} [{q}].{ext}")
                } else {
                    format!("{safe}.{ext}")
                };
                let parent = PathBuf::from(&job.save_path)
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."));
                let (name, path) = unique_save_path(&parent, &hint);
                job.filename = name;
                job.save_path = path;
                // Estimate size from best matching format when available.
                if let Some(est) = estimate_yt_size(&info, &opts) {
                    job.total_bytes = est;
                } else if let Some(dur) = info.duration {
                    // Fallback: duration × peak video+audio bitrate.
                    if let Some(est) = estimate_yt_size_from_bitrate(&info, &opts, dur) {
                        job.total_bytes = est;
                    }
                }
                info!("YouTube: {} ({})", info.title, info.id);
            }
            Err(e) => {
                warn!("YouTube probe failed: {e:#}");
                job.status = DownloadStatus::Failed;
                job.speed_bps = 0.0;
                update_state(mgr.clone(), job.clone()).await;
                return Err(e).context("YouTube probe failed");
            }
        }
        } else {
            info!(
                "YouTube: using extension metadata ({} bytes) — skipping probe",
                job.total_bytes
            );
            update_state(mgr.clone(), job.clone()).await;
        }

        if signal.stop.load(Ordering::Relaxed) {
            return Self::finalize_stop(mgr, job, None, &signal, "").await;
        }

        job.status = DownloadStatus::Downloading;
        // Initialise N virtual segments so the UI shows parallel fragment progress.
        // yt-dlp runs DEFAULT_CONCURRENT_FRAGMENTS workers internally (-N flag); we
        // mirror that count so the progress bar looks like a true multi-connection
        // download rather than a single block.
        let yt_seg_n = job.segment_count.max(1);
        let init_total = job.total_bytes;
        job.segments = (0..yt_seg_n)
            .map(|i| {
                let start = if init_total > 0 { (init_total * i as u64) / yt_seg_n as u64 } else { 0 };
                let end   = if init_total > 0 {
                    if i == yt_seg_n - 1 { init_total.saturating_sub(1) }
                    else { (init_total * (i as u64 + 1)) / yt_seg_n as u64 - 1 }
                } else { 0 };
                Segment::new(i, start, end)
            })
            .collect();
        update_state(mgr.clone(), job.clone()).await;

        let save_dir = PathBuf::from(&job.save_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let preferred = job.filename.clone();
        let stop = Arc::clone(&signal.stop);
        let job_id = job.id;

        let known_total = if job.total_bytes > 0 {
            Some(job.total_bytes)
        } else {
            None
        };

        // Sequential progress sink — spawning a task per tick raced and wiped
        // the UI back to 0 B / stale totals via out-of-order JobUpdated events.
        let (prog_tx, mut prog_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::youtube::YoutubeProgress>();
        let mgr_prog_task = mgr.clone();
        let prog_job_id = job_id;
        // yt_known_total = firm total at session start (from extension estimate or
        // prior paused state). DMPROG reports the SUM of DASH component stream bytes
        // (video + audio separately), which always exceeds the final merged container
        // size. Pinning total_bytes to this cap prevents the 1.7 GB → 1.8 GB jump.
        let yt_known_total = known_total;
        // has_firm_total is true whenever we started with any size estimate.
        // A total that only came from yt-dlp's running estimate is not firm:
        // keep following the estimate after Pause/Resume too.
        let has_firm_total = yt_known_total.is_some() && !job.size_is_estimate;
        let progress_task = tokio::spawn(async move {
            // Allow one backward movement of downloaded_bytes at the very start of
            // a download/resume session. On resume, yt-dlp restarts its internal
            // session counter from 0 before scanning cached fragments, so the first
            // few progress reports can be lower than the paused downloaded_bytes.
            let mut first_update = true;
            let mut last_emit = std::time::Instant::now() - std::time::Duration::from_secs(1);
            let mut last_phase = String::new();
            while let Some(progress) = prog_rx.recv().await {
                let mut m = mgr_prog_task.write().await;
                let Some(j) = m.jobs.get_mut(&prog_job_id) else {
                    continue;
                };
                if j.status != DownloadStatus::Downloading {
                    continue;
                }
                // Grow the total from a real content length. HLS peak-bitrate
                // estimates (the old ~4 GB figure) must not replace a size
                // measured from the segment playlist.
                let manifest = crate::youtube::is_stream_manifest_url(&j.url);
                let ignore_estimate = progress.is_estimate && (has_firm_total || manifest);
                if progress.is_estimate && manifest && !has_firm_total {
                    // No size was supplied for this stream (e.g. sent by the website).
                    // yt-dlp's estimate here is measured from the fragments already
                    // fetched (bytes so far / parts done x total parts), so follow it
                    // up and down. Without it the job shows "Unknown" and 0%.
                    if progress.total_bytes > 0 {
                        j.total_bytes = progress.total_bytes;
                        j.size_is_estimate = true;
                    }
                } else if progress.total_bytes > j.total_bytes && !ignore_estimate {
                    j.total_bytes = progress.total_bytes;
                }
                // Update downloaded: allow one backward movement at session start
                // so that on resume the display snaps to the true workdir bytes.
                if progress.downloaded_bytes >= j.downloaded_bytes || first_update {
                    j.downloaded_bytes = progress.downloaded_bytes;
                    first_update = false;
                }
                // If downloaded exceeds total (happens if total was an underestimate),
                // push total up to match it so the progress bar stays at 100%.
                if j.total_bytes > 0 && j.downloaded_bytes > j.total_bytes {
                    j.total_bytes = j.downloaded_bytes;
                }

                if progress.speed_bps > 1.0 {
                    j.speed_bps = progress.speed_bps;
                } else if j.speed_bps <= 1.0 && j.downloaded_bytes > 0 {
                    let elapsed = (chrono::Utc::now() - j.created_at)
                        .num_milliseconds()
                        .max(1) as f64
                        / 1000.0;
                    if elapsed > 0.5 {
                        j.speed_bps = j.downloaded_bytes as f64 / elapsed;
                    }
                }
                if j.speed_bps > 1.0 && j.total_bytes > j.downloaded_bytes {
                    j.eta_seconds =
                        Some((j.total_bytes - j.downloaded_bytes) as f64 / j.speed_bps);
                } else if let Some(eta) = progress.eta_seconds.filter(|e| *e > 0.0) {
                    j.eta_seconds = Some(eta);
                }

                if !progress.phase.is_empty() {
                    j.phase = Some(progress.phase.clone());
                }

                // Distribute downloaded bytes evenly across N virtual segments so
                // the UI shows a multi-connection progress bar matching yt-dlp's
                // internal -N concurrent fragment workers.
                let total = j.total_bytes;
                let downloaded = j.downloaded_bytes;
                let n = j.segment_count.max(1);
                let per_seg_speed = if n > 0 { j.speed_bps / n as f64 } else { j.speed_bps };
                if total > 0 && n > 1 {
                    let per_seg_bytes = (downloaded / n as u64).min(total / n as u64);
                    let remainder = downloaded.saturating_sub(per_seg_bytes * n as u64);
                    j.segments = (0..n)
                        .map(|i| {
                            let start = (total * i as u64) / n as u64;
                            let end = if i == n - 1 {
                                total.saturating_sub(1)
                            } else {
                                (total * (i as u64 + 1)) / n as u64 - 1
                            };
                            let seg_size = end.saturating_sub(start) + 1;
                            // Last segment absorbs the remainder so sum == downloaded.
                            let seg_dl = if i == n - 1 {
                                (per_seg_bytes + remainder).min(seg_size)
                            } else {
                                per_seg_bytes.min(seg_size)
                            };
                            Segment { id: i, start, end, downloaded: seg_dl, speed_bps: per_seg_speed }
                        })
                        .collect();
                } else if j.segments.is_empty() {
                    j.segments
                        .push(Segment::new(0, 0, total.saturating_sub(1)));
                } else {
                    j.segments[0].end = total.saturating_sub(1);
                    let span = j.segments[0]
                        .end
                        .saturating_sub(j.segments[0].start)
                        .saturating_add(1);
                    j.segments[0].downloaded = downloaded.min(span);
                    j.segments[0].speed_bps = j.speed_bps;
                }

                let phase_changed = progress.phase != last_phase;
                let due = last_emit.elapsed() >= std::time::Duration::from_millis(400);
                let snapshot = if phase_changed || due {
                    last_emit = std::time::Instant::now();
                    last_phase = progress.phase.clone();
                    Some(j.clone())
                } else {
                    None
                };
                if let Some(job) = snapshot {
                    m.events.send(DmEvent::JobUpdated { job });
                }
            }
        });

        let result = crate::youtube::download(
            job.id,
            &job.url,
            &save_dir,
            Some(&preferred),
            &opts,
            known_total,
            stop,
            Arc::clone(&signal.pid),
            move |progress| {
                let _ = prog_tx.send(progress);
            },
        )
        .await;

        let _ = progress_task.await;

        // Refresh job from manager (progress tasks may have newer totals).
        if let Some(latest) = mgr.read().await.jobs.get(&job_id).cloned() {
            job.total_bytes = latest.total_bytes;
            job.downloaded_bytes = latest.downloaded_bytes;
            job.segments = latest.segments;
            job.filename = latest.filename;
            job.save_path = latest.save_path;
        }

        if signal.stop.load(Ordering::Relaxed) {
            job.speed_bps = 0.0;
            return Self::finalize_stop(mgr, job, None, &signal, "").await;
        }

        match result {
            Ok(path) => {
                let final_name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&job.filename)
                    .to_string();
                job.filename = final_name;
                job.save_path = path.to_string_lossy().to_string();
                if let Ok(meta) = std::fs::metadata(&path) {
                    job.total_bytes = meta.len();
                    job.downloaded_bytes = meta.len();
                }
                job.status = DownloadStatus::Completed;
                job.completed_at = Some(chrono::Utc::now());
                job.speed_bps = 0.0;
                job.eta_seconds = Some(0.0);
                job.segments = vec![Segment {
                    id: 0,
                    start: 0,
                    end: job.total_bytes.saturating_sub(1),
                    downloaded: job.total_bytes,
                    speed_bps: 0.0,
                }];
                update_state(mgr.clone(), job.clone()).await;
                info!("YouTube download complete: {}", job.filename);
                Ok(())
            }
            Err(e) => {
                if signal.stop.load(Ordering::Relaxed) {
                    job.speed_bps = 0.0;
                    return Self::finalize_stop(mgr, job, None, &signal, "").await;
                }
                job.status = DownloadStatus::Failed;
                job.speed_bps = 0.0;
                update_state(mgr.clone(), job.clone()).await;
                Err(e)
            }
        }
    }

    async fn finalize_stop(
        mgr: Arc<RwLock<Self>>,
        mut job: DownloadJob,
        mmap: Option<SharedMmap>,
        signal: &JobSignal,
        tmp_path: &str,
    ) -> Result<()> {
        if let Some(map) = &mmap {
            let _ = map.sync_after_write(0, 0);
        }
        drop(mmap);

        if signal.cancel.load(Ordering::Relaxed) {
            remove_part_artifacts(&job.save_path);
            if job.engine == "youtube" {
                let save_dir = std::path::PathBuf::from(&job.save_path).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
                crate::youtube::cleanup_yt_temp(&save_dir, job.id);
            }
            if !tmp_path.is_empty() {
                let _ = std::fs::remove_file(tmp_path);
            }
            if signal.delete_file.load(Ordering::Relaxed) {
                let path = std::path::PathBuf::from(&job.save_path);
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                }
            }
            let mut m = mgr.write().await;
            m.persist();
            return Ok(());
        }

        // Durable pause: meta reflects flushed bytes only.
        if !job.segments.is_empty() {
            let meta = PartMetadata::from_job(&job, job.segments.clone());
            if let Err(e) = write_part_meta_atomic(&meta) {
                warn!("Failed to write pause .part.meta: {e:#}");
            }
        }

        job.status = DownloadStatus::Paused;
        job.speed_bps = 0.0;
        let mut m = mgr.write().await;
        let updated = if let Some(j) = m.jobs.get_mut(&job.id) {
            if j.status != DownloadStatus::Cancelled {
                if !job.segments.is_empty() {
                    j.segments = job.segments.clone();
                    j.downloaded_bytes = job.segments.iter().map(|s| s.downloaded).sum();
                }
                j.status = DownloadStatus::Paused;
                j.speed_bps = 0.0;
                Some(j.clone())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(updated) = updated {
            m.events.send(DmEvent::JobUpdated { job: updated });
            m.persist();
        }
        Ok(())
    }

    pub fn list_jobs(&self) -> Vec<&DownloadJob> {
        self.jobs.values().collect()
    }
}

fn is_googlevideo_url(url: &str) -> bool {
    url.contains("googlevideo.com")
}

fn sanitize_yt_name(title: &str) -> String {
    let mut s: String = title
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
        "YouTube video".into()
    } else {
        s
    }
}

fn estimate_yt_size(
    info: &crate::youtube::YoutubeInfo,
    opts: &crate::youtube::YoutubeOptions,
) -> Option<u64> {
    let q = opts
        .quality
        .as_deref()
        .unwrap_or("best")
        .trim()
        .to_ascii_lowercase();
    if q == "audio" || q == "audio-only" {
        return info
            .formats
            .iter()
            .filter(|f| f.is_audio && !f.is_video)
            .filter_map(|f| f.filesize.or(f.filesize_approx))
            .max();
    }
    let height = q.trim_end_matches('p').parse::<u32>().ok();
    let mut candidates: Vec<_> = info
        .formats
        .iter()
        .filter(|f| f.is_video)
        .collect();
    if let Some(h) = height {
        let capped: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|f| f.height.unwrap_or(0) <= h)
            .collect();
        if !capped.is_empty() {
            candidates = capped;
        }
    }
    let best_video = candidates
        .iter()
        .max_by_key(|f| {
            (
                f.height.unwrap_or(0),
                f.filesize.or(f.filesize_approx).unwrap_or(0),
            )
        })?;
    let video_size = best_video.filesize.or(best_video.filesize_approx).unwrap_or(0);
    let audio_size = if best_video.is_progressive {
        0
    } else {
        info.formats
            .iter()
            .filter(|f| f.is_audio && !f.is_video)
            .filter_map(|f| f.filesize.or(f.filesize_approx))
            .max()
            .unwrap_or(0)
    };
    let total = video_size + audio_size;
    if total > 0 {
        Some(total)
    } else {
        None
    }
}

fn estimate_yt_size_from_bitrate(
    info: &crate::youtube::YoutubeInfo,
    opts: &crate::youtube::YoutubeOptions,
    duration_secs: f64,
) -> Option<u64> {
    if duration_secs <= 0.0 {
        return None;
    }
    let q = opts
        .quality
        .as_deref()
        .unwrap_or("best")
        .trim()
        .to_ascii_lowercase();
    let height = q.trim_end_matches('p').parse::<u32>().ok();
    let mut v_tbr = 0.0;
    for f in info.formats.iter().filter(|f| f.is_video) {
        if let Some(h) = height {
            if f.height.unwrap_or(0) > h {
                continue;
            }
        }
        if let Some(tbr) = f.tbr {
            if tbr > v_tbr {
                v_tbr = tbr;
            }
        }
    }
    let a_tbr = info
        .formats
        .iter()
        .filter(|f| f.is_audio && !f.is_video)
        .filter_map(|f| f.tbr)
        .fold(0.0_f64, f64::max);
    let tbr = if q == "audio" || q == "audio-only" {
        a_tbr
    } else {
        v_tbr + a_tbr
    };
    if tbr <= 0.0 {
        return None;
    }
    // tbr is kbps → bytes
    Some(((tbr * 1000.0 / 8.0) * duration_secs) as u64)
}

fn unique_save_path(save_dir: &PathBuf, filename: &str) -> (String, String) {
    let mut filename = filename.to_string();
    let mut save_path = save_dir.join(&filename);
    if save_path.exists() {
        let path = std::path::Path::new(&filename);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&filename);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e))
            .unwrap_or_default();

        let mut counter = 1;
        loop {
            let new_name = format!("{} ({}){}", stem, counter, ext);
            save_path = save_dir.join(&new_name);
            if !save_path.exists() {
                filename = new_name;
                break;
            }
            counter += 1;
        }
    }
    (filename, save_path.to_string_lossy().to_string())
}

pub fn divide_into_segments(total: u64, count: usize) -> Vec<Segment> {
    if total == 0 || count == 0 {
        return vec![Segment::new(0, 0, 0)];
    }
    let chunk = total / count as u64;
    let mut segs = Vec::with_capacity(count);
    for i in 0..count {
        let start = i as u64 * chunk;
        let end = if i == count - 1 {
            total - 1
        } else {
            start + chunk - 1
        };
        segs.push(Segment::new(i, start, end));
    }
    segs
}

#[allow(clippy::too_many_arguments)]
fn spawn_segment_worker(
    workers: &mut JoinSet<Result<()>>,
    job_id: JobId,
    seg_id: usize,
    start: u64,
    end: u64,
    already: u64,
    steal: Arc<parking_lot::Mutex<StealState>>,
    url: String,
    client: Arc<HttpClient>,
    mmap: Option<SharedMmap>,
    tmp_path: String,
    meta_job: DownloadJob,
    tx: EventPublisher,
    user_agent: Option<String>,
    cookies: Option<String>,
    referer: Option<String>,
    stop: Arc<AtomicBool>,
    allow_steal: bool,
    bandwidth: Arc<BandwidthLimiter>,
) {
    let end_cap = steal
        .lock()
        .segments
        .iter()
        .find(|l| l.id == seg_id)
        .map(|l| Arc::clone(&l.end))
        .unwrap_or_else(|| Arc::new(AtomicU64::new(end)));
    spawn_segment_worker_with_cap(
        workers,
        job_id,
        seg_id,
        start,
        end,
        already,
        end_cap,
        steal,
        url,
        client,
        mmap,
        tmp_path,
        meta_job,
        tx,
        user_agent,
        cookies,
        referer,
        stop,
        allow_steal,
        bandwidth,
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_segment_worker_with_cap(
    workers: &mut JoinSet<Result<()>>,
    job_id: JobId,
    seg_id: usize,
    start: u64,
    end: u64,
    already: u64,
    end_cap: Arc<AtomicU64>,
    steal: Arc<parking_lot::Mutex<StealState>>,
    url: String,
    client: Arc<HttpClient>,
    mmap: Option<SharedMmap>,
    tmp_path: String,
    meta_job: DownloadJob,
    tx: EventPublisher,
    user_agent: Option<String>,
    cookies: Option<String>,
    referer: Option<String>,
    stop: Arc<AtomicBool>,
    allow_steal: bool,
    bandwidth: Arc<BandwidthLimiter>,
) {
    workers.spawn(async move {
        crate::engine::worker::download_segment(
            job_id,
            seg_id,
            start,
            end,
            end_cap,
            url,
            client,
            mmap,
            tmp_path,
            meta_job,
            tx,
            user_agent,
            cookies,
            referer,
            stop,
            Some(steal),
            allow_steal,
            already,
            bandwidth,
        )
        .await
    });
}
