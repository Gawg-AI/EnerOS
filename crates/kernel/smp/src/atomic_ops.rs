//! Atomic counter primitives — v0.17.0.
//!
//! Thin wrapper around `core::sync::atomic::AtomicU64` providing relaxed
//! increment and acquire/release load/store. On aarch64, `AtomicU64` uses
//! `ldxr`/`stxr` (exclusive) or `ldar`/`stlr` (acquire/release).

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};

/// A 64-bit atomic counter with relaxed increment and acquire/release load/store.
#[derive(Debug)]
pub struct AtomicCounter {
    value: AtomicU64,
}

impl AtomicCounter {
    /// Create a new counter initialized to `v`.
    pub const fn new(v: u64) -> Self {
        Self {
            value: AtomicU64::new(v),
        }
    }

    /// Atomically increment by 1 and return the new value.
    ///
    /// Uses `Relaxed` ordering — the increment itself is atomic, but no
    /// ordering guarantees are provided for surrounding memory operations.
    /// Pair with `load(Acquire)` for visibility.
    #[inline]
    pub fn inc(&self) -> u64 {
        self.value.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Load the current value with `Acquire` semantics.
    ///
    /// Ensures all subsequent memory operations observe writes that happened
    /// before the load.
    #[inline]
    pub fn load(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }

    /// Store `v` with `Release` semantics.
    ///
    /// Ensures all prior memory operations are visible before the store.
    #[inline]
    pub fn store(&self, v: u64) {
        self.value.store(v, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_atomic_counter_new() {
        let _g = lock();
        let c = AtomicCounter::new(42);
        assert_eq!(c.load(), 42);
    }

    #[test]
    fn test_atomic_counter_inc() {
        let _g = lock();
        let c = AtomicCounter::new(0);
        assert_eq!(c.inc(), 1);
        assert_eq!(c.inc(), 2);
    }

    #[test]
    fn test_atomic_counter_multiple_inc() {
        let _g = lock();
        let c = AtomicCounter::new(0);
        for _ in 0..100 {
            c.inc();
        }
        assert_eq!(c.load(), 100);
    }

    #[test]
    fn test_atomic_counter_store_load() {
        let _g = lock();
        let c = AtomicCounter::new(0);
        c.store(99);
        assert_eq!(c.load(), 99);
    }
}
