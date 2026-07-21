//! Heartbeat monitoring for link liveness detection (v0.30.2).
//!
//! Tracks heartbeat reception and detects timeouts based on configurable
//! interval, timeout window, and maximum missed count. Pure numeric
//! computation — no `alloc` required.

/// Heartbeat monitor for link liveness detection.
///
/// Tracks heartbeat reception and detects timeouts based on configurable
/// interval, timeout window, and maximum missed count.
#[derive(Debug, Clone)]
pub struct HeartbeatMonitor {
    interval_ms: u64,
    timeout_ms: u64,
    last_heartbeat: u64,
    missed_count: u32,
    max_missed: u32,
}

impl HeartbeatMonitor {
    /// Create a new `HeartbeatMonitor`.
    ///
    /// - `interval_ms`: expected heartbeat interval (reference only).
    /// - `timeout_ms`: timeout threshold; if no heartbeat is received within
    ///   this window, a miss is recorded.
    /// - `max_missed`: number of consecutive misses that triggers a link
    ///   failure.
    pub fn new(interval_ms: u64, timeout_ms: u64, max_missed: u32) -> Self {
        Self {
            interval_ms,
            timeout_ms,
            last_heartbeat: 0,
            missed_count: 0,
            max_missed,
        }
    }

    /// Called when a heartbeat is received.
    ///
    /// Updates `last_heartbeat` to `now` and resets `missed_count` to zero.
    pub fn on_heartbeat(&mut self, now: u64) {
        self.last_heartbeat = now;
        self.missed_count = 0;
    }

    /// Check whether the link has timed out at the given `now` timestamp.
    ///
    /// Algorithm:
    /// 1. If `now - last_heartbeat >= timeout_ms`, a miss is recorded:
    ///    - increment `missed_count`
    ///    - update `last_heartbeat = now` so the next check measures from
    ///      this miss (avoids double-counting one gap)
    /// 2. Returns `true` when `missed_count >= max_missed` (link failure).
    /// 3. Returns `false` while the link is still considered alive.
    pub fn check_timeout(&mut self, now: u64) -> bool {
        if now >= self.last_heartbeat && now.saturating_sub(self.last_heartbeat) >= self.timeout_ms
        {
            self.missed_count = self.missed_count.saturating_add(1);
            self.last_heartbeat = now;
        }
        self.missed_count >= self.max_missed
    }

    /// Returns `true` while the link is still considered alive
    /// (`missed_count < max_missed`).
    pub fn is_alive(&self) -> bool {
        self.missed_count < self.max_missed
    }

    /// Number of consecutive misses recorded so far.
    pub fn missed_count(&self) -> u32 {
        self.missed_count
    }

    /// Timestamp of the last received heartbeat (or last recorded miss).
    pub fn last_heartbeat(&self) -> u64 {
        self.last_heartbeat
    }

    /// Expected heartbeat interval in milliseconds (reference value).
    pub fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    /// Timeout threshold in milliseconds.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// Maximum consecutive misses allowed before link failure.
    pub fn max_missed(&self) -> u32 {
        self.max_missed
    }

    /// Reset the monitor to its initial state
    /// (`missed_count = 0`, `last_heartbeat = 0`).
    pub fn reset(&mut self) {
        self.missed_count = 0;
        self.last_heartbeat = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::HeartbeatMonitor;

    #[test]
    fn new_initial_state() {
        let m = HeartbeatMonitor::new(1000, 3000, 3);
        assert_eq!(m.missed_count(), 0);
        assert_eq!(m.last_heartbeat(), 0);
        assert!(m.is_alive());
    }

    #[test]
    fn on_heartbeat_updates_last_and_resets_missed() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 3);
        // Force a couple of misses first.
        assert!(!m.check_timeout(3000));
        assert!(!m.check_timeout(6000));
        assert_eq!(m.missed_count(), 2);

