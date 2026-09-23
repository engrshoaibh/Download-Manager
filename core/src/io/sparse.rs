//! Partial-download files and crash-safe resume metadata.
//!
//! # On-disk layout
//!
//! For a final path `D:\Downloads\video.mp4`:
//! - `video.mp4.part` — pre-allocated sparse payload (legacy: `video.mp4.tmp`)
//! - `video.mp4.part.meta` — JSON segment map written **atomically** (temp + `rename`)
//!
//! # Crash safety
//!
//! 1. Segment workers call [`SharedMmap::sync_after_write`] (`mmap` flush +
//!    [`File::sync_data`]) **before** updating `.part.meta`.
//! 2. Metadata is written to a sibling temp file in the same directory, then
//!    `rename()`d over the live meta path so a crash mid-write cannot leave a
//!    truncated JSON resume map.
//! 3. Resume never trusts progress that was not synced; the meta map is the
//!    source of truth for which byte ranges are durable.
//!
//! # Windows mmap / file locking
//!
//! See [`WINDOWS_MMAP.md`](WINDOWS_MMAP.md) in this directory for how Explorer
//! "open file location", previews, and antivirus interact with the mapped
//! `.part` handle held by the daemon.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use memmap2::MmapMut;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::engine::{DownloadJob, DownloadStatus, JobId, Segment};

/// Schema version for `.part.meta` JSON.
pub const PART_META_VERSION: u32 = 1;

/// Crash-safe resume sidecar written next to the `.part` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartMetadata {
    pub version: u32,
    pub job_id: JobId,
    pub url: String,
    pub save_path: String,
    pub filename: String,
    pub total_bytes: u64,
    pub supports_ranges: bool,
    pub segments: Vec<Segment>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
}

impl PartMetadata {
    pub fn from_job(job: &DownloadJob, segments: Vec<Segment>) -> Self {
        Self {
            version: PART_META_VERSION,
            job_id: job.id,
            url: job.url.clone(),
            save_path: job.save_path.clone(),
            filename: job.filename.clone(),
            total_bytes: job.total_bytes,
            supports_ranges: job.supports_ranges,
            segments,
            updated_at: Utc::now(),
            user_agent: job.user_agent.clone(),
            cookies: job.cookies.clone(),
            referer: job.referer.clone(),
        }
    }

    pub fn into_paused_job(self) -> DownloadJob {
        let downloaded_bytes = self.segments.iter().map(|s| s.downloaded).sum();
        let segment_count = self.segments.len().max(1);
        DownloadJob {
            id: self.job_id,
            url: self.url,
            filename: self.filename,
            save_path: self.save_path,
            total_bytes: self.total_bytes,
            downloaded_bytes,
            segments: self.segments,
            status: DownloadStatus::Paused,
            phase: None,
            speed_bps: 0.0,
            eta_seconds: None,
            created_at: self.updated_at,
            completed_at: None,
            supports_ranges: self.supports_ranges,
            segment_count,
            use_http3: false,
            user_agent: self.user_agent,
            cookies: self.cookies,
            referer: self.referer,
            quality: None,
            format: None,
            engine: "http".into(),
        }
    }
}

/// Payload path: `{save_path}.part`
pub fn part_data_path(save_path: &str) -> PathBuf {
    PathBuf::from(format!("{save_path}.part"))
}

/// Metadata path: `{save_path}.part.meta`
pub fn part_meta_path(save_path: &str) -> PathBuf {
    PathBuf::from(format!("{save_path}.part.meta"))
}

/// Legacy payload path used before `.part` migration.
pub fn legacy_tmp_path(save_path: &str) -> PathBuf {
    PathBuf::from(format!("{save_path}.tmp"))
}

/// Prefer `.part`, fall back to legacy `.tmp`.
pub fn resolve_partial_path(save_path: &str) -> Option<PathBuf> {
    let part = part_data_path(save_path);
    if part.exists() {
        return Some(part);
    }
    let tmp = legacy_tmp_path(save_path);
    if tmp.exists() {
        return Some(tmp);
    }
    None
}

