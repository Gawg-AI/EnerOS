//! Virtual address space and AddressSpace trait.

use core::fmt;
use core::ptr;

use eneros_hal::MemFlags;

use crate::page_table::{PageLevel, PageTable, PAGE_SIZE, PTE_ADDR_MASK, PTE_VALID};
use crate::vregion::Vregion;

/// Memory management error codes.
#[derive(Debug, PartialEq, Eq)]
pub enum MmError {
    /// Invalid address.
    InvalidAddr,
    /// Virtual address not mapped.
    NotMapped,
    /// Virtual address already mapped.
    AlreadyMapped,
    /// Out of page table pages.
    OutOfMemory,
    /// Address not page-aligned.
    Misaligned,
    /// Permission denied — cross-partition access rejected.
    PermissionDenied,
}

impl fmt::Display for MmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MmError::InvalidAddr => write!(f, "invalid address"),
            MmError::NotMapped => write!(f, "not mapped"),
            MmError::AlreadyMapped => write!(f, "already mapped"),
            MmError::OutOfMemory => write!(f, "out of memory (page table pool exhausted)"),
            MmError::Misaligned => write!(f, "address not page-aligned"),
            MmError::PermissionDenied => write!(f, "permission denied (cross-partition access)"),
        }
    }
}

/// Abstraction over virtual memory management.
pub trait AddressSpace {
    /// Map `size` bytes from `pa` to `va` with `flags`.
    fn map(&mut self, va: u64, pa: u64, size: u64, flags: MemFlags) -> Result<(), MmError>;
    /// Unmap `size` bytes starting at `va`.
    fn unmap(&mut self, va: u64, size: u64) -> Result<(), MmError>;
    /// Translate `va` to its physical address, or None if not mapped.
    fn translate(&self, va: u64) -> Option<u64>;
    /// Update protection flags for the page at `va`.
    fn set_flags(&mut self, va: u64, flags: MemFlags) -> Result<(), MmError>;
}

/// Maximum number of regions tracked per address space.
const MAX_REGIONS: usize = 16;

/// Static page table pool size.
const PT_POOL_SIZE: usize = 64;

/// Static page table pool for intermediate tables.
#[allow(static_mut_refs)]
static mut PAGE_TABLE_POOL: [PageTable; PT_POOL_SIZE] = [const { PageTable::new() }; PT_POOL_SIZE];

/// Allocation counter for the page table pool.
static mut PT_POOL_NEXT: usize = 0;

/// Allocates a page table from the static pool.
///
/// Returns the physical address of the allocated table, or None if exhausted.
/// In a real system this would be the physical address; here we use the
/// kernel virtual address of the static array as a stand-in.
#[allow(static_mut_refs)]
fn alloc_page_table() -> Option<u64> {
    unsafe {
        if PT_POOL_NEXT >= PT_POOL_SIZE {
            return None;
        }
        let idx = PT_POOL_NEXT;
        PT_POOL_NEXT += 1;
        let pt: &mut PageTable = &mut PAGE_TABLE_POOL[idx];
        // Zero out the table
        pt.entries.fill(0);
        Some(pt as *mut PageTable as u64)
    }
}

/// A virtual address space backed by a four-level page table.
pub struct Vspace {
    /// Physical address of the L0 (root) page table.
    pub root_paddr: u64,
    /// Address Space ID for TLB management.
    pub asid: u16,
    /// Tracked memory regions.
    pub regions: [Option<Vregion>; MAX_REGIONS],
}

impl Vspace {
    /// Creates a new virtual address space.
    pub const fn new(root_paddr: u64, asid: u16) -> Self {
        Self {
            root_paddr,
            asid,
            regions: [None; MAX_REGIONS],
        }
    }

    /// Flushes TLB entries for this address space (ASID-based).
    #[cfg(target_arch = "aarch64")]
    fn flush_tlb(&self) {
        unsafe {
            core::arch::asm!(
                "tlbi aside1, {0}",
                in(reg) (self.asid as u64) << 48,
            );
        }
    }

    #[cfg(not(target_arch = "aarch64"))]
    fn flush_tlb(&self) {
        // No-op on host (tests don't touch real hardware)
    }

    /// Reads a PTE at the given table address and index.
    unsafe fn read_pte(table_addr: u64, index: usize) -> u64 {
        ptr::read_volatile((table_addr as *const u64).add(index))
    }

    /// Writes a PTE at the given table address and index.
    unsafe fn write_pte(table_addr: u64, index: usize, value: u64) {
        ptr::write_volatile((table_addr as *mut u64).add(index), value);
    }

