//! Heap statistics and fragmentation metrics.
//!
//! [`HeapStats`] tracks allocation counters and a fragmentation ratio
//! (0–1000 permille) computed from the largest free block versus total
//! free bytes.

/// Heap statistics tracked by the kernel allocator.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeapStats {
    /// Total heap capacity in bytes.
    pub total_bytes: u64,
    /// Currently allocated bytes.
    pub allocated_bytes: u64,
    /// Currently free bytes.
    pub free_bytes: u64,
    /// Fragmentation ratio in permille (0–1000).
    pub fragmentation_ratio: u32,
    /// Total number of `alloc` calls.
    pub alloc_count: u64,
    /// Total number of `dealloc` calls.
    pub free_count: u64,
    /// Number of allocations served by the slab allocator.
    pub slab_hits: u64,
    /// Number of allocations served by the buddy allocator.
    pub buddy_hits: u64,
}

impl HeapStats {
    /// Computes the fragmentation ratio in permille.
    ///
    /// Formula: `(free_bytes - largest_free_block) * 1000 / free_bytes`.
    /// Returns `0` when `free_bytes == 0` (no free memory → no fragmentation).
    pub fn compute_fragmentation(free_bytes: u64, largest_free_block: u64) -> u32 {
        if free_bytes == 0 {
            return 0;
        }
        // largest_free_block must not exceed free_bytes; clamp defensively.
        let largest = if largest_free_block > free_bytes {
            free_bytes
        } else {
            largest_free_block
        };
        let fragmented = free_bytes - largest;
        ((fragmented * 1000) / free_bytes) as u32
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_default_all_zero() {
        let s = HeapStats::default();
        assert_eq!(s.total_bytes, 0);
        assert_eq!(s.allocated_bytes, 0);
        assert_eq!(s.free_bytes, 0);
        assert_eq!(s.fragmentation_ratio, 0);
        assert_eq!(s.alloc_count, 0);
        assert_eq!(s.free_count, 0);
        assert_eq!(s.slab_hits, 0);
        assert_eq!(s.buddy_hits, 0);
    }

    #[test]
    fn test_field_assignment() {
        let s = HeapStats {
            total_bytes: 4096,
            allocated_bytes: 1024,
            free_bytes: 3072,
            fragmentation_ratio: 500,
            alloc_count: 10,
            free_count: 5,
            slab_hits: 8,
            buddy_hits: 2,
        };
        assert_eq!(s.total_bytes, 4096);
        assert_eq!(s.allocated_bytes, 1024);
        assert_eq!(s.free_bytes, 3072);
        assert_eq!(s.fragmentation_ratio, 500);
        assert_eq!(s.alloc_count, 10);
        assert_eq!(s.free_count, 5);
        assert_eq!(s.slab_hits, 8);
        assert_eq!(s.buddy_hits, 2);
    }

    #[test]
    fn test_compute_fragmentation_zero_free() {
        // free_bytes == 0 → 0
        assert_eq!(HeapStats::compute_fragmentation(0, 0), 0);
        assert_eq!(HeapStats::compute_fragmentation(0, 1024), 0);
    }

    #[test]
    fn test_compute_fragmentation_no_fragmentation() {
        // largest == free → 0 fragmentation
        assert_eq!(HeapStats::compute_fragmentation(4096, 4096), 0);
        assert_eq!(HeapStats::compute_fragmentation(8192, 8192), 0);
    }

    #[test]
    fn test_compute_fragmentation_full() {
        // largest == 0 → 1000 permille (fully fragmented)
        assert_eq!(HeapStats::compute_fragmentation(4096, 0), 1000);
    }

    #[test]
    fn test_compute_fragmentation_half() {
        // free=4096, largest=2048 → (4096-2048)*1000/4096 = 500
        assert_eq!(HeapStats::compute_fragmentation(4096, 2048), 500);
    }

    #[test]
    fn test_compute_fragmentation_clamp() {
        // largest > free → clamped to free, ratio = 0
        assert_eq!(HeapStats::compute_fragmentation(1000, 2000), 0);
    }

    #[test]
    fn test_compute_fragmentation_partial() {
        // free=10000, largest=2500 → (10000-2500)*1000/10000 = 750
        assert_eq!(HeapStats::compute_fragmentation(10000, 2500), 750);
    }

    #[test]
    fn test_copy_clone() {
        let s = HeapStats {
            total_bytes: 100,
            alloc_count: 5,
            ..Default::default()
        };
        let s2 = s; // Copy
        let s3 = s; // Copy (Clone is not needed since HeapStats is Copy)
        assert_eq!(s.total_bytes, s2.total_bytes);
        assert_eq!(s.total_bytes, s3.total_bytes);
        assert_eq!(s2.alloc_count, s3.alloc_count);
    }
}