/// Write metadata via temp file + `rename` in the same directory (crash-safe).
pub fn write_part_meta_atomic(meta: &PartMetadata) -> Result<()> {
    let final_path = part_meta_path(&meta.save_path);
    let parent = final_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent)
        .with_context(|| format!("Failed to create directory {}", parent.display()))?;

    // Same-directory temp so rename is atomic on the same volume.
    let tmp_name = format!(
        ".{}.tmp-{}",
        final_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("part.meta"),
        std::process::id()
    );
    let tmp_path = parent.join(tmp_name);

    let json = serde_json::to_vec_pretty(meta).context("Failed to serialize .part.meta")?;
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| format!("Failed to create {}", tmp_path.display()))?;
        file.write_all(&json)
            .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
        // Durable meta contents before the rename becomes visible.
        file.sync_data()
            .with_context(|| format!("Failed to sync {}", tmp_path.display()))?;
    }

    fs::rename(&tmp_path, &final_path).with_context(|| {
        format!(
            "Failed to atomically replace {} with {}",
            final_path.display(),
            tmp_path.display()
        )
    })?;

    // Best-effort: flush directory entry on Unix; on Windows rename is enough.
    #[cfg(unix)]
    if let Ok(dir) = File::open(&parent) {
        let _ = dir.sync_data();
    }

    Ok(())
}

pub fn load_part_meta(save_path: &str) -> Result<Option<PartMetadata>> {
    let path = part_meta_path(save_path);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let meta: PartMetadata = serde_json::from_str(&raw)
        .with_context(|| format!("Corrupt .part.meta at {}", path.display()))?;
    Ok(Some(meta))
}

pub fn remove_part_artifacts(save_path: &str) {
    for p in [
        part_data_path(save_path),
        part_meta_path(save_path),
        legacy_tmp_path(save_path),
    ] {
        if p.exists() {
            if let Err(e) = fs::remove_file(&p) {
                warn!("Could not remove {}: {e}", p.display());
            }
        }
    }
}

/// A pre-allocated sparse file on disk.
/// On Windows, uses SetEndOfFile to reserve the space without writing zeros.
/// On Linux/macOS, uses ftruncate which creates a sparse file natively.
pub struct SparseFile {
    file: File,
}

/// Fail early when the target volume cannot hold `needed` bytes.
pub fn ensure_disk_space(path: &str, needed: u64) -> Result<()> {
    if needed == 0 {
        return Ok(());
    }
    let probe = Path::new(path);
    let dir = if probe.exists() {
        probe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| Path::new(".").to_path_buf())
    } else {
        probe
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| {
                if p.exists() {
                    p.to_path_buf()
                } else {
                    let mut cur = p.to_path_buf();
                    while !cur.exists() {
                        if !cur.pop() {
                            return PathBuf::from(".");
                        }
                    }
                    cur
                }
            })
            .unwrap_or_else(|| Path::new(".").to_path_buf())
    };

    let available = fs4::available_space(&dir).with_context(|| {
        format!(
            "Failed to query free disk space for {}",
            dir.display()
        )
    })?;

    let required = needed.saturating_add(1024 * 1024);
    if available < required {
        bail!(
            "Insufficient disk space: need {} ({} on disk), only {} available on {}",
            format_bytes(required),
            format_bytes(needed),
            format_bytes(available),
            dir.display()
        );
    }
    Ok(())
}

fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let v = n as f64;
    if v >= GB {
        format!("{:.2} GB", v / GB)
    } else if v >= MB {
        format!("{:.2} MB", v / MB)
    } else if v >= KB {
        format!("{:.2} KB", v / KB)
    } else {
        format!("{n} B")
    }
}

