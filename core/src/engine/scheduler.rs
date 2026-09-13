use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::engine::Segment;

/// Skip steal splits smaller than this so we do not thrash connections.
pub const MIN_STEAL_BYTES: u64 = 1024 * 1024;

use std::collections::HashSet;

/// Live segment plan used by workers to steal unfinished ranges.
pub struct StealState {
    pub segments: Vec<LiveSegment>,
    pub next_id: usize,
    pub active: HashSet<usize>,
}

pub struct LiveSegment {
    pub id: usize,
    pub start: u64,
    pub end: Arc<AtomicU64>,
    pub downloaded: Arc<AtomicU64>,
}

impl StealState {
    pub fn from_segments(segments: &[Segment]) -> Self {
        let live = segments
            .iter()
            .map(|s| LiveSegment {
                id: s.id,
                start: s.start,
                end: Arc::new(AtomicU64::new(s.end)),
                downloaded: Arc::new(AtomicU64::new(s.downloaded)),
            })
            .collect::<Vec<_>>();
        let next_id = live.len();
        Self {
            segments: live,
            next_id,
            active: HashSet::new(),
        }
    }

    pub fn set_active(&mut self, id: usize) {
        self.active.insert(id);
    }

    pub fn set_inactive(&mut self, id: usize) {
        self.active.remove(&id);
    }

    /// Split the largest remaining range and assign the trailing half to the idle worker.
    pub fn try_steal(
        &mut self,
        min_steal_bytes: u64,
    ) -> Option<(usize, u64, u64, u64, Arc<AtomicU64>)> {
        // 1. Prioritize resuming an inactive segment
        if let Some(target_idx) = self
            .segments
            .iter()
            .position(|s| !self.active.contains(&s.id) && remaining(s) > 0)
        {
            let target = &self.segments[target_idx];
            let downloaded = target.downloaded.load(Ordering::Relaxed);
            let start = target.start + downloaded;
            let end = target.end.load(Ordering::Acquire);
            let id = target.id;
            self.active.insert(id);
            return Some((id, start, end, downloaded, Arc::clone(&target.end)));
        }

        // 2. Limit maximum fragmentation to 32 segments
        if self.segments.len() >= 32 {
            return None;
        }

        // 3. Otherwise, split the largest remaining range
        let target_idx = self
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| remaining(s) > min_steal_bytes * 2)
            .max_by_key(|(_, s)| remaining(s))
            .map(|(i, _)| i)?;

        let (new_id, new_start, new_end, new_cap) = {
            let target = &self.segments[target_idx];
            let downloaded = target.downloaded.load(Ordering::Relaxed);
            let remaining_start = target.start + downloaded;
            let remaining_end = target.end.load(Ordering::Acquire);
            if remaining_end <= remaining_start {
                return None;
            }
            let mid = remaining_start + (remaining_end - remaining_start) / 2;
            if mid <= remaining_start || mid >= remaining_end {
                return None;
            }

            target.end.store(mid, Ordering::Release);
            let new_id = self.next_id;
            self.next_id += 1;
            let new_start = mid + 1;
            let new_cap = Arc::new(AtomicU64::new(remaining_end));
            (new_id, new_start, remaining_end, new_cap)
        };

        self.segments.push(LiveSegment {
            id: new_id,
            start: new_start,
            end: Arc::clone(&new_cap),
            downloaded: Arc::new(AtomicU64::new(0)),
        });

        self.active.insert(new_id);
        Some((new_id, new_start, new_end, 0, new_cap))
    }

    pub fn snapshot(&self) -> Vec<Segment> {
        self.segments
            .iter()
            .map(|s| Segment {
                id: s.id,
                start: s.start,
                end: s.end.load(Ordering::Relaxed),
                downloaded: s.downloaded.load(Ordering::Relaxed),
                speed_bps: 0.0,
            })
            .collect()
    }

    pub fn total_downloaded(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| s.downloaded.load(Ordering::Relaxed))
            .sum()
    }

    pub fn has_remaining(&self) -> bool {
        self.segments.iter().any(|s| remaining(s) > 0)
    }
}

fn remaining(seg: &LiveSegment) -> u64 {
    let end = seg.end.load(Ordering::Relaxed);
    let downloaded = seg.downloaded.load(Ordering::Relaxed);
    end.saturating_sub(seg.start).saturating_add(1).saturating_sub(downloaded)
}

/// The "In-Half" dynamic reallocation scheduler (unit-test helpers).
/// Live stealing uses [`StealState::try_steal`] instead.
#[cfg(test)]
pub struct Scheduler;

#[cfg(test)]
impl Scheduler {
    pub fn find_steal_target(segments: &mut Vec<Segment>, min_steal_bytes: u64) -> Option<usize> {
        segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.remaining() > min_steal_bytes * 2)
            .max_by_key(|(_, s)| s.remaining())
            .map(|(i, _)| i)
    }

    pub fn split_segment(segments: &mut Vec<Segment>, target_idx: usize, new_id: usize) -> Segment {
        let target = &mut segments[target_idx];
        let remaining_start = target.current_offset();
        let remaining_end = target.end;
        let mid = remaining_start + (remaining_end - remaining_start) / 2;

        target.end = mid;
        Segment::new(new_id, mid + 1, remaining_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_finds_largest() {
        let mut segs = vec![
            Segment::new(0, 0, 100),
            Segment::new(1, 101, 1000),
            Segment::new(2, 1001, 1100),
        ];

        let idx = Scheduler::find_steal_target(&mut segs, 50).unwrap();
        assert_eq!(idx, 1);
    }

    #[test]
    fn test_split_segment() {
        let mut segs = vec![Segment::new(0, 100, 999)];
        let new_seg = Scheduler::split_segment(&mut segs, 0, 1);

        assert_eq!(segs[0].end, 549);
        assert_eq!(new_seg.start, 550);
        assert_eq!(new_seg.end, 999);
    }

    #[test]
    fn test_try_steal() {
        let segs = vec![
            Segment::new(0, 0, 100),
            Segment::new(1, 0, 8 * 1024 * 1024),
        ];
        let mut state = StealState::from_segments(&segs);
        let (id, start, end, _) = state.try_steal(MIN_STEAL_BYTES).unwrap();
        assert_eq!(id, 2);
        assert!(end > start);
        assert!(state.segments[1].end.load(Ordering::Relaxed) < 8 * 1024 * 1024);
    }
}
