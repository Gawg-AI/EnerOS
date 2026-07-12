//! NMEA 0183 sentence parser for BeiDou GNSS timing.
//!
//! Supports the two sentence types needed for time synchronization:
//!
//! - **`$GNZDA`** — Time & date: `hhmmss.ss,dd,mm,yyyy,ltzh,ltzn`
//! - **`$GPRMC`** — Recommended minimum: time and fix status
//!
//! All parsing operates on `&[u8]` slices to avoid dynamic allocation.
//! Malformed sentences (bad checksum, truncated, illegal fields) return
//! [`Err(SyncError::ParseError)`](super::SyncError::ParseError) and never panic.

use crate::beidou::SyncError;

// ============================================================================
// Parsed message types
// ============================================================================

/// A successfully parsed NMEA sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NmeaMessage {
    /// `$xxZDA` — UTC time and date.
    Zda {
        hour: u8,
        minute: u8,
        second: u8,
        centisecond: u8,
        day: u8,
        month: u8,
        year: u16,
    },
    /// `$xxRMC` — recommended minimum: time and fix validity.
    Rmc {
        fix_valid: bool,
        hour: u8,
        minute: u8,
        second: u8,
    },
    /// Any other sentence type (talker/sentence not recognized).
    Unknown,
}

// ============================================================================
// Public entry point
// ============================================================================

/// Parse a single NMEA 0183 sentence.
///
/// The `line` must include the leading `$` and trailing `*XX` checksum. The
/// optional `\r\n` terminator is ignored. Sentences without a valid checksum
/// or with illegal field values return [`SyncError::ParseError`].
pub fn parse_nmea(line: &[u8]) -> Result<NmeaMessage, SyncError> {
    // Minimum length: $X*00 = 5 bytes.  In practice sentences are longer,
    // but we check structurally rather than by raw length.
    if line.len() < 6 {
        return Err(SyncError::ParseError);
    }

    // Must start with '$'.
    if line[0] != b'$' {
        return Err(SyncError::ParseError);
    }

    // Find the '*' checksum delimiter.  Strip optional trailing \r\n first.
    let end = line.iter().rposition(|&b| b == b'\n').unwrap_or(line.len());
    let end = if end > 0 && line[end - 1] == b'\r' {
        end - 1
    } else {
        end
    };

    let star_pos = line[..end]
        .iter()
        .position(|&b| b == b'*')
        .ok_or(SyncError::ParseError)?;

    // Need at least 2 hex digits after '*'.
    if end < star_pos + 3 {
        return Err(SyncError::ParseError);
    }

    // Compute XOR checksum of all bytes between '$' (exclusive) and '*'.
    let mut checksum: u8 = 0;
    for &b in &line[1..star_pos] {
        checksum ^= b;
    }

    // Parse the expected checksum (2 hex digits after '*').
    let expected_hi = hex_val(line[star_pos + 1])?;
    let expected_lo = hex_val(line[star_pos + 2])?;
    let expected = (expected_hi << 4) | expected_lo;

    if checksum != expected {
        return Err(SyncError::ParseError);
    }

    // Extract sentence content between '$' and '*'.
    let content = &line[1..star_pos];

    // Split into talker+sentence and the field list at the first ','.
    let (sentence, fields) = match content.iter().position(|&b| b == b',') {
        Some(pos) => (&content[..pos], &content[pos + 1..]),
        None => return Ok(NmeaMessage::Unknown), // no fields
    };

    // Match by sentence type (last 3 chars: ZDA / RMC / ...).
    if sentence.len() < 3 {
        return Ok(NmeaMessage::Unknown);
    }
    let stype = &sentence[sentence.len() - 3..];

    if stype == b"ZDA" {
        parse_zda(fields)
    } else if stype == b"RMC" {
        parse_rmc(fields)
    } else {
        Ok(NmeaMessage::Unknown)
    }
}

// ============================================================================
// ZDA parser:  $xxZDA,hhmmss.ss,dd,mm,yyyy,ltzh,ltzn,nn,rr*hh
// ============================================================================

