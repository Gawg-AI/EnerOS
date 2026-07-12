//! PL031 RTC driver and calendar conversion utilities.
//!
//! Provides:
//! - **PL031 RTC driver** for wall-clock time (battery-backed, MMIO)
//! - **Calendar conversion** based on Howard Hinnant's algorithm
//! - **`RtcTime`** human-readable time and **`TimeStamp`** nanosecond epoch

use core::ptr::{read_volatile, write_volatile};

// ============================================================================
// PL031 RTC register offsets (relative to base address)
// ============================================================================

/// Data Register — reads current seconds count.
const RTCDR: u64 = 0x00;
/// Load Register — writes to set the current seconds count.
const RTCLOAD: u64 = 0x20;
/// Control Register — bit 0 enables the RTC.
const RTCCR: u64 = 0x2c;

// ============================================================================
// Data structures
// ============================================================================

/// RTC time in human-readable form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtcTime {
    pub year: u16,   // e.g. 2026
    pub month: u8,   // 1-12
    pub day: u8,     // 1-31
    pub hour: u8,    // 0-23
    pub minute: u8,  // 0-59
    pub second: u8,  // 0-59
    pub weekday: u8, // 0=Sunday, 1=Monday, ..., 6=Saturday
}

/// Timestamp as nanoseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeStamp(pub u64);

// ============================================================================
// PL031 RTC driver
// ============================================================================

/// PL031 Real-Time Clock driver.
///
/// The PL031 is an AMBA APB peripheral providing a 32-bit seconds counter
/// backed by an external battery. All register accesses are 32-bit MMIO.
pub struct Pl031Rtc {
    base: u64,
}

impl Pl031Rtc {
    /// Create a new driver instance bound to the given MMIO base address.
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    /// Read the current seconds count (Unix epoch seconds) from RTCDR.
    #[allow(clippy::cast_possible_truncation)]
    pub fn read_secs(&self) -> u64 {
        // SAFETY: reading a 32-bit MMIO register at the configured base offset.
        // The caller is responsible for ensuring `base` points to a valid PL031.
        let ptr = (self.base + RTCDR) as *const u32;
        unsafe { read_volatile(ptr) as u64 }
    }

    /// Write the seconds count (for time calibration) to RTCLOAD.
    pub fn write_secs(&self, secs: u64) {
        // SAFETY: writing a 32-bit MMIO register at the configured base offset.
        // The caller is responsible for ensuring `base` points to a valid PL031.
        let ptr = (self.base + RTCLOAD) as *mut u32;
        unsafe { write_volatile(ptr, secs as u32) };
    }

    /// Read the current time as an `RtcTime`.
    ///
    /// If the RTC returns 0 the battery is assumed to have failed and the
    /// Unix epoch (1970-01-01 00:00:00, Thursday) is returned.
    pub fn read(&self) -> RtcTime {
        let secs = self.read_secs();
        if secs == 0 {
            // RTC battery failure — return the Unix epoch.
            RtcTime {
                year: 1970,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                weekday: 4, // 1970-01-01 was a Thursday
            }
        } else {
            secs_to_rtc(secs)
        }
    }

    /// Write an `RtcTime` to the RTC (for time calibration).
    pub fn write(&self, t: RtcTime) {
        let secs = rtc_to_secs(&t);
        self.write_secs(secs);
    }

    /// Enable the RTC by setting bit 0 of the control register.
    pub fn enable(&self) {
        // SAFETY: writing a 32-bit MMIO register at the configured base offset.
        let ptr = (self.base + RTCCR) as *mut u32;
        unsafe { write_volatile(ptr, 0x1) };
    }
}

// ============================================================================
// Howard Hinnant's calendar conversion algorithm
// ============================================================================

/// Convert a (year, month, day) civil date to days since 1970-01-01.
///
/// Algorithm by Howard Hinnant. All arithmetic uses `i64` to avoid overflow.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Convert days since 1970-01-01 to a (year, month, day) civil date.
///
/// Algorithm by Howard Hinnant. Returns `(year, month, day)` with month in
/// `[1, 12]` and day in `[1, 31]`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Compute the day of the week from days since 1970-01-01.
///
/// 1970-01-01 was a Thursday (weekday = 4). Returns a value in `[0, 6]`
/// where 0 = Sunday, 1 = Monday, ..., 6 = Saturday.
fn weekday_from_days(days: i64) -> u8 {
    let wd = (days % 7 + 4) % 7; // 1970-01-01 is Thursday (4)
    if wd < 0 {
        (wd + 7) as u8
    } else {
        wd as u8
    }
}

// ============================================================================
// Public conversion functions
// ============================================================================

/// Convert Unix epoch seconds to an `RtcTime`.
#[allow(clippy::cast_possible_truncation)]
pub fn secs_to_rtc(secs: u64) -> RtcTime {
    let days = (secs / 86400) as i64;
    let rem_secs = secs % 86400;
    let (year, month, day) = civil_from_days(days);
    let weekday = weekday_from_days(days);
    RtcTime {
        year: year as u16,
        month: month as u8,
        day: day as u8,
        hour: (rem_secs / 3600) as u8,
        minute: ((rem_secs % 3600) / 60) as u8,
        second: (rem_secs % 60) as u8,
        weekday,
    }
}

