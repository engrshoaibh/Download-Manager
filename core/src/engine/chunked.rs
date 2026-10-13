use anyhow::{bail, Context, Result};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::engine::native_payload::{ExtensionStream, SessionHeaders};

const MAX_CHUNK_RETRIES: u32 = 4;
/// Persist chunk progress at most this often (and always at the end of a chunk).
const CHECKPOINT_EVERY: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkProgress {
    pub chunk_index: usize,
    pub start_byte: u64,
    /// Next byte to fetch; `end_byte + 1` once the chunk is complete.
    pub current_byte: u64,
    pub end_byte: u64,
    pub is_finished: bool,
}

impl ChunkProgress {
    fn fresh(index: usize, start: u64, end: u64) -> Self {
        Self { chunk_index: index, start_byte: start, current_byte: start, end_byte: end, is_finished: false }
    }

    /// Saved state is only trusted if it describes exactly this byte range.
    fn matches_range(&self, start: u64, end: u64) -> bool {
        self.start_byte == start
            && self.end_byte == end
            && self.current_byte >= start
            && self.current_byte <= end + 1
    }
}

#[derive(Debug)]
pub struct ChunkedEngine {
    db: sled::Db,
    client: reqwest::Client,
}

/// Split `[0, len)` into `count` inclusive ranges.
fn chunk_ranges(len: u64, count: usize) -> Vec<(u64, u64)> {
    let count = count.max(1).min(len.max(1) as usize);
    let size = len.div_ceil(count as u64);
    (0..count)
        .map(|i| {
            let start = i as u64 * size;
            let end = if i == count - 1 { len - 1 } else { start + size - 1 };
            (start, end)
        })
        .filter(|(s, e)| s <= e)
        .collect()
}

impl ChunkedEngine {
    pub fn new(db_path: &Path) -> Result<Self> {
        let db = sled::open(db_path).context("Failed to open sled database")?;
        let client = reqwest::Client::builder().build()?;
        Ok(Self { db, client })
    }

    /// Download one stream into `output_tmp` using parallel ranged chunks.
    ///
    /// Progress is persisted per chunk so a restart resumes where it stopped — but only if
    /// the saved state still matches the payload file. A missing or resized temp file, or a
    /// chunk map from a different split, discards the saved state and starts that stream over,
    /// because resuming against stale state would leave zero-filled gaps.
    ///
    /// `progress` receives the running total of verified bytes for this stream.
    pub async fn download_stream(
        &self,
        task_id: &str,
        stream_type: &str,
        stream: &ExtensionStream,
        headers: &SessionHeaders,
        output_tmp: &Path,
        chunk_count: usize,
        cancel: CancellationToken,
        progress: Arc<AtomicU64>,
    ) -> Result<()> {
        if stream.content_length == 0 {
            bail!("{stream_type} stream has unknown length");
        }
        let mut req_headers = HeaderMap::new();
        req_headers.insert("User-Agent", headers.user_agent.parse()?);
        req_headers.insert("Referer", headers.referer.parse()?);
        req_headers.insert("Origin", headers.origin.parse()?);
        req_headers.insert("Cookie", headers.cookie.parse()?);

        let ranges = chunk_ranges(stream.content_length, chunk_count);
        let db_tree = self.db.open_tree(format!("{}_{}", task_id, stream_type))?;

        // Load saved chunk state and decide whether it can be trusted.
        let file_ok = std::fs::metadata(output_tmp)
            .map(|m| m.len() == stream.content_length)
            .unwrap_or(false);
        let mut states: Vec<ChunkProgress> = Vec::with_capacity(ranges.len());
        let mut trusted = file_ok;
        for (i, (start, end)) in ranges.iter().enumerate() {
            let saved = db_tree
                .get(format!("chunk_{i}"))?
                .and_then(|b| serde_json::from_slice::<ChunkProgress>(&b).ok())
                .filter(|s| s.matches_range(*start, *end));
            match saved {
                Some(s) => states.push(s),
                None => {
                    trusted = false;
                    states.push(ChunkProgress::fresh(i, *start, *end));
                }
            }
        }
        // Chunks recorded but the payload is gone/mismatched -> start over.
        let any_progress = states.iter().any(|s| s.current_byte > s.start_byte);
        if !trusted {
            if any_progress {
                warn!("{task_id}/{stream_type}: saved chunk state is stale (payload missing or changed); restarting stream");
            }
            states = ranges.iter().enumerate().map(|(i, (s, e))| ChunkProgress::fresh(i, *s, *e)).collect();
            db_tree.clear()?;
            let file = OpenOptions::new().create(true).write(true).truncate(true).open(output_tmp).await?;
            file.set_len(stream.content_length).await?;
            file.sync_all().await?;
        }

        progress.store(
            states.iter().map(|s| s.current_byte - s.start_byte).sum(),
            Ordering::Relaxed,
        );

        let mut tasks = Vec::new();
        for state in states.into_iter().filter(|s| !s.is_finished && s.current_byte <= s.end_byte) {
            let client = self.client.clone();
            let req_headers = req_headers.clone();
            let url = stream.url.clone();
            let tree = db_tree.clone();
            let output_path = output_tmp.to_path_buf();
            let cancel = cancel.clone();
            let progress = Arc::clone(&progress);
            tasks.push(tokio::spawn(async move {
                download_chunk(client, url, req_headers, output_path, tree, state, cancel, progress).await
            }));
        }
        for task in tasks {
            task.await.context("chunk task panicked")??;
        }
        if cancel.is_cancelled() {
            bail!("{stream_type} download cancelled");
        }

        // Every chunk must be recorded as finished before we trust the file.
        for (i, (start, end)) in ranges.iter().enumerate() {
            let ok = db_tree
                .get(format!("chunk_{i}"))?
                .and_then(|b| serde_json::from_slice::<ChunkProgress>(&b).ok())
                .map(|s| s.is_finished && s.start_byte == *start && s.end_byte == *end && s.current_byte == end + 1)
                .unwrap_or(false);
            if !ok {
                bail!("{stream_type} chunk {i} ({start}-{end}) did not complete");
            }
        }
        let len = std::fs::metadata(output_tmp)?.len();
        if len != stream.content_length {
            bail!("{stream_type} file is {len} bytes, expected {}", stream.content_length);
        }
        db_tree.clear()?;
        self.db.flush_async().await?;
        Ok(())
    }
}

