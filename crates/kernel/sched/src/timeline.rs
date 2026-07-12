//! Major frame timeline configuration — Phase 0 P0-F (v0.19.0).
//!
//! Provides the ARINC 653 major/minor frame data structures for partition
//! scheduling: [`PartitionId`], [`PartitionSlot`], and [`MajorFrame`].
//!
//! A major frame is a repeating cycle of partition time slots. Each slot
//! allocates a fixed duration (in ms) to a partition. The major frame cycles
//! indefinitely, giving each partition deterministic CPU time.
//!
//! Per D3, [`PartitionId`] is a newtype like `Tid`. Per D4, max 16 slots.

use crate::isolation::SchedError;

/// Maximum number of partition slots in a major frame.
pub const MAX_SLOTS: usize = 16;

/// Partition identifier (newtype over `u32`).
///
/// Like `Tid`, this is a lightweight identifier. `PartitionId(0)` is valid
/// (unlike `Tid(0)` which is reserved as invalid).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartitionId(pub u32);

/// A time slot in a major frame, allocating `duration_ms` to `partition`.
#[derive(Clone, Copy, Debug)]
pub struct PartitionSlot {
    pub partition: PartitionId,
    pub duration_ms: u32,
}

/// A major frame: a repeating cycle of partition time slots.
///
/// The frame cycles through `slots[0..slot_count]` indefinitely. Each slot
/// runs for its `duration_ms`, then the next slot begins. After the last
/// slot, the frame wraps back to slot 0.
#[derive(Debug)]
pub struct MajorFrame {
    pub slots: [PartitionSlot; MAX_SLOTS],
    pub slot_count: usize,
    pub period_ms: u32,
    pub current_slot: usize,
    pub frame_start_ns: u64,
}

impl MajorFrame {
    /// Create an empty major frame with no slots.
    pub const fn new() -> Self {
        Self {
            slots: [PartitionSlot {
                partition: PartitionId(0),
                duration_ms: 0,
            }; MAX_SLOTS],
            slot_count: 0,
            period_ms: 0,
            current_slot: 0,
            frame_start_ns: 0,
        }
    }

    /// Add a partition time slot to the frame.
    ///
    /// Returns `Err(SchedError::SlotFull)` if `MAX_SLOTS` (16) slots already
    /// exist. On success, appends the slot and accumulates `duration_ms`
    /// into `period_ms`.
    pub fn add_slot(&mut self, partition: PartitionId, duration_ms: u32) -> Result<(), SchedError> {
        if self.slot_count >= MAX_SLOTS {
            return Err(SchedError::SlotFull);
        }
        self.slots[self.slot_count] = PartitionSlot {
            partition,
            duration_ms,
        };
        self.period_ms += duration_ms;
        self.slot_count += 1;
        Ok(())
    }

    /// Advance to the next slot, wrapping back to 0 after the last slot.
    ///
    /// Returns the new `current_slot` index. If `slot_count == 0`, returns 0
    /// without modifying state.
    pub fn advance_slot(&mut self) -> usize {
        if self.slot_count == 0 {
            return 0;
        }
        self.current_slot = (self.current_slot + 1) % self.slot_count;
        self.current_slot
    }

    /// Returns the partition of the current slot, or `None` if the frame
    /// is empty (`slot_count == 0`).
    pub fn current_partition(&self) -> Option<PartitionId> {
        if self.slot_count == 0 {
            return None;
        }
        Some(self.slots[self.current_slot].partition)
    }

    /// Returns the duration of the current slot in nanoseconds.
    /// Returns 0 if the frame is empty.
    pub fn current_duration_ns(&self) -> u64 {
        if self.slot_count == 0 {
            return 0;
        }
        self.slots[self.current_slot].duration_ms as u64 * 1_000_000
    }
}

impl Default for MajorFrame {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_major_frame_is_empty() {
        let mf = MajorFrame::new();
        assert_eq!(mf.slot_count, 0);
        assert_eq!(mf.period_ms, 0);
        assert_eq!(mf.current_slot, 0);
        assert_eq!(mf.current_partition(), None);
        assert_eq!(mf.current_duration_ns(), 0);
    }

    #[test]
    fn test_add_slot_success() {
        let mut mf = MajorFrame::new();
        assert_eq!(mf.add_slot(PartitionId(0), 5), Ok(()));
        assert_eq!(mf.slot_count, 1);
        assert_eq!(mf.period_ms, 5);
        assert_eq!(mf.slots[0].partition, PartitionId(0));
        assert_eq!(mf.slots[0].duration_ms, 5);
    }

    #[test]
    fn test_add_multiple_slots() {
        let mut mf = MajorFrame::new();
        assert_eq!(mf.add_slot(PartitionId(0), 5), Ok(()));
        assert_eq!(mf.add_slot(PartitionId(1), 20), Ok(()));
        assert_eq!(mf.add_slot(PartitionId(0), 5), Ok(()));
        assert_eq!(mf.slot_count, 3);
        assert_eq!(mf.period_ms, 30);
    }

    #[test]
    fn test_add_slot_overflow() {
        let mut mf = MajorFrame::new();
        for _ in 0..MAX_SLOTS {
            assert_eq!(mf.add_slot(PartitionId(0), 1), Ok(()));
        }
        // 17th slot should fail
        assert_eq!(mf.add_slot(PartitionId(0), 1), Err(SchedError::SlotFull));
        assert_eq!(mf.slot_count, MAX_SLOTS);
    }

    #[test]
    fn test_advance_slot_wraps() {
        let mut mf = MajorFrame::new();
        let _ = mf.add_slot(PartitionId(0), 5);
        let _ = mf.add_slot(PartitionId(1), 10);
        let _ = mf.add_slot(PartitionId(2), 15);
        assert_eq!(mf.current_slot, 0);
        assert_eq!(mf.advance_slot(), 1);
        assert_eq!(mf.advance_slot(), 2);
        assert_eq!(mf.advance_slot(), 0); // wraps
        assert_eq!(mf.advance_slot(), 1);
    }

    #[test]
    fn test_advance_slot_empty_frame() {
        let mut mf = MajorFrame::new();
        assert_eq!(mf.advance_slot(), 0);
        assert_eq!(mf.current_slot, 0);
    }

    #[test]
    fn test_current_partition_and_duration() {
        let mut mf = MajorFrame::new();
        let _ = mf.add_slot(PartitionId(0), 5);
        let _ = mf.add_slot(PartitionId(1), 20);
        assert_eq!(mf.current_partition(), Some(PartitionId(0)));
        assert_eq!(mf.current_duration_ns(), 5_000_000);
        mf.advance_slot();
        assert_eq!(mf.current_partition(), Some(PartitionId(1)));
        assert_eq!(mf.current_duration_ns(), 20_000_000);
    }

    #[test]
    fn test_default_is_empty() {
        let mf = MajorFrame::default();
        assert_eq!(mf.slot_count, 0);
    }
}
