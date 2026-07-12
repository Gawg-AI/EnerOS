//! Virtual memory region descriptor.

use eneros_hal::MemFlags;

/// Physical backing type for a virtual memory region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backing {
    /// Identity mapping (va == pa).
    Identity,
    /// Mapping to a specific physical address.
    Phys(u64),
    /// Demand paging (allocated on fault).
    Demand,
}

/// A contiguous virtual memory region.
#[derive(Clone, Copy)]
pub struct Vregion {
    /// Starting virtual address.
    pub start_va: u64,
    /// Size in bytes.
    pub size: u64,
    /// Memory protection flags.
    pub flags: MemFlags,
    /// Physical backing type.
    pub backing: Backing,
}

impl Vregion {
    /// Creates a new virtual memory region.
    pub const fn new(start_va: u64, size: u64, flags: MemFlags, backing: Backing) -> Self {
        Self {
            start_va,
            size,
            flags,
            backing,
        }
    }

    /// Returns the end virtual address (exclusive).
    pub const fn end_va(&self) -> u64 {
        self.start_va + self.size
    }

    /// Returns true if `va` falls within this region.
    pub const fn contains(&self, va: u64) -> bool {
        va >= self.start_va && va < self.end_va()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backing_variants() {
        assert_eq!(Backing::Identity, Backing::Identity);
        assert_eq!(Backing::Phys(0x1000), Backing::Phys(0x1000));
        assert_ne!(Backing::Identity, Backing::Demand);
        assert_ne!(Backing::Phys(0), Backing::Phys(1));
    }

    #[test]
    fn test_vregion_new() {
        let v = Vregion::new(0x1000, 0x2000, MemFlags::normal(), Backing::Identity);
        assert_eq!(v.start_va, 0x1000);
        assert_eq!(v.size, 0x2000);
    }

    #[test]
    fn test_vregion_end_va() {
        let v = Vregion::new(0x1000, 0x2000, MemFlags::normal(), Backing::Identity);
        assert_eq!(v.end_va(), 0x3000);
    }

    #[test]
    fn test_vregion_contains() {
        let v = Vregion::new(0x1000, 0x2000, MemFlags::normal(), Backing::Identity);
        assert!(v.contains(0x1000)); // start
        assert!(v.contains(0x2000)); // middle
        assert!(!v.contains(0x3000)); // end (exclusive)
        assert!(!v.contains(0x0FFF)); // before
    }
}
