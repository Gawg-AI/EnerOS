//! Write amplification tracking.
//!
//! [Write amplification](https://en.wikipedia.org/wiki/Write_amplification) is
//! the ratio of actual flash writes to application-requested writes. In a
//! log-structured filesystem like littlefs2, garbage collection and metadata
//! updates cause the flash to receive more writes than the application
//! requests. A factor of 1.0–2.0 is typical; values above 3.0 indicate the
//! filesystem is spending excessive time on GC.
//!
//! [`WriteAmplificationTracker`] accumulates both application write bytes
//! (reported by the filesystem layer) and flash write bytes (reported by the
//! block device driver), and computes the ratio on demand.

use core::fmt;

// ============================================================================
// WriteAmplificationTracker
// ============================================================================

/// Tracks application vs. flash write bytes to compute write amplification.
///
/// # Usage
///
/// ```ignore
/// use eneros_fs::wear::WriteAmplificationTracker;
///
/// let mut tracker = WriteAmplificationTracker::new();
/// tracker.record_app_write(4096);   // application wrote 4 KB
/// tracker.record_flash_write(8192); // flash received 8 KB (GC overhead)
/// assert!((tracker.write_amplification() - 2.0).abs() < 0.01);
/// ```
#[derive(Clone, Debug)]
pub struct WriteAmplificationTracker {
    /// Total bytes written by the application.
    app_bytes_written: u64,
    /// Total bytes written to the flash device (including GC, metadata).
    flash_bytes_written: u64,
    /// Optional limit; `write_amplification()` above this triggers throttling.
    write_amp_limit: Option<f64>,
}

impl Default for WriteAmplificationTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteAmplificationTracker {
    /// Creates a new tracker with zero counts and no limit.
    pub const fn new() -> Self {
        Self {
            app_bytes_written: 0,
            flash_bytes_written: 0,
            write_amp_limit: None,
        }
    }

    /// Records `bytes` of application-level write.
    pub fn record_app_write(&mut self, bytes: u64) {
        self.app_bytes_written = self.app_bytes_written.saturating_add(bytes);
    }

    /// Records `bytes` of flash-level write (actual device I/O).
    pub fn record_flash_write(&mut self, bytes: u64) {
        self.flash_bytes_written = self.flash_bytes_written.saturating_add(bytes);
    }

    /// Returns the total application bytes written.
    pub fn app_bytes(&self) -> u64 {
        self.app_bytes_written
    }

    /// Returns the total flash bytes written.
    pub fn flash_bytes(&self) -> u64 {
        self.flash_bytes_written
    }

    /// Computes the write amplification factor: `flash_bytes / app_bytes`.
    ///
    /// Returns `0.0` if no application writes have been recorded.
    pub fn write_amplification(&self) -> f64 {
        if self.app_bytes_written == 0 {
            return 0.0;
        }
        self.flash_bytes_written as f64 / self.app_bytes_written as f64
    }

    /// Sets the write amplification limit. When [`Self::is_throttled`] returns
    /// `true`, the caller should reduce write throughput.
    pub fn set_write_amp_limit(&mut self, limit: f64) {
        self.write_amp_limit = Some(limit);
    }

    /// Clears the write amplification limit (disables throttling).
    pub fn clear_write_amp_limit(&mut self) {
        self.write_amp_limit = None;
    }

    /// Returns `true` if the write amplification exceeds the configured limit.
    ///
    /// Always returns `false` if no limit is set or no writes have been
    /// recorded.
    pub fn is_throttled(&self) -> bool {
        match self.write_amp_limit {
            Some(limit) if self.app_bytes_written > 0 => self.write_amplification() > limit,
            _ => false,
        }
    }

    /// Resets all counters to zero (preserves the limit setting).
    pub fn reset(&mut self) {
        self.app_bytes_written = 0;
        self.flash_bytes_written = 0;
    }
}

