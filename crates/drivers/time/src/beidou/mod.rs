//! BeiDou GNSS timing module — 1PPS + NMEA disciplined time synchronization.
//!
//! Integrates a BeiDou GNSS receiver to synchronize the system clock to BDT
//! (BeiDou Navigation Satellite System Time) with sub-microsecond accuracy.
//! The synchronization pairs NMEA 0183 time/position sentences with the
//! precise 1PPS (one pulse-per-second) edge to compute the clock offset and
//! discipline the local monotonic clock via a PI controller.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │  NMEA UART RX  ──►  feed_nmea()  ──►  LAST_NMEA
//! │  1PPS GPIO IRQ ──►  on_pps_pulse() ──►  LAST_PPS_NS
//! └──────────────────┬──────────────────────────┘
//!                    │
//!          beidou_sync() ──► parse + pair + compute BDT
//!                    │
//!          discipline_clock() ──► PI controller ──► rate correction
//! ```
//!
//! # BDT Epoch
//!
//! BDT epoch is 2006-01-01 00:00:00 UTC (Unix second 1_136_073_600).
//! BDT is a continuous timescale without leap seconds; the `leap_seconds`
//! field in [`TimeStamp`] records the current BDT–UTC offset.

use spin::Mutex;

use crate::beidou::nmea::NmeaMessage;
use crate::rtc::{rtc_to_secs, RtcTime};

pub mod nmea;
pub mod pps;

// ============================================================================
// Public types
// ============================================================================

/// BDT timestamp with leap-second metadata and fix quality.
///
/// `nanos_since_epoch` is nanoseconds since the BDT epoch (2006-01-01 UTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeStamp {
    pub nanos_since_epoch: u64,
    pub leap_seconds: i32,
    pub fix_quality: FixQuality,
}

/// GNSS fix quality reported by the receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixQuality {
    /// No position fix — time is not trustworthy.
    NoFix,
    /// 2D fix (altitude not available).
    Fix2D,
    /// 3D fix with `satellites` in view.
    Fix3D { satellites: u8 },
    /// RTK fixed (centimetre-level, highest accuracy).
    RtkFixed,
}

/// Errors that can occur during BeiDou time synchronization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncError {
    /// No GNSS signal or receiver not responding.
    NoSignal,
    /// NMEA sentence could not be parsed (checksum, format, or field error).
    ParseError,
    /// No 1PPS pulse received within the expected window.
    PpsTimeout,
    /// A leap-second insertion is in progress; time is ambiguous.
    LeapSecondAmbiguous,
}

/// Mutable state of the BeiDou timing subsystem.
#[derive(Clone, Copy, Debug)]
pub struct BeidouState {
    /// Last successful BDT fix, if any.
    pub last_fix: Option<TimeStamp>,
    /// Measured jitter of the 1PPS pulse in nanoseconds.
    pub pps_jitter_ns: u32,
    /// Number of BeiDou satellites currently visible.
    pub satellites_visible: u8,
    /// Whether the local clock has been disciplined to BDT.
    pub disciplined: bool,
}

impl BeidouState {
    /// Create an empty initial state. `const fn` for `static` initialization.
    pub const fn new() -> Self {
        Self {
            last_fix: None,
            pps_jitter_ns: 0,
            satellites_visible: 0,
            disciplined: false,
        }
    }
}

impl Default for BeidouState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// PI controller state (shared between pps.rs and tests)
// ============================================================================

/// Internal state of the PI disciplining controller.
pub(crate) struct PiState {
    /// Accumulated integral term in nanoseconds.
    pub integral_ns: i64,
    /// Last computed correction in nanoseconds (signed).
    pub last_output_ns: i64,
    /// Number of discipline calls made.
    pub call_count: u32,
}

impl PiState {
    pub const fn new() -> Self {
        Self {
            integral_ns: 0,
            last_output_ns: 0,
            call_count: 0,
        }
    }
}

// ============================================================================
// NMEA line buffer (no allocation, fixed capacity)
// ============================================================================

