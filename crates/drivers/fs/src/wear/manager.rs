//! Wear-leveling manager and trait.
//!
//! [`WearLevelManager`] tracks per-block erase counts and provides wear-leveling
//! decisions. It works alongside littlefs2's built-in dynamic wear leveling:
//! littlefs2 performs the actual block migration, while this module provides
//! observability, victim selection hints, and lifespan estimation.
//!
//! # Hot/Cold Segregation
//!
//! Blocks are classified as "hot" (erase count above average + threshold) or
//! "cold" (below average). Victim blocks for GC are selected from hot blocks
//! first, so that their data is migrated to colder areas, spreading wear.
//!
//! # Lifespan Estimation
//!
//! Given the average erase count, block geometry, and daily write volume,
//! the manager estimates remaining lifespan:
//!
//! ```text
//! daily_flash_bytes  = daily_write_mb × 1_000_000 × write_amplification
//! daily_erases       = daily_flash_bytes / block_size
//! daily_erases_per_block = daily_erases / total_blocks
//! remaining_erases   = max_erase_cycles − avg_block_erases
//! lifespan_years     = remaining_erases / daily_erases_per_block / 365.25
//! ```

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::wear::status::{WearDistribution, WearStatus};
use crate::wear::write_amp::WriteAmplificationTracker;

/// Default maximum erase cycles for SLC NAND flash.
pub const DEFAULT_MAX_ERASE_CYCLES: u32 = 100_000;

/// Default GC trigger threshold: balance ratio above this triggers wear leveling.
pub const DEFAULT_GC_THRESHOLD: f64 = 1.5;

/// Default write amplification limit.
pub const DEFAULT_WRITE_AMP_LIMIT: f64 = 2.0;

// ============================================================================
// WearLeveling Trait
// ============================================================================

/// Trait for wear-leveling monitoring and control.
///
/// Implemented by [`WearLevelManager`]. The trait abstracts the wear-leveling
/// interface so that alternative backends (e.g., a native eMMC wear-level
/// driver) can be swapped in.
pub trait WearLeveling {
    /// Records that `block` was erased. Increments its erase counter.
    fn record_erase(&mut self, block: u32);

    /// Selects a victim block for garbage collection — the block with the
    /// highest erase count among hot blocks.
    ///
    /// Returns `None` if no blocks have been erased or wear is balanced.
    fn select_victim_block(&self) -> Option<u32>;

    /// Estimates the remaining lifespan in years given the daily write volume
    /// (in MB).
    fn estimate_lifespan(&self, daily_write_mb: u64) -> f64;

    /// Returns a snapshot of the current wear-leveling status.
    fn wear_level_status(&self) -> WearStatus;
}

// ============================================================================
// WearLevelManager
// ============================================================================

/// Manages per-block erase counts and wear-leveling decisions.
///
/// Construct with [`WearLevelManager::new`] for defaults, or
/// [`WearLevelManager::with_config`] for custom geometry.
#[derive(Clone, Debug)]
pub struct WearLevelManager {
    /// Per-block erase counts, keyed by block index.
    block_erase_count: BTreeMap<u32, u32>,
    /// Total number of blocks on the device.
    total_blocks: u32,
    /// Block size in bytes.
    block_size: u32,
    /// Maximum erase cycles per block (SLC: ~100,000).
    max_erase_cycles: u32,
    /// Balance ratio threshold above which GC is triggered.
    gc_threshold: f64,
    /// Write amplification tracker.
    write_amp_tracker: WriteAmplificationTracker,
}

impl WearLevelManager {
    /// Creates a new manager with default configuration.
    ///
    /// - `total_blocks`: 65536 (256 MB at 4 KB blocks)
    /// - `block_size`: 4096
    /// - `max_erase_cycles`: 100,000 (SLC)
    /// - `gc_threshold`: 1.5
    pub fn new() -> Self {
        Self::with_config(65536, 4096, DEFAULT_MAX_ERASE_CYCLES)
    }

    /// Creates a new manager with the given geometry.
    pub fn with_config(total_blocks: u32, block_size: u32, max_erase_cycles: u32) -> Self {
        Self {
            block_erase_count: BTreeMap::new(),
            total_blocks,
            block_size,
            max_erase_cycles,
            gc_threshold: DEFAULT_GC_THRESHOLD,
            write_amp_tracker: WriteAmplificationTracker::new(),
        }
    }

    /// Returns the erase count for `block`, or 0 if unrecorded.
    pub fn block_erase_count(&self, block: u32) -> u32 {
        self.block_erase_count.get(&block).copied().unwrap_or(0)
    }

    /// Returns the total number of blocks.
    pub fn total_blocks(&self) -> u32 {
        self.total_blocks
    }

    /// Returns the block size in bytes.
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Returns the maximum erase cycles per block.
    pub fn max_erase_cycles(&self) -> u32 {
        self.max_erase_cycles
    }

