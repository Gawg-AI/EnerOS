//! Validation rules engine inspired by pandapower's `diagnostic_reports` and
//! cnpower's `validation_rules.py`.
//!
//! Provides system-level validation rules that go beyond equipment compliance
//! (covered by `compliance.rs`). These rules validate the operating state of
//! the whole network against national standards for:
//!
//! * **Voltage quality** — GB/T 12325 (voltage deviation), GB/T 15945 (frequency),
//!   GB/T 12326 (voltage fluctuation & flicker), GB/T 14549 (harmonics)
//! * **N-1 safety** — GB/T 38306 (N-1 contingency security for transmission)
//!   and DL/T 7233 (N-1 for distribution)
//! * **Short-circuit** — GB/T 15544 (short-circuit current calculation),
//!   breaking capacity check, fault clearing time
//!
//! Rules return a three-state `ValidationStatus` (Passed/Failed/Inconclusive)
//! so that missing data is never silently treated as compliance.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Status & finding
// ---------------------------------------------------------------------------

/// Three-state validation result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValidationStatus {
    /// The system satisfies the rule.
    Passed,
    /// The system violates the rule.
    Failed { detail: String },
    /// Required data is missing to evaluate the rule.
    Inconclusive { detail: String },
}

impl ValidationStatus {
    pub fn passed(&self) -> bool {
        matches!(self, ValidationStatus::Passed)
    }
    pub fn failed(&self) -> bool {
        matches!(self, ValidationStatus::Failed { .. })
    }
}

/// A single validation finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationFinding {
    /// Rule identifier (e.g., "VQ_DEV_001").
    pub rule_id: String,
    /// Human-readable rule description.
    pub description: String,
    /// Referenced national standard.
    pub standard: String,
    /// Validation result.
    pub status: ValidationStatus,
    /// Measured value (if available).
    pub measured_value: Option<f64>,
    /// Limit value (if applicable).
    pub limit_value: Option<f64>,
    /// Unit of the measured/limit values.
    pub unit: String,
}

impl ValidationFinding {
    fn passed(rule_id: &str, description: &str, standard: &str, unit: &str) -> Self {
        Self {
            rule_id: rule_id.to_string(),
            description: description.to_string(),
            standard: standard.to_string(),
            status: ValidationStatus::Passed,
            measured_value: None,
            limit_value: None,
            unit: unit.to_string(),
        }
    }

    fn failed(
        rule_id: &str,
        description: &str,
        standard: &str,
        measured: f64,
        limit: f64,
        unit: &str,
        detail: &str,
    ) -> Self {
        Self {
            rule_id: rule_id.to_string(),
            description: description.to_string(),
            standard: standard.to_string(),
            status: ValidationStatus::Failed {
                detail: detail.to_string(),
            },
            measured_value: Some(measured),
            limit_value: Some(limit),
            unit: unit.to_string(),
        }
    }

    fn inconclusive(
        rule_id: &str,
        description: &str,
        standard: &str,
        unit: &str,
        detail: &str,
    ) -> Self {
        Self {
            rule_id: rule_id.to_string(),
            description: description.to_string(),
            standard: standard.to_string(),
            status: ValidationStatus::Inconclusive {
                detail: detail.to_string(),
            },
            measured_value: None,
            limit_value: None,
            unit: unit.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Input data structures
// ---------------------------------------------------------------------------

/// Per-bus voltage observation used by voltage-quality rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BusVoltageObservation {
    /// Bus identifier.
    pub bus_id: String,
    /// Nominal voltage (kV).
    pub nominal_kv: f64,
    /// Measured voltage magnitude (kV).
    pub measured_kv: Option<f64>,
    /// Voltage total harmonic distortion (%) — for GB/T 14549.
    pub thd_percent: Option<f64>,
    /// Long-term flicker severity (Plt) — for GB/T 12326.
    pub plt: Option<f64>,
}

/// System-wide frequency observation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrequencyObservation {
    /// Nominal frequency (Hz), typically 50.0 in China.
    pub nominal_hz: f64,
    /// Measured frequency (Hz).
    pub measured_hz: Option<f64>,
}

/// N-1 contingency observation for a single branch outage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContingencyObservation {
    /// Identifier of the outaged branch.
    pub branch_id: String,
    /// Post-contingency maximum bus voltage deviation (p.u.).
    pub max_voltage_deviation_pu: Option<f64>,
    /// Post-contingency maximum branch loading (%).
    pub max_loading_percent: Option<f64>,
    /// Whether any bus collapsed (voltage → 0) after the contingency.
    pub bus_collapse: bool,
}

