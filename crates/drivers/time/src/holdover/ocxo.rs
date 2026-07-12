//! OCXO (Oven-Controlled Crystal Oscillator) frequency compensation model.
//!
//! When the primary BeiDou reference is lost, the OCXO maintains time with
//! frequency compensation. This module provides a linear drift model with
//! temperature compensation to extrapolate time over the holdover period.
//!
//! # Drift Model
//!
//! The extrapolated time is computed as:
//!
//! ```text
//! base_drift    = elapsed_ns * freq_offset_ppb / 1_000_000_000
//! temp_comp     = temperature_c * temp_coeff * elapsed_ns / 1_000_000_000
//! extrapolated  = elapsed + base_drift + temp_comp
//! ```
//!
//! # 24h Accuracy
//!
//! A typical OCXO has frequency stability <= 1e-9/day (1 ppb/day). Over 24
//! hours, the drift is:
//!
//! ```text
//! drift = 86_400_000_000_000 ns * 1 / 1_000_000_000 = 86_400 ns ~ 86.4 us
//! ```
//!
//! This is well below the 1 ms/24h requirement.

use core::time::Duration;

// ============================================================================
// OCXO compensation model
// ============================================================================

/// OCXO frequency compensation model.
///
/// Tracks the frequency offset (in parts-per-billion), temperature
/// coefficient, and the time since the last calibration against a primary
/// reference (e.g., BeiDou 1PPS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OcxoModel {
    /// Frequency offset in parts-per-billion (ppb). Positive means the
    /// oscillator runs fast; negative means it runs slow.
    pub freq_offset_ppb: i64,
    /// Temperature coefficient in ppb/degree-C. Describes how the frequency
    /// changes with temperature.
    pub temp_coeff: i64,
    /// Time since the last calibration against a primary reference.
    pub last_calibration: Duration,
}

impl OcxoModel {
    /// Create a zero-drift model (perfect oscillator).
    pub const fn new() -> Self {
        Self {
            freq_offset_ppb: 0,
            temp_coeff: 0,
            last_calibration: Duration::ZERO,
        }
    }

    /// Create a model with the given frequency offset and temperature
    /// coefficient. `last_calibration` starts at zero (just calibrated).
    pub const fn with_params(freq_offset_ppb: i64, temp_coeff: i64) -> Self {
        Self {
            freq_offset_ppb,
            temp_coeff,
            last_calibration: Duration::ZERO,
        }
    }
}

// ============================================================================
// Time extrapolation
// ============================================================================

