use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use tracing::{debug, warn};

use crate::api::events::EventPublisher;
use crate::engine::{DownloadJob, DmEvent, JobId};
use crate::io::sparse::{write_part_meta_atomic, PartMetadata, SharedMmap};
use crate::network::client::{classify_download_error, ErrorClass, HttpClient};
use crate::settings::BandwidthLimiter;

use super::scheduler::{StealState, MIN_STEAL_BYTES};

const STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Max attempts per chunk (initial try + retries). Exhaustion isolates the
/// failure — siblings keep running and unfinished bytes stay stealable.
const MAX_RANGE_ATTEMPTS: u32 = 5;

/// Downloads a byte-range, then keeps stealing unfinished work until none remains.
pub async fn download_segment(
    job_id: JobId,
    mut seg_id: usize,
    mut start: u64,
    mut end: u64,
    mut end_cap: Arc<AtomicU64>,
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
    steal: Option<Arc<parking_lot::Mutex<StealState>>>,
    allow_steal: bool,
    mut already: u64,
    bandwidth: Arc<BandwidthLimiter>,
) -> anyhow::Result<()> {
    struct ActiveGuard {
        steal: Option<Arc<parking_lot::Mutex<crate::engine::scheduler::StealState>>>,
        seg_id: usize,
    }
    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            if let Some(steal) = &self.steal {
                steal.lock().set_inactive(self.seg_id);
            }
        }
    }
    let mut active_guard = ActiveGuard {
        steal: steal.clone(),
        seg_id,
    };

    // Spread the initial SYN/TLS burst so many workers do not all connect at once.
    if seg_id > 0 {
        let delay = std::time::Duration::from_millis((seg_id as u64 * 25).min(400));
        tokio::time::sleep(delay).await;
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        let mut attempt = 0u32;
        let mut chunk_done = false;
        loop {
            match download_range(
                job_id,
                seg_id,
                start,
                end,
                already,
                &end_cap,
                &url,
                &client,
                mmap.as_ref(),
                &tmp_path,
                &meta_job,
                &tx,
                user_agent.as_deref(),
                cookies.as_deref(),
                referer.as_deref(),
                &stop,
                steal.as_ref(),
                &bandwidth,
            )
            .await
            {
                Ok(_) => {
                    chunk_done = true;
                    break;
                }
                Err(e) => {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }

                    already = steal
                        .as_ref()
                        .and_then(|state| {
                            state
                                .lock()
                                .segments
                                .iter()
                                .find(|s| s.id == seg_id)
                                .map(|s| s.downloaded.load(Ordering::Relaxed))
                        })
                        .unwrap_or(already);

                    let class = classify_download_error(&e);
                    attempt += 1;

                    // Local I/O failures are job-level; HTTP/network stay isolated.
                    if is_local_io_error(&e) {
                        return Err(e);
                    }

                    if class == ErrorClass::Fatal || attempt >= MAX_RANGE_ATTEMPTS {
                        warn!(
                            "Segment {seg_id} isolating after {attempt}/{MAX_RANGE_ATTEMPTS} \
                             attempts ({class:?}): {e:#} — siblings continue"
                        );
                        // Leave remaining bytes in StealState for other workers.
                        break;
                    }

                    let backoff =
                        std::time::Duration::from_millis(200 * 2u64.pow(attempt.saturating_sub(1)));
                    warn!(
                        "Segment {} retry {}/{} after retryable error: {e:#}",
                        seg_id, attempt, MAX_RANGE_ATTEMPTS
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }

        if !chunk_done {
            debug!("Segment {seg_id} abandoned unfinished range; seeking steal work");
        }

        if !allow_steal {
            break;
        }
        let Some(state) = &steal else { break };
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let Some((new_id, new_start, new_end, new_already, new_cap)) = ({
            let mut guard = state.lock();
            guard.try_steal(MIN_STEAL_BYTES)
        }) else {
            break;
        };

        debug!(
            "Worker {} stole segment {} ({}-{})",
            seg_id, new_id, new_start, new_end
        );
        seg_id = new_id;
        active_guard.seg_id = new_id;
        start = new_start;
        end = new_end;
        end_cap = new_cap;
        already = new_already;
    }

    Ok(())
}

fn is_local_io_error(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if cause.downcast_ref::<std::io::Error>().is_some() {
            return true;
        }
        let msg = cause.to_string();
        if msg.contains("Write out of bounds")
            || msg.contains("Failed to flush")
            || msg.contains("sync_data")
        {
            return true;
        }
    }
    false
}