/// Short-circuit observation at a bus.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShortCircuitObservation {
    /// Bus identifier.
    pub bus_id: String,
    /// Nominal voltage (kV).
    pub nominal_kv: f64,
    /// Calculated three-phase short-circuit current (kA).
    pub ik_3ph_ka: Option<f64>,
    /// Breaking capacity of the local breaker (kA).
    pub breaker_capacity_ka: Option<f64>,
    /// Fault clearing time (s).
    pub fault_clearing_time_s: Option<f64>,
}

/// Aggregated system state for validation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemStateSnapshot {
    /// Per-bus voltage observations.
    pub buses: Vec<BusVoltageObservation>,
    /// System frequency observation.
    pub frequency: Option<FrequencyObservation>,
    /// N-1 contingency observations.
    pub contingencies: Vec<ContingencyObservation>,
    /// Short-circuit observations.
    pub short_circuits: Vec<ShortCircuitObservation>,
}

// ---------------------------------------------------------------------------
// Validator
// ---------------------------------------------------------------------------

/// Validation rule engine.
///
/// Each `check_*` method evaluates one rule family and returns a list of
/// findings (one per bus / contingency / fault location). Use `validate_all`
/// to run every rule family in sequence.
#[derive(Debug, Clone, Default)]
pub struct ValidationRuleEngine {
    /// Voltage deviation limits per nominal voltage level (kV → ±%).
    /// Defaults follow GB/T 12325-2008.
    voltage_deviation_limits: Vec<(f64, f64)>,
    /// Frequency deviation limit (Hz). Default ±0.2 Hz per GB/T 15945.
    frequency_deviation_hz: f64,
    /// THD limit (%). Default 5% per GB/T 14549 for ≤1 kV buses.
    thd_limit_percent: f64,
    /// Long-term flicker limit (Plt). Default 1.0 per GB/T 12326.
    plt_limit: f64,
    /// Post-contingency voltage deviation limit (p.u.).
    n1_voltage_deviation_pu: f64,
    /// Post-contingency loading limit (%).
    n1_loading_percent: f64,
    /// Short-circuit breaking capacity safety margin (%).
    /// Breaker capacity must exceed fault current by at least this margin.
    sc_breaker_margin_percent: f64,
    /// Maximum fault clearing time (s) per GB/T 15544.
    sc_max_clearing_time_s: f64,
}

impl ValidationRuleEngine {
    /// Create a new engine with Chinese national standard defaults.
    pub fn new() -> Self {
        Self {
            // (nominal_kv_threshold, ±deviation_percent)
            // ≥35 kV: ±7%, 10–35 kV: ±7%, ≤1 kV: ±7% (GB/T 12325-2008)
            // 220 kV and above: ±5% per system operator practice
            voltage_deviation_limits: vec![
                (220.0, 5.0),
                (35.0, 7.0),
                (1.0, 7.0),
                (0.0, 10.0),
            ],
            frequency_deviation_hz: 0.2,
            thd_limit_percent: 5.0,
            plt_limit: 1.0,
            n1_voltage_deviation_pu: 0.1,
            n1_loading_percent: 100.0,
            sc_breaker_margin_percent: 10.0,
            sc_max_clearing_time_s: 0.25,
        }
    }

    /// Look up the voltage deviation limit for a given nominal voltage.
    fn voltage_deviation_limit(&self, nominal_kv: f64) -> f64 {
        for (threshold, limit) in &self.voltage_deviation_limits {
            if nominal_kv >= *threshold {
                return *limit;
            }
        }
        10.0
    }

    // -----------------------------------------------------------------------
    // Voltage quality rules
    // -----------------------------------------------------------------------