fn parse_zda(fields: &[u8]) -> Result<NmeaMessage, SyncError> {
    let mut it = FieldSplitter::new(fields);

    let time_field = it.next().ok_or(SyncError::ParseError)?;
    let day_field = it.next().ok_or(SyncError::ParseError)?;
    let month_field = it.next().ok_or(SyncError::ParseError)?;
    let year_field = it.next().ok_or(SyncError::ParseError)?;

    let (hour, minute, second, centisecond) = parse_time(time_field)?;
    let day = parse_u8(day_field)?;
    let month = parse_u8(month_field)?;
    let year = parse_u16(year_field)?;

    // Range validation.  Allow second == 60 for leap-second insertion.
    if hour > 23 || minute > 59 || second > 60 || day == 0 || day > 31 || month == 0 || month > 12 {
        return Err(SyncError::ParseError);
    }
    if centisecond > 99 {
        return Err(SyncError::ParseError);
    }
    // BeiDou started in 2006; reject implausible years.
    if !(2006..=2100).contains(&year) {
        return Err(SyncError::ParseError);
    }

    Ok(NmeaMessage::Zda {
        hour,
        minute,
        second,
        centisecond,
        day,
        month,
        year,
    })
}

// ============================================================================
// RMC parser:  $xxRMC,hhmmss.ss,A,llll.ll,a,yyyyy.yy,a,x,x,ddmmyy,...*hh
// ============================================================================

fn parse_rmc(fields: &[u8]) -> Result<NmeaMessage, SyncError> {
    let mut it = FieldSplitter::new(fields);

    let time_field = it.next().ok_or(SyncError::ParseError)?;
    let status_field = it.next().ok_or(SyncError::ParseError)?;

    let (hour, minute, second, _centisecond) = parse_time(time_field)?;

    // Status: 'A' = valid fix, 'V' = invalid.
    let fix_valid = match status_field {
        [b'A'] | [b'a'] => true,
        [b'V'] | [b'v'] => false,
        _ => return Err(SyncError::ParseError),
    };

    if hour > 23 || minute > 59 || second > 60 {
        return Err(SyncError::ParseError);
    }

    Ok(NmeaMessage::Rmc {
        fix_valid,
        hour,
        minute,
        second,
    })
}

// ============================================================================
// Field splitting (zero-allocation iterator over comma-separated fields)
// ============================================================================

/// Splits a byte slice at commas, yielding each field in order.
///
/// Unlike `str::split(',')`, this correctly yields empty fields for
/// consecutive commas (e.g. `,,` yields two empty slices).
struct FieldSplitter<'a> {
    data: &'a [u8],
    pos: usize,
    exhausted: bool,
}

impl<'a> FieldSplitter<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            exhausted: false,
        }
    }

    /// Return the next field, or `None` when no more fields remain.
    fn next(&mut self) -> Option<&'a [u8]> {
        if self.exhausted {
            return None;
        }
        let start = self.pos;
        let end = self.data[start..]
            .iter()
            .position(|&b| b == b',')
            .map(|p| start + p)
            .unwrap_or(self.data.len());

        if end >= self.data.len() {
            self.exhausted = true;
        } else {
            self.pos = end + 1; // skip the comma
        }
        Some(&self.data[start..end])
    }
}

// ============================================================================
// Numeric parsing helpers
// ============================================================================

/// Parse an NMEA time field `hhmmss.ss` into (hour, minute, second, centisecond).
fn parse_time(s: &[u8]) -> Result<(u8, u8, u8, u8), SyncError> {
    // Minimum: "hhmmss" (6 digits).
    if s.len() < 6 {
        return Err(SyncError::ParseError);
    }
    let hour = two_digits(s, 0)?;
    let minute = two_digits(s, 2)?;
    let second = two_digits(s, 4)?;

    // Optional fractional part: ".cc" (1-2 centisecond digits).
    // "hhmmss" = 6 chars; "hhmmss." = 7; "hhmmss.X" = 8; "hhmmss.XX" = 9.
    let centisecond = if s.len() > 7 && s[6] == b'.' {
        if s.len() == 8 {
            // Single digit after '.': multiply by 10 (".5" → 50 cs).
            one_digit(s, 7)? * 10
        } else {
            // Two or more digits: take the first two.
            two_digits(s, 7)?
        }
    } else {
        0
    };

    Ok((hour, minute, second, centisecond))
}

