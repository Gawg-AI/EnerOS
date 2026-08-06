//! Feedback reconciliation (CTL-010).
//!
//! Before the safety validator (CTL-008) trusts a telemetry value, that value
//! must be **reconciled**: fresh (within `max_age`), good quality, and within a
//! sane physical range. Stale, bad-quality or out-of-range feedback is rejected
//! — the safety validator then treats the relevant context as unknown, which
//! drives a fail-safe (deny) decision rather than acting on garbage.
//!
//! This module is the independent feedback path: it does not consume the
//! command generator's view of plant state. It produces a [`Reconciled`] verdict
//! that the validator consumes.

use eneros_telemetry_model::quality::QualityFlag;

/// A raw telemetry sample for a single quantity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeedbackSample {
    pub value: f64,
    pub quality: QualityFlag,
    pub timestamp_epoch: u64,
}

/// Reconciliation verdict.
#[derive(Debug, Clone, PartialEq)]
pub enum Reconciled {
    /// Fresh, good-quality, in-range. The value may be trusted.
    Ok(f64),
    /// Feedback is too old (now - timestamp > max_age).
    Stale { age_secs: u64, max_age_secs: u64 },
    /// Quality flag indicates the value is not trustworthy.
    BadQuality(QualityFlag),
    /// Value is outside the sane physical range.
    OutOfRange { value: f64, min: f64, max: f64 },
}

/// Reconcile one feedback sample against freshness / quality / range rules.
pub fn reconcile(
    sample: &FeedbackSample,
    now: u64,
    max_age_secs: u64,
    sane_min: f64,
    sane_max: f64,
) -> Reconciled {
    // 1. Freshness.
    let age = now.saturating_sub(sample.timestamp_epoch);
    if age > max_age_secs {
        return Reconciled::Stale {
            age_secs: age,
            max_age_secs,
        };
    }
    // 2. Quality. Only `Good` is trusted directly; everything else is rejected.
    if sample.quality != QualityFlag::Good {
        return Reconciled::BadQuality(sample.quality);
    }
    // 3. Sane range.
    if sample.value < sane_min || sample.value > sane_max {
        return Reconciled::OutOfRange {
            value: sample.value,
            min: sane_min,
            max: sane_max,
        };
    }
    Reconciled::Ok(sample.value)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_telemetry_model::quality::QualityFlag;

    use super::*;

    fn sample(value: f64, quality: QualityFlag, ts: u64) -> FeedbackSample {
        FeedbackSample {
            value,
            quality,
            timestamp_epoch: ts,
        }
    }

    #[test]
    fn test_ok_fresh_good_in_range() {
        let s = sample(50.0, QualityFlag::Good, 1_000);
        assert_eq!(reconcile(&s, 1_010, 60, 0.0, 100.0), Reconciled::Ok(50.0));
    }

    #[test]
    fn test_stale_rejected() {
        let s = sample(50.0, QualityFlag::Good, 1_000);
        assert_eq!(
            reconcile(&s, 1_100, 60, 0.0, 100.0),
            Reconciled::Stale {
                age_secs: 100,
                max_age_secs: 60
            }
        );
    }

    #[test]
    fn test_bad_quality_rejected() {
        let s = sample(50.0, QualityFlag::Invalid, 1_000);
        assert_eq!(
            reconcile(&s, 1_010, 60, 0.0, 100.0),
            Reconciled::BadQuality(QualityFlag::Invalid)
        );
    }

    #[test]
    fn test_questionable_rejected() {
        let s = sample(50.0, QualityFlag::Questionable, 1_000);
        assert!(matches!(
            reconcile(&s, 1_010, 60, 0.0, 100.0),
            Reconciled::BadQuality(QualityFlag::Questionable)
        ));
    }

    #[test]
    fn test_out_of_range_rejected() {
        let s = sample(150.0, QualityFlag::Good, 1_000);
        assert!(matches!(
            reconcile(&s, 1_010, 60, 0.0, 100.0),
            Reconciled::OutOfRange { .. }
        ));
    }
}