    /// GB/T 12325-2008 — Voltage deviation at each bus.
    pub fn check_voltage_deviation(&self, state: &SystemStateSnapshot) -> Vec<ValidationFinding> {
        state
            .buses
            .iter()
            .map(|bus| self.check_one_voltage_deviation(bus))
            .collect()
    }

    fn check_one_voltage_deviation(&self, bus: &BusVoltageObservation) -> ValidationFinding {
        let limit = self.voltage_deviation_limit(bus.nominal_kv);
        let rule_id = format!("VQ_DEV_{}", bus.bus_id);
        let description = format!("Voltage deviation at bus {}", bus.bus_id);
        let standard = "GB/T 12325-2008";

        match bus.measured_kv {
            Some(measured) => {
                if bus.nominal_kv <= 0.0 {
                    return ValidationFinding::inconclusive(
                        &rule_id,
                        &description,
                        standard,
                        "%",
                        "nominal voltage is non-positive",
                    );
                }
                let deviation_pct = ((measured - bus.nominal_kv) / bus.nominal_kv).abs() * 100.0;
                if deviation_pct <= limit {
                    ValidationFinding::passed(&rule_id, &description, standard, "%")
                } else {
                    ValidationFinding::failed(
                        &rule_id,
                        &description,
                        standard,
                        deviation_pct,
                        limit,
                        "%",
                        &format!(
                            "bus {} voltage {:.2} kV deviates {:.2}% from nominal {:.2} kV (limit ±{:.0}%)",
                            bus.bus_id, measured, deviation_pct, bus.nominal_kv, limit
                        ),
                    )
                }
            }
            None => ValidationFinding::inconclusive(
                &rule_id,
                &description,
                standard,
                "%",
                "no voltage measurement available",
            ),
        }
    }

    /// GB/T 15945-2008 — System frequency deviation.
    pub fn check_frequency_deviation(&self, state: &SystemStateSnapshot) -> Vec<ValidationFinding> {
        let mut findings = Vec::new();
        if let Some(freq) = &state.frequency {
            match freq.measured_hz {
                Some(measured) => {
                    if freq.nominal_hz <= 0.0 {
                        findings.push(ValidationFinding::inconclusive(
                            "VQ_FREQ_001",
                            "System frequency deviation",
                            "GB/T 15945-2008",
                            "Hz",
                            "nominal frequency is non-positive",
                        ));
                    } else {
                        let deviation = (measured - freq.nominal_hz).abs();
                        if deviation <= self.frequency_deviation_hz {
                            findings.push(ValidationFinding::passed(
                                "VQ_FREQ_001",
                                "System frequency deviation",
                                "GB/T 15945-2008",
                                "Hz",
                            ));
                        } else {
                            findings.push(ValidationFinding::failed(
                                "VQ_FREQ_001",
                                "System frequency deviation",
                                "GB/T 15945-2008",
                                deviation,
                                self.frequency_deviation_hz,
                                "Hz",
                                &format!(
                                    "measured {:.3} Hz deviates {:.3} Hz from nominal {:.1} Hz (limit ±{:.1} Hz)",
                                    measured, deviation, freq.nominal_hz, self.frequency_deviation_hz
                                ),
                            ));
                        }
                    }
                }
                None => findings.push(ValidationFinding::inconclusive(
                    "VQ_FREQ_001",
                    "System frequency deviation",
                    "GB/T 15945-2008",
                    "Hz",
                    "no frequency measurement available",
                )),
            }
        }
        findings
    }

    /// GB/T 14549-1993 — Voltage total harmonic distortion (THD).
    pub fn check_harmonics(&self, state: &SystemStateSnapshot) -> Vec<ValidationFinding> {
        state
            .buses
            .iter()
            .filter(|b| b.thd_percent.is_some())
            .map(|bus| {
                let rule_id = format!("VQ_HARM_{}", bus.bus_id);
                let description = format!("Voltage THD at bus {}", bus.bus_id);
                let standard = "GB/T 14549-1993";
                let thd = bus.thd_percent.unwrap();
                if thd <= self.thd_limit_percent {
                    ValidationFinding::passed(&rule_id, &description, standard, "%")
                } else {
                    ValidationFinding::failed(
                        &rule_id,
                        &description,
                        standard,
                        thd,
                        self.thd_limit_percent,
                        "%",
                        &format!("bus {} THD {:.2}% exceeds limit {:.0}%", bus.bus_id, thd, self.thd_limit_percent),
                    )
                }
            })
            .collect()
    }

