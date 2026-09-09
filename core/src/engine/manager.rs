use std::collections::HashMap;
use std::sync::Arc;
use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info};

use super::{DownloadJob, DownloadStatus, DmEvent, JobId, Segment};
use crate::io::sparse::SparseFile;
use crate::network::client::HttpClient;

const DEFAULT_SEGMENT_COUNT: usize = 16;
const MAX_SEGMENT_COUNT: usize = 32;

/// Manages all active and queued download jobs
pub struct DownloadManager {
    pub jobs: HashMap<JobId, DownloadJob>,
    pub event_tx: broadcast::Sender<DmEvent>,
    pub http_client: Arc<HttpClient>,
}

impl DownloadManager {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            jobs: HashMap::new(),
            event_tx,
            http_client: Arc::new(HttpClient::new()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DmEvent> {
        self.event_tx.subscribe()
    }

    /// Add a new download job to the queue and return it
    pub fn add_download(
        &mut self,
        url: String,
        save_dir: String,
        segment_count: Option<usize>,
    ) -> Result<DownloadJob> {
        let url = url.trim().to_string();
        let segs = segment_count
            .unwrap_or(DEFAULT_SEGMENT_COUNT)
            .min(MAX_SEGMENT_COUNT);

        let mut filename = url
            .split('/')
            .last()
            .and_then(|s| s.split('?').next())
            .filter(|s| !s.is_empty())
            .unwrap_or("download")
            .to_string();

        let save_dir = PathBuf::from(&save_dir);
        let mut save_path = save_dir.join(&filename);
        if save_path.exists() {
            let path = std::path::Path::new(&filename);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(&filename);
            let ext = path.extension().and_then(|e| e.to_str()).map(|e| format!(".{}", e)).unwrap_or_default();
            
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

        let save_path = save_path.to_string_lossy().to_string();

        let job = DownloadJob::new(url.clone(), filename, save_path.clone(), segs);
        let job_id = job.id;

        // Broadcast job added
        let _ = self.event_tx.send(DmEvent::JobAdded { job: job.clone() });

        self.jobs.insert(job_id, job.clone());

        Ok(job)
    }

    /// Cancel a job
    pub fn cancel(&mut self, id: &JobId) -> bool {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = DownloadStatus::Cancelled;
            let _ = self.event_tx.send(DmEvent::JobUpdated { job: job.clone() });
            true
        } else {
            false
        }
    }

    /// Core download execution — runs in a spawned task
    pub async fn run_download(
        mgr: Arc<RwLock<Self>>,
        mut job: DownloadJob,
        client: Arc<HttpClient>,
        user_agent: Option<String>,
        cookies: Option<String>,
    ) -> Result<()> {
        let job_id = job.id;
        let tx = { mgr.read().await.event_tx.clone() };

        // Helper closure to update central state and broadcast
        let update_state = |mgr: Arc<RwLock<Self>>, updated_job: DownloadJob| async move {
            let mut m = mgr.write().await;
            if let Some(j) = m.jobs.get_mut(&updated_job.id) {
                *j = updated_job.clone();
                let _ = m.event_tx.send(DmEvent::JobUpdated { job: updated_job });
            }
        };

        // Step 1: Probe server capabilities
        job.status = DownloadStatus::Probing;
        update_state(mgr.clone(), job.clone()).await;

        let info = client
            .probe(&job.url, user_agent.as_deref(), cookies.as_deref())
            .await
            .context("Failed to probe download URL")?;

        job.total_bytes = info.content_length;
        job.supports_ranges = info.supports_ranges;
        job.use_http3 = info.supports_http3;

        // Step 2: Pre-allocate sparse file on disk using a temporary extension (if size is known)
        let tmp_path = format!("{}.tmp", job.save_path);
        
        let mmap = if job.total_bytes > 0 {
            let sparse = SparseFile::create(&tmp_path, job.total_bytes)
                .context("Failed to pre-allocate disk space")?;
            Some(Arc::new(std::sync::Mutex::new(sparse.into_mmap()?)))
        } else {
            None
        };

        // Step 3: Decide segmentation strategy
        let actual_segs = if job.supports_ranges && job.total_bytes > 0 {
            job.segment_count
        } else {
            1 // Server doesn't support range — single stream fallback
        };

        // Divide file into initial segments
        job.segments = divide_into_segments(job.total_bytes, actual_segs);
        job.status = DownloadStatus::Downloading;
        update_state(mgr.clone(), job.clone()).await;

        info!("Starting download: {} | {} segments | HTTP3={}", job.filename, actual_segs, job.use_http3);

        // Step 4: Spawn segment workers
        let url = job.url.clone();
        let mut workers = Vec::new();

        for seg in &job.segments {
            let seg_id = seg.id;
            let seg_start = seg.start;
            let seg_end = seg.end;
            let url_clone = url.clone();
            let client_clone = Arc::clone(&client);
            let mmap_clone = mmap.clone();
            let tmp_path_clone = tmp_path.clone();
            let tx_clone = tx.clone();
            let ua_clone = user_agent.clone();
            let cookies_clone = cookies.clone();

            let handle = tokio::spawn(async move {
                crate::engine::worker::download_segment(
                    job_id,
                    seg_id,
                    seg_start,
                    seg_end,
                    url_clone,
                    client_clone,
                    mmap_clone,
                    tmp_path_clone,
                    tx_clone,
                    ua_clone,
                    cookies_clone,
                )
                .await
            });

            workers.push(handle);
        }

        // Wait for all workers
        for handle in workers {
            handle.await??;
        }

        // Step 5: Flush mmap and mark complete (if we used mmap)
        if let Some(mmap_arc) = &mmap {
            let guard = mmap_arc.lock().unwrap();
            guard.flush()?;
        }
        
        // Ensure mmap is dropped so Windows releases the file lock before renaming
        drop(mmap);

        // Rename .tmp file to final file
        if let Err(e) = std::fs::rename(&tmp_path, &job.save_path) {
            tracing::warn!("Failed to rename {} to {}: {}", tmp_path, job.save_path, e);
        }

        job.status = DownloadStatus::Completed;
        update_state(mgr.clone(), job.clone()).await;

        info!("✅ Download complete: {}", job.filename);
        Ok(())
    }

    pub fn list_jobs(&self) -> Vec<&DownloadJob> {
        self.jobs.values().collect()
    }
}

/// Divide [0, total_bytes) into `count` evenly-distributed segments
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