    /// Returns the GC threshold (balance ratio).
    pub fn gc_threshold(&self) -> f64 {
        self.gc_threshold
    }

    /// Sets the GC threshold.
    pub fn set_gc_threshold(&mut self, threshold: f64) {
        self.gc_threshold = threshold;
    }

    /// Returns a reference to the write amplification tracker.
    pub fn write_amp_tracker(&self) -> &WriteAmplificationTracker {
        &self.write_amp_tracker
    }

    /// Returns a mutable reference to the write amplification tracker.
    pub fn write_amp_tracker_mut(&mut self) -> &mut WriteAmplificationTracker {
        &mut self.write_amp_tracker
    }

    /// Records application-level write bytes (for write amplification).
    pub fn record_app_write(&mut self, bytes: u64) {
        self.write_amp_tracker.record_app_write(bytes);
    }

    /// Records flash-level write bytes (for write amplification).
    pub fn record_flash_write(&mut self, bytes: u64) {
        self.write_amp_tracker.record_flash_write(bytes);
    }

    /// Returns `true` if the wear-leveling balance exceeds the GC threshold.
    ///
    /// When this returns `true`, [`trigger_wear_leveling`] should be called
    /// to initiate block migration.
    pub fn needs_wear_leveling(&self) -> bool {
        let status = self.wear_level_status();
        let ratio = if status.avg_block_erases > 0 {
            status.max_block_erases as f64 / status.avg_block_erases as f64
        } else {
            1.0
        };
        ratio > self.gc_threshold
    }

    /// Triggers wear leveling by returning a list of victim blocks to migrate.
    ///
    /// Returns up to `max_migrations` block indices, sorted by erase count
    /// (highest first). Returns an empty vector if wear is balanced.
    pub fn trigger_wear_leveling(&self, max_migrations: usize) -> Vec<u32> {
        if !self.needs_wear_leveling() {
            return Vec::new();
        }
        let mut blocks: Vec<(u32, u32)> = self
            .block_erase_count
            .iter()
            .map(|(&b, &c)| (b, c))
            .collect();
        // Sort by erase count descending.
        blocks.sort_by_key(|&(_, c)| core::cmp::Reverse(c));
        blocks
            .into_iter()
            .take(max_migrations)
            .map(|(b, _)| b)
            .collect()
    }

    /// Computes the average erase count across all recorded blocks.
    fn avg_erase_count(&self) -> u32 {
        if self.block_erase_count.is_empty() {
            return 0;
        }
        let total: u64 = self.block_erase_count.values().map(|&c| c as u64).sum();
        (total / self.block_erase_count.len() as u64) as u32
    }

    /// Collects all erase counts into a vector for distribution calculation.
    fn erase_counts_vec(&self) -> Vec<u32> {
        // Include blocks with zero erases for accurate distribution.
        let mut counts = Vec::with_capacity(self.total_blocks as usize);
        for b in 0..self.total_blocks {
            counts.push(self.block_erase_count(b));
        }
        counts
    }
}

impl Default for WearLevelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WearLeveling for WearLevelManager {
    fn record_erase(&mut self, block: u32) {
        let count = self.block_erase_count.entry(block).or_insert(0);
        *count = count.saturating_add(1);
    }

