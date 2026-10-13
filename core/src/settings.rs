use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

const APP_DIR: &str = "download-manager";
const SETTINGS_FILE: &str = "settings.json";

/// Daemon-wide knobs persisted under the local app data directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSettings {
    /// Soft cap on bytes/sec across all jobs. `0` = unlimited.
    #[serde(default)]
    pub bandwidth_limit_bps: u64,
    /// Max concurrent range connections across all jobs (semaphore permits).
    #[serde(default = "default_max_global_connections")]
    pub max_global_connections: usize,
    /// Hours before orphaned `.part` files (no jobs.json entry) are deleted.
    /// Younger orphans with `.part.meta` are re-imported as paused jobs.
    #[serde(default = "default_orphan_part_retention_hours")]
    pub orphan_part_retention_hours: u64,
    /// Max completed/failed jobs retained in `jobs.json` (`0` = unlimited).
    /// Active and paused jobs are never pruned by this cap.
    #[serde(default = "default_job_history_limit")]
    pub job_history_limit: usize,
    /// Netscape `cookies.txt` handed to yt-dlp when YouTube asks to "confirm you're not a bot".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube_cookies_file: Option<String>,
    /// Browser yt-dlp should read cookies from (`firefox`, `chrome`, `edge`, `brave`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube_cookies_browser: Option<String>,
}

fn default_max_global_connections() -> usize {
    64
}

fn default_orphan_part_retention_hours() -> u64 {
    168 // 7 days
}

fn default_job_history_limit() -> usize {
    200
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            bandwidth_limit_bps: 0,
            max_global_connections: default_max_global_connections(),
            orphan_part_retention_hours: default_orphan_part_retention_hours(),
            job_history_limit: default_job_history_limit(),
            youtube_cookies_file: None,
            youtube_cookies_browser: None,
        }
    }
}

pub fn settings_path() -> PathBuf {
    // DM_DATA_DIR lets tests and portable installs keep state out of the user profile.
    std::env::var_os("DM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(APP_DIR)
        })
        .join(SETTINGS_FILE)
}

pub fn load_settings() -> DaemonSettings {
    let path = settings_path();
    match fs::read_to_string(&path) {
        Ok(raw) if !raw.trim().is_empty() => match serde_json::from_str(&raw) {
            Ok(s) => {
                info!("Loaded settings from {}", path.display());
                s
            }
            Err(e) => {
                warn!("Invalid settings file ({}): {e:#} — using defaults", path.display());
                DaemonSettings::default()
            }
        },
        _ => DaemonSettings::default(),
    }
}

pub fn save_settings(settings: &DaemonSettings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create settings dir: {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(settings)?;
    fs::write(&path, raw)
        .with_context(|| format!("Failed to write settings: {}", path.display()))?;
    Ok(())
}

/// Token-bucket bandwidth limiter shared by every segment worker.
///
/// Permits represent bytes. A background task refills at `bandwidth_limit_bps`.
/// When the limit is `0`, [`throttle`] is a no-op.
pub struct BandwidthLimiter {
    limit_bps: AtomicU64,
    tokens: Arc<Semaphore>,
    /// Caps concurrent range GETs across all jobs.
    pub connections: Arc<Semaphore>,
    max_connections: AtomicU64,
}

impl BandwidthLimiter {
    pub fn new(settings: &DaemonSettings) -> Arc<Self> {
        let limit = settings.bandwidth_limit_bps;
        let max_conn = settings.max_global_connections.max(1);
        // Burst ≈ 500ms of budget so short spikes do not stall.
        let initial = if limit == 0 {
            Semaphore::MAX_PERMITS.min(1024 * 1024)
        } else {
            ((limit / 2).max(64 * 1024) as usize).min(Semaphore::MAX_PERMITS)
        };

        let limiter = Arc::new(Self {
            limit_bps: AtomicU64::new(limit),
            tokens: Arc::new(Semaphore::new(initial)),
            connections: Arc::new(Semaphore::new(max_conn)),
            max_connections: AtomicU64::new(max_conn as u64),
        });

        Self::spawn_refill(Arc::clone(&limiter));
        limiter
    }

    fn spawn_refill(limiter: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let limit = limiter.limit_bps.load(Ordering::Relaxed);
                if limit == 0 {
                    continue;
                }
                let per_tick = (limit / 10).max(1) as usize;
                let burst = ((limit / 2).max(64 * 1024) as usize).min(Semaphore::MAX_PERMITS);
                let available = limiter.tokens.available_permits();
                if available < burst {
                    let add = (burst - available).min(per_tick);
                    if add > 0 {
                        limiter.tokens.add_permits(add);
                    }
                }
            }
        });
    }

    pub fn apply(&self, settings: &DaemonSettings) {
        self.limit_bps
            .store(settings.bandwidth_limit_bps, Ordering::Relaxed);

        let new_max = settings.max_global_connections.max(1) as u64;
        let old_max = self.max_connections.swap(new_max, Ordering::Relaxed);
        if new_max > old_max {
            self.connections
                .add_permits((new_max - old_max) as usize);
        }
        // Shrinking below current in-flight usage is best-effort: new acquires
        // wait until enough permits are released.
        debug!(
            "Bandwidth limiter updated: limit_bps={} max_conn={}",
            settings.bandwidth_limit_bps, new_max
        );
    }

    /// Block until `bytes` of global budget are available (no-op if unlimited).
    pub async fn throttle(&self, bytes: usize) {
        let limit = self.limit_bps.load(Ordering::Relaxed);
        if limit == 0 || bytes == 0 {
            return;
        }

        let mut left = bytes;
        while left > 0 {
            // acquire_many is capped at u32::MAX; chunk large reads.
            let n = left.min(256 * 1024).min(Semaphore::MAX_PERMITS);
            match self.tokens.acquire_many(n as u32).await {
                Ok(permit) => {
                    // Consume tokens (do not release — refill restores budget).
                    permit.forget();
                    left -= n;
                }
                Err(_) => return, // semaphore closed — skip limiting
            }
        }
    }

    pub async fn acquire_connection(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        self.connections.clone().acquire_owned().await.ok()
    }
}
