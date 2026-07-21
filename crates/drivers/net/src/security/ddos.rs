//! DDoS protection (v0.30.0).
//!
//! Provides per-IP SYN flood detection using a sliding window rate limiter.
//! Each source IP is tracked independently; when the number of SYN packets
//! observed within the configured window exceeds the threshold, subsequent
//! SYNs from that IP are rejected until the window resets.
//!
//! # Design
//!
//! - **Per-IP tracking**: A [`BTreeMap<Ipv4Addr, SynInfo>`] maps each source
//!   IP to its SYN count and window start timestamp. `Ipv4Addr` (alias for
//!   `smoltcp::wire::Ipv4Address` = `core::net::Ipv4Addr`) implements `Ord`,
//!   so it is usable as a `BTreeMap` key without wrapping.
//! - **Fixed window**: When `now - window_start >= window_ms`, the counter
//!   resets to 0 and the window start advances to `now`. This is a simple,
//!   deterministic scheme suitable for embedded deployments.
//! - **no_std**: Uses `alloc::collections::BTreeMap` (no `std`), per
//!   Blueprint §43.1 no_std compliance.
//!
//! # Usage
//!
//! ```
//! use eneros_net::security::ddos::DdosProtector;
//! use eneros_net::tcpip::addr::ipv4_addr;
//!
//! // Allow at most 100 SYNs per second per source IP.
//! let mut ddos = DdosProtector::new(100, 1000);
//!
//! // Normal client: under threshold → allowed.
//! assert!(ddos.check_syn(ipv4_addr(192, 168, 1, 10), 0));
//!
//! // Flood: exceed threshold → blocked.
//! for _ in 0..150 {
//!     ddos.check_syn(ipv4_addr(10, 0, 0, 1), 0);
//! }
//! assert!(!ddos.check_syn(ipv4_addr(10, 0, 0, 1), 0));
//! assert!(ddos.is_under_attack());
//! ```

use alloc::collections::BTreeMap;

use crate::tcpip::addr::Ipv4Addr;

/// Per-IP SYN tracking info.
#[derive(Debug, Clone, Copy)]
pub struct SynInfo {
    /// Number of SYN packets observed in the current window.
    pub syn_count: u32,
    /// Window start timestamp in milliseconds.
    pub window_start: u64,
}

/// DDoS protector detecting SYN Flood attacks.
///
/// Tracks per-source-IP SYN rates within a fixed time window. SYNs exceeding
/// the configured threshold are flagged as suspicious and rejected.
pub struct DdosProtector {
    syn_tracker: BTreeMap<Ipv4Addr, SynInfo>,
    syn_rate_threshold: u32,
    window_ms: u64,
}

/// Security errors for the network security subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityError {
    /// Packet blocked by firewall rules.
    BlockedByFirewall,
    /// Packet dropped due to rate limiting.
    RateLimited,
    /// Connection limit exceeded for the source.
    ConnectionLimitExceeded,
    /// Suspicious activity detected (e.g. SYN flood).
    SuspiciousActivity,
}

impl DdosProtector {
    /// Create a new DDoS protector.
    ///
    /// # Arguments
    ///
    /// * `syn_rate_threshold` — Maximum number of SYNs allowed per source IP
    ///   within `window_ms`. A source exceeding this count is considered an
    ///   attacker.
    /// * `window_ms` — Length of the sliding counting window in milliseconds.
    ///   A value of 0 is treated as "always reset" (every check resets the
    ///   window), which effectively disables protection.
    pub const fn new(syn_rate_threshold: u32, window_ms: u64) -> Self {
        Self {
            syn_tracker: BTreeMap::new(),
            syn_rate_threshold,
            window_ms,
        }
    }

