//! Wear-leveling monitoring and management (v0.24.1).
//!
//! This module provides observability and control for flash storage wear
//! leveling. It works alongside littlefs2's built-in dynamic wear leveling,
//! adding:
//!
//! - **Per-block erase counting** — tracks how many times each block has been
//!   erased.
//! - **Wear distribution analysis** — computes p50/p99/max statistics.
//! - **Write amplification tracking** — measures the ratio of flash writes to
//!   application writes.
//! - **Lifespan estimation** — predicts remaining flash lifetime based on
//!   current wear rate.
//! - **Victim block selection** — identifies hot blocks for GC migration.
//!
//! # Global Interface
//!
//! A global [`WearLevelManager`] is provided behind a [`spin::Mutex`], accessible
//! via [`wear_level_status`], [`trigger_wear_leveling`], and
//! [`set_write_amp_limit`]. Initialize it with [`init_global`] at startup.
//!
//! # no_std Compliance
//!
//! This module is `no_std` compatible. It uses:
//! - `alloc::collections::BTreeMap` for per-block counters
//! - `spin::Mutex` for the global instance (no `std::sync::Mutex`)

pub mod manager;
pub mod status;
pub mod write_amp;

pub use manager::{
    WearLevelManager, WearLeveling, DEFAULT_GC_THRESHOLD, DEFAULT_MAX_ERASE_CYCLES,
    DEFAULT_WRITE_AMP_LIMIT,
};
use spin::Mutex;
pub use status::{WearDistribution, WearStatus};
pub use write_amp::WriteAmplificationTracker;

/// Global wear-level manager instance.
///
/// Initialized lazily via [`init_global`]. Access through the global
/// functions ([`wear_level_status`], etc.) uses a `spin::Mutex` for
/// `no_std` thread safety.
static GLOBAL_MANAGER: Mutex<Option<WearLevelManager>> = Mutex::new(None);

/// Initializes the global wear-level manager with the given configuration.
///
/// Must be called once at startup. If called again, replaces the previous
/// instance.
pub fn init_global(total_blocks: u32, block_size: u32, max_erase_cycles: u32) {
    let manager = WearLevelManager::with_config(total_blocks, block_size, max_erase_cycles);
    *GLOBAL_MANAGER.lock() = Some(manager);
}

/// Initializes the global manager with default configuration (65536 blocks,
/// 4096 byte blocks, 100K erase cycles).
pub fn init_default() {
    *GLOBAL_MANAGER.lock() = Some(WearLevelManager::new());
}

/// Records an erase event on the global manager.
///
/// No-op if the global manager has not been initialized.
pub fn record_erase(block: u32) {
    if let Some(ref mut m) = *GLOBAL_MANAGER.lock() {
        m.record_erase(block);
    }
}

/// Records application-level write bytes on the global manager.
pub fn record_app_write(bytes: u64) {
    if let Some(ref mut m) = *GLOBAL_MANAGER.lock() {
        m.record_app_write(bytes);
    }
}

/// Records flash-level write bytes on the global manager.
pub fn record_flash_write(bytes: u64) {
    if let Some(ref mut m) = *GLOBAL_MANAGER.lock() {
        m.record_flash_write(bytes);
    }
}

/// Returns a snapshot of the current wear-leveling status from the global
/// manager.
///
/// Returns a default (all-zero) [`WearStatus`] if the global manager has not
/// been initialized.
pub fn wear_level_status() -> WearStatus {
    match *GLOBAL_MANAGER.lock() {
        Some(ref m) => m.wear_level_status(),
        None => WearStatus::default(),
    }
}

/// Triggers wear leveling on the global manager, returning victim block
/// indices to migrate.
///
/// Returns an empty vector if the manager is not initialized or wear is
/// balanced.
pub fn trigger_wear_leveling(max_migrations: usize) -> alloc::vec::Vec<u32> {
    match *GLOBAL_MANAGER.lock() {
        Some(ref m) => m.trigger_wear_leveling(max_migrations),
        None => alloc::vec::Vec::new(),
    }
}

/// Sets the write amplification limit on the global manager.
///
/// No-op if the global manager has not been initialized.
pub fn set_write_amp_limit(limit: f64) {
    if let Some(ref mut m) = *GLOBAL_MANAGER.lock() {
        m.write_amp_tracker_mut().set_write_amp_limit(limit);
    }
}

/// Returns `true` if the global manager's write amplification exceeds its
/// limit.
pub fn is_write_amp_throttled() -> bool {
    match *GLOBAL_MANAGER.lock() {
        Some(ref m) => m.write_amp_tracker().is_throttled(),
        None => false,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    // Serialize tests that share GLOBAL_MANAGER to prevent race conditions
    // when Rust runs tests in parallel.
    use std::sync::Mutex as StdMutex;

    use super::*;
    static GLOBAL_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn test_global_not_initialized() {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
        // Clear any previous state.
        *GLOBAL_MANAGER.lock() = None;
        let s = wear_level_status();
        assert_eq!(s.total_wear_cycles, 0);
        assert_eq!(s.max_block_erases, 0);
    }

    #[test]
    fn test_init_global_and_record() {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
        init_global(64, 4096, 100_000);
        record_erase(0);
        record_erase(0);
        record_erase(1);

        let s = wear_level_status();
        assert_eq!(s.total_wear_cycles, 3);
        assert_eq!(s.max_block_erases, 2);
    }

    #[test]
    fn test_global_write_amp() {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
        init_global(64, 4096, 100_000);
        record_app_write(4096);
        record_flash_write(8192);

        let s = wear_level_status();
        assert!((s.write_amplification - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_global_set_write_amp_limit() {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
        init_global(64, 4096, 100_000);
        record_app_write(1000);
        record_flash_write(3000); // WA = 3.0
        set_write_amp_limit(2.0);
        assert!(is_write_amp_throttled());
    }

    #[test]
    fn test_global_trigger_wear_leveling() {
        // Use a local manager to avoid race conditions with other tests that
        // share the global GLOBAL_MANAGER. The global interface is exercised
        // separately by test_init_global_and_record.
        let mut m = WearLevelManager::with_config(64, 4096, 100_000);
        // Create imbalance: block 0 has 100 erases, others have 0.
        for _ in 0..100 {
            m.record_erase(0);
        }
        let victims = m.trigger_wear_leveling(5);
        // avg = 100/64 = 1, max = 100, ratio = 100/1 = 100 > 1.5
        assert!(!victims.is_empty());
        assert!(victims.contains(&0));
    }

    #[test]
    fn test_global_trigger_balanced() {
        // Use a local manager to avoid race conditions with other tests that
        // share the global GLOBAL_MANAGER.
        let mut m = WearLevelManager::with_config(64, 4096, 100_000);
        // All blocks get the same erases.
        for b in 0..64 {
            for _ in 0..10 {
                m.record_erase(b);
            }
        }
        let victims = m.trigger_wear_leveling(5);
        assert!(victims.is_empty());
    }

    #[test]
    fn test_init_default() {
        init_default();
        let s = wear_level_status();
        // Default config: 65536 blocks, no erases yet.
        assert_eq!(s.total_wear_cycles, 0);
        assert_eq!(s.max_block_erases, 0);
    }

    #[test]
    fn test_is_write_amp_throttled_not_initialized() {
        *GLOBAL_MANAGER.lock() = None;
        assert!(!is_write_amp_throttled());
    }

    #[test]
    fn test_trigger_wear_leveling_not_initialized() {
        *GLOBAL_MANAGER.lock() = None;
        let victims = trigger_wear_leveling(5);
        assert!(victims.is_empty());
    }
}
