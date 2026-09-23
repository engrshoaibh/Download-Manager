use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod manager;
pub mod scheduler;
pub mod segment;
pub mod worker;
pub mod chunked;
pub mod native_payload;

/// Unique identifier for a download job
pub type JobId = Uuid;

/// Status of a download job
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Queued,
    Probing,       // Sending HEAD request
    Downloading,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

/// A single byte-range segment assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: usize,
    pub start: u64,
    pub end: u64,        // inclusive
    pub downloaded: u64,
    pub speed_bps: f64,  // bytes per second (rolling average)
}

impl Segment {
    pub fn new(id: usize, start: u64, end: u64) -> Self {
        Self { id, start, end, downloaded: 0, speed_bps: 0.0 }
    }

    pub fn remaining(&self) -> u64 {
        (self.end - self.start + 1).saturating_sub(self.downloaded)
    }

    pub fn current_offset(&self) -> u64 {
        self.start + self.downloaded
    }
}

/// A full download job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadJob {
    pub id: JobId,
    pub url: String,
    pub filename: String,
    pub save_path: String,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub segments: Vec<Segment>,
    pub status: DownloadStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub speed_bps: f64,
    pub eta_seconds: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub supports_ranges: bool,
    pub segment_count: usize,
    pub use_http3: bool,
    /// Optional request headers kept so Resume works after a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    /// YouTube quality hint: "best", "1080", "720", "audio", …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Raw yt-dlp `-f` format selector (overrides quality when set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// `http` (segmented) or `youtube` (yt-dlp + FFmpeg).
    #[serde(default = "default_engine")]
    pub engine: String,
}

fn default_engine() -> String {
    "http".into()
}

impl DownloadJob {
    pub fn new(url: String, filename: String, save_path: String, segment_count: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            url,
            filename,
            save_path,
            total_bytes: 0,
            downloaded_bytes: 0,
            segments: Vec::new(),
            status: DownloadStatus::Queued,
            phase: None,
            speed_bps: 0.0,
            eta_seconds: None,
            created_at: Utc::now(),
            completed_at: None,
            supports_ranges: false,
            segment_count,
            use_http3: false,
            user_agent: None,
            cookies: None,
            referer: None,
            quality: None,
            format: None,
            engine: "http".into(),
        }
    }

    pub fn progress_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.downloaded_bytes as f64 / self.total_bytes as f64) * 100.0
    }
}

/// Events broadcast over WebSocket to the UI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DmEvent {
    JobAdded { job: DownloadJob },
    JobUpdated { job: DownloadJob },
    JobRemoved { id: JobId },
    SegmentProgress {
        job_id: JobId,
        segment_id: usize,
        downloaded: u64,
        speed_bps: f64,
    },
    Error { id: Option<JobId>, message: String },
}