    /// Walks the page table to find the L3 leaf PTE for `va`.
    ///
    /// Returns `(table_addr, index, pte_value)` for the L3 entry, or None
    /// if any intermediate level is missing.
    unsafe fn walk_to_l3(&self, va: u64) -> Option<(u64, usize, u64)> {
        let levels = [PageLevel::L0, PageLevel::L1, PageLevel::L2, PageLevel::L3];

        let mut table_addr = self.root_paddr;

        for (i, level) in levels.iter().enumerate() {
            let idx = PageTable::index(*level, va);
            let pte = Self::read_pte(table_addr, idx);

            if pte & PTE_VALID == 0 {
                return None;
            }

            if i == 3 {
                // L3 leaf
                return Some((table_addr, idx, pte));
            }

            // Follow to next level
            table_addr = pte & PTE_ADDR_MASK;
        }

        None
    }

    /// Walks the page table, allocating intermediate tables as needed,
    /// to reach the L3 entry for `va`.
    ///
    /// Returns `(l3_table_addr, l3_index, l3_pte)` or an error.
    unsafe fn walk_or_alloc(&mut self, va: u64) -> Result<(u64, usize, u64), MmError> {
        let levels = [PageLevel::L0, PageLevel::L1, PageLevel::L2, PageLevel::L3];

        let mut table_addr = self.root_paddr;

        for (i, level) in levels.iter().enumerate() {
            let idx = PageTable::index(*level, va);
            let pte = Self::read_pte(table_addr, idx);

            if i == 3 {
                // L3 leaf
                return Ok((table_addr, idx, pte));
            }

            if pte & PTE_VALID == 0 {
                // Allocate a new page table
                let new_pt = alloc_page_table().ok_or(MmError::OutOfMemory)?;
                let table_pte = PageTable::make_table(new_pt);
                Self::write_pte(table_addr, idx, table_pte);
                table_addr = new_pt;
            } else {
                // Follow existing table
                table_addr = pte & PTE_ADDR_MASK;
            }
        }

        unreachable!()
    }
}

impl AddressSpace for Vspace {
    fn map(&mut self, va: u64, pa: u64, size: u64, flags: MemFlags) -> Result<(), MmError> {
        // Alignment check
        if va & 0xFFF != 0 || pa & 0xFFF != 0 {
            return Err(MmError::Misaligned);
        }

        let mut off = 0u64;
        while off < size {
            let cur_va = va + off;
            let cur_pa = pa + off;

            unsafe {
                let (table_addr, idx, existing) = self.walk_or_alloc(cur_va)?;

                // Check for already mapped
                if existing & PTE_VALID != 0 {
                    return Err(MmError::AlreadyMapped);
                }

                // Write leaf PTE
                let leaf = PageTable::make_leaf(cur_pa, flags);
                Self::write_pte(table_addr, idx, leaf);
            }

            off += PAGE_SIZE;
        }

        self.flush_tlb();
        Ok(())
    }

    fn unmap(&mut self, va: u64, size: u64) -> Result<(), MmError> {
        let mut off = 0u64;
        while off < size {
            let cur_va = va + off;
            unsafe {
                if let Some((table_addr, idx, _pte)) = self.walk_to_l3(cur_va) {
                    Self::write_pte(table_addr, idx, 0);
                } else {
                    return Err(MmError::NotMapped);
                }
            }
            off += PAGE_SIZE;
        }

        self.flush_tlb();
        Ok(())
    }

    fn translate(&self, va: u64) -> Option<u64> {
        unsafe {
            let (_table, _idx, pte) = self.walk_to_l3(va)?;
            if pte & PTE_VALID == 0 {
                return None;
            }
            // Extract physical address from leaf PTE
            Some(pte & PTE_ADDR_MASK)
        }
    }

