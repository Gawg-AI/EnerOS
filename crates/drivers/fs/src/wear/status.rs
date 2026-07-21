//! Wear-leveling status reporting types.
//!
//! [`WearStatus`] and [`WearDistribution`] provide a snapshot of the
//! flash storage's wear-leveling health. These are populated by the
//! [`WearLevelManager`](crate::wear::WearLevelManager) from per-block
//! erase counters and the [`WriteAmplificationTracker`](crate::wear::WriteAmplificationTracker).

use core::fmt;

// ============================================================================
// WearDistribution
// ============================================================================

/// Statistical distribution of per-block erase counts.
///
/// Computed from the raw per-block counters via [`WearDistribution::from_counts`].
/// A healthy filesystem has a low `max_erases / avg` ratio (ideally < 1.5×),
/// indicating that wear is spread evenly across blocks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WearDistribution {
    /// 50th percentile (median) erase count.
    pub p50: u32,
    /// 99th percentile erase count — the level below which 99% of blocks sit.
    pub p99: u32,
    /// Maximum erase count across all blocks.
    pub max_erases: u32,
}

impl WearDistribution {
    /// Computes the distribution from a slice of per-block erase counts.
    ///
    /// The input does not need to be sorted; this function sorts a copy.
    /// An empty input yields all-zero percentiles.
    pub fn from_counts(counts: &[u32]) -> Self {
        if counts.is_empty() {
            return Self::default();
        }
        let mut sorted: alloc::vec::Vec<u32> = alloc::vec::Vec::from_iter(counts.iter().copied());
        sorted.sort_unstable();
        let len = sorted.len();
        let max_erases = sorted[len - 1];
        // p50: median element.
        let p50 = sorted[len / 2];
        // p99: index at the 99th percentile (clamped to last element).
        let p99_idx = ((len as u64 * 99) / 100) as usize;
        let p99_idx = p99_idx.min(len - 1);
        let p99 = sorted[p99_idx];
        Self {
            p50,
            p99,
            max_erases,
        }
    }

    /// Returns the wear-leveling balance ratio: `max_erases / p50`.
    ///
    /// A value close to 1.0 means wear is evenly distributed. Values above
    /// 1.5 indicate poor wear leveling and may trigger GC.
    ///
    /// Returns `0.0` if `p50` is zero (no erases recorded).
    pub fn balance_ratio(&self) -> f64 {
        if self.p50 == 0 {
            return 0.0;
        }
        self.max_erases as f64 / self.p50 as f64
    }
}

impl fmt::Display for WearDistribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "p50={}, p99={}, max={}",
            self.p50, self.p99, self.max_erases
        )
    }
}

// ============================================================================
// WearStatus
// ============================================================================

/// A snapshot of the storage subsystem's wear-leveling health.
///
/// Returned by [`WearLevelManager::wear_level_status`](crate::wear::WearLevelManager::wear_level_status)
/// and the global [`wear_level_status`](crate::wear::wear_level_status) function.
#[derive(Clone, Debug, Default)]
pub struct WearStatus {
    /// Sum of erase counts across all blocks.
    pub total_wear_cycles: u64,
    /// Maximum per-block erase count.
    pub max_block_erases: u32,
    /// Average erase count across all blocks (total / block_count).
    pub avg_block_erases: u32,
    /// Statistical distribution of erase counts.
    pub wear_distribution: WearDistribution,
    /// Current write amplification factor (flash_bytes / app_bytes).
    /// 0.0 if no writes have been recorded.
    pub write_amplification: f64,
    /// Estimated remaining lifespan in years, based on current wear rate.
    /// `0.0` if insufficient data.
    pub estimated_lifespan_years: f64,
}

impl WearStatus {
    /// Returns `true` if the wear-leveling balance is within the healthy
    /// threshold (`max / avg < 1.5`).
    pub fn is_balanced(&self) -> bool {
        if self.avg_block_erases == 0 {
            return true;
        }
        let ratio = self.max_block_erases as f64 / self.avg_block_erases as f64;
        ratio < 1.5
    }

    /// Returns `true` if the write amplification is within the acceptable
    /// limit (< 2.0).
    pub fn is_write_amp_healthy(&self) -> bool {
        self.write_amplification < 2.0
    }
}