impl SparseFile {
    /// Create (or truncate) a file at `path` with exactly `size` bytes reserved.
    pub fn create(path: &str, size: u64) -> Result<Self> {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        ensure_disk_space(path, size)?;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("Failed to open file: {}", path))?;

        file.set_len(size)?;
        file.sync_data()
            .context("Failed to sync sparse file length to disk")?;

        Ok(Self { file })
    }

    /// Open an existing temp/part file so a paused download can resume.
    pub fn open(path: &str, size: u64) -> Result<Self> {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        let existing = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size > existing {
            ensure_disk_space(path, size - existing)?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .with_context(|| format!("Failed to open file: {}", path))?;

        if file.metadata()?.len() != size {
            file.set_len(size)?;
            file.sync_data()
                .context("Failed to sync resized sparse file")?;
        }

        Ok(Self { file })
    }

    pub fn into_mmap(self) -> Result<SharedMmap> {
        let mmap = unsafe { MmapMut::map_mut(&self.file) }
            .context("Failed to create memory map")?;

        Ok(SharedMmap(Arc::new(SharedMmapInner {
            file: self.file,
            mmap,
        })))
    }
}

struct SharedMmapInner {
    /// Kept open for [`File::sync_data`] after segment completion.
    file: File,
    mmap: MmapMut,
}

/// Memory map that allows concurrent writes to disjoint byte ranges.
///
/// Segment workers never overlap, so a global mutex is unnecessary and was
/// serializing every chunk write across 16–32 connections.
///
/// **Windows:** the underlying `File` stays open for the life of the download.
/// Opening the same `.part` from Explorer preview / another process can fail
/// with sharing violations — see `WINDOWS_MMAP.md`.
#[derive(Clone)]
pub struct SharedMmap(Arc<SharedMmapInner>);

// SAFETY: callers write only to disjoint ranges assigned to each segment.
unsafe impl Send for SharedMmapInner {}
unsafe impl Sync for SharedMmapInner {}

impl SharedMmap {
    pub fn write_at(&self, offset: u64, data: &[u8]) -> Result<()> {
        let offset = offset as usize;
        let end = offset + data.len();
        let len = self.0.mmap.len();

        if end > len {
            anyhow::bail!(
                "Write out of bounds: offset={} len={} mmap_size={}",
                offset,
                data.len(),
                len
            );
        }

        unsafe {
            let dest = self.0.mmap.as_ptr() as *mut u8;
            std::ptr::copy_nonoverlapping(data.as_ptr(), dest.add(offset), data.len());
        }
        Ok(())
    }

    pub fn flush(&self) -> Result<()> {
        self.0.mmap.flush().context("Failed to flush mmap to disk")
    }

    /// Flush the mapped pages for a completed segment, then `File::sync_data`
    /// so resume metadata never records ranges that are still in the page cache.
    pub fn sync_after_write(&self, offset: u64, len: u64) -> Result<()> {
        if len > 0 {
            let off = offset as usize;
            let n = len as usize;
            let map_len = self.0.mmap.len();
            if off < map_len {
                let n = n.min(map_len - off);
                self.0
                    .mmap
                    .flush_range(off, n)
                    .context("Failed to flush mmap range")?;
            }
        } else {
            self.0.mmap.flush().context("Failed to flush mmap")?;
        }
        self.0
            .file
            .sync_data()
            .context("Failed to sync_data partial download file")?;
        Ok(())
    }
}

/// Result of the daemon startup orphan sweep.
#[derive(Debug, Default)]
pub struct OrphanSweepReport {
    pub resumed: usize,
    pub cleaned: usize,
    pub skipped: usize,
}