    /// GB/T 12326-2008 — Long-term flicker severity (Plt).
    pub fn check_flicker(&self, state: &SystemStateSnapshot) -> Vec<ValidationFinding> {
        state
            .buses
            .iter()
            .filter(|b| b.plt.is_some())
            .map(|bus| {
                let rule_id = format!("VQ_FLK_{}", bus.bus_id);
                let description = format!("Long-term flicker (Plt) at bus {}", bus.bus_id);
                let standard = "GB/T 12326-2008";
                let plt = bus.plt.unwrap();
                if plt <= self.plt_limit {
                    ValidationFinding::passed(&rule_id, &description, standard, "")
                } else {
                    ValidationFinding::failed(
                        &rule_id,
                        &description,
                        standard,
                        plt,
                        self.plt_limit,
                        "",
                        &format!("bus {} Plt {:.2} exceeds limit {:.1}", bus.bus_id, plt, self.plt_limit),
                    )
                }
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // N-1 safety rules
    // -----------------------------------------------------------------------

    /// GB/T 38306 / DL/T 7233 — N-1 contingency security.
    ///
    /// For each contingency, checks:
    /// * No bus collapse
    /// * Post-contingency voltage deviation within ±10%
    /// * Post-contingency branch loading within 100%
    pub fn check_n1_security(&self, state: &SystemStateSnapshot) -> Vec<ValidationFinding> {
        state
            .contingencies
            .iter()
            .map(|c| self.check_one_contingency(c))
            .collect()
    }

    fn check_one_contingency(&self, c: &ContingencyObservation) -> ValidationFinding {
        let rule_id = format!("N1_{}", c.branch_id);
        let description = format!("N-1 contingency on branch {}", c.branch_id);
        let standard = "GB/T 38306-2025 / DL/T 7233-2017";

        if c.bus_collapse {
            return ValidationFinding::failed(
                &rule_id,
                &description,
                standard,
                1.0,
                0.0,
                "",
                &format!("bus collapse after outage of branch {}", c.branch_id),
            );
        }

        if let Some(dev) = c.max_voltage_deviation_pu {
            if dev > self.n1_voltage_deviation_pu {
                return ValidationFinding::failed(
                    &rule_id,
                    &description,
                    standard,
                    dev,
                    self.n1_voltage_deviation_pu,
                    "p.u.",
                    &format!(
                        "branch {} outage causes voltage deviation {:.3} p.u. > {:.2} p.u.",
                        c.branch_id, dev, self.n1_voltage_deviation_pu
                    ),
                );
            }
        }

        if let Some(loading) = c.max_loading_percent {
            if loading > self.n1_loading_percent {
                return ValidationFinding::failed(
                    &rule_id,
                    &description,
                    standard,
                    loading,
                    self.n1_loading_percent,
                    "%",
                    &format!(
                        "branch {} outage causes loading {:.1}% > {:.0}%",
                        c.branch_id, loading, self.n1_loading_percent
                    ),
                );
            }
        }

        if c.max_voltage_deviation_pu.is_none() && c.max_loading_percent.is_none() {
            ValidationFinding::inconclusive(
                &rule_id,
                &description,
                standard,
                "",
                "no post-contingency metrics available",
            )
        } else {
            ValidationFinding::passed(&rule_id, &description, standard, "")
        }
    }

    // -----------------------------------------------------------------------
    // Short-circuit rules
    // -----------------------------------------------------------------------

    /// GB/T 15544 — Three-phase short-circuit current vs breaker capacity.
    pub fn check_short_circuit_capacity(
        &self,
        state: &SystemStateSnapshot,
    ) -> Vec<ValidationFinding> {
        state
            .short_circuits
            .iter()
            .map(|sc| self.check_one_sc_capacity(sc))
            .collect()
    }

    fn check_one_sc_capacity(&self, sc: &ShortCircuitObservation) -> ValidationFinding {
        let rule_id = format!("SC_CAP_{}", sc.bus_id);
        let description = format!("Short-circuit breaking capacity at bus {}", sc.bus_id);
        let standard = "GB/T 15544.1-2023";

        match (sc.ik_3ph_ka, sc.breaker_capacity_ka) {
            (Some(ik), Some(cap)) => {
                let required = ik * (1.0 + self.sc_breaker_margin_percent / 100.0);
                if cap >= required {
                    ValidationFinding::passed(&rule_id, &description, standard, "kA")
                } else {
                    ValidationFinding::failed(
                        &rule_id,
                        &description,
                        standard,
                        ik,
                        cap,
                        "kA",
                        &format!(
                            "bus {} Ik={:.2} kA requires breaker ≥ {:.2} kA (with {:.0}% margin), actual capacity {:.2} kA",
                            sc.bus_id, ik, required, self.sc_breaker_margin_percent, cap
                        ),
                    )
                }
            }
            _ => ValidationFinding::inconclusive(
                &rule_id,
                &description,
                standard,
                "kA",
                "missing fault current or breaker capacity",
            ),
        }
    }

    /// GB/T 15544 — Fault clearing time.
    pub fn check_fault_clearing_time(&self, state: &SystemStateSnapshot) -> Vec<ValidationFinding> {
        state
            .short_circuits
            .iter()
            .filter(|sc| sc.fault_clearing_time_s.is_some())
            .map(|sc| {
                let rule_id = format!("SC_TIME_{}", sc.bus_id);
                let description = format!("Fault clearing time at bus {}", sc.bus_id);
                let standard = "GB/T 15544.1-2023";
                let t = sc.fault_clearing_time_s.unwrap();
                if t <= self.sc_max_clearing_time_s {
                    ValidationFinding::passed(&rule_id, &description, standard, "s")
                } else {
                    ValidationFinding::failed(
                        &rule_id,
                        &description,
                        standard,
                        t,
                        self.sc_max_clearing_time_s,
                        "s",
                        &format!(
                            "bus {} clearing time {:.3} s exceeds limit {:.2} s",
                            sc.bus_id, t, self.sc_max_clearing_time_s
                        ),
                    )
                }
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Aggregate
    // -----------------------------------------------------------------------

    /// Run all validation rules and return all findings.
    pub fn validate_all(&self, state: &SystemStateSnapshot) -> Vec<ValidationFinding> {
        let mut findings = Vec::new();
        findings.extend(self.check_voltage_deviation(state));
        findings.extend(self.check_frequency_deviation(state));
        findings.extend(self.check_harmonics(state));
        findings.extend(self.check_flicker(state));
        findings.extend(self.check_n1_security(state));
        findings.extend(self.check_short_circuit_capacity(state));
        findings.extend(self.check_fault_clearing_time(state));
        findings
    }

    /// Summarize findings into a pass/fail/inconclusive count.
    pub fn summarize(findings: &[ValidationFinding]) -> ValidationSummary {
        let mut summary = ValidationSummary::default();
        for f in findings {
            match &f.status {
                ValidationStatus::Passed => summary.passed += 1,
                ValidationStatus::Failed { .. } => summary.failed += 1,
                ValidationStatus::Inconclusive { .. } => summary.inconclusive += 1,
            }
        }
        summary
    }
}

/// Aggregate validation summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidationSummary {
    pub passed: usize,
    pub failed: usize,
    pub inconclusive: usize,
}

impl ValidationSummary {
    pub fn total(&self) -> usize {
        self.passed + self.failed + self.inconclusive
    }
    pub fn all_passed(&self) -> bool {
        self.failed == 0 && self.inconclusive == 0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bus(bus_id: &str, nominal: f64, measured: Option<f64>) -> BusVoltageObservation {
        BusVoltageObservation {
            bus_id: bus_id.to_string(),
            nominal_kv: nominal,
            measured_kv: measured,
            thd_percent: None,
            plt: None,
        }
    }

    #[test]
    fn test_voltage_deviation_passes_within_limit() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            buses: vec![make_bus("B1", 10.0, Some(10.5))],
            ..Default::default()
        };
        let findings = engine.check_voltage_deviation(&state);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].status.passed(), "5% deviation should pass ±7% limit");
    }

    #[test]
    fn test_voltage_deviation_fails_beyond_limit() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            buses: vec![make_bus("B1", 10.0, Some(11.5))],
            ..Default::default()
        };
        let findings = engine.check_voltage_deviation(&state);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].status.failed());
    }

    #[test]
    fn test_voltage_deviation_inconclusive_without_measurement() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            buses: vec![make_bus("B1", 10.0, None)],
            ..Default::default()
        };
        let findings = engine.check_voltage_deviation(&state);
        assert!(matches!(findings[0].status, ValidationStatus::Inconclusive { .. }));
    }

    #[test]
    fn test_voltage_deviation_uses_220kv_limit() {
        // 220 kV buses have ±5% limit
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            buses: vec![make_bus("B220", 220.0, Some(232.0))], // ~5.45% deviation
            ..Default::default()
        };
        let findings = engine.check_voltage_deviation(&state);
        assert!(findings[0].status.failed(), "5.45% should exceed ±5% limit at 220 kV");
    }

    #[test]
    fn test_frequency_deviation_passes() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            frequency: Some(FrequencyObservation {
                nominal_hz: 50.0,
                measured_hz: Some(50.15),
            }),
            ..Default::default()
        };
        let findings = engine.check_frequency_deviation(&state);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].status.passed());
    }

    #[test]
    fn test_frequency_deviation_fails() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            frequency: Some(FrequencyObservation {
                nominal_hz: 50.0,
                measured_hz: Some(50.5),
            }),
            ..Default::default()
        };
        let findings = engine.check_frequency_deviation(&state);
        assert!(findings[0].status.failed());
    }

    #[test]
    fn test_harmonics_check() {
        let engine = ValidationRuleEngine::new();
        let mut bus = make_bus("B1", 0.4, Some(0.4));
        bus.thd_percent = Some(6.0); // exceeds 5% limit
        let state = SystemStateSnapshot {
            buses: vec![bus],
            ..Default::default()
        };
        let findings = engine.check_harmonics(&state);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].status.failed());
    }

    #[test]
    fn test_flicker_check() {
        let engine = ValidationRuleEngine::new();
        let mut bus = make_bus("B1", 10.0, Some(10.0));
        bus.plt = Some(1.5); // exceeds 1.0 limit
        let state = SystemStateSnapshot {
            buses: vec![bus],
            ..Default::default()
        };
        let findings = engine.check_flicker(&state);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].status.failed());
    }

    #[test]
    fn test_n1_security_passes() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            contingencies: vec![ContingencyObservation {
                branch_id: "L1".to_string(),
                max_voltage_deviation_pu: Some(0.05),
                max_loading_percent: Some(85.0),
                bus_collapse: false,
            }],
            ..Default::default()
        };
        let findings = engine.check_n1_security(&state);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].status.passed());
    }

    #[test]
    fn test_n1_security_fails_on_collapse() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            contingencies: vec![ContingencyObservation {
                branch_id: "L1".to_string(),
                max_voltage_deviation_pu: None,
                max_loading_percent: None,
                bus_collapse: true,
            }],
            ..Default::default()
        };
        let findings = engine.check_n1_security(&state);
        assert!(findings[0].status.failed());
    }

    #[test]
    fn test_n1_security_fails_on_overload() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            contingencies: vec![ContingencyObservation {
                branch_id: "L2".to_string(),
                max_voltage_deviation_pu: Some(0.03),
                max_loading_percent: Some(120.0),
                bus_collapse: false,
            }],
            ..Default::default()
        };
        let findings = engine.check_n1_security(&state);
        assert!(findings[0].status.failed());
    }

    #[test]
    fn test_short_circuit_capacity_passes() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            short_circuits: vec![ShortCircuitObservation {
                bus_id: "B1".to_string(),
                nominal_kv: 10.0,
                ik_3ph_ka: Some(20.0),
                breaker_capacity_ka: Some(25.0), // 25% margin > 10% required
                fault_clearing_time_s: None,
            }],
            ..Default::default()
        };
        let findings = engine.check_short_circuit_capacity(&state);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].status.passed());
    }

    #[test]
    fn test_short_circuit_capacity_fails() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            short_circuits: vec![ShortCircuitObservation {
                bus_id: "B1".to_string(),
                nominal_kv: 10.0,
                ik_3ph_ka: Some(20.0),
                breaker_capacity_ka: Some(21.0), // 5% margin < 10% required
                fault_clearing_time_s: None,
            }],
            ..Default::default()
        };
        let findings = engine.check_short_circuit_capacity(&state);
        assert!(findings[0].status.failed());
    }

    #[test]
    fn test_fault_clearing_time_passes() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            short_circuits: vec![ShortCircuitObservation {
                bus_id: "B1".to_string(),
                nominal_kv: 10.0,
                ik_3ph_ka: None,
                breaker_capacity_ka: None,
                fault_clearing_time_s: Some(0.15),
            }],
            ..Default::default()
        };
        let findings = engine.check_fault_clearing_time(&state);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].status.passed());
    }

    #[test]
    fn test_fault_clearing_time_fails() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            short_circuits: vec![ShortCircuitObservation {
                bus_id: "B1".to_string(),
                nominal_kv: 10.0,
                ik_3ph_ka: None,
                breaker_capacity_ka: None,
                fault_clearing_time_s: Some(0.40),
            }],
            ..Default::default()
        };
        let findings = engine.check_fault_clearing_time(&state);
        assert!(findings[0].status.failed());
    }

    #[test]
    fn test_validate_all_runs_every_rule_family() {
        let engine = ValidationRuleEngine::new();
        let mut bus = make_bus("B1", 10.0, Some(10.5));
        bus.thd_percent = Some(3.0);
        bus.plt = Some(0.5);
        let state = SystemStateSnapshot {
            buses: vec![bus],
            frequency: Some(FrequencyObservation {
                nominal_hz: 50.0,
                measured_hz: Some(50.1),
            }),
            contingencies: vec![ContingencyObservation {
                branch_id: "L1".to_string(),
                max_voltage_deviation_pu: Some(0.05),
                max_loading_percent: Some(80.0),
                bus_collapse: false,
            }],
            short_circuits: vec![ShortCircuitObservation {
                bus_id: "B1".to_string(),
                nominal_kv: 10.0,
                ik_3ph_ka: Some(20.0),
                breaker_capacity_ka: Some(25.0),
                fault_clearing_time_s: Some(0.20),
            }],
        };
        let findings = engine.validate_all(&state);
        // 1 voltage + 1 freq + 1 THD + 1 flicker + 1 N-1 + 1 SC cap + 1 SC time = 7
        assert_eq!(findings.len(), 7);
        let summary = ValidationRuleEngine::summarize(&findings);
        assert_eq!(summary.passed, 7);
        assert_eq!(summary.failed, 0);
        assert!(summary.all_passed());
    }

    #[test]
    fn test_summary_counts_correctly() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot {
            buses: vec![
                make_bus("B1", 10.0, Some(11.5)), // fail
                make_bus("B2", 10.0, Some(10.5)), // pass
                make_bus("B3", 10.0, None),       // inconclusive
            ],
            ..Default::default()
        };
        let findings = engine.validate_all(&state);
        let summary = ValidationRuleEngine::summarize(&findings);
        assert!(summary.failed >= 1);
        assert!(summary.passed >= 1);
        assert!(summary.inconclusive >= 1);
        assert!(!summary.all_passed());
    }

    #[test]
    fn test_empty_state_returns_no_findings() {
        let engine = ValidationRuleEngine::new();
        let state = SystemStateSnapshot::default();
        let findings = engine.validate_all(&state);
        assert!(findings.is_empty());
        let summary = ValidationRuleEngine::summarize(&findings);
        assert_eq!(summary.total(), 0);
        assert!(summary.all_passed()); // vacuously true
    }
}
