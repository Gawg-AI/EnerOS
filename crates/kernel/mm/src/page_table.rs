//! ARM64 four-level page table implementation.
//!
//! Implements the ARMv8-A page table structure with 48-bit virtual
//! addresses and 4KB granule size. Each page table has 512 entries
//! (4KB / 8 bytes per entry).

use eneros_hal::MemFlags;

/// Page size in bytes (4KB).
pub const PAGE_SIZE: u64 = 4096;

/// Number of entries per page table (4KB / 8B = 512).
pub const TABLE_ENTRIES: usize = 512;

// PTE bit flags
/// Valid bit — entry is valid.
pub const PTE_VALID: u64 = 1 << 0;
/// Table bit — entry points to a next-level page table (non-leaf).
pub const PTE_TABLE: u64 = 1 << 1;
/// Access Flag — must be set to avoid faults.
pub const PTE_AF: u64 = 1 << 10;
/// Inner Shareable.
pub const PTE_SH_INNER: u64 = 3 << 8;
/// Privileged Execute Never.
pub const PTE_PXN: u64 = 1 << 53;
/// Execute Never (all ELs).
pub const PTE_XN: u64 = 1 << 54;
/// Normal memory attribute index (MAIR[0]).
pub const MT_NORMAL: u64 = 0 << 2;
/// Device memory attribute index (MAIR[1]).
pub const MT_DEVICE: u64 = 1 << 2;

/// Mask to extract the physical address from a PTE (bits 47:12, 48-bit PA).
pub const PTE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

/// A 64-bit page table entry.
#[derive(Clone, Copy)]
pub struct Pte(pub u64);

/// Page table level in the four-level hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLevel {
    /// Level 0 — PGD (Page Global Directory).
    L0,
    /// Level 1 — PUD (Page Upper Directory).
    L1,
    /// Level 2 — PMD (Page Middle Directory).
    L2,
    /// Level 3 — PTE (leaf, 4KB page).
    L3,
}

impl PageLevel {
    /// Returns the numeric level (0-3).
    pub const fn as_u8(self) -> u8 {
        match self {
            PageLevel::L0 => 0,
            PageLevel::L1 => 1,
            PageLevel::L2 => 2,
            PageLevel::L3 => 3,
        }
    }
}

/// A page table containing 512 entries.
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; TABLE_ENTRIES],
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PageTable {
    /// Creates a new zeroed page table.
    pub const fn new() -> Self {
        Self {
            entries: [0; TABLE_ENTRIES],
        }
    }

    /// Extracts the 9-bit index for `level` from virtual address `va`.
    ///
    /// L0: bits[47:39], L1: bits[38:30], L2: bits[29:21], L3: bits[20:12]
    pub fn index(level: PageLevel, va: u64) -> usize {
        let shift = 39 - (level.as_u8() as u64) * 9;
        ((va >> shift) & 0x1FF) as usize
    }

    /// Constructs a leaf PTE (L3, 4KB page) mapping `pa` with `flags`.
    pub fn make_leaf(pa: u64, flags: MemFlags) -> u64 {
        let mut pte = (pa & !0xFFF) | PTE_VALID | PTE_AF | PTE_SH_INNER;
        let mt = if flags.device { MT_DEVICE } else { MT_NORMAL };
        pte |= mt;
        if !flags.executable {
            pte |= PTE_XN;
        }
        if !flags.writable {
            pte |= PTE_PXN;
        }
        pte
    }

    /// Constructs a table PTE (L0-L2, pointing to a child page table).
    pub fn make_table(child_pa: u64) -> u64 {
        (child_pa & !0xFFF) | PTE_VALID | PTE_TABLE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_size() {
        assert_eq!(PAGE_SIZE, 4096);
    }

    #[test]
    fn test_table_entries() {
        assert_eq!(TABLE_ENTRIES, 512);
    }

    #[test]
    fn test_pte_flags() {
        assert_eq!(PTE_VALID, 1);
        assert_eq!(PTE_TABLE, 2);
        assert_eq!(PTE_AF, 1024);
        assert_eq!(PTE_SH_INNER, 3 << 8);
        assert_eq!(PTE_PXN, 1u64 << 53);
        assert_eq!(PTE_XN, 1u64 << 54);
        assert_eq!(MT_NORMAL, 0);
        assert_eq!(MT_DEVICE, 4);
    }

    #[test]
    fn test_page_level_as_u8() {
        assert_eq!(PageLevel::L0.as_u8(), 0);
        assert_eq!(PageLevel::L1.as_u8(), 1);
        assert_eq!(PageLevel::L2.as_u8(), 2);
        assert_eq!(PageLevel::L3.as_u8(), 3);
    }

    #[test]
    fn test_index_l3() {
        // VA 0x1000 should have L3 index 1
        let idx = PageTable::index(PageLevel::L3, 0x1000);
        assert_eq!(idx, 1);
    }

    #[test]
    fn test_index_l3_zero() {
        let idx = PageTable::index(PageLevel::L3, 0);
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_index_l0() {
        // VA with bit 39 set should have L0 index 1
        let va = 1u64 << 39;
        let idx = PageTable::index(PageLevel::L0, va);
        assert_eq!(idx, 1);
    }

    #[test]
    fn test_make_leaf_normal() {
        let flags = MemFlags::normal();
        let pte = PageTable::make_leaf(0x40000000, flags);
        assert!(pte & PTE_VALID != 0);
        assert!(pte & PTE_AF != 0);
        assert!(pte & PTE_SH_INNER != 0);
        assert!(pte & PTE_XN != 0); // not executable
        assert!(pte & PTE_PXN == 0); // writable
        assert!(pte & PTE_ADDR_MASK == 0x40000000); // pa preserved
    }

    #[test]
    fn test_make_leaf_device() {
        let flags = MemFlags::device();
        let pte = PageTable::make_leaf(0x09000000, flags);
        assert!(pte & MT_DEVICE != 0);
        assert!(pte & PTE_XN != 0);
        assert!(pte & PTE_PXN == 0); // writable
    }

    #[test]
    fn test_make_leaf_code() {
        let flags = MemFlags::code();
        let pte = PageTable::make_leaf(0x80000, flags);
        assert!(pte & PTE_XN == 0); // executable
        assert!(pte & PTE_PXN != 0); // not writable -> PXN set
    }

    #[test]
    fn test_make_table() {
        let pte = PageTable::make_table(0x40001000);
        assert!(pte & PTE_VALID != 0);
        assert!(pte & PTE_TABLE != 0);
        assert!(pte & PTE_ADDR_MASK == 0x40001000);
    }

    #[test]
    fn test_page_table_new() {
        let pt = PageTable::new();
        for i in 0..TABLE_ENTRIES {
            assert_eq!(pt.entries[i], 0);
        }
    }
}