    /// Check an incoming SYN packet from `src` at timestamp `now` (ms).
    ///
    /// Algorithm:
    /// 1. Look up or insert `src`'s [`SynInfo`] (initial `syn_count=0`,
    ///    `window_start=now`).
    /// 2. If `now - window_start >= window_ms`, reset the window:
    ///    `window_start = now`, `syn_count = 0`.
    /// 3. Increment `syn_count`.
    /// 4. If `syn_count > syn_rate_threshold`, return `false` (suspected
    ///    attack — drop the SYN).
    /// 5. Otherwise return `true` (allow the SYN).
    ///
    /// # Arguments
    ///
    /// * `src` — Source IPv4 address of the SYN packet.
    /// * `now` — Current monotonic timestamp in milliseconds.
    ///
    /// # Returns
    ///
    /// `true` if the SYN is allowed, `false` if it should be dropped.
    pub fn check_syn(&mut self, src: Ipv4Addr, now: u64) -> bool {
        // Insert a fresh entry if this is the first SYN from `src`.
        let info = self.syn_tracker.entry(src).or_insert(SynInfo {
            syn_count: 0,
            window_start: now,
        });

        // Reset the window if it has elapsed. The subtraction is safe because
        // we only enter this branch when `now >= window_start` (entry was just
        // inserted with `window_start = now`, or a prior call set it ≤ now).
        if now >= info.window_start && now - info.window_start >= self.window_ms {
            info.window_start = now;
            info.syn_count = 0;
        } else if now < info.window_start {
            // Clock went backwards (shouldn't happen with monotonic time, but
            // guard anyway): reset the window to avoid underflow.
            info.window_start = now;
            info.syn_count = 0;
        }

        info.syn_count = info.syn_count.saturating_add(1);

        info.syn_count <= self.syn_rate_threshold
    }

    /// Return `true` if any tracked IP currently exceeds the SYN threshold.
    ///
    /// This is a global "under attack" indicator: it scans all tracked IPs
    /// and returns `true` if at least one has `syn_count > syn_rate_threshold`.
    /// Note that this does NOT account for window expiry — callers wanting an
    /// accurate per-IP status should use [`Self::check_syn`] return value.
    pub fn is_under_attack(&self) -> bool {
        self.syn_tracker
            .values()
            .any(|info| info.syn_count > self.syn_rate_threshold)
    }

    /// Clear all tracked SYN state.
    ///
    /// After calling this, [`Self::tracked_count`] returns 0 and
    /// [`Self::is_under_attack`] returns `false`.
    pub fn reset(&mut self) {
        self.syn_tracker.clear();
    }

    /// Return the configured SYN rate threshold (SYNs per window).
    pub const fn syn_rate_threshold(&self) -> u32 {
        self.syn_rate_threshold
    }

    /// Return the configured window length in milliseconds.
    pub const fn window_ms(&self) -> u64 {
        self.window_ms
    }

