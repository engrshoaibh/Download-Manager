use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::sync::broadcast;
use tracing::debug;

use crate::engine::{DmEvent, JobId};
use crate::io::sparse::MmapFile;
use crate::network::client::HttpClient;

/// Downloads a single byte-range segment directly into the memory-mapped file
pub async fn download_segment(
    job_id: JobId,
    seg_id: usize,
    start: u64,
    end: u64,
    url: String,
    client: Arc<HttpClient>,
    mmap: Option<Arc<Mutex<MmapFile>>>,
    tmp_path: String,
    tx: broadcast::Sender<DmEvent>,
    user_agent: Option<String>,
    cookies: Option<String>,
) -> Result<()> {
    let range_header = if start == 0 && end == 0 {
        // Unknown length, no range header
        String::new()
    } else {
        format!("bytes={}-{}", start, end)
    };

    debug!("Segment {} starting: Range: {}", seg_id, if range_header.is_empty() { "none" } else { &range_header });

    // If we have no range header (sequential mode), just do a normal GET
    let response = if range_header.is_empty() {
        let mut req = client.inner.get(&url);
        if let Some(ua) = &user_agent { req = req.header("User-Agent", ua); }
        if let Some(c) = &cookies { req = req.header("Cookie", c); }
        req.send().await.map_err(|e| anyhow::anyhow!("GET request failed: {}", e))?
    } else {
        client
            .get_range(&url, &range_header, user_agent.as_deref(), cookies.as_deref())
            .await?
    };

    let mut offset = start;
    let mut total_downloaded: u64 = 0;
    let mut last_report = Instant::now();
    let mut bytes_since_last_report: u64 = 0;

    let mut stream = response.bytes_stream();

    // Prepare sequential file if in fallback mode
    let mut seq_file = if mmap.is_none() {
        Some(std::fs::OpenOptions::new().create(true).append(true).open(&tmp_path)?)
    } else {
        None
    };

    loop {
        let next_result = tokio::time::timeout(std::time::Duration::from_secs(15), stream.next()).await;
        let chunk_option = match next_result {
            Ok(opt) => opt,
            Err(_) => return Err(anyhow::anyhow!("Stream read timed out after 15s")),
        };

        let chunk_result = match chunk_option {
            Some(res) => res,
            None => break,
        };

        let chunk = chunk_result?;
        let len = chunk.len() as u64;

        if let Some(mmap_arc) = &mmap {
            // Zero-copy write directly into mmap at the correct file offset
            let mut mmap_guard = mmap_arc.lock().unwrap();
            mmap_guard.write_at(offset, &chunk)?;
        } else if let Some(file) = &mut seq_file {
            // Sequential write
            use std::io::Write;
            file.write_all(&chunk)?;
        }

        offset += len;
        total_downloaded += len;
        bytes_since_last_report += len;

        // Report progress approximately every 100ms
        let elapsed = last_report.elapsed();
        if elapsed.as_millis() >= 100 {
            let speed_bps = bytes_since_last_report as f64 / elapsed.as_secs_f64();
            bytes_since_last_report = 0;
            last_report = Instant::now();

            let _ = tx.send(DmEvent::SegmentProgress {
                job_id,
                segment_id: seg_id,
                downloaded: total_downloaded,
                speed_bps,
            });
        }
    }

    let expected = if start == 0 && end == 0 { total_downloaded } else { end - start + 1 };
    if total_downloaded != expected {
        return Err(anyhow::anyhow!(
            "Segment size mismatch: expected {}, got {}",
            expected,
            total_downloaded
        ));
    }

    debug!("Segment {} complete: {} bytes written", seg_id, total_downloaded);
    Ok(())
}
