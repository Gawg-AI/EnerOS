//! Core affinity mask.
//!
//! This module provides `CoreMask`, a 64-bit bitmask where each bit
//! represents whether a thread may be scheduled onto the corresponding
//! core. `CoreMask::default()` (all zeros) is treated as "no affinity
//! restriction" by the scheduler — i.e. the thread may run on any core.
//!
//! Per the D2 design decision, this module depends only on `core::*`.

/// Maximum number of cores addressable by a `CoreMask`.
pub const MAX_CORES: u32 = 64;

/// 64-bit core affinity mask.
///
/// Bit `i` set means the thread may run on core `i`. `CoreMask::default()`
/// (zero) means "no restriction" — the scheduler treats it as eligible for
/// all cores.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct CoreMask(pub u64);

impl CoreMask {
    /// Mask selecting only `core`.
    ///
    /// # Panics (debug only)
    ///
    /// Panics in debug builds if `core >= 64` (shift overflow). In release
    /// builds the shift wraps per Rust's `<<` semantics.
    pub fn single(core: u32) -> Self {
        debug_assert!(core < MAX_CORES, "core index out of range: {core}");
        Self(1u64 << core)
    }

    /// Mask selecting cores `0..count`.
    ///
    /// Handles the `count == 64` boundary by returning `u64::MAX` (all bits
    /// set), avoiding the UB of `1u64 << 64`. `count == 0` yields the empty
    /// mask.
    pub fn all(count: u32) -> Self {
        if count >= MAX_CORES {
            Self(u64::MAX)
        } else {
            Self((1u64 << count) - 1)
        }
    }

    /// Whether `core` is set in this mask.
    pub fn contains(&self, core: u32) -> bool {
        if core >= MAX_CORES {
            return false;
        }
        (self.0 >> core) & 1 == 1
    }

    /// Add `core` to the mask (set its bit).
    pub fn add(&mut self, core: u32) {
        if core < MAX_CORES {
            self.0 |= 1u64 << core;
        }
    }

    /// Remove `core` from the mask (clear its bit).
    pub fn remove(&mut self, core: u32) {
        if core < MAX_CORES {
            self.0 &= !(1u64 << core);
        }
    }

    /// Number of cores set in the mask.
    pub fn count(&self) -> u32 {
        self.0.count_ones()
    }

    /// Whether no cores are set (empty mask).
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Whether this mask shares any set bit with `other`.
    pub fn intersects(&self, other: CoreMask) -> bool {
        (self.0 & other.0) != 0
    }

    /// Raw underlying `u64`.
    pub fn bits(&self) -> u64 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_mask_sets_one_bit() {
        let m = CoreMask::single(3);
        assert_eq!(m.0, 1u64 << 3);
        assert!(m.contains(3));
        assert!(!m.contains(2));
        assert!(!m.contains(4));
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn test_all_mask_inclusive_range() {
        // count == 0 → empty
        assert_eq!(CoreMask::all(0).0, 0);
        assert!(CoreMask::all(0).is_empty());
        // count == 4 → bits 0..3
        let m4 = CoreMask::all(4);
        assert_eq!(m4.0, 0b1111);
        for i in 0..4 {
            assert!(m4.contains(i));
        }
        assert!(!m4.contains(4));
        assert_eq!(m4.count(), 4);
    }

    #[test]
    fn test_all_mask_boundary_64() {
        // count == 64 must not invoke UB via `1u64 << 64`.
        let m = CoreMask::all(64);
        assert_eq!(m.0, u64::MAX);
        for i in 0..64 {
            assert!(m.contains(i), "bit {i} should be set");
        }
        assert_eq!(m.count(), 64);
        assert!(!m.is_empty());
    }

    #[test]
    fn test_add_remove() {
        let mut m = CoreMask::default();
        assert!(m.is_empty());
        m.add(1);
        m.add(5);
        assert!(m.contains(1));
        assert!(m.contains(5));
        assert!(!m.contains(0));
        assert_eq!(m.count(), 2);
        m.remove(1);
        assert!(!m.contains(1));
        assert!(m.contains(5));
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn test_add_out_of_range_ignored() {
        let mut m = CoreMask::default();
        m.add(64); // out of range, ignored
        m.add(100); // out of range, ignored
        assert!(m.is_empty());
        assert_eq!(m.count(), 0);
    }

    #[test]
    fn test_contains_out_of_range_false() {
        let m = CoreMask::single(0);
        assert!(!m.contains(64));
        assert!(!m.contains(u32::MAX));
    }

    #[test]
    fn test_intersects() {
        let a = CoreMask::all(4); // 0b1111
        let b = CoreMask::single(2); // 0b0100
        let c = CoreMask::single(5); // 0b100000
        let empty = CoreMask::default();
        assert!(a.intersects(b));
        assert!(!a.intersects(c));
        assert!(!a.intersects(empty));
        assert!(!empty.intersects(empty));
    }

    #[test]
    fn test_default_is_empty() {
        let m = CoreMask::default();
        assert_eq!(m.0, 0);
        assert!(m.is_empty());
        assert_eq!(m.count(), 0);
        assert_eq!(m.bits(), 0);
    }
}
