//! Shared memory region management — Phase 0 stub (v0.20.0).
//!
//! Provides [`grant_shared_mem`], a placeholder for shared memory region
//! allocation. In Phase 0, this always returns a fixed-address region.
//! A real implementation (Phase 1+) would interface with the memory
//! management unit to carve out and map shared pages.

/// A shared memory region descriptor.
///
/// Describes a contiguous physical memory region shared between an owner
/// thread and a consumer thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedMemRegion {
    /// Physical base address of the region.
    pub phys: u64,
    /// Size in bytes.
    pub size: usize,
    /// Owner thread ID (the grantor).
    pub owner: u32,
    /// Consumer thread ID (the grantee).
    pub consumer: u32,
}

/// Grant a shared memory region between `owner` and `consumer`.
///
/// Phase 0 stub: always returns `Some(SharedMemRegion { phys: 0x8000_0000,
/// size, owner, consumer })`. A real implementation would allocate
/// physical pages, set up page-table mappings for both threads, and
/// return the region descriptor.
///
/// Returns `None` only if allocation fails (never in Phase 0).
pub fn grant_shared_mem(owner: u32, consumer: u32, size: usize) -> Option<SharedMemRegion> {
    Some(SharedMemRegion {
        phys: 0x8000_0000,
        size,
        owner,
        consumer,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_shared_mem() {
        let region = grant_shared_mem(1, 2, 4096);
        assert!(region.is_some());

        let r = region.unwrap();
        assert_eq!(
            r.phys, 0x8000_0000,
            "Phase 0 stub returns fixed phys address"
        );
        assert_eq!(r.size, 4096);
        assert_eq!(r.owner, 1);
        assert_eq!(r.consumer, 2);
    }

    #[test]
    fn test_grant_shared_mem_different_sizes() {
        let r1 = grant_shared_mem(10, 20, 256).unwrap();
        assert_eq!(r1.size, 256);

        let r2 = grant_shared_mem(30, 40, 65536).unwrap();
        assert_eq!(r2.size, 65536);
        assert_eq!(r2.owner, 30);
        assert_eq!(r2.consumer, 40);
    }
}
