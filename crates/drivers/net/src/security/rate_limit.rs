//! Connection tracking and rate limiting (v0.30.0).
//!
//! Provides per-IP connection counting, total connection caps, and a sliding
//! 1-second rate window to mitigate connection-flood style attacks. Used by
//! the firewall engine ([`super::firewall::Firewall`]) to decide whether to
//! admit a new connection.

use alloc::collections::BTreeMap;

use crate::tcpip::addr::Ipv4Addr;

/// Sliding rate window length in milliseconds.
const RATE_WINDOW_MS: u64 = 1000;

/// Per-IP connection info for tracking.
#[derive(Debug, Clone)]
pub struct ConnInfo {
    /// Active connection count for this IP.
    pub count: u32,
    /// Timestamp (ms) of the most recent connection attempt.
    pub last_connect: u64,
    /// Start timestamp (ms) of the current 1-second rate window.
    pub rate_window: u64,
    /// Number of connection attempts within the current rate window.
    pub rate_count: u32,
}

/// Connection tracker with per-IP and total limits.
///
/// Tracks active connections per source IPv4 address and enforces two caps:
/// a per-IP simultaneous connection limit and a global total connection limit.
/// Additionally maintains a 1-second sliding rate window per IP so callers can
/// query whether an IP is sending new connections too quickly.
pub struct ConnectionTracker {
    connections: BTreeMap<Ipv4Addr, ConnInfo>,
    max_per_ip: u32,
    max_total: u32,
    total: u32,
}

/// Rate limit configuration.
///
/// Plain configuration record consumed by callers that need to derive a
/// [`ConnectionTracker`] policy. The tracker itself does not store this; the
/// `is_rate_limited` check takes the per-second threshold as a parameter.
#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    /// Maximum simultaneous connections.
    pub max_connections: u32,
    /// Maximum new connections per second.
    pub max_rate_per_sec: u32,
}

impl ConnectionTracker {
    /// Create a new tracker with the given per-IP and total connection caps.
    pub fn new(max_per_ip: u32, max_total: u32) -> Self {
        Self {
            connections: BTreeMap::new(),
            max_per_ip,
            max_total,
            total: 0,
        }
    }

    /// Attempt to register a new connection from `ip` at timestamp `now` (ms).
    ///
    /// Returns `true` if the connection is admitted, `false` if either the
    /// per-IP or total cap would be exceeded. On success the per-IP count,
    /// total count, and rate window counter are all updated. The rate window
    /// resets (clearing `rate_count`) whenever `now - rate_window >= 1000` ms.
    pub fn try_connect(&mut self, ip: Ipv4Addr, now: u64) -> bool {
        // Reject if global total cap is already reached.
        if self.total >= self.max_total {
            return false;
        }

        // Reject if this IP already holds its full per-IP quota.
        let current_count = self.connections.get(&ip).map(|c| c.count).unwrap_or(0);
        if current_count >= self.max_per_ip {
            return false;
        }

        // Insert a fresh entry if this is the IP's first connection.
        let info = self.connections.entry(ip).or_insert(ConnInfo {
            count: 0,
            last_connect: now,
            rate_window: now,
            rate_count: 0,
        });

        // Reset the rate window once it has fully elapsed.
        if now.saturating_sub(info.rate_window) >= RATE_WINDOW_MS {
            info.rate_window = now;
            info.rate_count = 0;
        }

        info.rate_count = info.rate_count.saturating_add(1);
        info.count = info.count.saturating_add(1);
        info.last_connect = now;

        self.total = self.total.saturating_add(1);
        true
    }

    /// Register a disconnect from `ip`.
    ///
    /// Decrements the per-IP count and the global total. When an IP's count
    /// reaches zero its entry is removed entirely. Disconnecting an unknown IP
    /// (or an IP whose count is already zero) is a no-op.
    pub fn disconnect(&mut self, ip: Ipv4Addr) {
        let mut should_remove = false;
        if let Some(info) = self.connections.get_mut(&ip) {
            if info.count > 0 {
                info.count -= 1;
                self.total = self.total.saturating_sub(1);
            }
            should_remove = info.count == 0;
        }
        if should_remove {
            self.connections.remove(&ip);
        }
    }

    /// Query whether `ip` has exceeded `max_per_sec` connections in its
    /// current rate window. Unknown IPs are never rate-limited.
    pub fn is_rate_limited(&self, ip: Ipv4Addr, max_per_sec: u32) -> bool {
        match self.connections.get(&ip) {
            Some(info) => info.rate_count > max_per_sec,
            None => false,
        }
    }

    /// Return the current active connection count for `ip` (0 if unknown).
    pub fn count_for(&self, ip: Ipv4Addr) -> u32 {
        self.connections.get(&ip).map(|c| c.count).unwrap_or(0)
    }

    /// Return the total number of active connections across all IPs.
    pub fn total(&self) -> u32 {
        self.total
    }

    /// Return the configured per-IP connection cap.
    pub fn max_per_ip(&self) -> u32 {
        self.max_per_ip
    }

