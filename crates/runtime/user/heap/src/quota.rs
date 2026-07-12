//! Quota management for the user-space heap allocator.
//!
//! Provides [`Quota`] — a simple byte-counter with a configurable limit — and
//! [`OomHandler`] — the type of an optional out-of-memory handler function.
//!
//! A `limit` of `0` means **unlimited**: [`Quota::check`] always returns
//! `true` regardless of `used` or the requested `size`.

/// OOM handler type: a function that never returns (`!`).
///
/// When set, the handler is invoked instead of the default panic behaviour.
/// Since the return type is `!`, the handler must diverge (e.g. `panic!`,
/// `loop {}`, or `core::hint::spin_loop` in an infinite loop).
pub type OomHandler = Option<fn() -> !>;

/// Quota tracker for user-space heap allocations.
///
/// Fields are public so that the parent module can update `used` in lock-step
/// with buddy allocator operations. External callers should prefer the
/// high-level [`crate::set_quota`] / [`crate::used`] functions.
#[derive(Debug)]
pub struct Quota {
    /// Maximum bytes allowed. `0` means unlimited.
    pub limit: usize,
    /// Current bytes in use.
    pub used: usize,
}

impl Quota {
    /// Creates a new quota with the given `limit` (0 = unlimited).
    pub const fn new(limit: usize) -> Self {
        Self { limit, used: 0 }
    }

    /// Returns `true` if `size` bytes can be allocated without exceeding the
    /// limit. Always returns `true` when `limit == 0` (unlimited).
    pub fn check(&self, size: usize) -> bool {
        self.limit == 0 || self.used.saturating_add(size) <= self.limit
    }

    /// Records `size` bytes as allocated (saturating add).
    pub fn add_used(&mut self, size: usize) {
        self.used = self.used.saturating_add(size);
    }

    /// Records `size` bytes as freed (saturating sub, never underflows).
    pub fn sub_used(&mut self, size: usize) {
        self.used = self.used.saturating_sub(size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_new() {
        let q = Quota::new(1024);
        assert_eq!(q.limit, 1024);
        assert_eq!(q.used, 0);
    }

    #[test]
    fn test_quota_check_pass() {
        let q = Quota::new(1024);
        assert!(q.check(512));
    }

    #[test]
    fn test_quota_check_fail() {
        let mut q = Quota::new(1024);
        q.add_used(768);
        // 768 + 512 = 1280 > 1024
        assert!(!q.check(512));
    }

    #[test]
    fn test_quota_unlimited() {
        let q = Quota::new(0);
        assert!(q.check(999_999));
    }

    #[test]
    fn test_quota_add_used() {
        let mut q = Quota::new(1024);
        q.add_used(100);
        assert_eq!(q.used, 100);
    }

    #[test]
    fn test_quota_sub_used() {
        let mut q = Quota::new(1024);
        q.add_used(100);
        q.sub_used(40);
        assert_eq!(q.used, 60);
    }

    #[test]
    fn test_quota_sub_overflow() {
        let mut q = Quota::new(1024);
        q.add_used(100);
        // 100 - 200 should saturate to 0, not underflow.
        q.sub_used(200);
        assert_eq!(q.used, 0);
    }
}