    /// Return the number of source IPs currently being tracked.
    pub fn tracked_count(&self) -> usize {
        self.syn_tracker.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcpip::addr::ipv4_addr;

    #[test]
    fn new_initial_state() {
        let ddos = DdosProtector::new(100, 1000);
        assert_eq!(ddos.syn_rate_threshold(), 100);
        assert_eq!(ddos.window_ms(), 1000);
        assert_eq!(ddos.tracked_count(), 0);
        assert!(!ddos.is_under_attack());
    }

    #[test]
    fn check_syn_allows_under_threshold() {
        let mut ddos = DdosProtector::new(100, 1000);
        let src = ipv4_addr(192, 168, 1, 10);
        // First 100 SYNs are allowed (count 1..100 ≤ 100).
        for i in 1..=100u32 {
            assert!(ddos.check_syn(src, 0), "SYN #{} should be allowed", i);
        }
        assert_eq!(ddos.tracked_count(), 1);
        assert!(!ddos.is_under_attack());
    }

    #[test]
    fn check_syn_blocks_over_threshold() {
        let mut ddos = DdosProtector::new(10, 1000);
        let src = ipv4_addr(10, 0, 0, 1);
        // SYNs 1..10 are allowed (count ≤ 10).
        for _ in 0..10 {
            assert!(ddos.check_syn(src, 0));
        }
        // SYN #11 exceeds threshold → blocked.
        assert!(!ddos.check_syn(src, 0));
        // Further SYNs remain blocked while count stays above threshold.
        assert!(!ddos.check_syn(src, 0));
        assert!(ddos.is_under_attack());
    }

    #[test]
    fn check_syn_window_reset_allows_again() {
        let mut ddos = DdosProtector::new(5, 1000);
        let src = ipv4_addr(172, 16, 0, 1);

        // Exhaust the threshold at t=0 (counts 1..5 allowed, #6 blocked).
        for _ in 0..5 {
            assert!(ddos.check_syn(src, 0));
        }
        assert!(!ddos.check_syn(src, 0));

        // Still within the window (t=500 < 1000) → still blocked.
        assert!(!ddos.check_syn(src, 500));

        // Window elapses (t=1000 >= 1000) → counter resets, SYN allowed.
        assert!(ddos.check_syn(src, 1000));
        assert!(!ddos.is_under_attack());
    }

    #[test]
    fn check_syn_multiple_ips_independent() {
        let mut ddos = DdosProtector::new(3, 1000);
        let a = ipv4_addr(10, 0, 0, 1);
        let b = ipv4_addr(10, 0, 0, 2);

        // IP A exhausts its allowance.
        assert!(ddos.check_syn(a, 0));
        assert!(ddos.check_syn(a, 0));
        assert!(ddos.check_syn(a, 0));
        assert!(!ddos.check_syn(a, 0)); // A blocked at count 4 > 3.

        // IP B is independent — still allowed.
        assert!(ddos.check_syn(b, 0));
        assert!(ddos.check_syn(b, 0));
        assert!(ddos.check_syn(b, 0));
        assert!(!ddos.check_syn(b, 0)); // B blocked at count 4 > 3.

        assert_eq!(ddos.tracked_count(), 2);
        assert!(ddos.is_under_attack());
    }

    #[test]
    fn is_under_attack_no_attack() {
        let mut ddos = DdosProtector::new(10, 1000);
        let src = ipv4_addr(192, 168, 1, 1);
        // Under threshold → no attack.
        for _ in 0..5 {
            ddos.check_syn(src, 0);
        }
        assert!(!ddos.is_under_attack());
    }

    #[test]
    fn is_under_attack_with_attack() {
        let mut ddos = DdosProtector::new(2, 1000);
        let attacker = ipv4_addr(203, 0, 113, 1);
        let normal = ipv4_addr(192, 168, 1, 50);

        // Normal client stays under threshold.
        ddos.check_syn(normal, 0);
        // Attacker floods.
        ddos.check_syn(attacker, 0);
        ddos.check_syn(attacker, 0);
        ddos.check_syn(attacker, 0); // count 3 > 2 → attacker flagged.

        assert!(ddos.is_under_attack());
    }

    #[test]
    fn reset_clears_state() {
        let mut ddos = DdosProtector::new(2, 1000);
        let src = ipv4_addr(10, 0, 0, 1);
        // Trigger an attack state.
        ddos.check_syn(src, 0);
        ddos.check_syn(src, 0);
        ddos.check_syn(src, 0);
        assert!(ddos.is_under_attack());
        assert_eq!(ddos.tracked_count(), 1);

        ddos.reset();

        assert_eq!(ddos.tracked_count(), 0);
        assert!(!ddos.is_under_attack());

        // After reset, the same IP is allowed again (fresh entry).
        assert!(ddos.check_syn(src, 1000));
    }

    #[test]
    fn tracked_count_reflects_distinct_ips() {
        let mut ddos = DdosProtector::new(100, 1000);
        assert_eq!(ddos.tracked_count(), 0);

        ddos.check_syn(ipv4_addr(10, 0, 0, 1), 0);
        assert_eq!(ddos.tracked_count(), 1);

        ddos.check_syn(ipv4_addr(10, 0, 0, 2), 0);
        assert_eq!(ddos.tracked_count(), 2);

        // Same IP again — does not increase count.
        ddos.check_syn(ipv4_addr(10, 0, 0, 1), 0);
        assert_eq!(ddos.tracked_count(), 2);

        ddos.check_syn(ipv4_addr(10, 0, 0, 3), 0);
        assert_eq!(ddos.tracked_count(), 3);
    }

    #[test]
    fn boundary_threshold_zero_blocks_everything() {
        // threshold = 0 means any SYN (count ≥ 1 > 0) is blocked.
        let mut ddos = DdosProtector::new(0, 1000);
        let src = ipv4_addr(10, 0, 0, 1);
        assert!(!ddos.check_syn(src, 0));
        assert!(ddos.is_under_attack());
        assert_eq!(ddos.syn_rate_threshold(), 0);
    }

    #[test]
    fn boundary_window_zero_always_resets() {
        // window_ms = 0 means every check resets the window, so count never
        // accumulates past 1 → always allowed (as long as threshold ≥ 1).
        let mut ddos = DdosProtector::new(5, 0);
        let src = ipv4_addr(10, 0, 0, 1);
        // Each call resets the window, increments to 1, then 1 ≤ 5 → allowed.
        for _ in 0..20 {
            assert!(ddos.check_syn(src, 0));
        }
        assert!(!ddos.is_under_attack());
        assert_eq!(ddos.window_ms(), 0);
    }

    #[test]
    fn accessors_return_configured_values() {
        let ddos = DdosProtector::new(42, 5000);
        assert_eq!(ddos.syn_rate_threshold(), 42);
        assert_eq!(ddos.window_ms(), 5000);
    }

    #[test]
    fn check_syn_clock_going_backwards_is_safe() {
        // Defensive: if `now` decreases between calls, the protector should
        // not panic or underflow. It resets the window instead.
        let mut ddos = DdosProtector::new(5, 1000);
        let src = ipv4_addr(10, 0, 0, 1);
        assert!(ddos.check_syn(src, 1000));
        // Clock goes backwards — must not panic.
        assert!(ddos.check_syn(src, 500));
    }

    #[test]
    fn window_reset_preserves_tracked_ip() {
        // After a window reset, the IP entry remains tracked (not removed),
        // only its counter is zeroed. This keeps `tracked_count` stable.
        let mut ddos = DdosProtector::new(3, 1000);
        let src = ipv4_addr(10, 0, 0, 1);

        ddos.check_syn(src, 0);
        ddos.check_syn(src, 0);
        ddos.check_syn(src, 0);
        assert!(!ddos.check_syn(src, 0)); // blocked at count 4 > 3.
        assert_eq!(ddos.tracked_count(), 1);

        // Window elapses → counter resets, IP still tracked.
        assert!(ddos.check_syn(src, 1000));
        assert_eq!(ddos.tracked_count(), 1); // still 1, not 2.
    }

    #[test]
    fn security_error_variants_distinct() {
        // Sanity check: all SecurityError variants are distinct and comparable.
        assert_ne!(SecurityError::BlockedByFirewall, SecurityError::RateLimited);
        assert_ne!(
            SecurityError::ConnectionLimitExceeded,
            SecurityError::SuspiciousActivity
        );
        assert_eq!(SecurityError::RateLimited, SecurityError::RateLimited);
        // Verify Copy + Clone work (used by error propagation paths).
        let err = SecurityError::BlockedByFirewall;
        let err_copy = err;
        assert_eq!(err, err_copy);
    }

    #[test]
    fn check_syn_repeated_window_resets_stay_allowed() {
        // Repeatedly crossing the window boundary keeps resetting the counter,
        // so a slow-but-steady client never gets blocked.
        let mut ddos = DdosProtector::new(5, 1000);
        let src = ipv4_addr(10, 0, 0, 1);
        // Send 3 SYNs per window — never exceeds 5.
        for window_idx in 0..10u64 {
            let t = window_idx * 1000;
            assert!(ddos.check_syn(src, t));
            assert!(ddos.check_syn(src, t));
            assert!(ddos.check_syn(src, t));
        }
        assert!(!ddos.is_under_attack());
        assert_eq!(ddos.tracked_count(), 1);
    }
}