/// Extrapolate the elapsed time using the OCXO compensation model.
///
/// Given the nominal `elapsed` time, the oscillator's frequency offset, and
/// the current `temperature_c`, returns the compensated (extrapolated) time.
///
/// # Algorithm
///
/// - Base drift: `elapsed_ns * freq_offset_ppb / 1_000_000_000`
/// - Temperature compensation: `temperature_c * temp_coeff * elapsed_ns / 1_000_000_000`
/// - Result: `elapsed + base_drift + temp_comp`
///
/// Uses `i128` internally to avoid overflow for large durations.
pub fn extrapolate_time(model: &OcxoModel, elapsed: Duration, temperature_c: i32) -> Duration {
    // Use i128 for all intermediate computations to avoid overflow.
    #[allow(clippy::cast_possible_wrap)]
    let elapsed_ns = elapsed.as_nanos() as i128;
    let freq = i128::from(model.freq_offset_ppb);
    let temp = i128::from(temperature_c);
    let temp_coeff = i128::from(model.temp_coeff);

    // Base frequency drift (signed).
    let base_drift_ns = elapsed_ns * freq / 1_000_000_000;

    // Temperature compensation (signed).
    let temp_comp_ns = temp * temp_coeff * elapsed_ns / 1_000_000_000;

    // Extrapolated time = elapsed + drifts.
    let extrapolated_ns = elapsed_ns + base_drift_ns + temp_comp_ns;

    // Clamp to non-negative (a negative extrapolation makes no sense).
    if extrapolated_ns <= 0 {
        Duration::ZERO
    } else {
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        Duration::from_nanos(extrapolated_ns as u64)
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 24h holdover with 1 ppb OCXO: drift must be < 1 ms (key acceptance test).
    #[test]
    fn test_extrapolate_24h_drift_under_1ms() {
        let model = OcxoModel::with_params(1, 0); // 1 ppb, no temp coeff
        let day = Duration::from_secs(86_400);
        let extrapolated = extrapolate_time(&model, day, 25);
        let drift = extrapolated.as_nanos().abs_diff(day.as_nanos());
        assert!(
            drift < 1_000_000,
            "24h drift must be < 1ms (1_000_000 ns), got {drift} ns"
        );
        // Expected: 86_400 ns ~ 86.4 us
        assert_eq!(drift, 86_400);
    }

    /// 24h holdover with 10 ppb OCXO: drift must still be < 1 ms.
    #[test]
    fn test_extrapolate_24h_10ppb_under_1ms() {
        let model = OcxoModel::with_params(10, 0);
        let day = Duration::from_secs(86_400);
        let extrapolated = extrapolate_time(&model, day, 25);
        let drift = extrapolated.as_nanos().abs_diff(day.as_nanos());
        assert!(
            drift < 1_000_000,
            "24h drift at 10ppb must be < 1ms, got {drift} ns"
        );
        assert_eq!(drift, 864_000); // 864 us
    }

    /// Zero-drift model (perfect oscillator) returns the original time.
    #[test]
    fn test_zero_drift_model() {
        let model = OcxoModel::new();
        let elapsed = Duration::from_secs(3600);
        let result = extrapolate_time(&model, elapsed, 25);
        assert_eq!(result, elapsed);
    }

    /// Temperature compensation is correctly applied.
    #[test]
    fn test_temperature_compensation() {
        // freq_offset = 0, temp_coeff = 1 ppb/degree-C, at 50 degrees-C
        // temp_comp = 50 * 1 * elapsed_ns / 1e9
        let model = OcxoModel::with_params(0, 1);
        let elapsed = Duration::from_secs(3600); // 1 hour
        let result = extrapolate_time(&model, elapsed, 50);

        // Expected temp_comp = 50 * 1 * 3_600_000_000_000 / 1_000_000_000 = 180_000 ns
        let expected_drift = 180_000u64;
        let actual_drift = result.as_nanos() - elapsed.as_nanos();
        assert_eq!(actual_drift, u128::from(expected_drift));
    }

    /// Negative frequency offset (clock running slow) produces less time.
    #[test]
    fn test_negative_freq_offset() {
        let model = OcxoModel::with_params(-1, 0);
        let elapsed = Duration::from_secs(86_400);
        let result = extrapolate_time(&model, elapsed, 25);
        // Drift = -86_400 ns, so extrapolated < elapsed
        assert!(result < elapsed);
        let diff = elapsed.as_nanos() - result.as_nanos();
        assert_eq!(diff, 86_400);
    }

    /// High temperature with negative temp_coeff reduces the extrapolated time.
    #[test]
    fn test_negative_temp_coeff() {
        let model = OcxoModel::with_params(0, -1);
        let elapsed = Duration::from_secs(3600);
        let result = extrapolate_time(&model, elapsed, 50);
        // temp_comp = 50 * (-1) * 3.6e12 / 1e9 = -180_000 ns
        assert!(result < elapsed);
        let diff = elapsed.as_nanos() - result.as_nanos();
        assert_eq!(diff, 180_000);
    }

    /// Combined freq offset and temperature compensation.
    #[test]
    fn test_combined_drift() {
        // freq_offset = 2 ppb, temp_coeff = 1 ppb/degree-C, at 10 degrees-C
        // base_drift = 2 * 3.6e12 / 1e9 = 7200 ns
        // temp_comp = 10 * 1 * 3.6e12 / 1e9 = 36000 ns
        // total drift = 43200 ns
        let model = OcxoModel::with_params(2, 1);
        let elapsed = Duration::from_secs(3600);
        let result = extrapolate_time(&model, elapsed, 10);
        let drift = result.as_nanos() - elapsed.as_nanos();
        assert_eq!(drift, 43_200);
    }

    /// Very large drift that would make extrapolated time negative is clamped
    /// to zero.
    #[test]
    fn test_negative_extrapolation_clamped() {
        // Large negative freq offset over a long period.
        let model = OcxoModel::with_params(-10_000_000_000, 0); // -10 ppm... very large
        let elapsed = Duration::from_secs(3600);
        let result = extrapolate_time(&model, elapsed, 25);
        // drift = 3.6e12 * (-10e9) / 1e9 = -3.6e13 -> extrapolated = 3.6e12 - 3.6e13 < 0
        assert_eq!(result, Duration::ZERO);
    }
}