/// Scan download directories for `.part` / `.part.meta` with no `jobs.json` entry.
///
/// - Younger than `retention`: import as paused jobs when meta is present ("offer resume").
/// - Older than `retention`: delete orphaned `.part` + `.part.meta` (+ legacy `.tmp`).
pub fn sweep_orphaned_parts(
    jobs: &mut Vec<DownloadJob>,
    retention: std::time::Duration,
) -> OrphanSweepReport {
    let mut report = OrphanSweepReport::default();
    let mut known_ids: std::collections::HashSet<Uuid> = jobs.iter().map(|j| j.id).collect();
    let known_parts: std::collections::HashSet<PathBuf> = jobs
        .iter()
        .filter_map(|j| resolve_partial_path(&j.save_path))
        .map(|p| canonicalize_lossy(&p))
        .collect();
    let known_saves: std::collections::HashSet<String> =
        jobs.iter().map(|j| j.save_path.clone()).collect();

    let mut dirs: std::collections::HashSet<PathBuf> = jobs
        .iter()
        .filter_map(|j| Path::new(&j.save_path).parent().map(|p| p.to_path_buf()))
        .collect();

    if let Some(dl) = dirs::download_dir() {
        dirs.insert(dl);
    }

    let now = SystemTime::now();

    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("Orphan sweep: cannot read {}: {e}", dir.display());
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            let is_part = name.ends_with(".part") && !name.ends_with(".part.meta");
            let is_tmp = name.ends_with(".tmp") && !name.contains(".json");
            if !is_part && !is_tmp {
                continue;
            }

            let canon = canonicalize_lossy(&path);
            if known_parts.contains(&canon) {
                continue;
            }

            let save_path = if is_part {
                path.to_string_lossy()
                    .strip_suffix(".part")
                    .unwrap_or(&path.to_string_lossy())
                    .to_string()
            } else {
                path.to_string_lossy()
                    .strip_suffix(".tmp")
                    .unwrap_or(&path.to_string_lossy())
                    .to_string()
            };

            if known_saves.contains(&save_path) {
                report.skipped += 1;
                continue;
            }

            let age = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| now.duration_since(m).ok())
                .unwrap_or(retention);

            let meta = load_part_meta(&save_path).ok().flatten();

            if age <= retention {
                if let Some(meta) = meta {
                    if known_ids.contains(&meta.job_id) {
                        report.skipped += 1;
                        continue;
                    }
                    info!(
                        "Orphan .part offered for resume: {} ({} bytes)",
                        meta.filename, meta.total_bytes
                    );
                    let job_id = meta.job_id;
                    let job = meta.into_paused_job();
                    known_ids.insert(job_id);
                    jobs.push(job);
                    report.resumed += 1;
                } else {
                    info!(
                        "Orphan partial without meta (keeping until retention): {}",
                        path.display()
                    );
                    report.skipped += 1;
                }
            } else {
                info!(
                    "Cleaning orphaned partial older than retention: {}",
                    path.display()
                );
                remove_part_artifacts(&save_path);
                report.cleaned += 1;
            }
        }
    }

    report
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_meta_roundtrip() {
        let dir = std::env::temp_dir().join(format!("dm-part-meta-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let save = dir.join("file.bin").to_string_lossy().to_string();

        let meta = PartMetadata {
            version: PART_META_VERSION,
            job_id: Uuid::new_v4(),
            url: "https://example.com/f".into(),
            save_path: save.clone(),
            filename: "file.bin".into(),
            total_bytes: 100,
            supports_ranges: true,
            segments: vec![Segment::new(0, 0, 99)],
            updated_at: Utc::now(),
            user_agent: None,
            cookies: None,
            referer: None,
        };
        write_part_meta_atomic(&meta).unwrap();
        let loaded = load_part_meta(&save).unwrap().unwrap();
        assert_eq!(loaded.total_bytes, 100);
        assert_eq!(loaded.job_id, meta.job_id);
        remove_part_artifacts(&save);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn part_paths() {
        assert_eq!(
            part_data_path("C:/d/a.mp4"),
            PathBuf::from("C:/d/a.mp4.part")
        );
        assert_eq!(
            part_meta_path("C:/d/a.mp4"),
            PathBuf::from("C:/d/a.mp4.part.meta")
        );
    }
}