    /// Return the configured global total connection cap.
    pub fn max_total(&self) -> u32 {
        self.max_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcpip::addr::ipv4_addr;

    fn ip(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        ipv4_addr(a, b, c, d)
    }

    #[test]
    fn test_new_initial_state() {
        let t = ConnectionTracker::new(5, 100);
        assert_eq!(t.total(), 0);
        assert_eq!(t.max_per_ip(), 5);
        assert_eq!(t.max_total(), 100);
        assert_eq!(t.count_for(ip(192, 168, 1, 1)), 0);
    }

    #[test]
    fn test_try_connect_single_ip_success() {
        let mut t = ConnectionTracker::new(5, 100);
        assert!(t.try_connect(ip(192, 168, 1, 1), 0));
        assert_eq!(t.count_for(ip(192, 168, 1, 1)), 1);
        assert_eq!(t.total(), 1);
    }

    #[test]
    fn test_try_connect_multiple_within_per_ip_limit() {
        let mut t = ConnectionTracker::new(3, 100);
        assert!(t.try_connect(ip(10, 0, 0, 1), 0));
        assert!(t.try_connect(ip(10, 0, 0, 1), 10));
        assert!(t.try_connect(ip(10, 0, 0, 1), 20));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 3);
        assert_eq!(t.total(), 3);
    }