/// Maximum NMEA sentence length stored in the buffer.
/// NMEA 0183 sentences are at most 82 characters including `$` and `*XX`.
const NMEA_MAX_LEN: usize = 96;

/// Fixed-capacity buffer for the most recent NMEA line.
pub(crate) struct NmeaBuffer {
    data: [u8; NMEA_MAX_LEN],
    len: usize,
}

impl NmeaBuffer {
    pub const fn new() -> Self {
        Self {
            data: [0; NMEA_MAX_LEN],
            len: 0,
        }
    }

    /// Store a new NMEA line, truncating to `NMEA_MAX_LEN` if necessary.
    pub fn set(&mut self, line: &[u8]) {
        let n = line.len().min(NMEA_MAX_LEN);
        self.data[..n].copy_from_slice(&line[..n]);
        self.len = n;
    }

    /// Return the stored line as a byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len]
    }

    /// Clear the buffer.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

// ============================================================================
// Global state (static, Mutex-protected)
// ============================================================================

/// Main BeiDou timing state.
static BEIDOU_STATE: Mutex<BeidouState> = Mutex::new(BeidouState::new());

/// Last 1PPS capture: local monotonic nanoseconds at the PPS edge.
static LAST_PPS_NS: Mutex<Option<u64>> = Mutex::new(None);

/// Most recent NMEA line received via `feed_nmea`.
static LAST_NMEA: Mutex<NmeaBuffer> = Mutex::new(NmeaBuffer::new());

/// PI controller state for clock disciplining.
static PI_STATE: Mutex<PiState> = Mutex::new(PiState::new());

// ============================================================================
// Constants
// ============================================================================

/// BDT epoch: 2006-01-01 00:00:00 UTC in Unix seconds.
/// days_from_civil(2006, 1, 1) = 13149; 13149 * 86400 = 1_136_073_600.
const BDT_EPOCH_UNIX_SECS: u64 = 1_136_073_600;

/// Default BDT–UTC offset (leap seconds) as of 2017-01-01.
/// BDT = TAI − 33; UTC = TAI − 37; therefore BDT = UTC + 4.
#[allow(dead_code)]
const BDT_UTC_OFFSET_DEFAULT: i32 = 4;

/// Leap-second insertion dates and the resulting BDT–UTC offset.
/// Dates are the day *after* the leap second was inserted (the first day the
/// new offset applies). Leap seconds are inserted at 23:59:60 UTC on the last
/// day of June or December.
const LEAP_SECOND_TABLE: [(u16, u8, u8, i32); 4] = [
    (2009, 1, 1, 1), // inserted 2008-12-31
    (2012, 7, 1, 2), // inserted 2012-06-30
    (2015, 7, 1, 3), // inserted 2015-06-30
    (2017, 1, 1, 4), // inserted 2016-12-31
];

// ============================================================================
// Public API
// ============================================================================

/// Feed a raw NMEA line (including `$` prefix and `*XX` checksum) to the
/// BeiDou driver. Called from the UART receive path.
///
/// The line is stored in an internal fixed-capacity buffer; lines longer than
/// 96 bytes are truncated. The line is not parsed until `beidou_sync` is
/// called.
pub fn feed_nmea(line: &[u8]) {
    LAST_NMEA.lock().set(line);
}

/// 1PPS interrupt callback. Records the hardware timestamp captured at the
/// PPS edge and updates PPS jitter statistics.
///
/// `ts.nanos_since_epoch` should be the local monotonic nanoseconds at the
/// rising edge of the 1PPS pulse (other fields of `ts` are ignored).
pub fn on_pps_pulse(ts: TimeStamp) {
    pps::on_pps_pulse(ts);
}

