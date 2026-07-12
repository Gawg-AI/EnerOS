//! Bad block tracking with reserved-block replacement and wear leveling.
//!
//! [`BadBlockTable`] maintains a list of bad block indices and a pool of
//! reserved blocks (located at the end of the device) used as replacements.
//! When a block goes bad, the caller asks for a replacement from the reserved
//! pool; the mapping is recorded so future accesses to the bad block can be
//! transparently redirected.
//!
//! # Wear Leveling
//!
//! [`BadBlockTable::wear_level`] reports a 0–100 score based on the ratio of
//! bad blocks to total blocks. [`BadBlockTable::remaining_life`] is the
//! complement (100 minus wear level).

use alloc::vec::Vec;

use crate::error::StorageError;

/// Tracks bad blocks and allocates replacement blocks from a reserved pool.
///
/// The reserved pool occupies the last `reserved_count` blocks of the device.
/// Replacements are handed out sequentially from the start of the pool.
pub struct BadBlockTable {
    /// Sorted list of bad block indices (deduplicated).
    bad_blocks: Vec<u64>,
    /// First reserved block index (start of the replacement pool).
    reserved_start: u64,
    /// Total number of reserved blocks available for replacement.
    reserved_count: u64,
    /// Next available reserved block to hand out as a replacement.
    next_reserved: u64,
    /// Total blocks in the device (including reserved).
    total_blocks: u64,
    /// Timestamp of the last bad-block scan (driver-defined units).
    last_check_time: u64,
}

impl BadBlockTable {
    /// Creates a new bad block table for a device with `total_blocks` blocks
    /// and `reserved_count` replacement blocks located at the end.
    ///
    /// # Panics
    ///
    /// Panics if `reserved_count > total_blocks` (the reserved pool would
    /// exceed the device size).
    pub fn new(total_blocks: u64, reserved_count: u64) -> Self {
        assert!(
            reserved_count <= total_blocks,
            "reserved_count ({}) cannot exceed total_blocks ({})",
            reserved_count,
            total_blocks
        );
        let reserved_start = total_blocks - reserved_count;
        BadBlockTable {
            bad_blocks: Vec::new(),
            reserved_start,
            reserved_count,
            next_reserved: reserved_start,
            total_blocks,
            last_check_time: 0,
        }
    }

    /// Returns `true` if `block_idx` is recorded as bad.
    pub fn is_bad(&self, block_idx: u64) -> bool {
        self.bad_blocks.contains(&block_idx)
    }

    /// Marks `block_idx` as bad. Duplicate marks are silently ignored.
    pub fn mark_bad(&mut self, block_idx: u64) {
        if !self.is_bad(block_idx) {
            self.bad_blocks.push(block_idx);
        }
    }

    /// Allocates a replacement block from the reserved pool for `block_idx`.
    ///
    /// Returns [`StorageError::HardwareFault`] when the reserved pool is
    /// exhausted. The bad block is also recorded in the table.
    pub fn get_replacement(&mut self, block_idx: u64) -> Result<u64, StorageError> {
        self.mark_bad(block_idx);
        if self.next_reserved >= self.total_blocks {
            return Err(StorageError::HardwareFault {
                detail: "reserved block pool exhausted",
            });
        }
        let replacement = self.next_reserved;
        self.next_reserved += 1;
        Ok(replacement)
    }

    /// Returns the number of bad blocks currently recorded.
    pub fn count(&self) -> usize {
        self.bad_blocks.len()
    }

    /// Returns a wear-level score in 0–100 based on the bad block ratio.
    ///
    /// 0 means no bad blocks (no wear); 100 means the entire device is bad.
    pub fn wear_level(&self) -> u8 {
        if self.total_blocks == 0 {
            return 0;
        }
        let bad = self.bad_blocks.len() as u64;
        // Scale to 0–100.
        let wear = (bad * 100) / self.total_blocks;
        // Clamp to u8 (defensive; wear ≤ 100 by construction when bad ≤ total).
        wear.min(100) as u8
    }

    /// Returns the remaining life score in 0–100 (inverse of wear level).
    pub fn remaining_life(&self) -> u8 {
        100u8.saturating_sub(self.wear_level())
    }

    /// Returns the timestamp of the last bad-block scan.
    pub fn last_check_time(&self) -> u64 {
        self.last_check_time
    }

    /// Records the timestamp of the most recent bad-block scan.
    pub fn set_check_time(&mut self, time: u64) {
        self.last_check_time = time;
    }

    /// Returns the first reserved block index (start of the replacement pool).
    #[allow(dead_code)]
    pub fn reserved_start(&self) -> u64 {
        self.reserved_start
    }

