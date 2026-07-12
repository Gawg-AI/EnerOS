//! Physical memory partition and isolation.
//!
//! A [`Partition`] owns a set of allowed physical address ranges and
//! enforces that all memory accesses fall within those ranges and the
//! partition's quota.

use crate::vspace::{MmError, Vspace};

/// Maximum number of physical address ranges per partition.
const MAX_PHYS_RANGES: usize = 8;

/// A half-open physical address range `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaddrRange {
    pub start: u64,
    pub end: u64,
}

impl PaddrRange {
    /// Creates a new physical address range.
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// Returns true if `pa` is within `[start, end)`.
    pub const fn contains(&self, pa: u64) -> bool {
        pa >= self.start && pa < self.end
    }

    /// Returns true if this range overlaps with `other`.
    pub const fn overlaps(&self, other: &PaddrRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Returns true if this range is empty (start >= end).
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// A memory partition with isolated physical address ranges.
pub struct Partition {
    /// Partition ID.
    pub id: u32,
    /// Human-readable name.
    pub name: &'static str,
    /// The partition's virtual address space.
    pub vspace: Vspace,
    /// Allowed physical address ranges (empty slots have start==0 && end==0).
    pub allowed_phys: [PaddrRange; MAX_PHYS_RANGES],
    /// Memory quota upper limit in bytes.
    pub quota: u64,
    /// Current memory usage in bytes.
    pub used: u64,
}

impl Partition {
    /// Creates a new partition with no physical ranges.
    pub fn new(id: u32, name: &'static str, vspace: Vspace, quota: u64) -> Self {
        Self {
            id,
            name,
            vspace,
            allowed_phys: [PaddrRange::new(0, 0); MAX_PHYS_RANGES],
            quota,
            used: 0,
        }
    }

    /// Adds a physical address range to the partition.
    ///
    /// Returns `Err(MmError::OutOfMemory)` if all slots are full.
    pub fn add_phys_range(&mut self, range: PaddrRange) -> Result<(), MmError> {
        for slot in self.allowed_phys.iter_mut() {
            if slot.is_empty() {
                *slot = range;
                return Ok(());
            }
        }
        Err(MmError::OutOfMemory)
    }

    /// Checks whether `[pa, pa+size)` is within an allowed range and quota.
    pub fn check_access(&self, pa: u64, size: u64) -> Result<(), MmError> {
        let end = pa.checked_add(size).ok_or(MmError::InvalidAddr)?;

        // Check if fully within an allowed range
        let mut found = false;
        for r in self.allowed_phys.iter() {
            if r.is_empty() {
                continue;
            }
            if pa >= r.start && end <= r.end {
                found = true;
                break;
            }
        }
        if !found {
            return Err(MmError::PermissionDenied);
        }

        // Check quota
        if self.used + size > self.quota {
            return Err(MmError::OutOfMemory);
        }

        Ok(())
    }

    /// Returns true if this partition's physical ranges do not overlap
    /// with `other`'s ranges.
    pub fn is_isolated_from(&self, other: &Partition) -> bool {
        for a in self.allowed_phys.iter() {
            if a.is_empty() {
                continue;
            }
            for b in other.allowed_phys.iter() {
                if b.is_empty() {
                    continue;
                }
                if a.overlaps(b) {
                    return false;
                }
            }
        }
        true
    }

    /// Allocates `size` bytes of physical memory (bump allocator).
    ///
    /// Returns the starting physical address, or `Err(MmError::OutOfMemory)`
    /// if quota is exceeded.
    pub fn alloc_phys(&mut self, size: u64) -> Result<u64, MmError> {
        if self.used + size > self.quota {
            return Err(MmError::OutOfMemory);
        }

        // Find the first non-empty range and bump-allocate from it
        for r in self.allowed_phys.iter() {
            if r.is_empty() {
                continue;
            }
            let pa = r.start + self.used;
            self.used += size;
            return Ok(pa);
        }

        Err(MmError::OutOfMemory)
    }

    /// Frees `size` bytes at `pa` (accounting only — no actual reclamation).
    pub fn free_phys(&mut self, _pa: u64, size: u64) {
        if size <= self.used {
            self.used -= size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vspace() -> Vspace {
        Vspace::new(0x40000000, 1)
    }

    #[test]
    fn test_paddr_range_contains() {
        let r = PaddrRange::new(0x1000, 0x2000);
        assert!(r.contains(0x1000)); // start
        assert!(r.contains(0x1500)); // middle
        assert!(!r.contains(0x2000)); // end (exclusive)
        assert!(!r.contains(0x0FFF)); // before
    }

    #[test]
    fn test_paddr_range_overlaps() {
        let a = PaddrRange::new(0x1000, 0x2000);
        assert!(a.overlaps(&PaddrRange::new(0x1500, 0x2500))); // partial overlap
        assert!(a.overlaps(&PaddrRange::new(0x1000, 0x2000))); // identical
        assert!(!a.overlaps(&PaddrRange::new(0x2000, 0x3000))); // adjacent (no overlap)
        assert!(!a.overlaps(&PaddrRange::new(0x0000, 0x1000))); // adjacent before
    }

    #[test]
    fn test_paddr_range_is_empty() {
        assert!(PaddrRange::new(0, 0).is_empty());
        assert!(PaddrRange::new(0x1000, 0x1000).is_empty());
        assert!(!PaddrRange::new(0x1000, 0x2000).is_empty());
    }

    #[test]
    fn test_partition_new() {
        let p = Partition::new(1, "test", make_vspace(), 0x100000);
        assert_eq!(p.id, 1);
        assert_eq!(p.name, "test");
        assert_eq!(p.quota, 0x100000);
        assert_eq!(p.used, 0);
        for r in &p.allowed_phys {
            assert!(r.is_empty());
        }
    }

    #[test]
    fn test_add_phys_range() {
        let mut p = Partition::new(1, "test", make_vspace(), 0x100000);
        assert!(p.add_phys_range(PaddrRange::new(0x1000, 0x2000)).is_ok());
        assert!(p.add_phys_range(PaddrRange::new(0x3000, 0x4000)).is_ok());
        assert_eq!(p.allowed_phys[0], PaddrRange::new(0x1000, 0x2000));
        assert_eq!(p.allowed_phys[1], PaddrRange::new(0x3000, 0x4000));
    }

    #[test]
    fn test_check_access_allowed() {
        let mut p = Partition::new(1, "test", make_vspace(), 0x100000);
        p.add_phys_range(PaddrRange::new(0x1000, 0x2000)).unwrap();
        assert!(p.check_access(0x1000, 0x1000).is_ok()); // full range
        assert!(p.check_access(0x1500, 0x500).is_ok()); // sub-range
    }

    #[test]
    fn test_check_access_denied() {
        let mut p = Partition::new(1, "test", make_vspace(), 0x100000);
        p.add_phys_range(PaddrRange::new(0x1000, 0x2000)).unwrap();
        assert_eq!(
            p.check_access(0x2000, 0x1000),
            Err(MmError::PermissionDenied)
        );
        assert_eq!(p.check_access(0x0FFF, 0x10), Err(MmError::PermissionDenied));
        assert_eq!(
            p.check_access(0x90000000, 0x1000),
            Err(MmError::PermissionDenied)
        );
    }

    #[test]
    fn test_check_access_quota_exceeded() {
        let mut p = Partition::new(1, "test", make_vspace(), 0x1000);
        p.add_phys_range(PaddrRange::new(0x1000, 0x10000)).unwrap();
        // quota is 0x1000, request 0x2000
        assert_eq!(p.check_access(0x1000, 0x2000), Err(MmError::OutOfMemory));
    }

    #[test]
    fn test_is_isolated_true() {
        let mut a = Partition::new(1, "A", make_vspace(), 0x100000);
        a.add_phys_range(PaddrRange::new(0x1000, 0x2000)).unwrap();

        let mut b = Partition::new(2, "B", make_vspace(), 0x100000);
        b.add_phys_range(PaddrRange::new(0x3000, 0x4000)).unwrap();

        assert!(a.is_isolated_from(&b));
        assert!(b.is_isolated_from(&a));
    }

    #[test]
    fn test_is_isolated_false() {
        let mut a = Partition::new(1, "A", make_vspace(), 0x100000);
        a.add_phys_range(PaddrRange::new(0x1000, 0x3000)).unwrap();

        let mut b = Partition::new(2, "B", make_vspace(), 0x100000);
        b.add_phys_range(PaddrRange::new(0x2000, 0x4000)).unwrap();

        assert!(!a.is_isolated_from(&b));
        assert!(!b.is_isolated_from(&a));
    }

    #[test]
    fn test_alloc_phys() {
        let mut p = Partition::new(1, "test", make_vspace(), 0x10000);
        p.add_phys_range(PaddrRange::new(0x1000, 0x10000)).unwrap();

        let pa1 = p.alloc_phys(0x1000).unwrap();
        assert_eq!(pa1, 0x1000);
        assert_eq!(p.used, 0x1000);

        let pa2 = p.alloc_phys(0x2000).unwrap();
        assert_eq!(pa2, 0x2000);
        assert_eq!(p.used, 0x3000);
    }

    #[test]
    fn test_alloc_phys_quota_exceeded() {
        let mut p = Partition::new(1, "test", make_vspace(), 0x1000);
        p.add_phys_range(PaddrRange::new(0x1000, 0x10000)).unwrap();

        assert_eq!(p.alloc_phys(0x2000), Err(MmError::OutOfMemory));
    }

    #[test]
    fn test_free_phys() {
        let mut p = Partition::new(1, "test", make_vspace(), 0x10000);
        p.add_phys_range(PaddrRange::new(0x1000, 0x10000)).unwrap();

        p.alloc_phys(0x3000).unwrap();
        assert_eq!(p.used, 0x3000);

        p.free_phys(0x1000, 0x1000);
        assert_eq!(p.used, 0x2000);
    }
}