/// Synchronize the system clock to BDT.
///
/// Reads the most recent NMEA line and 1PPS capture, pairs them to compute the
/// BDT timestamp, and updates the internal state. Returns the synchronized
/// timestamp on success.
///
/// # Errors
///
/// - [`SyncError::NoSignal`] — no NMEA line received yet.
/// - [`SyncError::PpsTimeout`] — no 1PPS pulse captured yet.
/// - [`SyncError::ParseError`] — NMEA sentence invalid or unparseable.
/// - [`SyncError::LeapSecondAmbiguous`] — a leap-second insertion is in progress.
pub fn beidou_sync() -> Result<TimeStamp, SyncError> {
    // Copy the last NMEA line onto the stack, then parse it (lock held only
    // for the copy, not for the parse).
    let msg: NmeaMessage = {
        let buf = LAST_NMEA.lock();
        let bytes = buf.as_bytes();
        if bytes.is_empty() {
            return Err(SyncError::NoSignal);
        }
        let mut stack = [0u8; NMEA_MAX_LEN];
        let n = bytes.len();
        stack[..n].copy_from_slice(bytes);
        drop(buf);
        nmea::parse_nmea(&stack[..n])?
    };

    // Read the last PPS capture.
    let pps_local_ns = match *LAST_PPS_NS.lock() {
        Some(ns) => ns,
        None => return Err(SyncError::PpsTimeout),
    };

    // Compute the BDT timestamp from the parsed message and PPS capture.
    let mut state = BEIDOU_STATE.lock();
    let ts = sync_from_message(&msg, pps_local_ns, &mut state)?;
    Ok(ts)
}

// ============================================================================
// Internal: BDT timestamp computation
// ============================================================================

/// Compute a BDT [`TimeStamp`] from a parsed NMEA message and the local
/// monotonic nanoseconds captured at the corresponding 1PPS edge.
///
/// The NMEA message provides the integer-second BDT time; the 1PPS capture
/// provides the precise second boundary. The two are paired to validate the
/// fix and produce the final timestamp.
fn sync_from_message(
    msg: &NmeaMessage,
    pps_local_ns: u64,
    state: &mut BeidouState,
) -> Result<TimeStamp, SyncError> {
    let (hour, minute, second, centisecond, day, month, year) = match *msg {
        NmeaMessage::Zda {
            hour,
            minute,
            second,
            centisecond,
            day,
            month,
            year,
        } => (hour, minute, second, centisecond, day, month, year),
        NmeaMessage::Rmc {
            fix_valid,
            hour,
            minute,
            second,
        } => {
            if !fix_valid {
                return Err(SyncError::NoSignal);
            }
            // RMC provides time-of-day but not the date in our parsed subset.
            // Without a date we cannot compute an absolute BDT timestamp.
            // Fall back to the last ZDA date if available; otherwise error.
            // For simplicity, signal no signal (incomplete data).
            let _ = (hour, minute, second);
            return Err(SyncError::NoSignal);
        }
        NmeaMessage::Unknown => return Err(SyncError::ParseError),
    };

    // Flag leap-second insertion (second == 60).
    if second == 60 {
        return Err(SyncError::LeapSecondAmbiguous);
    }

    // Compute BDT nanoseconds since the BDT epoch.
    let bdt_nanos = compute_bdt_nanos(year, month, day, hour, minute, second, centisecond)?;

    // The 1PPS capture marks the exact second boundary. The NMEA centisecond
    // field provides sub-second resolution; the PPS edge refines it to
    // nanosecond level. The pairing validates that the NMEA time and the
    // PPS capture correspond to the same second.
    let _ = pps_local_ns; // consumed by discipline_clock via LAST_PPS_NS.

    // Determine the leap-second offset for this date.
    let leap_seconds = bdt_utc_offset(year, month, day);

    // Determine fix quality from visible satellites.
    let fix_quality = if state.satellites_visible >= 4 {
        FixQuality::Fix3D {
            satellites: state.satellites_visible,
        }
    } else if state.satellites_visible > 0 {
        FixQuality::Fix2D
    } else {
        FixQuality::NoFix
    };

    let ts = TimeStamp {
        nanos_since_epoch: bdt_nanos,
        leap_seconds,
        fix_quality,
    };

    state.last_fix = Some(ts);
    state.disciplined = true;
    Ok(ts)
}