/// Convert an `RtcTime` to Unix epoch seconds.
pub fn rtc_to_secs(t: &RtcTime) -> u64 {
    let days = days_from_civil(t.year as i64, t.month as i64, t.day as i64);
    let secs_in_day = t.hour as u64 * 3600 + t.minute as u64 * 60 + t.second as u64;
    (days as u64) * 86400 + secs_in_day
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 0 seconds → 1970-01-01 00:00:00, Thursday (weekday = 4).
    #[test]
    fn test_secs_to_rtc_epoch() {
        let t = secs_to_rtc(0);
        assert_eq!(t.year, 1970);
        assert_eq!(t.month, 1);
        assert_eq!(t.day, 1);
        assert_eq!(t.hour, 0);
        assert_eq!(t.minute, 0);
        assert_eq!(t.second, 0);
        assert_eq!(t.weekday, 4); // Thursday
    }

    /// 1970-01-01 00:00:00 → 0 seconds.
    #[test]
    fn test_rtc_to_secs_epoch() {
        let t = RtcTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            weekday: 4,
        };
        assert_eq!(rtc_to_secs(&t), 0);
    }

    /// Round-trip conversion for several timestamps must be identity.
    #[test]
    fn test_roundtrip() {
        let samples: [u64; 8] = [
            0,
            1,
            86_400,        // exactly 1 day
            951_782_400,   // 2000-02-29 leap day
            1_700_000_000, // 2023-11-14
            1_735_689_600, // 2025-01-01
            1_767_225_600, // 2026-01-01
            1_800_000_000, // 2027-01-15 approx
        ];
        for &secs in &samples {
            let t = secs_to_rtc(secs);
            let back = rtc_to_secs(&t);
            assert_eq!(secs, back, "roundtrip failed for secs = {secs}");
        }
    }

    /// 2024-02-29 (leap day) converts correctly.
    #[test]
    fn test_leap_year() {
        let t = RtcTime {
            year: 2024,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
            second: 0,
            weekday: 2, // Tuesday
        };
        let secs = rtc_to_secs(&t);
        assert_eq!(secs, 1_709_164_800);
        let back = secs_to_rtc(secs);
        assert_eq!(back.year, 2024);
        assert_eq!(back.month, 2);
        assert_eq!(back.day, 29);
    }

    /// 2023-02-28 (last day of a non-leap February).
    #[test]
    fn test_non_leap_year() {
        let t = RtcTime {
            year: 2023,
            month: 2,
            day: 28,
            hour: 23,
            minute: 59,
            second: 59,
            weekday: 0,
        };
        let secs = rtc_to_secs(&t);
        // Adding one second should land on 2023-03-01 00:00:00.
        let next = secs_to_rtc(secs + 1);
        assert_eq!(next.year, 2023);
        assert_eq!(next.month, 3);
        assert_eq!(next.day, 1);
        assert_eq!(next.hour, 0);
        assert_eq!(next.minute, 0);
        assert_eq!(next.second, 0);
    }

    /// 2026-01-31 23:59:59 + 1 second → 2026-02-01 00:00:00.
    #[test]
    fn test_month_end() {
        let t = RtcTime {
            year: 2026,
            month: 1,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            weekday: 0,
        };
        let secs = rtc_to_secs(&t);
        let next = secs_to_rtc(secs + 1);
        assert_eq!(next.year, 2026);
        assert_eq!(next.month, 2);
        assert_eq!(next.day, 1);
        assert_eq!(next.hour, 0);
        assert_eq!(next.minute, 0);
        assert_eq!(next.second, 0);
    }

    /// Verify weekday for several known dates.
    #[test]
    fn test_weekday() {
        // 1970-01-01 is Thursday (4).
        assert_eq!(secs_to_rtc(0).weekday, 4);
        // 951_782_400 = 2000-02-29 is Tuesday (2).
        assert_eq!(secs_to_rtc(951_782_400).weekday, 2);
        // 1_735_689_600 = 2025-01-01 is Wednesday (3).
        assert_eq!(secs_to_rtc(1_735_689_600).weekday, 3);
        // 2026-07-12 — verify via days_from_civil.
        let days = days_from_civil(2026, 7, 12);
        assert_eq!(weekday_from_days(days), 0); // Sunday
                                                // 1_767_225_600 = 2026-01-01 is Thursday (4).
        assert_eq!(secs_to_rtc(1_767_225_600).weekday, 4);
    }

    /// 2025-12-31 23:59:59 + 1 second → 2026-01-01 00:00:00.
    #[test]
    fn test_year_boundary() {
        let t = RtcTime {
            year: 2025,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            weekday: 0,
        };
        let secs = rtc_to_secs(&t);
        let next = secs_to_rtc(secs + 1);
        assert_eq!(next.year, 2026);
        assert_eq!(next.month, 1);
        assert_eq!(next.day, 1);
        assert_eq!(next.hour, 0);
        assert_eq!(next.minute, 0);
        assert_eq!(next.second, 0);
    }

    /// Verify a known timestamp: 1_752_500_000 ≈ 2025-07-14 13:33:20.
    #[test]
    fn test_secs_to_rtc_known() {
        let t = secs_to_rtc(1_752_500_000);
        assert_eq!(t.year, 2025);
        assert_eq!(t.month, 7);
        assert_eq!(t.day, 14);
        assert_eq!(t.hour, 13);
        assert_eq!(t.minute, 33);
        assert_eq!(t.second, 20);
    }

    /// secs = 0 returns the Unix epoch (1970-01-01 00:00:00, Thursday).
    #[test]
    fn test_rtc_default_on_zero() {
        let t = secs_to_rtc(0);
        assert_eq!(
            t,
            RtcTime {
                year: 1970,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                weekday: 4,
            }
        );
    }
}
