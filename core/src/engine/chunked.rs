use anyhow::{Context, Result};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, AsyncSeekExt};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::engine::native_payload::{ExtensionStream, SessionHeaders};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkProgress {
    pub chunk_index: usize,
    pub start_byte: u64,
    pub current_byte: u64,
    pub end_byte: u64,
    pub is_finished: bool,
}

#[derive(Debug)]
pub struct ChunkedEngine {
    db: sled::Db,
    client: reqwest::Client,
}

impl ChunkedEngine {
    pub fn new(db_path: &Path) -> Result<Self> {
        let db = sled::open(db_path).context("Failed to open sled database")?;
        let client = reqwest::Client::builder().build()?;
        Ok(Self { db, client })
    }

    pub async fn download_stream(
        &self,
        task_id: &str,
        stream_type: &str,
        stream: &ExtensionStream,
        headers: &SessionHeaders,
        output_tmp: &Path,
        chunk_count: usize,
        cancel: CancellationToken,
    ) -> Result<()> {
        let mut req_headers = HeaderMap::new();
        req_headers.insert("User-Agent", headers.user_agent.parse()?);
        req_headers.insert("Referer", headers.referer.parse()?);
        req_headers.insert("Origin", headers.origin.parse()?);
        req_headers.insert("Cookie", headers.cookie.parse()?);

        // Pre-allocation
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(output_tmp)
            .await?;
        file.set_len(stream.content_length).await?;

        let chunk_size = (stream.content_length as f64 / chunk_count as f64).ceil() as u64;
        let mut tasks = Vec::new();

        let db_tree = self.db.open_tree(format!("{}_{}", task_id, stream_type))?;

        for i in 0..chunk_count {
            let start = i as u64 * chunk_size;
            let end = if i == chunk_count - 1 {
                stream.content_length - 1
            } else {
                start + chunk_size - 1
            };

            let state_key = format!("chunk_{}", i);
            let state = if let Some(bytes) = db_tree.get(&state_key)? {
                serde_json::from_slice::<ChunkProgress>(&bytes)?
            } else {
                ChunkProgress {
                    chunk_index: i,
                    start_byte: start,
                    current_byte: start,
                    end_byte: end,
                    is_finished: false,
                }
            };

            if state.is_finished {
                continue;
            }

            let client = self.client.clone();
            let req_headers = req_headers.clone();
            let url = stream.url.clone();
            let tree = db_tree.clone();
            let state_key = state_key.clone();
            let output_path = output_tmp.to_path_buf();
            let cancel = cancel.clone();

            let handle = tokio::spawn(async move {
                let range = format!("bytes={}-{}", state.current_byte, state.end_byte);
                let mut req = client.get(&url).headers(req_headers);
                req = req.header("Range", range);

                let resp = req.send().await?;
                if resp.status().as_u16() == 403 {
                    return Err(anyhow::anyhow!("HTTP 403: EXPIRED_URL"));
                }
                resp.error_for_status_ref()?;

                use futures_util::StreamExt;
                let mut byte_stream = resp.bytes_stream();
                let mut current_offset = state.current_byte;

                let mut file = OpenOptions::new().write(true).open(&output_path).await?;
                file.seek(std::io::SeekFrom::Start(current_offset)).await?;

                let mut current_state = state;

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            info!("Chunk {} paused/cancelled", i);
                            break;
                        }
                        chunk_res = byte_stream.next() => {
                            match chunk_res {
                                Some(Ok(chunk)) => {
                                    file.write_all(&chunk).await?;
                                    current_offset += chunk.len() as u64;
                                    
                                    // Update state
                                    current_state.current_byte = current_offset;
                                    if current_offset > current_state.end_byte {
                                        current_state.is_finished = true;
                                    }
                                    
                                    let state_bytes = serde_json::to_vec(&current_state)?;
                                    tree.insert(&state_key, state_bytes)?;
                                }
                                Some(Err(e)) => return Err(e.into()),
                                None => {
                                    current_state.is_finished = true;
                                    let state_bytes = serde_json::to_vec(&current_state)?;
                                    tree.insert(&state_key, state_bytes)?;
                                    break;
                                }
                            }
                        }
                    }
                }
                
                Ok::<_, anyhow::Error>(())
            });

            tasks.push(handle);
        }

        for task in tasks {
            task.await??;
        }

        Ok(())
    }
}