async fn download_range(
    job_id: JobId,
    seg_id: usize,
    start: u64,
    end: u64,
    already: u64,
    end_cap: &AtomicU64,
    url: &str,
    client: &HttpClient,
    mmap: Option<&SharedMmap>,
    tmp_path: &str,
    meta_job: &DownloadJob,
    tx: &EventPublisher,
    user_agent: Option<&str>,
    cookies: Option<&str>,
    referer: Option<&str>,
    stop: &AtomicBool,
    steal: Option<&Arc<parking_lot::Mutex<StealState>>>,
    bandwidth: &BandwidthLimiter,
) -> Result<u64> {
    let unknown_length = start == 0 && end == 0;
    let resume_at = start.saturating_add(already);
    let request_end = end_cap.load(Ordering::Acquire).min(end);
    if !unknown_length && resume_at > request_end {
        return Ok(already);
    }
    let range_header = if unknown_length {
        String::new()
    } else {
        format!("bytes={}-{}", resume_at, request_end)
    };

    debug!(
        "Segment {} starting: Range: {}",
        seg_id,
        if range_header.is_empty() {
            "none"
        } else {
            &range_header
        }
    );

    let _conn_permit = bandwidth.acquire_connection().await;

    let response = if range_header.is_empty() {
        client
            .get_range(url, "", user_agent, cookies, referer)
            .await?
    } else {
        client
            .get_range(url, &range_header, user_agent, cookies, referer)
            .await?
    };

    let mut offset = resume_at;
    let mut total_downloaded: u64 = already;

    let mut stream = response.bytes_stream();

    let mut seq_file = if mmap.is_none() {
        Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(tmp_path)?,
        )
    } else {
        None
    };

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(total_downloaded);
        }

        let allowed_end = end_cap.load(Ordering::Acquire);
        if !unknown_length && offset > allowed_end {
            break;
        }

        let next_result = tokio::time::timeout(STALL_TIMEOUT, stream.next()).await;
        let chunk_option = match next_result {
            Ok(opt) => opt,
            Err(_) => return Err(anyhow::anyhow!("Stream stalled for 60s")),
        };

        let chunk_result = match chunk_option {
            Some(res) => res,
            None => break,
        };

        let chunk = chunk_result?;
        let mut data = chunk.as_ref();

        if !unknown_length {
            let max_off = allowed_end.saturating_add(1);
            if offset >= max_off {
                break;
            }
            let remaining = (max_off - offset) as usize;
            if data.len() > remaining {
                data = &data[..remaining];
            }
        }

        if data.is_empty() {
            break;
        }

        bandwidth.throttle(data.len()).await;

        let len = data.len() as u64;

        if let Some(map) = mmap {
            map.write_at(offset, data)?;
        } else if let Some(file) = &mut seq_file {
            use std::io::Write;
            file.write_all(data)?;
        }

        offset += len;
        total_downloaded += len;

        // Atomically publish bytes for the UI tick. Do not flood the event bus —
        // 16 connections × 150ms was ~100 WS messages/s and froze the app.
        if let Some(state) = steal {
            if let Some(seg) = state.lock().segments.iter().find(|s| s.id == seg_id) {
                seg.downloaded.store(total_downloaded, Ordering::Relaxed);
            }
        }
    }

    // Final snap so the UI can show the completed segment before the next job tick.
    tx.send(DmEvent::SegmentProgress {
        job_id,
        segment_id: seg_id,
        downloaded: total_downloaded,
        speed_bps: 0.0,
    });

    let final_end = end_cap.load(Ordering::Acquire);
    let expected = if unknown_length {
        total_downloaded
    } else {
        final_end.saturating_sub(start).saturating_add(1)
    };

    if !stop.load(Ordering::Relaxed) && total_downloaded < expected {
        return Err(anyhow::anyhow!(
            "Segment size mismatch: expected {}, got {}",
            expected,
            total_downloaded
        ));
    }

    // Durable before meta: never record progress that is still in the page cache.
    let written_span = total_downloaded.saturating_sub(already);
    if let Some(map) = mmap {
        map.sync_after_write(resume_at, written_span)?;
    } else if let Some(file) = &mut seq_file {
        file.sync_data()?;
    }

    if let Some(state) = steal {
        let segments = state.lock().snapshot();
        let meta = PartMetadata::from_job(meta_job, segments);
        write_part_meta_atomic(&meta)?;
    }

    debug!(
        "Segment {} complete: {} bytes written (synced + .part.meta updated)",
        seg_id, total_downloaded
    );
    Ok(total_downloaded)
}
