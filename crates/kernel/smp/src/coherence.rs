//! Memory barriers and cache maintenance operations — v0.17.0.
//!
//! ARMv8 is a weak memory model; explicit barriers (`dmb`/`dsb`/`isb`) are
//! required to guarantee ordering and visibility between cores. Cache
//! maintenance (`dc civac`/`dc ivac`) is required for non-coherent DMA.
//!
//! All `asm!` calls are gated behind `#[cfg(target_arch = "aarch64")]`; on
//! the host target these functions are no-ops so unit tests can run.

#![allow(dead_code)]

/// Cacheline size in bytes (ARMv8 AArch64 standard).
pub const CACHELINE_SIZE: usize = 64;

/// Data Memory Barrier (Inner Shareable).
#[inline]
#[cfg(target_arch = "aarch64")]
pub fn dmb() {
    unsafe {
        core::arch::asm!("dmb ish", options(nostack, preserves_flags, readonly));
    }
}

/// Data Memory Barrier — host stub (no-op).
#[inline]
#[cfg(not(target_arch = "aarch64"))]
pub fn dmb() {}

/// Data Synchronization Barrier (Inner Shareable).
///
/// Waits for all memory accesses to complete before continuing.
#[inline]
#[cfg(target_arch = "aarch64")]
pub fn dsb() {
    unsafe {
        core::arch::asm!("dsb ish", options(nostack, preserves_flags, readonly));
    }
}

/// Data Synchronization Barrier — host stub (no-op).
#[inline]
#[cfg(not(target_arch = "aarch64"))]
pub fn dsb() {}

/// Instruction Synchronization Barrier.
///
/// Flushes the instruction pipeline so that instructions fetched after the
/// barrier reflect any prior memory writes.
#[inline]
#[cfg(target_arch = "aarch64")]
pub fn isb() {
    unsafe {
        core::arch::asm!("isb", options(nostack, preserves_flags, readonly));
    }
}

/// Instruction Synchronization Barrier — host stub (no-op).
#[inline]
#[cfg(not(target_arch = "aarch64"))]
pub fn isb() {}

/// Clean (and invalidate) cache lines covering `[addr, addr+size)`.
///
/// Aligns `addr` down to a cacheline boundary and extends `size` upward to
/// cover the full cachelines. Uses `dc civac` (clean + invalidate by VA to
/// PoC). Finishes with `dsb()` to ensure completion.
pub fn cache_clean(addr: u64, size: usize) {
    let line = CACHELINE_SIZE as u64;
    let start = addr & !(line - 1);
    let end = (addr + size as u64 + line - 1) & !(line - 1);
    let mut a = start;
    while a < end {
        clean_line(a);
        a += line;
    }
    dsb();
}

/// Invalidate cache lines covering `[addr, addr+size)`.
///
/// Discards cached data so that subsequent reads fetch from memory. Uses
/// `dc ivac` (invalidate by VA to PoC). Finishes with `dsb()`.
pub fn cache_invalidate(addr: u64, size: usize) {
    let line = CACHELINE_SIZE as u64;
    let start = addr & !(line - 1);
    let end = (addr + size as u64 + line - 1) & !(line - 1);
    let mut a = start;
    while a < end {
        invalidate_line(a);
        a += line;
    }
    dsb();
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn clean_line(addr: u64) {
    unsafe {
        core::arch::asm!("dc civac, {}", in(reg) addr, options(nostack, preserves_flags, readonly));
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn clean_line(_addr: u64) {}

#[cfg(target_arch = "aarch64")]
#[inline]
fn invalidate_line(addr: u64) {
    unsafe {
        core::arch::asm!("dc ivac, {}", in(reg) addr, options(nostack, preserves_flags, readonly));
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn invalidate_line(_addr: u64) {}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_dmb_dsb_isb_host_noop() {
        let _g = lock();
        // On the host target these are no-ops; just verify they don't panic.
        dmb();
        dsb();
        isb();
    }

    #[test]
    fn test_cache_clean_host_noop() {
        let _g = lock();
        cache_clean(0x1000, 128);
    }

    #[test]
    fn test_cache_invalidate_host_noop() {
        let _g = lock();
        cache_invalidate(0x2000, 64);
    }

    #[test]
    fn test_cache_clean_zero_size() {
        let _g = lock();
        cache_clean(0x3000, 0);
    }

    #[test]
    fn test_cacheline_size() {
        let _g = lock();
        assert_eq!(CACHELINE_SIZE, 64);
    }
}