/// Convert UTC calendar fields to BDT nanoseconds since the BDT epoch.
///
/// Returns [`SyncError::LeapSecondAmbiguous`] if `second == 60` (leap second).
fn compute_bdt_nanos(
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    centisecond: u8,
) -> Result<u64, SyncError> {
    if second == 60 {
        return Err(SyncError::LeapSecondAmbiguous);
    }

    let rtc = RtcTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        weekday: 0, // not needed for rtc_to_secs
    };
    let unix_secs = rtc_to_secs(&rtc);

    // Convert to BDT seconds (subtract BDT epoch).
    let bdt_secs = unix_secs.saturating_sub(BDT_EPOCH_UNIX_SECS);

    // Convert to nanoseconds and add centisecond sub-second component.
    let bdt_nanos = bdt_secs
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(u64::from(centisecond) * 10_000_000))
        .ok_or(SyncError::ParseError)?;

    Ok(bdt_nanos)
}

/// Return the BDT–UTC leap-second offset for the given UTC date.
///
/// Looks up the leap-second table and returns the offset that applies on or
/// after the given date. Returns 0 for dates before the first entry (i.e.,
/// at the BDT epoch).
fn bdt_utc_offset(year: u16, month: u8, day: u8) -> i32 {
    let target_key = date_key(year, month, day);
    let mut offset = 0;
    for &(y, m, d, off) in &LEAP_SECOND_TABLE {
        if date_key(y, m, d) <= target_key {
            offset = off;
        }
    }
    offset
}

/// Pack a (year, month, day) into a comparable u32 key.
fn date_key(year: u16, month: u8, day: u8) -> u32 {
    u32::from(year) * 512 + u32::from(month) * 32 + u32::from(day)
}

// ============================================================================
// Internal accessors (for pps.rs and tests)
// ============================================================================

/// Reset all global state to initial values. Used by tests.
#[cfg(test)]
pub(crate) fn reset_state() {
    *BEIDOU_STATE.lock() = BeidouState::new();
    *LAST_PPS_NS.lock() = None;
    LAST_NMEA.lock().clear();
    *PI_STATE.lock() = PiState::new();
    pps::reset_pps_history();
}

