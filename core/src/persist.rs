use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use crate::engine::{DownloadJob, DownloadStatus};
use crate::io::sparse::{self, sweep_orphaned_parts};
use crate::settings::DaemonSettings;

const APP_DIR: &str = "download-manager";
const STATE_FILE: &str = "jobs.json";

/// Current on-disk schema for `jobs.json`.
///
/// Bump this when the envelope or job fields change incompatibly, and add a
/// step in [`migrate_jobs_file`].
pub const SCHEMA_VERSION: u32 = 1;

/// Persisted document wrapping the job list (replaces the legacy bare array).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobsFile {
    pub schema_version: u32,
    pub jobs: Vec<DownloadJob>,
}

/// `%LOCALAPPDATA%/download-manager/jobs.json` (or platform equivalent).
pub fn state_path() -> PathBuf {
    // DM_DATA_DIR lets tests and portable installs keep state out of the user profile.
    std::env::var_os("DM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(APP_DIR)
        })
        .join(STATE_FILE)
}

pub fn load_jobs() -> Result<Vec<DownloadJob>> {
    let path = state_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read job state: {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse job state JSON: {}", path.display()))?;

    let mut file = parse_and_migrate(value)
        .with_context(|| format!("Failed to migrate job state: {}", path.display()))?;

    // Interrupted mid-flight → paused so the user can Resume after reboot.
    for job in &mut file.jobs {
        if matches!(
            job.status,
            DownloadStatus::Queued | DownloadStatus::Probing | DownloadStatus::Downloading
        ) {
            job.status = DownloadStatus::Paused;
            job.speed_bps = 0.0;
            job.eta_seconds = None;
        }
        job.speed_bps = 0.0;

        // Prefer durable `.part.meta` segment map over a possibly stale jobs.json snapshot.
        if job.status != DownloadStatus::Completed {
            if let Ok(Some(meta)) = sparse::load_part_meta(&job.save_path) {
                if !meta.segments.is_empty() {
                    job.segments = meta.segments;
                    job.downloaded_bytes = job.segments.iter().map(|s| s.downloaded).sum();
                    if meta.total_bytes > 0 {
                        job.total_bytes = meta.total_bytes;
                    }
                }
            }
        }
    }
    file.jobs.retain(|j| j.status != DownloadStatus::Cancelled);

    // Drop unfinished jobs whose partial file vanished (user deleted it).
    file.jobs.retain(|j| {
        if j.status == DownloadStatus::Completed {
            return true;
        }
        if j.total_bytes == 0 || j.segments.is_empty() {
            return true;
        }
        if sparse::resolve_partial_path(&j.save_path).is_some() || Path::new(&j.save_path).exists()
        {
            true
        } else if j.downloaded_bytes > 0 {
            warn!(
                "Dropping job {} — partial file missing for {}",
                j.filename, j.save_path
            );
            false
        } else {
            true
        }
    });

    if file.schema_version != SCHEMA_VERSION {
        // Should not happen after migrate, but keep invariant clear.
        file.schema_version = SCHEMA_VERSION;
    }

    info!(
        "Restored {} download(s) from {} (schema v{})",
        file.jobs.len(),
        path.display(),
        file.schema_version
    );
    Ok(file.jobs)
}

/// Accept legacy bare arrays (schema 0) or versioned envelopes; migrate forward.
fn parse_and_migrate(value: Value) -> Result<JobsFile> {
    let mut file = match value {
        Value::Array(arr) => {
            info!("Migrating jobs.json from legacy array (schema v0 → v{SCHEMA_VERSION})");
            let jobs: Vec<DownloadJob> = serde_json::from_value(Value::Array(arr))
                .context("Legacy jobs array is invalid")?;
            JobsFile {
                schema_version: 0,
                jobs,
            }
        }
        Value::Object(map) => {
            let version = map
                .get("schema_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;
            // Allow missing `jobs` key → empty list.
            let jobs_val = map.get("jobs").cloned().unwrap_or(Value::Array(vec![]));
            let jobs: Vec<DownloadJob> =
                serde_json::from_value(jobs_val).context("Invalid jobs list in jobs.json")?;
            JobsFile {
                schema_version: version,
                jobs,
            }
        }
        other => bail!("Unexpected jobs.json root type: {other}"),
    };

    migrate_jobs_file(&mut file)?;
    Ok(file)
}

/// Apply sequential migrations until [`SCHEMA_VERSION`].
fn migrate_jobs_file(file: &mut JobsFile) -> Result<()> {
    while file.schema_version < SCHEMA_VERSION {
        let from = file.schema_version;
        match from {
            0 => {
                // v0 → v1: wrap bare array into envelope (already done in parse).
                // Placeholder for any per-job field defaults introduced in v1.
                for job in &mut file.jobs {
                    if job.engine.is_empty() {
                        job.engine = "http".into();
                    }
                }
                file.schema_version = 1;
            }
            // Future: 1 => { ...; file.schema_version = 2; }
            other => bail!(
                "Unsupported jobs.json schema_version {other} (this build understands up to {SCHEMA_VERSION})"
            ),
        }
        info!("Migrated jobs.json schema v{from} → v{}", file.schema_version);
    }

    if file.schema_version > SCHEMA_VERSION {
        bail!(
            "jobs.json schema_version {} is newer than this daemon (supports {})",
            file.schema_version,
            SCHEMA_VERSION
        );
    }
    Ok(())
}

/// Import or clean orphaned `.part` files, then persist if the job list changed.
pub fn apply_orphan_sweep(jobs: &mut Vec<DownloadJob>, settings: &DaemonSettings) -> Result<()> {
    let hours = settings.orphan_part_retention_hours.max(1);
    let retention = std::time::Duration::from_secs(hours.saturating_mul(3600));
    let before = jobs.len();
    let report = sweep_orphaned_parts(jobs, retention);
    if report.resumed > 0 || report.cleaned > 0 {
        info!(
            "Orphan .part sweep: resumed={}, cleaned={}, skipped={} (retention {}h)",
            report.resumed, report.cleaned, report.skipped, hours
        );
    }
    if jobs.len() != before || report.cleaned > 0 {
        save_jobs(jobs, settings)?;
    }
    Ok(())
}

/// Persist jobs with history pruning. Writes are atomic (temp + sync + rename).
///
/// Returns the job list that was written (after pruning), so callers can sync
/// in-memory state with disk.
pub fn save_jobs(jobs: &[DownloadJob], settings: &DaemonSettings) -> Result<Vec<DownloadJob>> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create state dir: {}", parent.display()))?;
    }

    let mut to_save: Vec<DownloadJob> = jobs
        .iter()
        .filter(|j| j.status != DownloadStatus::Cancelled)
        .cloned()
        .collect();

    let pruned = prune_job_history(&mut to_save, settings.job_history_limit);
    if pruned > 0 {
        info!(
            "Pruned {pruned} old completed/failed job(s) (cap {})",
            settings.job_history_limit
        );
    }

    let file = JobsFile {
        schema_version: SCHEMA_VERSION,
        jobs: to_save.clone(),
    };
    atomic_write_jobs_file(&path, &file)?;
    Ok(to_save)
}