/// Parse two consecutive ASCII digits at `pos` into a u8 [0..=99].
fn two_digits(s: &[u8], pos: usize) -> Result<u8, SyncError> {
    if pos + 1 >= s.len() {
        return Err(SyncError::ParseError);
    }
    let hi = digit_val(s[pos])?;
    let lo = digit_val(s[pos + 1])?;
    Ok(hi * 10 + lo)
}

/// Parse a single ASCII digit at `pos`.
fn one_digit(s: &[u8], pos: usize) -> Result<u8, SyncError> {
    if pos >= s.len() {
        return Err(SyncError::ParseError);
    }
    digit_val(s[pos])
}

/// Convert an ASCII byte to its numeric value (0-9).
fn digit_val(b: u8) -> Result<u8, SyncError> {
    if b.is_ascii_digit() {
        Ok(b - b'0')
    } else {
        Err(SyncError::ParseError)
    }
}

/// Convert an ASCII hex byte to its numeric value (0-15).
fn hex_val(b: u8) -> Result<u8, SyncError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(SyncError::ParseError),
    }
}

/// Parse a decimal u8 field (e.g. "12" → 12). Empty field is an error.
fn parse_u8(s: &[u8]) -> Result<u8, SyncError> {
    if s.is_empty() {
        return Err(SyncError::ParseError);
    }
    let mut val: u8 = 0;
    for &b in s {
        let d = digit_val(b)?;
        val = val
            .checked_mul(10)
            .and_then(|v| v.checked_add(d))
            .ok_or(SyncError::ParseError)?;
    }
    Ok(val)
}