    #[test]
    fn test_try_connect_exceeds_max_per_ip() {
        let mut t = ConnectionTracker::new(2, 100);
        assert!(t.try_connect(ip(10, 0, 0, 1), 0));
        assert!(t.try_connect(ip(10, 0, 0, 1), 10));
        // Third connection from same IP should be rejected.
        assert!(!t.try_connect(ip(10, 0, 0, 1), 20));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 2);
        assert_eq!(t.total(), 2);
    }

    #[test]
    fn test_try_connect_exceeds_max_total() {
        let mut t = ConnectionTracker::new(10, 2);
        assert!(t.try_connect(ip(10, 0, 0, 1), 0));
        assert!(t.try_connect(ip(10, 0, 0, 2), 10));
        // Third connection from a new IP should be rejected by the total cap.
        assert!(!t.try_connect(ip(10, 0, 0, 3), 20));
        assert_eq!(t.total(), 2);
        assert_eq!(t.count_for(ip(10, 0, 0, 3)), 0);
    }

    #[test]
    fn test_try_connect_rate_window_reset() {
        let mut t = ConnectionTracker::new(10, 100);
        // Two connects within the first window.
        t.try_connect(ip(10, 0, 0, 1), 0);
        t.try_connect(ip(10, 0, 0, 1), 500);
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 2);
        // Verify rate_count accumulated to 2.
        assert!(!t.is_rate_limited(ip(10, 0, 0, 1), 5));
        assert!(t.is_rate_limited(ip(10, 0, 0, 1), 1));

        // A connect at t=1000 resets the window (>= 1000 ms elapsed).
        t.try_connect(ip(10, 0, 0, 1), 1000);
        // After reset, rate_count should be 1 (only the latest attempt).
        assert!(!t.is_rate_limited(ip(10, 0, 0, 1), 1));
        assert!(t.is_rate_limited(ip(10, 0, 0, 1), 0));
    }

    #[test]
    fn test_rate_count_increments_within_window() {
        let mut t = ConnectionTracker::new(10, 100);
        t.try_connect(ip(10, 0, 0, 1), 0);
        t.try_connect(ip(10, 0, 0, 1), 100);
        t.try_connect(ip(10, 0, 0, 1), 200);
        t.try_connect(ip(10, 0, 0, 1), 300);
        // 4 attempts in the same window.
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 4);
        assert!(t.is_rate_limited(ip(10, 0, 0, 1), 3));
        assert!(!t.is_rate_limited(ip(10, 0, 0, 1), 4));
    }

    #[test]
    fn test_try_connect_multiple_ips_independent() {
        let mut t = ConnectionTracker::new(2, 100);
        assert!(t.try_connect(ip(10, 0, 0, 1), 0));
        assert!(t.try_connect(ip(10, 0, 0, 2), 10));
        assert!(t.try_connect(ip(10, 0, 0, 1), 20));
        // IP1 at its per-IP cap; IP2 still has room.
        assert!(!t.try_connect(ip(10, 0, 0, 1), 30));
        assert!(t.try_connect(ip(10, 0, 0, 2), 40));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 2);
        assert_eq!(t.count_for(ip(10, 0, 0, 2)), 2);
        assert_eq!(t.total(), 4);
    }

    #[test]
    fn test_disconnect_decrements_count() {
        let mut t = ConnectionTracker::new(5, 100);
        t.try_connect(ip(10, 0, 0, 1), 0);
        t.try_connect(ip(10, 0, 0, 1), 10);
        t.disconnect(ip(10, 0, 0, 1));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 1);
        assert_eq!(t.total(), 1);
    }

    #[test]
    fn test_disconnect_removes_entry_at_zero() {
        let mut t = ConnectionTracker::new(5, 100);
        t.try_connect(ip(10, 0, 0, 1), 0);
        t.disconnect(ip(10, 0, 0, 1));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 0);
        assert_eq!(t.total(), 0);
        // The entry should be fully gone: a fresh connect must re-seed the
        // rate window rather than accumulating into a stale one.
        assert!(t.try_connect(ip(10, 0, 0, 1), 5000));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 1);
    }

    #[test]
    fn test_disconnect_unknown_ip_noop() {
        let mut t = ConnectionTracker::new(5, 100);
        t.try_connect(ip(10, 0, 0, 1), 0);
        // Disconnecting an unknown IP must not panic or alter state.
        t.disconnect(ip(10, 0, 0, 99));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 1);
        assert_eq!(t.total(), 1);
        assert_eq!(t.count_for(ip(10, 0, 0, 99)), 0);
    }

    #[test]
    fn test_disconnect_does_not_underflow() {
        let mut t = ConnectionTracker::new(5, 100);
        t.try_connect(ip(10, 0, 0, 1), 0);
        t.disconnect(ip(10, 0, 0, 1));
        // Second disconnect when count is already 0 — must stay at 0.
        t.disconnect(ip(10, 0, 0, 1));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 0);
        assert_eq!(t.total(), 0);
    }

    #[test]
    fn test_is_rate_limited_under_limit() {
        let mut t = ConnectionTracker::new(10, 100);
        t.try_connect(ip(10, 0, 0, 1), 0);
        t.try_connect(ip(10, 0, 0, 1), 100);
        // rate_count == 2, threshold 5 → not limited.
        assert!(!t.is_rate_limited(ip(10, 0, 0, 1), 5));
    }

    #[test]
    fn test_is_rate_limited_over_limit() {
        let mut t = ConnectionTracker::new(10, 100);
        t.try_connect(ip(10, 0, 0, 1), 0);
        t.try_connect(ip(10, 0, 0, 1), 100);
        t.try_connect(ip(10, 0, 0, 1), 200);
        // rate_count == 3, threshold 2 → 3 > 2 → limited.
        assert!(t.is_rate_limited(ip(10, 0, 0, 1), 2));
    }

    #[test]
    fn test_is_rate_limited_unknown_ip() {
        let t = ConnectionTracker::new(5, 100);
        // Unknown IP is never rate-limited.
        assert!(!t.is_rate_limited(ip(10, 0, 0, 99), 0));
    }

    #[test]
    fn test_count_for_unknown_ip() {
        let t = ConnectionTracker::new(5, 100);
        assert_eq!(t.count_for(ip(10, 0, 0, 99)), 0);
    }

    #[test]
    fn test_max_accessors() {
        let t = ConnectionTracker::new(7, 42);
        assert_eq!(t.max_per_ip(), 7);
        assert_eq!(t.max_total(), 42);
    }

    #[test]
    fn test_mixed_multi_ip_scenario() {
        let mut t = ConnectionTracker::new(3, 10);

        // Three IPs each open two connections.
        assert!(t.try_connect(ip(10, 0, 0, 1), 0));
        assert!(t.try_connect(ip(10, 0, 0, 1), 10));
        assert!(t.try_connect(ip(10, 0, 0, 2), 20));
        assert!(t.try_connect(ip(10, 0, 0, 2), 30));
        assert!(t.try_connect(ip(10, 0, 0, 3), 40));
        assert!(t.try_connect(ip(10, 0, 0, 3), 50));
        assert_eq!(t.total(), 6);

        // IP1's third connection is still within the per-IP cap of 3.
        assert!(t.try_connect(ip(10, 0, 0, 1), 60));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 3);
        assert_eq!(t.total(), 7);
        // IP1's fourth connection exceeds the per-IP cap.
        assert!(!t.try_connect(ip(10, 0, 0, 1), 70));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 3);

        // IP1 disconnects one; now it can reconnect.
        t.disconnect(ip(10, 0, 0, 1));
        assert!(t.try_connect(ip(10, 0, 0, 1), 80));
        assert_eq!(t.count_for(ip(10, 0, 0, 1)), 3);
        assert_eq!(t.total(), 7);

        // Disconnect all of IP2.
        t.disconnect(ip(10, 0, 0, 2));
        t.disconnect(ip(10, 0, 0, 2));
        assert_eq!(t.count_for(ip(10, 0, 0, 2)), 0);
        assert_eq!(t.total(), 5);
    }

    #[test]
    fn test_try_connect_zero_per_ip_rejects_all() {
        let mut t = ConnectionTracker::new(0, 100);
        assert!(!t.try_connect(ip(10, 0, 0, 1), 0));
        assert_eq!(t.total(), 0);
    }

    #[test]
    fn test_try_connect_zero_max_total_rejects_all() {
        let mut t = ConnectionTracker::new(5, 0);
        assert!(!t.try_connect(ip(10, 0, 0, 1), 0));
        assert_eq!(t.total(), 0);
    }
}