/// Keep all active/paused jobs; retain only the newest `limit` completed/failed.
///
/// Returns how many history entries were dropped. `limit == 0` means unlimited.
pub fn prune_job_history(jobs: &mut Vec<DownloadJob>, limit: usize) -> usize {
    if limit == 0 {
        return 0;
    }

    let mut history_idx: Vec<usize> = jobs
        .iter()
        .enumerate()
        .filter(|(_, j)| {
            matches!(
                j.status,
                DownloadStatus::Completed | DownloadStatus::Failed
            )
        })
        .map(|(i, _)| i)
        .collect();

    if history_idx.len() <= limit {
        return 0;
    }

    history_idx.sort_by(|&a, &b| {
        let ja = &jobs[a];
        let jb = &jobs[b];
        let ta = ja.completed_at.unwrap_or(ja.created_at);
        let tb = jb.completed_at.unwrap_or(jb.created_at);
        tb.cmp(&ta) // newest first
    });

    let drop: std::collections::HashSet<usize> =
        history_idx.into_iter().skip(limit).collect();
    let before = jobs.len();
    let mut i = 0;
    jobs.retain(|_| {
        let keep = !drop.contains(&i);
        i += 1;
        keep
    });
    before - jobs.len()
}

/// Write-temp-then-rename with `sync_data` so a crash mid-save cannot leave a
/// truncated `jobs.json`.
fn atomic_write_jobs_file(path: &Path, file: &JobsFile) -> Result<()> {
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let tmp_name = format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(STATE_FILE),
        std::process::id()
    );
    let tmp_path = parent.join(tmp_name);

    let json = serde_json::to_vec_pretty(file).context("Failed to serialize jobs")?;
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| format!("Failed to create {}", tmp_path.display()))?;
        f.write_all(&json)
            .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
        f.sync_data()
            .with_context(|| format!("Failed to sync {}", tmp_path.display()))?;
    }

    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "Failed to atomically replace {} (from {})",
            path.display(),
            tmp_path.display()
        )
    })?;

    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(&parent) {
        let _ = dir.sync_data();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_job(status: DownloadStatus, age_secs: i64) -> DownloadJob {
        let created = Utc::now() - chrono::Duration::seconds(age_secs);
        DownloadJob {
            phase: None,
            id: Uuid::new_v4(),
            url: "https://example.com/f".into(),
            filename: "f.bin".into(),
            save_path: "f.bin".into(),
            total_bytes: 10,
            downloaded_bytes: 10,
            segments: vec![],
            status,
            speed_bps: 0.0,
            eta_seconds: None,
            created_at: created,
            completed_at: Some(created),
            supports_ranges: true,
            segment_count: 1,
            use_http3: false,
            user_agent: None,
            cookies: None,
            referer: None,
            quality: None,
            format: None,
            engine: "http".into(),
            error: None,
            size_is_estimate: false,
        }
    }

    #[test]
    fn migrates_legacy_array() {
        let arr = serde_json::json!([{
            "id": Uuid::new_v4(),
            "url": "https://ex.com/a",
            "filename": "a.bin",
            "save_path": "a.bin",
            "total_bytes": 1,
            "downloaded_bytes": 0,
            "segments": [],
            "status": "paused",
            "speed_bps": 0.0,
            "eta_seconds": null,
            "created_at": Utc::now().to_rfc3339(),
            "completed_at": null,
            "supports_ranges": false,
            "segment_count": 1,
            "use_http3": false
        }]);
        let file = parse_and_migrate(arr).unwrap();
        assert_eq!(file.schema_version, SCHEMA_VERSION);
        assert_eq!(file.jobs.len(), 1);
        assert_eq!(file.jobs[0].engine, "http");
    }

    #[test]
    fn prune_keeps_active_and_newest_history() {
        let mut jobs = vec![
            sample_job(DownloadStatus::Paused, 0),
            sample_job(DownloadStatus::Completed, 100),
            sample_job(DownloadStatus::Completed, 50),
            sample_job(DownloadStatus::Failed, 10),
            sample_job(DownloadStatus::Completed, 200),
        ];
        let dropped = prune_job_history(&mut jobs, 2);
        assert_eq!(dropped, 2);
        assert_eq!(jobs.len(), 3); // 1 paused + 2 history
        assert!(jobs.iter().any(|j| j.status == DownloadStatus::Paused));
        assert_eq!(
            jobs.iter()
                .filter(|j| matches!(
                    j.status,
                    DownloadStatus::Completed | DownloadStatus::Failed
                ))
                .count(),
            2
        );
    }

    #[test]
    fn prune_zero_means_unlimited() {
        let mut jobs = vec![
            sample_job(DownloadStatus::Completed, 1),
            sample_job(DownloadStatus::Completed, 2),
        ];
        assert_eq!(prune_job_history(&mut jobs, 0), 0);
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn atomic_roundtrip_envelope() {
        let dir = std::env::temp_dir().join(format!("dm-jobs-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jobs.json");
        let file = JobsFile {
            schema_version: SCHEMA_VERSION,
            jobs: vec![sample_job(DownloadStatus::Paused, 0)],
        };
        atomic_write_jobs_file(&path, &file).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let parsed: JobsFile = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert_eq!(parsed.jobs.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
