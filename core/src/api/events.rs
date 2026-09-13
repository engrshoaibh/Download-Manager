//! Sequenced daemon events for WebSocket clients.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::engine::{DownloadJob, DmEvent, JobId};

/// Capacity of the process-wide broadcast bus (producers never block).
pub const EVENT_BUS_CAPACITY: usize = 1024;

/// Per-WebSocket outbound buffer; when full, oldest pending messages are dropped.
pub const WS_OUTBOUND_CAPACITY: usize = 128;

/// Wire format for every WS server message (snapshot or delta).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    pub seq: u64,
    #[serde(flatten)]
    pub body: WsBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsBody {
    Snapshot {
        jobs: Vec<DownloadJob>,
    },
    JobAdded {
        job: DownloadJob,
    },
    JobUpdated {
        job: DownloadJob,
    },
    JobRemoved {
        id: JobId,
    },
    SegmentProgress {
        job_id: JobId,
        segment_id: usize,
        downloaded: u64,
        speed_bps: f64,
    },
    Error {
        id: Option<JobId>,
        message: String,
    },
}

impl From<DmEvent> for WsBody {
    fn from(event: DmEvent) -> Self {
        match event {
            DmEvent::JobAdded { job } => WsBody::JobAdded { job },
            DmEvent::JobUpdated { job } => WsBody::JobUpdated { job },
            DmEvent::JobRemoved { id } => WsBody::JobRemoved { id },
            DmEvent::SegmentProgress {
                job_id,
                segment_id,
                downloaded,
                speed_bps,
            } => WsBody::SegmentProgress {
                job_id,
                segment_id,
                downloaded,
                speed_bps,
            },
            DmEvent::Error { id, message } => WsBody::Error { id, message },
        }
    }
}

/// Client → server control messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientMessage {
    /// Ask for a fresh full snapshot (e.g. after detecting a seq gap).
    Resync,
    Ping,
}

/// Cloneable publisher: assigns monotonic `seq` and fans out without blocking jobs.
#[derive(Clone)]
pub struct EventPublisher {
    tx: broadcast::Sender<WsMessage>,
    seq: Arc<AtomicU64>,
}

impl EventPublisher {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self {
            tx,
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Last assigned sequence number (0 if nothing published yet).
    pub fn current_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WsMessage> {
        self.tx.subscribe()
    }

    /// Publish a delta event with the next monotonic sequence number.
    pub fn send(&self, event: DmEvent) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let msg = WsMessage {
            seq,
            body: WsBody::from(event),
        };
        // Ignore lagging/no-subscriber errors — producers must never stall.
        let _ = self.tx.send(msg);
    }

    /// Build a snapshot stamped with the **current** seq (does not advance).
    ///
    /// Clients treat this as the baseline; subsequent deltas must be `seq+1`, …
    pub fn snapshot(&self, jobs: Vec<DownloadJob>) -> WsMessage {
        WsMessage {
            seq: self.current_seq(),
            body: WsBody::Snapshot { jobs },
        }
    }
}

impl Default for EventPublisher {
    fn default() -> Self {
        Self::new()
    }
}
