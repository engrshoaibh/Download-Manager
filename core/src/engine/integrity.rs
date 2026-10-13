//! Integrity checks around segmented downloads: is the saved state still trustworthy
//! before resuming, did every byte actually arrive before completing, and does the
//! finished file end up where the job says it is.
//!
//! The segment list (`.part.meta` / jobs file) and the `.part` payload are written
//! independently. If either goes missing or disagrees with the other, resuming from the
//! stale half would silently produce a file full of zeros, so we detect that and restart.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::Segment;

/// Outcome of comparing saved segment state with the payload on disk.
#[derive(Debug, PartialEq, Eq)]
pub enum ResumeCheck {
    /// Saved state matches the payload; continue from the recorded offsets.
    Resume,
    /// Saved state cannot be trusted; discard it and download from scratch.
    Restart(String),
}

/// Decide whether a paused/failed job can resume from `segments` + the `.part` file.
pub fn check_resume(segments: &[Segment], total_bytes: u64, part_path: &Path) -> ResumeCheck {
    if segments.is_empty() || total_bytes == 0 {
        return ResumeCheck::Restart("no saved segment state".into());
    }
    if segments.iter().all(|s| s.downloaded == 0) {
        // Nothing recorded as downloaded: a fresh start is equivalent and always safe.
        return ResumeCheck::Restart("no progress recorded".into());
    }
    let len = match std::fs::metadata(part_path) {
        Ok(m) => m.len(),
        Err(_) => return ResumeCheck::Restart(format!("partial file missing: {}", part_path.display())),
    };
    if len != total_bytes {
        return ResumeCheck::Restart(format!(
            "partial file is {len} bytes, expected {total_bytes}"
        ));
    }
    if let Err(e) = check_layout(segments, total_bytes) {
        return ResumeCheck::Restart(format!("segment state inconsistent: {e}"));
    }
    ResumeCheck::Resume
}

/// Segments must tile `[0, total)` exactly (no gaps, no overlaps) and never claim more
/// bytes than their range holds.
pub fn check_layout(segments: &[Segment], total_bytes: u64) -> Result<()> {
    if total_bytes == 0 {
        return Ok(());
    }
    let mut sorted: Vec<&Segment> = segments.iter().collect();
    sorted.sort_by_key(|s| s.start);
    let mut expected_start = 0u64;
    for s in sorted {
        if s.end < s.start {
            bail!("segment {} has end {} before start {}", s.id, s.end, s.start);
        }
        if s.start != expected_start {
            bail!(
                "gap or overlap before segment {} (starts at {}, expected {})",
                s.id,
                s.start,
                expected_start
            );
        }
        let len = s.end - s.start + 1;
        if s.downloaded > len {
            bail!("segment {} claims {} bytes of a {}-byte range", s.id, s.downloaded, len);
        }
        expected_start = s.end + 1;
    }
    if expected_start != total_bytes {
        bail!("segments cover {expected_start} bytes, expected {total_bytes}");
    }
    Ok(())
}

/// Every byte of every segment must have been written.
pub fn verify_complete(segments: &[Segment], total_bytes: u64) -> Result<()> {
    check_layout(segments, total_bytes)?;
    let missing: Vec<String> = segments
        .iter()
        .filter(|s| s.downloaded != s.end - s.start + 1)
        .map(|s| format!("#{} {}-{} ({}/{})", s.id, s.start, s.end, s.downloaded, s.end - s.start + 1))
        .collect();
    if !missing.is_empty() {
        bail!("incomplete segments: {}", missing.join(", "));
    }
    Ok(())
}

/// Move the finished payload into place, verifying size first and retrying transient
/// failures (antivirus / indexer locks on Windows). Falls back to copy + delete when a
/// rename cannot cross volumes. Keeps the `.part` file on failure so Resume can retry.
pub async fn finalize_file(tmp: &Path, dest: &Path, expected_len: Option<u64>) -> Result<()> {
    let meta = std::fs::metadata(tmp)
        .with_context(|| format!("finished file is missing: {}", tmp.display()))?;
    if let Some(want) = expected_len {
        if meta.len() != want {
            bail!("finished file is {} bytes, expected {want}", meta.len());
        }
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if dest.exists() {
        std::fs::remove_file(dest)
            .with_context(|| format!("could not replace existing {}", dest.display()))?;
    }

    let mut last_err = None;
    for attempt in 0..6u64 {
        match std::fs::rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(150 * (attempt + 1))).await;
            }
        }
    }
    // Last resort: copy then remove (cross-device moves).
    match std::fs::copy(tmp, dest) {
        Ok(n) if expected_len.map_or(true, |w| w == n) => {
            let _ = std::fs::remove_file(tmp);
            Ok(())
        }
        Ok(n) => {
            let _ = std::fs::remove_file(dest);
            bail!("copy wrote {n} bytes, expected {}", expected_len.unwrap_or(n))
        }
        Err(e) => bail!(
            "could not move {} to {}: rename: {}; copy: {e}",
            tmp.display(),
            dest.display(),
            last_err.map(|e| e.to_string()).unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: usize, start: u64, end: u64, downloaded: u64) -> Segment {
        Segment { id, start, end, downloaded, speed_bps: 0.0 }
    }

    #[test]
    fn layout_accepts_exact_tiling() {
        let s = vec![seg(0, 0, 49, 10), seg(2, 75, 99, 0), seg(1, 50, 74, 5)];
        assert!(check_layout(&s, 100).is_ok());
    }

    #[test]
    fn layout_rejects_gap_overlap_and_overclaim() {
        assert!(check_layout(&[seg(0, 0, 49, 0), seg(1, 60, 99, 0)], 100).is_err());
        assert!(check_layout(&[seg(0, 0, 59, 0), seg(1, 50, 99, 0)], 100).is_err());
        assert!(check_layout(&[seg(0, 0, 49, 51), seg(1, 50, 99, 0)], 100).is_err());
        assert!(check_layout(&[seg(0, 0, 49, 0)], 100).is_err());
    }

    #[test]
    fn complete_requires_every_byte() {
        let done = vec![seg(0, 0, 49, 50), seg(1, 50, 99, 50)];
        assert!(verify_complete(&done, 100).is_ok());
        let short = vec![seg(0, 0, 49, 50), seg(1, 50, 99, 49)];
        assert!(verify_complete(&short, 100).is_err());
    }

    #[test]
    fn resume_restarts_when_part_file_missing_or_wrong_size() {
        let dir = std::env::temp_dir().join(format!("dm-integrity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("a.bin.part");
        let segs = vec![seg(0, 0, 49, 20), seg(1, 50, 99, 0)];

        assert!(matches!(check_resume(&segs, 100, &part), ResumeCheck::Restart(_)));
        std::fs::write(&part, vec![0u8; 80]).unwrap();
        assert!(matches!(check_resume(&segs, 100, &part), ResumeCheck::Restart(_)));
        std::fs::write(&part, vec![0u8; 100]).unwrap();
        assert_eq!(check_resume(&segs, 100, &part), ResumeCheck::Resume);
        std::fs::remove_dir_all(&dir).ok();
    }
}