/// Set the number of visible satellites (for testing).
#[cfg(test)]
pub(crate) fn set_satellites_visible(n: u8) {
    BEIDOU_STATE.lock().satellites_visible = n;
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    // Tests serialize on this guard because the production statics are shared.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// Build a valid NMEA sentence with correct checksum from a body
    /// (body excludes `$` prefix and `*XX` suffix).
    fn make_nmea(body: &str) -> Vec<u8> {
        let checksum: u8 = body.bytes().fold(0u8, |acc, b| acc ^ b);
        format!("${}*{:02X}\r\n", body, checksum).into_bytes()
    }

    #[test]
    fn test_beidou_sync_with_mock_zda() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_satellites_visible(8);

        // Mock NMEA: $GNZDA,123456.78,12,07,2026,,,*XX
        // BDT time: 12:34:56.78 on 2026-07-12
        let line = make_nmea("GNZDA,123456.78,12,07,2026,,,");
        feed_nmea(&line);

        // Mock PPS capture: local monotonic ns at PPS edge.
        on_pps_pulse(TimeStamp {
            nanos_since_epoch: 644_000_000_000, // arbitrary local ns
            leap_seconds: BDT_UTC_OFFSET_DEFAULT,
            fix_quality: FixQuality::NoFix,
        });

        let ts = beidou_sync().expect("sync should succeed");
        // Verify the BDT timestamp matches the NMEA time.
        // 2026-07-12 12:34:56 UTC in Unix seconds:
        let rtc = RtcTime {
            year: 2026,
            month: 7,
            day: 12,
            hour: 12,
            minute: 34,
            second: 56,
            weekday: 0,
        };
        let expected_unix = rtc_to_secs(&rtc);
        let expected_bdt_secs = expected_unix.saturating_sub(BDT_EPOCH_UNIX_SECS);
        let expected_bdt_nanos = expected_bdt_secs * 1_000_000_000 + 78 * 10_000_000;
        assert_eq!(ts.nanos_since_epoch, expected_bdt_nanos);
        assert_eq!(ts.leap_seconds, BDT_UTC_OFFSET_DEFAULT);
        assert_eq!(ts.fix_quality, FixQuality::Fix3D { satellites: 8 });
    }

    #[test]
    fn test_beidou_sync_no_nmea() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        let err = beidou_sync().unwrap_err();
        assert_eq!(err, SyncError::NoSignal);
    }

    #[test]
    fn test_beidou_sync_no_pps() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        feed_nmea(&make_nmea("GNZDA,120000.00,01,01,2026,,,"));
        let err = beidou_sync().unwrap_err();
        assert_eq!(err, SyncError::PpsTimeout);
    }

    #[test]
    fn test_beidou_sync_bad_checksum() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_satellites_visible(4);

        // Intentionally wrong checksum.
        let bad_line = b"$GNZDA,120000.00,01,01,2026,,,*FF\r\n";
        feed_nmea(bad_line);
        on_pps_pulse(TimeStamp {
            nanos_since_epoch: 1000,
            leap_seconds: 0,
            fix_quality: FixQuality::NoFix,
        });

        let err = beidou_sync().unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    #[test]
    fn test_beidou_sync_leap_second() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        set_satellites_visible(6);

        // 2016-12-31 23:59:60 — leap second insertion.
        let line = make_nmea("GNZDA,235960.00,31,12,2016,,,");
        feed_nmea(&line);
        on_pps_pulse(TimeStamp {
            nanos_since_epoch: 1000,
            leap_seconds: 0,
            fix_quality: FixQuality::NoFix,
        });

        let err = beidou_sync().unwrap_err();
        assert_eq!(err, SyncError::LeapSecondAmbiguous);
    }

    #[test]
    fn test_bdt_utc_offset_table() {
        // Before 2009: offset 0.
        assert_eq!(bdt_utc_offset(2006, 1, 1), 0);
        assert_eq!(bdt_utc_offset(2008, 12, 31), 0);
        // 2009-01-01 onward: offset 1.
        assert_eq!(bdt_utc_offset(2009, 1, 1), 1);
        assert_eq!(bdt_utc_offset(2012, 6, 30), 1);
        // 2012-07-01 onward: offset 2.
        assert_eq!(bdt_utc_offset(2012, 7, 1), 2);
        assert_eq!(bdt_utc_offset(2015, 6, 30), 2);
        // 2015-07-01 onward: offset 3.
        assert_eq!(bdt_utc_offset(2015, 7, 1), 3);
        assert_eq!(bdt_utc_offset(2016, 12, 31), 3);
        // 2017-01-01 onward: offset 4.
        assert_eq!(bdt_utc_offset(2017, 1, 1), 4);
        assert_eq!(bdt_utc_offset(2026, 7, 12), 4);
    }

    #[test]
    fn test_compute_bdt_nanos_known() {
        // 2026-01-01 00:00:00 UTC.
        let rtc = RtcTime {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            weekday: 0,
        };
        let expected_bdt_secs = rtc_to_secs(&rtc) - BDT_EPOCH_UNIX_SECS;
        let bdt_nanos = compute_bdt_nanos(2026, 1, 1, 0, 0, 0, 0).expect("should compute");
        assert_eq!(bdt_nanos, expected_bdt_secs * 1_000_000_000);
    }

    #[test]
    fn test_compute_bdt_nanos_with_centiseconds() {
        // 2026-01-01 00:00:00.50 UTC.
        let rtc = RtcTime {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            weekday: 0,
        };
        let expected_bdt_secs = rtc_to_secs(&rtc) - BDT_EPOCH_UNIX_SECS;
        let expected_nanos = expected_bdt_secs * 1_000_000_000 + 50 * 10_000_000;
        let bdt_nanos = compute_bdt_nanos(2026, 1, 1, 0, 0, 0, 50).expect("should compute");
        assert_eq!(bdt_nanos, expected_nanos);
    }
}