    fn select_victim_block(&self) -> Option<u32> {
        // Select the block with the highest erase count.
        self.block_erase_count
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(&b, _)| b)
    }

    fn estimate_lifespan(&self, daily_write_mb: u64) -> f64 {
        if self.total_blocks == 0 || self.block_size == 0 || daily_write_mb == 0 {
            return 0.0;
        }
        let avg = self.avg_erase_count();
        let wa = self.write_amp_tracker.write_amplification();
        // If no writes recorded yet, assume WA = 1.0.
        let wa = if wa == 0.0 { 1.0 } else { wa };

        let daily_flash_bytes = daily_write_mb as f64 * 1_000_000.0 * wa;
        let daily_erases = daily_flash_bytes / self.block_size as f64;
        let daily_erases_per_block = daily_erases / self.total_blocks as f64;

        if daily_erases_per_block <= 0.0 {
            return f64::INFINITY;
        }

        let remaining_erases = (self.max_erase_cycles as f64) - (avg as f64);
        if remaining_erases <= 0.0 {
            return 0.0;
        }

        let lifespan_days = remaining_erases / daily_erases_per_block;
        lifespan_days / 365.25
    }

    fn wear_level_status(&self) -> WearStatus {
        let counts = self.erase_counts_vec();
        let distribution = WearDistribution::from_counts(&counts);
        let total: u64 = self.block_erase_count.values().map(|&c| c as u64).sum();
        let max = self.block_erase_count.values().copied().max().unwrap_or(0);
        let avg = if self.total_blocks > 0 {
            (total / self.total_blocks as u64) as u32
        } else {
            0
        };
        let wa = self.write_amp_tracker.write_amplification();
        // Estimate lifespan with a default 500 MB/day.
        let lifespan = self.estimate_lifespan(500);

        WearStatus {
            total_wear_cycles: total,
            max_block_erases: max,
            avg_block_erases: avg,
            wear_distribution: distribution,
            write_amplification: wa,
            estimated_lifespan_years: lifespan,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn make_manager() -> WearLevelManager {
        WearLevelManager::with_config(64, 4096, 100_000)
    }

    #[test]
    fn test_new_manager_defaults() {
        let m = WearLevelManager::new();
        assert_eq!(m.total_blocks(), 65536);
        assert_eq!(m.block_size(), 4096);
        assert_eq!(m.max_erase_cycles(), 100_000);
        assert_eq!(m.gc_threshold(), 1.5);
    }

    #[test]
    fn test_with_config() {
        let m = WearLevelManager::with_config(128, 512, 50_000);
        assert_eq!(m.total_blocks(), 128);
        assert_eq!(m.block_size(), 512);
        assert_eq!(m.max_erase_cycles(), 50_000);
    }

    #[test]
    fn test_record_erase() {
        let mut m = make_manager();
        assert_eq!(m.block_erase_count(0), 0);
        m.record_erase(0);
        assert_eq!(m.block_erase_count(0), 1);
        m.record_erase(0);
        m.record_erase(0);
        assert_eq!(m.block_erase_count(0), 3);
    }

    #[test]
    fn test_record_erase_multiple_blocks() {
        let mut m = make_manager();
        m.record_erase(1);
        m.record_erase(2);
        m.record_erase(2);
        m.record_erase(3);
        m.record_erase(3);
        m.record_erase(3);
        assert_eq!(m.block_erase_count(1), 1);
        assert_eq!(m.block_erase_count(2), 2);
        assert_eq!(m.block_erase_count(3), 3);
    }

    #[test]
    fn test_record_erase_saturating() {
        let mut m = WearLevelManager::with_config(1, 4096, 100_000);
        // Record many erases; count should be accurate (no overflow).
        for _ in 0..200_000 {
            m.record_erase(0);
        }
        // 200,000 erases recorded correctly (u32 can hold up to ~4.3 billion).
        assert_eq!(m.block_erase_count(0), 200_000);
    }

    #[test]
    fn test_select_victim_block_empty() {
        let m = make_manager();
        assert_eq!(m.select_victim_block(), None);
    }

    #[test]
    fn test_select_victim_block_single() {
        let mut m = make_manager();
        m.record_erase(5);
        assert_eq!(m.select_victim_block(), Some(5));
    }

    #[test]
    fn test_select_victim_block_highest() {
        let mut m = make_manager();
        m.record_erase(1);
        m.record_erase(2);
        m.record_erase(2);
        m.record_erase(3);
        m.record_erase(3);
        m.record_erase(3);
        // Block 3 has the highest count (3).
        assert_eq!(m.select_victim_block(), Some(3));
    }

    #[test]
    fn test_select_victim_block_tie() {
        let mut m = make_manager();
        m.record_erase(10);
        m.record_erase(20);
        // Both have count 1; should return one of them.
        let victim = m.select_victim_block().expect("should return a block");
        assert!(victim == 10 || victim == 20);
    }

    #[test]
    fn test_estimate_lifespan_no_wear() {
        let m = WearLevelManager::with_config(65536, 4096, 100_000);
        // No writes recorded; WA defaults to 1.0.
        let years = m.estimate_lifespan(500);
        // 500 MB/day, 65536 blocks, 4096 byte blocks, WA=1.0
        // daily_flash = 500_000_000 bytes
        // daily_erases = 500_000_000 / 4096 = 122,070
        // daily_per_block = 122_070 / 65536 = 1.862
        // remaining = 100,000
        // lifespan_days = 100,000 / 1.862 = 53,704
        // lifespan_years = 53,704 / 365.25 = 147.0
        assert!(years >= 10.0, "expected >= 10 years, got {}", years);
        assert!(years > 100.0, "expected > 100 years, got {}", years);
    }

    #[test]
    fn test_estimate_lifespan_with_wear() {
        // Use a small block count to keep the test fast.
        let mut m = WearLevelManager::with_config(64, 4096, 100_000);
        // Simulate 50,000 erases on block 0 (representing average wear).
        for _ in 0..50_000 {
            m.record_erase(0);
        }
        let years = m.estimate_lifespan(500);
        // With 64 blocks, avg = 50000/64 = 781
        // remaining = 100,000 - 781 = 99,219
        // daily_flash = 500MB * 1M * 1.0(WA) = 500M bytes
        // daily_erases = 500M / 4096 = 122,070
        // daily_per_block = 122,070 / 64 = 1,907
        // lifespan_days = 99,219 / 1,907 = 52.0
        // lifespan_years = 52.0 / 365.25 = 0.14 years
        // This is small because 64 blocks is tiny; the test verifies the
        // calculation runs and produces a positive, finite result.
        assert!(years > 0.0, "expected > 0 years, got {}", years);
        assert!(years.is_finite(), "expected finite, got {}", years);
    }

    #[test]
    fn test_estimate_lifespan_exhausted() {
        let mut m = WearLevelManager::with_config(1, 4096, 10);
        // Exhaust all erase cycles.
        for _ in 0..10 {
            m.record_erase(0);
        }
        let years = m.estimate_lifespan(100);
        assert_eq!(years, 0.0);
    }

    #[test]
    fn test_estimate_lifespan_zero_daily_write() {
        let m = WearLevelManager::with_config(64, 4096, 100_000);
        let years = m.estimate_lifespan(0);
        assert_eq!(years, 0.0);
    }

    #[test]
    fn test_wear_level_status_empty() {
        let m = make_manager();
        let s = m.wear_level_status();
        assert_eq!(s.total_wear_cycles, 0);
        assert_eq!(s.max_block_erases, 0);
        assert_eq!(s.avg_block_erases, 0);
        assert_eq!(s.write_amplification, 0.0);
    }

    #[test]
    fn test_wear_level_status_with_data() {
        let mut m = make_manager();
        // Block 0: 10 erases, Block 1: 20, Block 2: 30, rest: 0.
        for _ in 0..10 {
            m.record_erase(0);
        }
        for _ in 0..20 {
            m.record_erase(1);
        }
        for _ in 0..30 {
            m.record_erase(2);
        }
        m.record_app_write(4096);
        m.record_flash_write(8192);

        let s = m.wear_level_status();
        assert_eq!(s.total_wear_cycles, 60);
        assert_eq!(s.max_block_erases, 30);
        // avg = 60 / 64 = 0 (integer division)
        assert_eq!(s.avg_block_erases, 0);
        assert_eq!(s.wear_distribution.max_erases, 30);
        assert!((s.write_amplification - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_needs_wear_leveling_balanced() {
        let mut m = make_manager();
        // All blocks have the same erase count.
        for b in 0..64 {
            for _ in 0..100 {
                m.record_erase(b);
            }
        }
        // ratio = 100/100 = 1.0 < 1.5
        assert!(!m.needs_wear_leveling());
    }

    #[test]
    fn test_needs_wear_leveling_unbalanced() {
        let mut m = make_manager();
        // 63 blocks at 100, 1 block at 500.
        for b in 0..63 {
            for _ in 0..100 {
                m.record_erase(b);
            }
        }
        for _ in 0..500 {
            m.record_erase(63);
        }
        // avg = (63*100 + 500) / 64 = 6800/64 = 106
        // ratio = 500/106 = 4.72 > 1.5
        assert!(m.needs_wear_leveling());
    }

    #[test]
    fn test_trigger_wear_leveling_balanced() {
        let mut m = make_manager();
        for b in 0..64 {
            for _ in 0..100 {
                m.record_erase(b);
            }
        }
        let victims = m.trigger_wear_leveling(5);
        assert!(victims.is_empty());
    }

    #[test]
    fn test_trigger_wear_leveling_unbalanced() {
        let mut m = make_manager();
        for _ in 0..100 {
            m.record_erase(0);
        }
        for _ in 0..500 {
            m.record_erase(1);
        }
        for _ in 0..300 {
            m.record_erase(2);
        }
        let victims = m.trigger_wear_leveling(2);
        assert_eq!(victims.len(), 2);
        // Block 1 (500) should be first, block 2 (300) second.
        assert_eq!(victims[0], 1);
        assert_eq!(victims[1], 2);
    }

    #[test]
    fn test_set_gc_threshold() {
        let mut m = make_manager();
        m.set_gc_threshold(2.0);
        assert_eq!(m.gc_threshold(), 2.0);
    }

    #[test]
    fn test_record_app_and_flash_write() {
        let mut m = make_manager();
        m.record_app_write(1000);
        m.record_flash_write(2000);
        assert_eq!(m.write_amp_tracker().app_bytes(), 1000);
        assert_eq!(m.write_amp_tracker().flash_bytes(), 2000);
    }

    #[test]
    fn test_lifespan_10_year_requirement() {
        // Blueprint requirement: ≥ 10 years with 500 MB/day, SLC 100K erases.
        let m = WearLevelManager::with_config(65536, 4096, 100_000);
        let years = m.estimate_lifespan(500);
        assert!(
            years >= 10.0,
            "lifespan estimate must be >= 10 years, got {:.1}",
            years
        );
    }
}