/// Parse a decimal u16 field (e.g. "2026" → 2026). Empty field is an error.
fn parse_u16(s: &[u8]) -> Result<u16, SyncError> {
    if s.is_empty() {
        return Err(SyncError::ParseError);
    }
    let mut val: u16 = 0;
    for &b in s {
        let d = digit_val(b)? as u16;
        val = val
            .checked_mul(10)
            .and_then(|v| v.checked_add(d))
            .ok_or(SyncError::ParseError)?;
    }
    Ok(val)
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    /// Build a valid NMEA sentence with correct checksum.
    /// `body` excludes the leading `$` and trailing `*XX`.
    fn make_nmea(body: &str) -> Vec<u8> {
        let checksum: u8 = body.bytes().fold(0u8, |acc, b| acc ^ b);
        format!("${}*{:02X}\r\n", body, checksum).into_bytes()
    }

    // ---- ZDA parsing ----

    #[test]
    fn test_parse_zda_normal() {
        let line = make_nmea("GNZDA,123456.78,12,07,2026,,,");
        let msg = parse_nmea(&line).expect("ZDA should parse");
        match msg {
            NmeaMessage::Zda {
                hour,
                minute,
                second,
                centisecond,
                day,
                month,
                year,
            } => {
                assert_eq!(hour, 12);
                assert_eq!(minute, 34);
                assert_eq!(second, 56);
                assert_eq!(centisecond, 78);
                assert_eq!(day, 12);
                assert_eq!(month, 7);
                assert_eq!(year, 2026);
            }
            other => panic!("expected Zda, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_zda_bd_talker() {
        // BeiDou-specific talker "BD" should also match.
        let line = make_nmea("BDZDA,080000.00,01,01,2026,,,");
        let msg = parse_nmea(&line).expect("BDZDA should parse");
        match msg {
            NmeaMessage::Zda {
                hour,
                minute,
                second,
                day,
                month,
                year,
                ..
            } => {
                assert_eq!((hour, minute, second), (8, 0, 0));
                assert_eq!((day, month, year), (1, 1, 2026));
            }
            other => panic!("expected Zda, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_zda_no_centiseconds() {
        // Some receivers omit the fractional part.
        let line = make_nmea("GNZDA,080000,01,01,2026,,,");
        let msg = parse_nmea(&line).expect("should parse without centiseconds");
        if let NmeaMessage::Zda { centisecond, .. } = msg {
            assert_eq!(centisecond, 0);
        } else {
            panic!("expected Zda");
        }
    }

    #[test]
    fn test_parse_zda_single_centisecond_digit() {
        // ".5" means 50 centiseconds (0.5 seconds).
        let line = make_nmea("GNZDA,080000.5,01,01,2026,,,");
        let msg = parse_nmea(&line).expect("should parse single-digit centisecond");
        if let NmeaMessage::Zda { centisecond, .. } = msg {
            assert_eq!(centisecond, 50);
        } else {
            panic!("expected Zda");
        }
    }

    // ---- RMC parsing ----

    #[test]
    fn test_parse_rmc_valid() {
        let line = make_nmea("GPRMC,123456.78,A,4807.038,N,01131.000,E,022.4,084.4,120726,,,A");
        let msg = parse_nmea(&line).expect("RMC should parse");
        match msg {
            NmeaMessage::Rmc {
                fix_valid,
                hour,
                minute,
                second,
            } => {
                assert!(fix_valid);
                assert_eq!((hour, minute, second), (12, 34, 56));
            }
            other => panic!("expected Rmc, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_rmc_invalid_fix() {
        let line = make_nmea("GPRMC,123456.78,V,4807.038,N,01131.000,E,022.4,084.4,120726,,,A");
        let msg = parse_nmea(&line).expect("RMC should parse");
        if let NmeaMessage::Rmc { fix_valid, .. } = msg {
            assert!(!fix_valid);
        } else {
            panic!("expected Rmc");
        }
    }

    #[test]
    fn test_parse_gnrmc_talker() {
        // GNSS combined talker "GN" should also match.
        let line = make_nmea("GNRMC,080000.00,A,,,,,0.0,0.0,010126,,,A");
        let msg = parse_nmea(&line).expect("GNRMC should parse");
        if let NmeaMessage::Rmc {
            fix_valid,
            hour,
            minute,
            second,
        } = msg
        {
            assert!(fix_valid);
            assert_eq!((hour, minute, second), (8, 0, 0));
        } else {
            panic!("expected Rmc");
        }
    }

    // ---- Checksum errors ----

    #[test]
    fn test_parse_bad_checksum() {
        // Replace correct checksum with a wrong one.
        let bad_line = b"$GNZDA,120000.00,01,01,2026,,,*FF\r\n";
        let err = parse_nmea(bad_line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    #[test]
    fn test_parse_checksum_case_insensitive() {
        // Lowercase hex digits should be accepted.
        let body = "GNZDA,120000.00,01,01,2026,,,";
        let checksum: u8 = body.bytes().fold(0u8, |acc, b| acc ^ b);
        let line = format!("${}*{:02x}\r\n", body, checksum);
        let msg = parse_nmea(line.as_bytes());
        assert!(msg.is_ok(), "lowercase checksum should be accepted");
    }

    // ---- Truncated / malformed ----

    #[test]
    fn test_parse_truncated_too_short() {
        assert_eq!(parse_nmea(b"").unwrap_err(), SyncError::ParseError);
        assert_eq!(parse_nmea(b"$").unwrap_err(), SyncError::ParseError);
        assert_eq!(parse_nmea(b"$GN").unwrap_err(), SyncError::ParseError);
    }

    #[test]
    fn test_parse_no_star() {
        let line = b"$GNZDA,120000.00,01,01,2026,,,XX";
        let err = parse_nmea(line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    #[test]
    fn test_parse_no_dollar() {
        let line = b"GNZDA,120000.00,01,01,2026,,,*XX";
        let err = parse_nmea(line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    #[test]
    fn test_parse_checksum_truncated() {
        // *X (only one hex digit after *)
        let line = b"$GNZDA,120000.00,01,01,2026,,,*5";
        let err = parse_nmea(line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    // ---- Illegal fields ----

    #[test]
    fn test_parse_zda_illegal_hour() {
        // hour = 25
        let line = make_nmea("GNZDA,250000.00,01,01,2026,,,");
        let err = parse_nmea(&line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    #[test]
    fn test_parse_zda_illegal_day() {
        // day = 00
        let line = make_nmea("GNZDA,120000.00,00,01,2026,,,");
        let err = parse_nmea(&line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    #[test]
    fn test_parse_zda_illegal_month() {
        // month = 13
        let line = make_nmea("GNZDA,120000.00,01,13,2026,,,");
        let err = parse_nmea(&line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    #[test]
    fn test_parse_zda_non_numeric_field() {
        // day = "AB"
        let line = make_nmea("GNZDA,120000.00,AB,01,2026,,,");
        let err = parse_nmea(&line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    #[test]
    fn test_parse_zda_year_out_of_range() {
        // year = 1999 (before BeiDou)
        let line = make_nmea("GNZDA,120000.00,01,01,1999,,,");
        let err = parse_nmea(&line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    // ---- Leap second boundary ----

    #[test]
    fn test_parse_zda_leap_second_june() {
        // 2012-06-30 23:59:60 — leap second insertion.
        let line = make_nmea("GNZDA,235960.00,30,06,2012,,,");
        let msg = parse_nmea(&line).expect("leap second should parse");
        if let NmeaMessage::Zda { second, .. } = msg {
            assert_eq!(second, 60, "second == 60 during leap second");
        } else {
            panic!("expected Zda");
        }
    }

    #[test]
    fn test_parse_zda_leap_second_december() {
        // 2016-12-31 23:59:60 — leap second insertion.
        let line = make_nmea("GNZDA,235960.00,31,12,2016,,,");
        let msg = parse_nmea(&line).expect("leap second should parse");
        if let NmeaMessage::Zda { second, .. } = msg {
            assert_eq!(second, 60);
        } else {
            panic!("expected Zda");
        }
    }

    #[test]
    fn test_parse_zda_illegal_second() {
        // second = 61 is never valid.
        let line = make_nmea("GNZDA,235961.00,31,12,2016,,,");
        let err = parse_nmea(&line).unwrap_err();
        assert_eq!(err, SyncError::ParseError);
    }

    // ---- Unknown sentences ----

    #[test]
    fn test_parse_unknown_sentence() {
        let line = make_nmea("GPGGA,123456.78,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,");
        let msg = parse_nmea(&line).expect("unknown sentence should not error");
        assert_eq!(msg, NmeaMessage::Unknown);
    }

    #[test]
    fn test_parse_empty_fields() {
        // ZDA with empty local-time fields (common for GNSS receivers).
        let line = make_nmea("GNZDA,120000.00,01,01,2026,,,,,,");
        let msg = parse_nmea(&line).expect("should parse with empty trailing fields");
        if let NmeaMessage::Zda {
            day, month, year, ..
        } = msg
        {
            assert_eq!((day, month, year), (1, 1, 2026));
        } else {
            panic!("expected Zda");
        }
    }

    // ---- Field splitter ----

    #[test]
    fn test_field_splitter_basic() {
        let mut it = FieldSplitter::new(b"a,b,c");
        assert_eq!(it.next(), Some(b"a" as &[u8]));
        assert_eq!(it.next(), Some(b"b" as &[u8]));
        assert_eq!(it.next(), Some(b"c" as &[u8]));
        assert_eq!(it.next(), None);
    }

    #[test]
    fn test_field_splitter_empty_fields() {
        let mut it = FieldSplitter::new(b",,a,,");
        assert_eq!(it.next(), Some(b"" as &[u8]));
        assert_eq!(it.next(), Some(b"" as &[u8]));
        assert_eq!(it.next(), Some(b"a" as &[u8]));
        assert_eq!(it.next(), Some(b"" as &[u8]));
        assert_eq!(it.next(), Some(b"" as &[u8]));
        assert_eq!(it.next(), None);
    }

    #[test]
    fn test_field_splitter_single() {
        let mut it = FieldSplitter::new(b"hello");
        assert_eq!(it.next(), Some(b"hello" as &[u8]));
        assert_eq!(it.next(), None);
    }
}