impl fmt::Display for WearStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "wear: total={}, max={}, avg={}, wa={:.2}, life={:.1}y, {}",
            self.total_wear_cycles,
            self.max_block_erases,
            self.avg_block_erases,
            self.write_amplification,
            self.estimated_lifespan_years,
            self.wear_distribution
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_wear_distribution_empty() {
        let d = WearDistribution::from_counts(&[]);
        assert_eq!(d.p50, 0);
        assert_eq!(d.p99, 0);
        assert_eq!(d.max_erases, 0);
    }

    #[test]
    fn test_wear_distribution_single() {
        let d = WearDistribution::from_counts(&[42]);
        assert_eq!(d.p50, 42);
        assert_eq!(d.p99, 42);
        assert_eq!(d.max_erases, 42);
    }

    #[test]
    fn test_wear_distribution_uniform() {
        // All blocks have the same erase count.
        let counts = [100u32; 64];
        let d = WearDistribution::from_counts(&counts);
        assert_eq!(d.p50, 100);
        assert_eq!(d.p99, 100);
        assert_eq!(d.max_erases, 100);
        // Perfectly balanced: ratio = 1.0
        let ratio = d.balance_ratio();
        assert!((ratio - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_wear_distribution_skewed() {
        // 63 blocks at 100, 1 block at 1000.
        // With 64 elements, p99_idx = (64*99)/100 = 63 → sorted[63] = 1000.
        // So p99 = 1000 (the 99th percentile falls on the outlier).
        let mut counts = [100u32; 64];
        counts[63] = 1000;
        let d = WearDistribution::from_counts(&counts);
        assert_eq!(d.p50, 100);
        assert_eq!(d.max_erases, 1000);
        assert_eq!(d.p99, 1000);
        // Balance ratio = 1000/100 = 10.0 (poorly balanced).
        let ratio = d.balance_ratio();
        assert!((ratio - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_wear_distribution_skewed_large() {
        // 199 blocks at 100, 1 block at 1000 (200 total).
        // p99_idx = (200*99)/100 = 198 → sorted[198] = 100.
        // p99 = 100 (99% of blocks are at 100).
        let mut counts = [100u32; 200];
        counts[199] = 1000;
        let d = WearDistribution::from_counts(&counts);
        assert_eq!(d.p50, 100);
        assert_eq!(d.max_erases, 1000);
        assert_eq!(d.p99, 100);
    }

    #[test]
    fn test_wear_distribution_unsorted_input() {
        let counts = [300, 100, 200, 50, 150];
        let d = WearDistribution::from_counts(&counts);
        // Sorted: [50, 100, 150, 200, 300]
        assert_eq!(d.p50, 150); // median (index 2)
        assert_eq!(d.max_erases, 300);
    }

    #[test]
    fn test_wear_distribution_large_set() {
        // 100 blocks, erase counts 0..99.
        let counts: Vec<u32> = (0..100u32).collect();
        let d = WearDistribution::from_counts(&counts);
        assert_eq!(d.p50, 50); // index 50
        assert_eq!(d.max_erases, 99);
        // p99: index = (100 * 99) / 100 = 99 → sorted[99] = 99
        assert_eq!(d.p99, 99);
    }

    #[test]
    fn test_balance_ratio_zero_p50() {
        let d = WearDistribution {
            p50: 0,
            p99: 0,
            max_erases: 100,
        };
        assert_eq!(d.balance_ratio(), 0.0);
    }

    #[test]
    fn test_wear_status_default() {
        let s = WearStatus::default();
        assert_eq!(s.total_wear_cycles, 0);
        assert_eq!(s.max_block_erases, 0);
        assert_eq!(s.avg_block_erases, 0);
        assert_eq!(s.write_amplification, 0.0);
        assert_eq!(s.estimated_lifespan_years, 0.0);
    }

    #[test]
    fn test_wear_status_is_balanced_no_wear() {
        let s = WearStatus::default();
        assert!(s.is_balanced());
    }

    #[test]
    fn test_wear_status_is_balanced_healthy() {
        let s = WearStatus {
            total_wear_cycles: 6400,
            max_block_erases: 120,
            avg_block_erases: 100,
            wear_distribution: WearDistribution {
                p50: 100,
                p99: 120,
                max_erases: 120,
            },
            write_amplification: 1.2,
            estimated_lifespan_years: 15.0,
        };
        // ratio = 120/100 = 1.2 < 1.5 → balanced
        assert!(s.is_balanced());
        assert!(s.is_write_amp_healthy());
    }

    #[test]
    fn test_wear_status_is_balanced_unhealthy() {
        let s = WearStatus {
            total_wear_cycles: 6400,
            max_block_erases: 500,
            avg_block_erases: 100,
            wear_distribution: WearDistribution {
                p50: 100,
                p99: 500,
                max_erases: 500,
            },
            write_amplification: 2.5,
            estimated_lifespan_years: 5.0,
        };
        // ratio = 500/100 = 5.0 > 1.5 → not balanced
        assert!(!s.is_balanced());
        assert!(!s.is_write_amp_healthy());
    }

    #[test]
    fn test_wear_status_display() {
        let s = WearStatus {
            total_wear_cycles: 1000,
            max_block_erases: 50,
            avg_block_erases: 30,
            wear_distribution: WearDistribution {
                p50: 30,
                p99: 50,
                max_erases: 50,
            },
            write_amplification: 1.5,
            estimated_lifespan_years: 10.0,
        };
        let s_str = format!("{}", s);
        assert!(s_str.contains("total=1000"));
        assert!(s_str.contains("max=50"));
        assert!(s_str.contains("wa=1.50"));
        assert!(s_str.contains("life=10.0y"));
    }

    #[test]
    fn test_wear_distribution_display() {
        let d = WearDistribution {
            p50: 100,
            p99: 200,
            max_erases: 300,
        };
        let s = format!("{}", d);
        assert!(s.contains("p50=100"));
        assert!(s.contains("p99=200"));
        assert!(s.contains("max=300"));
    }
}