impl fmt::Display for WriteAmplificationTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "wa={:.2} (app={}, flash={})",
            self.write_amplification(),
            self.app_bytes_written,
            self.flash_bytes_written
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_new_tracker() {
        let t = WriteAmplificationTracker::new();
        assert_eq!(t.app_bytes(), 0);
        assert_eq!(t.flash_bytes(), 0);
        assert_eq!(t.write_amplification(), 0.0);
        assert!(!t.is_throttled());
    }

    #[test]
    fn test_record_app_write() {
        let mut t = WriteAmplificationTracker::new();
        t.record_app_write(1000);
        assert_eq!(t.app_bytes(), 1000);
        t.record_app_write(500);
        assert_eq!(t.app_bytes(), 1500);
    }

    #[test]
    fn test_record_flash_write() {
        let mut t = WriteAmplificationTracker::new();
        t.record_flash_write(2000);
        assert_eq!(t.flash_bytes(), 2000);
        t.record_flash_write(3000);
        assert_eq!(t.flash_bytes(), 5000);
    }

    #[test]
    fn test_write_amplification_no_writes() {
        let t = WriteAmplificationTracker::new();
        assert_eq!(t.write_amplification(), 0.0);
    }

    #[test]
    fn test_write_amplification_ratio_2x() {
        let mut t = WriteAmplificationTracker::new();
        t.record_app_write(4096);
        t.record_flash_write(8192);
        let wa = t.write_amplification();
        assert!((wa - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_write_amplification_ratio_1x() {
        let mut t = WriteAmplificationTracker::new();
        t.record_app_write(1000);
        t.record_flash_write(1000);
        let wa = t.write_amplification();
        assert!((wa - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_write_amplification_accumulated() {
        let mut t = WriteAmplificationTracker::new();
        // First write: 1x
        t.record_app_write(1000);
        t.record_flash_write(1000);
        // Second write: 3x (GC heavy)
        t.record_app_write(1000);
        t.record_flash_write(3000);
        // Total: app=2000, flash=4000, wa=2.0
        let wa = t.write_amplification();
        assert!((wa - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_set_limit_and_throttle() {
        let mut t = WriteAmplificationTracker::new();
        t.set_write_amp_limit(2.0);

        // No writes yet → not throttled.
        assert!(!t.is_throttled());

        // 1.5x amplification → under limit.
        t.record_app_write(1000);
        t.record_flash_write(1500);
        assert!(!t.is_throttled());

        // Increase flash writes to 3.0x → throttled.
        t.record_flash_write(1500); // total flash = 3000
        assert!(t.is_throttled());
    }

    #[test]
    fn test_clear_limit() {
        let mut t = WriteAmplificationTracker::new();
        t.set_write_amp_limit(1.5);
        t.record_app_write(1000);
        t.record_flash_write(3000);
        assert!(t.is_throttled());

        t.clear_write_amp_limit();
        assert!(!t.is_throttled());
    }

    #[test]
    fn test_reset() {
        let mut t = WriteAmplificationTracker::new();
        t.record_app_write(1000);
        t.record_flash_write(2000);
        t.set_write_amp_limit(2.0);

        t.reset();
        assert_eq!(t.app_bytes(), 0);
        assert_eq!(t.flash_bytes(), 0);
        assert_eq!(t.write_amplification(), 0.0);
        // Limit should be preserved.
        t.record_app_write(100);
        t.record_flash_write(300);
        assert!(t.is_throttled());
    }

    #[test]
    fn test_saturating_add() {
        let mut t = WriteAmplificationTracker::new();
        t.record_app_write(u64::MAX);
        t.record_app_write(1); // should saturate, not overflow
        assert_eq!(t.app_bytes(), u64::MAX);
    }

    #[test]
    fn test_display() {
        let mut t = WriteAmplificationTracker::new();
        t.record_app_write(4096);
        t.record_flash_write(8192);
        let s = format!("{}", t);
        assert!(s.contains("wa=2.00"));
        assert!(s.contains("app=4096"));
        assert!(s.contains("flash=8192"));
    }

    #[test]
    fn test_default() {
        let t = WriteAmplificationTracker::default();
        assert_eq!(t.app_bytes(), 0);
        assert_eq!(t.flash_bytes(), 0);
    }
}