/// Fetch one chunk, retrying from the last persisted offset on network errors or early EOF.
async fn download_chunk(
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    path: std::path::PathBuf,
    tree: sled::Tree,
    mut state: ChunkProgress,
    cancel: CancellationToken,
    progress: Arc<AtomicU64>,
) -> Result<()> {
    use futures_util::StreamExt;
    let key = format!("chunk_{}", state.chunk_index);
    let save = |s: &ChunkProgress| -> Result<()> {
        tree.insert(key.as_bytes(), serde_json::to_vec(s)?)?;
        Ok(())
    };

    let mut attempt = 0u32;
    while state.current_byte <= state.end_byte {
        if cancel.is_cancelled() {
            save(&state)?;
            info!("Chunk {} paused/cancelled", state.chunk_index);
            return Ok(());
        }
        let result: Result<()> = async {
            let resp = client
                .get(&url)
                .headers(headers.clone())
                .header("Range", format!("bytes={}-{}", state.current_byte, state.end_byte))
                .send()
                .await?;
            if resp.status().as_u16() == 403 {
                bail!("HTTP 403: EXPIRED_URL");
            }
            if resp.status().as_u16() != 206 {
                // A 200 would mean the server ignored Range and is sending the whole file.
                bail!("server returned HTTP {} for a ranged request", resp.status());
            }
            let mut file = OpenOptions::new().write(true).open(&path).await?;
            file.seek(std::io::SeekFrom::Start(state.current_byte)).await?;
            let mut stream = resp.bytes_stream();
            let mut last_save = Instant::now();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        file.flush().await?;
                        return Ok(());
                    }
                    next = stream.next() => match next {
                        Some(Ok(bytes)) => {
                            // Never write past this chunk's range, whatever the server sends.
                            let room = (state.end_byte + 1 - state.current_byte) as usize;
                            let take = bytes.len().min(room);
                            file.write_all(&bytes[..take]).await?;
                            state.current_byte += take as u64;
                            progress.fetch_add(take as u64, Ordering::Relaxed);
                            if state.current_byte > state.end_byte {
                                break;
                            }
                            if last_save.elapsed() >= CHECKPOINT_EVERY {
                                file.flush().await?;
                                save(&state)?;
                                last_save = Instant::now();
                            }
                        }
                        Some(Err(e)) => return Err(e.into()),
                        None => break,
                    }
                }
            }
            file.flush().await?;
            file.sync_data().await?;
            Ok(())
        }
        .await;

        // Persist whatever was safely written, whether we succeeded or not.
        if state.current_byte > state.end_byte {
            state.is_finished = true;
        }
        save(&state)?;

        match result {
            Ok(()) if state.is_finished || cancel.is_cancelled() => break,
            Ok(()) => {
                // Stream ended before the chunk did: retry from the saved offset.
                attempt += 1;
                warn!("Chunk {} ended early at {}/{}", state.chunk_index, state.current_byte, state.end_byte);
            }
            Err(e) if e.to_string().contains("EXPIRED_URL") => return Err(e),
            Err(e) => {
                attempt += 1;
                warn!("Chunk {} error (attempt {attempt}): {e:#}", state.chunk_index);
                if attempt > MAX_CHUNK_RETRIES {
                    return Err(e.context(format!("chunk {} failed", state.chunk_index)));
                }
            }
        }
        if attempt > MAX_CHUNK_RETRIES {
            bail!("chunk {} kept ending early", state.chunk_index);
        }
        tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_tile_the_file() {
        for (len, n) in [(100u64, 8usize), (7, 8), (1, 4), (1_000_003, 8)] {
            let r = chunk_ranges(len, n);
            assert_eq!(r.first().unwrap().0, 0);
            assert_eq!(r.last().unwrap().1, len - 1);
            for w in r.windows(2) {
                assert_eq!(w[0].1 + 1, w[1].0);
            }
        }
    }

    #[test]
    fn stale_state_is_rejected() {
        let s = ChunkProgress { chunk_index: 0, start_byte: 0, current_byte: 50, end_byte: 99, is_finished: false };
        assert!(s.matches_range(0, 99));
        assert!(!s.matches_range(0, 199));
        assert!(!s.matches_range(10, 99));
    }
}
