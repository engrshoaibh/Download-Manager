use crate::engine::Segment;

/// The "In-Half" dynamic reallocation scheduler.
///
/// When a worker finishes its segment early, find the segment with the largest
/// remaining byte count and split it — giving the idle worker the trailing half.
pub struct Scheduler;

impl Scheduler {
    /// Find which segment to steal from (the one with most remaining bytes)
    /// and return the new start offset for the stealing worker.
    ///
    /// Returns `None` if no segments have enough remaining bytes to split.
    pub fn find_steal_target(segments: &mut Vec<Segment>, min_steal_bytes: u64) -> Option<usize> {
        // Find the segment with the most remaining bytes
        let target_idx = segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.remaining() > min_steal_bytes * 2)
            .max_by_key(|(_, s)| s.remaining())
            .map(|(i, _)| i)?;

        Some(target_idx)
    }

    /// Split a segment at its midpoint.
    ///
    /// The original segment is truncated to the first half.
    /// Returns the new segment (second half) to assign to the idle worker.
    pub fn split_segment(segments: &mut Vec<Segment>, target_idx: usize, new_id: usize) -> Segment {
        let target = &mut segments[target_idx];
        let remaining_start = target.current_offset();
        let remaining_end = target.end;
        let mid = remaining_start + (remaining_end - remaining_start) / 2;

        // Shrink original segment to first half
        target.end = mid;

        // New segment takes the second half
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
            Segment::new(1, 101, 1000), // largest remaining = 900
            Segment::new(2, 1001, 1100),
        ];

        let idx = Scheduler::find_steal_target(&mut segs, 50).unwrap();
        assert_eq!(idx, 1);
    }

    #[test]
    fn test_split_segment() {
        let mut segs = vec![Segment::new(0, 100, 999)]; // 900 bytes
        let new_seg = Scheduler::split_segment(&mut segs, 0, 1);

        // First half: 100..549
        assert_eq!(segs[0].end, 549);
        // Second half: 550..999
        assert_eq!(new_seg.start, 550);
        assert_eq!(new_seg.end, 999);
    }
}