        m.on_heartbeat(7000);
        assert_eq!(m.last_heartbeat(), 7000);
        assert_eq!(m.missed_count(), 0);
        assert!(m.is_alive());
    }

    #[test]
    fn check_timeout_no_timeout_returns_false() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 3);
        m.on_heartbeat(1000);
        // Within the timeout window.
        assert!(!m.check_timeout(3500));
        assert_eq!(m.missed_count(), 0);
    }

    #[test]
    fn check_timeout_single_miss_below_max_returns_false() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 3);
        m.on_heartbeat(1000);
        // 4000 - 1000 = 3000 >= 3000 → one miss, but missed_count=1 < max=3.
        assert!(!m.check_timeout(4000));
        assert_eq!(m.missed_count(), 1);
    }

    #[test]
    fn check_timeout_reaches_max_missed_returns_true() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 3);
        m.on_heartbeat(1000);
        // Miss 1.
        assert!(!m.check_timeout(4000));
        // Miss 2.
        assert!(!m.check_timeout(7000));
        // Miss 3 → failure.
        assert!(m.check_timeout(10000));
        assert_eq!(m.missed_count(), 3);
    }

    #[test]
    fn check_timeout_updates_last_heartbeat_after_miss() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 5);
        m.on_heartbeat(1000);
        // Miss at t=4000 → last_heartbeat should jump to 4000.
        assert!(!m.check_timeout(4000));
        assert_eq!(m.last_heartbeat(), 4000);
        // Calling again at the same instant should NOT register another miss.
        assert!(!m.check_timeout(4000));
        assert_eq!(m.missed_count(), 1);
        // A short additional delta below timeout should also not trigger.
        assert!(!m.check_timeout(5000));
        assert_eq!(m.missed_count(), 1);
    }

    #[test]
    fn is_alive_true_below_max() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 3);
        m.on_heartbeat(0);
        assert!(!m.check_timeout(3000));
        assert!(!m.check_timeout(6000));
        assert!(m.is_alive());
    }

    #[test]
    fn is_alive_false_at_or_above_max() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 2);
        m.on_heartbeat(0);
        assert!(!m.check_timeout(3000)); // missed=1
        assert!(m.check_timeout(6000)); // missed=2 → failure
        assert!(!m.is_alive());
    }

    #[test]
    fn on_heartbeat_resets_missed_count_after_timeouts() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 3);
        m.on_heartbeat(0);
        assert!(!m.check_timeout(3000));
        assert!(!m.check_timeout(6000));
        assert_eq!(m.missed_count(), 2);
        // Recover with a fresh heartbeat.
        m.on_heartbeat(6500);
        assert_eq!(m.missed_count(), 0);
        assert!(m.is_alive());
        // Need three more misses to fail again.
        assert!(!m.check_timeout(9500));
        assert!(!m.check_timeout(12500));
        assert!(m.check_timeout(15500));
    }

    #[test]
    fn reset_clears_state() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 3);
        m.on_heartbeat(500);
        assert!(!m.check_timeout(3500));
        assert!(!m.check_timeout(6500));
        assert_eq!(m.missed_count(), 2);
        m.reset();
        assert_eq!(m.missed_count(), 0);
        assert_eq!(m.last_heartbeat(), 0);
        assert!(m.is_alive());
    }

    #[test]
    fn accessors_return_configured_values() {
        let m = HeartbeatMonitor::new(1500, 4500, 7);
        assert_eq!(m.interval_ms(), 1500);
        assert_eq!(m.timeout_ms(), 4500);
        assert_eq!(m.max_missed(), 7);
    }

    #[test]
    fn boundary_max_missed_one_first_timeout_fails() {
        let mut m = HeartbeatMonitor::new(1000, 3000, 1);
        m.on_heartbeat(1000);
        // First miss immediately triggers failure when max_missed=1.
        assert!(m.check_timeout(4000));
        assert_eq!(m.missed_count(), 1);
        assert!(!m.is_alive());
    }

    #[test]
    fn check_timeout_no_heartbeat_seen_yet_uses_initial_zero() {
        // last_heartbeat starts at 0; large `now` should count as a miss.
        let mut m = HeartbeatMonitor::new(1000, 3000, 2);
        assert!(!m.check_timeout(3000)); // 3000 - 0 >= 3000 → missed=1
        assert!(m.check_timeout(6000)); // missed=2 → failure
    }
}