    fn set_flags(&mut self, va: u64, flags: MemFlags) -> Result<(), MmError> {
        unsafe {
            let (table_addr, idx, pte) = self.walk_or_alloc(va)?;
            if pte & PTE_VALID == 0 {
                return Err(MmError::NotMapped);
            }
            // Preserve physical address, update flags
            let pa = pte & PTE_ADDR_MASK;
            let leaf = PageTable::make_leaf(pa, flags);
            Self::write_pte(table_addr, idx, leaf);
        }
        self.flush_tlb();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mm_error_variants() {
        assert_eq!(MmError::InvalidAddr, MmError::InvalidAddr);
        assert_eq!(MmError::NotMapped, MmError::NotMapped);
        assert_eq!(MmError::AlreadyMapped, MmError::AlreadyMapped);
        assert_eq!(MmError::OutOfMemory, MmError::OutOfMemory);
        assert_eq!(MmError::Misaligned, MmError::Misaligned);
    }

    #[test]
    fn test_mm_error_display() {
        assert_eq!(format!("{}", MmError::InvalidAddr), "invalid address");
        assert_eq!(format!("{}", MmError::NotMapped), "not mapped");
        assert_eq!(format!("{}", MmError::AlreadyMapped), "already mapped");
        assert!(format!("{}", MmError::OutOfMemory).contains("out of memory"));
        assert_eq!(
            format!("{}", MmError::Misaligned),
            "address not page-aligned"
        );
    }

    #[test]
    fn test_vspace_new() {
        let vs = Vspace::new(0x40000000, 1);
        assert_eq!(vs.root_paddr, 0x40000000);
        assert_eq!(vs.asid, 1);
        for r in &vs.regions {
            assert!(r.is_none());
        }
    }

    // We need a testable Vspace. Since the real page table walk uses
    // volatile memory at root_paddr, we provide a host-only test using
    // a local PageTable array as the root.

    #[test]
    #[allow(static_mut_refs)]
    fn test_map_misaligned() {
        // Use a static PageTable as root for testing
        static mut TEST_ROOT: PageTable = PageTable::new();
        unsafe {
            // Reset pool counter for this test
            PT_POOL_NEXT = 0;
            let root_addr = &TEST_ROOT as *const PageTable as u64;
            let mut vs = Vspace::new(root_addr, 0);
            // VA not aligned
            let result = vs.map(0x1001, 0x2000, PAGE_SIZE, MemFlags::normal());
            assert_eq!(result, Err(MmError::Misaligned));
            // PA not aligned
            let result = vs.map(0x1000, 0x2001, PAGE_SIZE, MemFlags::normal());
            assert_eq!(result, Err(MmError::Misaligned));
        }
    }

    #[test]
    #[allow(static_mut_refs)]
    fn test_map_and_translate() {
        static mut TEST_ROOT2: PageTable = PageTable::new();
        unsafe {
            PT_POOL_NEXT = 0;
            let root_addr = &TEST_ROOT2 as *const PageTable as u64;
            let mut vs = Vspace::new(root_addr, 0);

            // Map VA 0x1000 to PA 0x9000
            let result = vs.map(0x1000, 0x9000, PAGE_SIZE, MemFlags::normal());
            assert!(result.is_ok());

            // Translate should return the PA
            let pa = vs.translate(0x1000);
            assert_eq!(pa, Some(0x9000));
        }
    }

    #[test]
    #[allow(static_mut_refs)]
    fn test_map_already_mapped() {
        static mut TEST_ROOT3: PageTable = PageTable::new();
        unsafe {
            PT_POOL_NEXT = 0;
            let root_addr = &TEST_ROOT3 as *const PageTable as u64;
            let mut vs = Vspace::new(root_addr, 0);

            // First map succeeds
            let r1 = vs.map(0x2000, 0x8000, PAGE_SIZE, MemFlags::normal());
            assert!(r1.is_ok());

            // Second map of same VA fails
            let r2 = vs.map(0x2000, 0x7000, PAGE_SIZE, MemFlags::normal());
            assert_eq!(r2, Err(MmError::AlreadyMapped));
        }
    }

    #[test]
    #[allow(static_mut_refs)]
    fn test_unmap() {
        static mut TEST_ROOT4: PageTable = PageTable::new();
        unsafe {
            PT_POOL_NEXT = 0;
            let root_addr = &TEST_ROOT4 as *const PageTable as u64;
            let mut vs = Vspace::new(root_addr, 0);

            vs.map(0x3000, 0xA000, PAGE_SIZE, MemFlags::normal())
                .unwrap();
            assert_eq!(vs.translate(0x3000), Some(0xA000));

            vs.unmap(0x3000, PAGE_SIZE).unwrap();
            assert_eq!(vs.translate(0x3000), None);
        }
    }

    #[test]
    #[allow(static_mut_refs)]
    fn test_translate_unmapped() {
        static mut TEST_ROOT5: PageTable = PageTable::new();
        unsafe {
            let root_addr = &TEST_ROOT5 as *const PageTable as u64;
            let vs = Vspace::new(root_addr, 0);
            assert_eq!(vs.translate(0x1000), None);
        }
    }
}