    /// Returns the number of remaining replacement blocks in the pool.
    #[allow(dead_code)]
    pub fn remaining_reserved(&self) -> u64 {
        self.total_blocks.saturating_sub(self.next_reserved)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn make_table() -> BadBlockTable {
        // 1000 user blocks + 100 reserved = 1100 total.
        BadBlockTable::new(1100, 100)
    }

    #[test]
    fn test_new_table_empty() {
        let t = make_table();
        assert_eq!(t.count(), 0);
        assert!(!t.is_bad(0));
        assert!(!t.is_bad(500));
        assert_eq!(t.wear_level(), 0);
        assert_eq!(t.remaining_life(), 100);
        assert_eq!(t.last_check_time(), 0);
    }

    #[test]
    fn test_reserved_pool_location() {
        let t = make_table();
        // Reserved pool starts at block 1000 (1100 - 100).
        assert_eq!(t.reserved_start(), 1000);
        assert_eq!(t.remaining_reserved(), 100);
    }

    #[test]
    fn test_mark_bad_and_query() {
        let mut t = make_table();
        t.mark_bad(42);
        t.mark_bad(100);
        assert!(t.is_bad(42));
        assert!(t.is_bad(100));
        assert!(!t.is_bad(43));
        assert_eq!(t.count(), 2);
    }

    #[test]
    fn test_mark_bad_duplicate() {
        let mut t = make_table();
        t.mark_bad(42);
        t.mark_bad(42);
        t.mark_bad(42);
        assert_eq!(t.count(), 1);
        assert!(t.is_bad(42));
    }

    #[test]
    fn test_get_replacement() {
        let mut t = make_table();
        let r = t.get_replacement(42).expect("replacement should succeed");
        assert_eq!(r, 1000); // first reserved block
        assert!(t.is_bad(42));
        assert_eq!(t.remaining_reserved(), 99);

        let r2 = t.get_replacement(50).expect("replacement should succeed");
        assert_eq!(r2, 1001);
        assert!(t.is_bad(50));
        assert_eq!(t.remaining_reserved(), 98);
    }

    #[test]
    fn test_get_replacement_marks_bad() {
        let mut t = make_table();
        let _ = t.get_replacement(7).unwrap();
        // get_replacement should have marked block 7 as bad.
        assert!(t.is_bad(7));
    }

    #[test]
    fn test_exhaust_reserved_pool() {
        let mut t = BadBlockTable::new(10, 3);
        assert_eq!(t.get_replacement(0).unwrap(), 7);
        assert_eq!(t.get_replacement(1).unwrap(), 8);
        assert_eq!(t.get_replacement(2).unwrap(), 9);
        // Pool exhausted.
        let err = t.get_replacement(3).unwrap_err();
        match err {
            StorageError::HardwareFault { detail } => {
                assert!(detail.contains("exhausted"));
            }
            other => panic!("expected HardwareFault, got {:?}", other),
        }
    }

    #[test]
    fn test_wear_level_and_remaining_life() {
        let mut t = BadBlockTable::new(1000, 100);
        // 0 bad blocks → wear 0, life 100.
        assert_eq!(t.wear_level(), 0);
        assert_eq!(t.remaining_life(), 100);

        // 100 bad blocks out of 1000 total → wear 10, life 90.
        for i in 0..100 {
            t.mark_bad(i);
        }
        assert_eq!(t.wear_level(), 10);
        assert_eq!(t.remaining_life(), 90);

        // 500 bad blocks → wear 50, life 50.
        let mut t2 = BadBlockTable::new(1000, 100);
        for i in 0..500 {
            t2.mark_bad(i);
        }
        assert_eq!(t2.wear_level(), 50);
        assert_eq!(t2.remaining_life(), 50);
    }

    #[test]
    fn test_wear_level_full_device() {
        let mut t = BadBlockTable::new(100, 0);
        for i in 0..100 {
            t.mark_bad(i);
        }
        assert_eq!(t.wear_level(), 100);
        assert_eq!(t.remaining_life(), 0);
    }

    #[test]
    fn test_wear_level_zero_total() {
        let t = BadBlockTable::new(0, 0);
        assert_eq!(t.wear_level(), 0);
        assert_eq!(t.remaining_life(), 100);
    }

    #[test]
    fn test_check_time() {
        let mut t = make_table();
        assert_eq!(t.last_check_time(), 0);
        t.set_check_time(12345);
        assert_eq!(t.last_check_time(), 12345);
        t.set_check_time(99999);
        assert_eq!(t.last_check_time(), 99999);
    }

    #[test]
    #[should_panic(expected = "reserved_count")]
    fn test_reserved_exceeds_total_panics() {
        let _ = BadBlockTable::new(100, 200);
    }

    #[test]
    fn test_zero_reserved() {
        let mut t = BadBlockTable::new(100, 0);
        // No reserved blocks — any replacement request fails immediately.
        let err = t.get_replacement(5).unwrap_err();
        assert!(matches!(err, StorageError::HardwareFault { .. }));
    }
}
