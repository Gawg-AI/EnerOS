//! Compliance rule engine inspired by cnpower's `compliance_constraints.py`.
//!
//! Provides national standard (GB/T) compliance checking for power equipment
//! and operating conditions. Rules are organized by equipment type and
//! return a three-state result: Passed, Failed, or Inconclusive (missing data).

use serde::{Deserialize, Serialize};

/// Three-state compliance check result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComplianceStatus {
    /// Equipment/condition meets the standard.
    Passed,
    /// Equipment/condition violates the standard.
    Failed(String),
    /// Required data is missing to determine compliance.
    Inconclusive(String),
}

/// A single compliance finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceFinding {
    /// Rule identifier (e.g., "TR2_LOAD_001").
    pub rule_id: String,
    /// Rule description.
    pub description: String,
    /// Referenced national standard (e.g., "GB/T 6451-2023").
    pub standard: String,
    /// Check result.
    pub status: ComplianceStatus,
    /// Measured value (if available).
    pub measured_value: Option<f64>,
    /// Limit value (if applicable).
    pub limit_value: Option<f64>,
    /// Unit of the measured/limit values.
    pub unit: String,
}

/// Equipment operating conditions for compliance checking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatingConditions {
    /// Voltage magnitude (p.u.).
    pub voltage_pu: Option<f64>,
    /// Current magnitude (kA).
    pub current_ka: Option<f64>,
    /// Loading percentage (%).
    pub loading_percent: Option<f64>,
    /// Ambient temperature (°C).
    pub ambient_temp_c: Option<f64>,
    /// Active power (MW).
    pub p_mw: Option<f64>,
    /// Reactive power (MVar).
    pub q_mvar: Option<f64>,
    /// Apparent power (MVA).
    pub s_mva: Option<f64>,
    /// Short circuit current (kA).
    pub short_circuit_current_ka: Option<f64>,
    /// Power factor.
    pub power_factor: Option<f64>,
    /// SOC for energy storage (%).
    pub soc_percent: Option<f64>,
}

/// Equipment specification for compliance checking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EquipmentSpec {
    /// Equipment type (e.g., "transformer", "cable", "switchgear").
    pub equipment_type: String,
    /// Rated capacity (MVA for transformer, kA for breaker).
    pub rated_capacity: Option<f64>,
    /// Rated voltage (kV).
    pub rated_voltage_kv: Option<f64>,
    /// Rated current (kA).
    pub rated_current_ka: Option<f64>,
    /// Rated short circuit breaking current (kA).
    pub rated_breaking_current_ka: Option<f64>,
    /// Rated frequency (Hz).
    pub rated_frequency_hz: Option<f64>,
    /// Impedance voltage percentage (for transformers).
    pub impedance_percent: Option<f64>,
    /// Normal loading limit (%).
    pub normal_loading_limit_percent: Option<f64>,
    /// Emergency loading limit (%).
    pub emergency_loading_limit_percent: Option<f64>,
    /// Maximum allowable temperature (°C).
    pub max_temp_c: Option<f64>,
}

/// Compliance checker that evaluates equipment against national standards.
pub struct ComplianceChecker;

impl ComplianceChecker {
    /// Check transformer loading compliance (GB/T 6451-2023).
    ///
    /// Rule TR2_LOAD_001: Normal loading should not exceed rated capacity.
    pub fn check_transformer_loading(
        spec: &EquipmentSpec,
        operating: &OperatingConditions,
    ) -> ComplianceFinding {
        let rule_id = "TR2_LOAD_001".to_string();
        let standard = "GB/T 6451-2023".to_string();
        let description = "Transformer loading must not exceed rated capacity".to_string();

        match (operating.loading_percent, spec.normal_loading_limit_percent) {
            (Some(loading), Some(limit)) => {
                if loading <= limit {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Passed,
                        measured_value: Some(loading),
                        limit_value: Some(limit),
                        unit: "%".to_string(),
                    }
                } else {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Failed(format!(
                            "Loading {:.1}% exceeds limit {:.1}%",
                            loading, limit
                        )),
                        measured_value: Some(loading),
                        limit_value: Some(limit),
                        unit: "%".to_string(),
                    }
                }
            }
            (Some(_), None) => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("normal_loading_limit_percent not specified".to_string()),
                measured_value: operating.loading_percent,
                limit_value: None,
                unit: "%".to_string(),
            },
            (None, _) => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("loading_percent not measured".to_string()),
                measured_value: None,
                limit_value: spec.normal_loading_limit_percent,
                unit: "%".to_string(),
            },
        }
    }

    /// Check transformer thermal compliance (GB/T 1094.7-2024 for oil-immersed,
    /// GB/T 1094.11-2022 for dry-type).
    ///
    /// Rule TR2_THERMAL_001: Operating temperature must not exceed maximum.
    pub fn check_transformer_thermal(
        spec: &EquipmentSpec,
        operating: &OperatingConditions,
    ) -> ComplianceFinding {
        let rule_id = "TR2_THERMAL_001".to_string();
        let standard = "GB/T 1094.7-2024".to_string();
        let description = "Transformer temperature must not exceed maximum allowable".to_string();

        match (operating.ambient_temp_c, spec.max_temp_c) {
            (Some(temp), Some(max_temp)) => {
                // Estimate hotspot temperature: ambient + loading_factor * gradient
                // Simplified: use ambient + 20°C margin for normal loading
                let estimated_hotspot = temp + 20.0
                    + operating.loading_percent.unwrap_or(50.0) * 0.3;
                if estimated_hotspot <= max_temp {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Passed,
                        measured_value: Some(estimated_hotspot),
                        limit_value: Some(max_temp),
                        unit: "°C".to_string(),
                    }
                } else {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Failed(format!(
                            "Estimated hotspot {:.1}°C exceeds max {:.1}°C",
                            estimated_hotspot, max_temp
                        )),
                        measured_value: Some(estimated_hotspot),
                        limit_value: Some(max_temp),
                        unit: "°C".to_string(),
                    }
                }
            }
            (Some(temp), None) => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("max_temp_c not specified".to_string()),
                measured_value: Some(temp),
                limit_value: None,
                unit: "°C".to_string(),
            },
            (None, _) => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("ambient_temp_c not measured".to_string()),
                measured_value: None,
                limit_value: spec.max_temp_c,
                unit: "°C".to_string(),
            },
        }
    }

    /// Check voltage deviation compliance (GB/T 12325-2008).
    ///
    /// Rule VOLTAGE_DEV_001: Voltage deviation must be within ±7% (10kV) or ±5% (0.4kV).
    pub fn check_voltage_deviation(
        rated_voltage_kv: f64,
        operating: &OperatingConditions,
    ) -> ComplianceFinding {
        let rule_id = "VOLTAGE_DEV_001".to_string();
        let standard = "GB/T 12325-2008".to_string();
        let description = "Voltage deviation must be within allowable range".to_string();

        // Limits per GB/T 12325:
        // 35kV+: ±5%, 10kV: ±7%, 0.4kV: +7%/-10% (use +7% symmetric for simplicity)
        let limit_percent = if rated_voltage_kv >= 35.0 {
            5.0
        } else {
            // 10kV and 0.4kV both use ±7% (0.4kV lower limit is -10% but we check symmetric)
            7.0
        };

        match operating.voltage_pu {
            Some(v_pu) => {
                let deviation_percent = (v_pu - 1.0).abs() * 100.0;
                if deviation_percent <= limit_percent {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Passed,
                        measured_value: Some(deviation_percent),
                        limit_value: Some(limit_percent),
                        unit: "%".to_string(),
                    }
                } else {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Failed(format!(
                            "Voltage deviation {:.2}% exceeds limit ±{:.0}%",
                            deviation_percent, limit_percent
                        )),
                        measured_value: Some(deviation_percent),
                        limit_value: Some(limit_percent),
                        unit: "%".to_string(),
                    }
                }
            }
            None => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("voltage_pu not measured".to_string()),
                measured_value: None,
                limit_value: Some(limit_percent),
                unit: "%".to_string(),
            },
        }
    }

    /// Check cable ampacity compliance (GB/T 12706-2020).
    ///
    /// Rule CAB_LOAD_001: Cable current must not exceed rated ampacity.
    pub fn check_cable_ampacity(
        spec: &EquipmentSpec,
        operating: &OperatingConditions,
    ) -> ComplianceFinding {
        let rule_id = "CAB_LOAD_001".to_string();
        let standard = "GB/T 12706-2020".to_string();
        let description = "Cable current must not exceed rated ampacity".to_string();

        match (operating.current_ka, spec.rated_current_ka) {
            (Some(current), Some(rated)) => {
                let loading = current / rated * 100.0;
                if loading <= 100.0 {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Passed,
                        measured_value: Some(loading),
                        limit_value: Some(100.0),
                        unit: "%".to_string(),
                    }
                } else {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Failed(format!(
                            "Cable loading {:.1}% exceeds rated ampacity",
                            loading
                        )),
                        measured_value: Some(loading),
                        limit_value: Some(100.0),
                        unit: "%".to_string(),
                    }
                }
            }
            (Some(_), None) => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("rated_current_ka not specified".to_string()),
                measured_value: operating.current_ka,
                limit_value: None,
                unit: "kA".to_string(),
            },
            (None, _) => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("current_ka not measured".to_string()),
                measured_value: None,
                limit_value: spec.rated_current_ka,
                unit: "kA".to_string(),
            },
        }
    }

    /// Check circuit breaker breaking capacity (GB/T 1984-2024).
    ///
    /// Rule SWG_BREAK_001: Breaker rated breaking current must exceed fault current.
    pub fn check_breaker_capacity(
        spec: &EquipmentSpec,
        operating: &OperatingConditions,
    ) -> ComplianceFinding {
        let rule_id = "SWG_BREAK_001".to_string();
        let standard = "GB/T 1984-2024".to_string();
        let description = "Breaker rated breaking current must exceed fault current".to_string();

        match (operating.short_circuit_current_ka, spec.rated_breaking_current_ka) {
            (Some(fault_current), Some(rated_breaking)) => {
                if rated_breaking >= fault_current {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Passed,
                        measured_value: Some(fault_current),
                        limit_value: Some(rated_breaking),
                        unit: "kA".to_string(),
                    }
                } else {
                    ComplianceFinding {
                        rule_id,
                        description,
                        standard,
                        status: ComplianceStatus::Failed(format!(
                            "Fault current {:.1}kA exceeds breaker rating {:.1}kA",
                            fault_current, rated_breaking
                        )),
                        measured_value: Some(fault_current),
                        limit_value: Some(rated_breaking),
                        unit: "kA".to_string(),
                    }
                }
            }
            (Some(_), None) => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("rated_breaking_current_ka not specified".to_string()),
                measured_value: operating.short_circuit_current_ka,
                limit_value: None,
                unit: "kA".to_string(),
            },
            (None, _) => ComplianceFinding {
                rule_id,
                description,
                standard,
                status: ComplianceStatus::Inconclusive("short_circuit_current_ka not measured".to_string()),
                measured_value: None,
                limit_value: spec.rated_breaking_current_ka,
                unit: "kA".to_string(),
            },
        }
    }

    /// Run all applicable compliance checks for given equipment.
    pub fn check_all(
        spec: &EquipmentSpec,
        operating: &OperatingConditions,
    ) -> Vec<ComplianceFinding> {
        let mut findings = Vec::new();

        match spec.equipment_type.as_str() {
            "transformer" => {
                findings.push(Self::check_transformer_loading(spec, operating));
                findings.push(Self::check_transformer_thermal(spec, operating));
            }
            "cable" | "line" => {
                findings.push(Self::check_cable_ampacity(spec, operating));
            }
            "switchgear" | "breaker" => {
                findings.push(Self::check_breaker_capacity(spec, operating));
            }
            _ => {}
        }

        // Voltage deviation applies to all equipment at a bus
        if let Some(rated_kv) = spec.rated_voltage_kv {
            findings.push(Self::check_voltage_deviation(rated_kv, operating));
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transformer_loading_pass() {
        let spec = EquipmentSpec {
            equipment_type: "transformer".to_string(),
            rated_capacity: Some(10.0),
            normal_loading_limit_percent: Some(85.0),
            ..Default::default()
        };
        let operating = OperatingConditions {
            loading_percent: Some(70.0),
            ..Default::default()
        };

        let finding = ComplianceChecker::check_transformer_loading(&spec, &operating);
        assert_eq!(finding.status, ComplianceStatus::Passed);
    }

    #[test]
    fn test_transformer_loading_fail() {
        let spec = EquipmentSpec {
            equipment_type: "transformer".to_string(),
            normal_loading_limit_percent: Some(85.0),
            ..Default::default()
        };
        let operating = OperatingConditions {
            loading_percent: Some(95.0),
            ..Default::default()
        };

        let finding = ComplianceChecker::check_transformer_loading(&spec, &operating);
        assert!(matches!(finding.status, ComplianceStatus::Failed(_)));
    }

    #[test]
    fn test_transformer_loading_inconclusive() {
        let spec = EquipmentSpec {
            equipment_type: "transformer".to_string(),
            ..Default::default()
        };
        let operating = OperatingConditions::default();

        let finding = ComplianceChecker::check_transformer_loading(&spec, &operating);
        assert!(matches!(finding.status, ComplianceStatus::Inconclusive(_)));
    }

    #[test]
    fn test_voltage_deviation_10kv() {
        let operating = OperatingConditions {
            voltage_pu: Some(1.05), // 5% deviation, within ±7%
            ..Default::default()
        };
        let finding = ComplianceChecker::check_voltage_deviation(10.0, &operating);
        assert_eq!(finding.status, ComplianceStatus::Passed);

        let operating_fail = OperatingConditions {
            voltage_pu: Some(1.10), // 10% deviation, exceeds ±7%
            ..Default::default()
        };
        let finding = ComplianceChecker::check_voltage_deviation(10.0, &operating_fail);
        assert!(matches!(finding.status, ComplianceStatus::Failed(_)));
    }

    #[test]
    fn test_cable_ampacity() {
        let spec = EquipmentSpec {
            equipment_type: "cable".to_string(),
            rated_current_ka: Some(0.4),
            ..Default::default()
        };
        let operating_ok = OperatingConditions {
            current_ka: Some(0.3),
            ..Default::default()
        };
        let finding = ComplianceChecker::check_cable_ampacity(&spec, &operating_ok);
        assert_eq!(finding.status, ComplianceStatus::Passed);

        let operating_fail = OperatingConditions {
            current_ka: Some(0.5),
            ..Default::default()
        };
        let finding = ComplianceChecker::check_cable_ampacity(&spec, &operating_fail);
        assert!(matches!(finding.status, ComplianceStatus::Failed(_)));
    }

    #[test]
    fn test_breaker_capacity() {
        let spec = EquipmentSpec {
            equipment_type: "switchgear".to_string(),
            rated_breaking_current_ka: Some(25.0),
            ..Default::default()
        };
        let operating_ok = OperatingConditions {
            short_circuit_current_ka: Some(20.0),
            ..Default::default()
        };
        let finding = ComplianceChecker::check_breaker_capacity(&spec, &operating_ok);
        assert_eq!(finding.status, ComplianceStatus::Passed);

        let operating_fail = OperatingConditions {
            short_circuit_current_ka: Some(30.0),
            ..Default::default()
        };
        let finding = ComplianceChecker::check_breaker_capacity(&spec, &operating_fail);
        assert!(matches!(finding.status, ComplianceStatus::Failed(_)));
    }

    #[test]
    fn test_check_all_transformer() {
        let spec = EquipmentSpec {
            equipment_type: "transformer".to_string(),
            rated_voltage_kv: Some(10.0),
            normal_loading_limit_percent: Some(85.0),
            max_temp_c: Some(140.0),
            ..Default::default()
        };
        let operating = OperatingConditions {
            loading_percent: Some(70.0),
            ambient_temp_c: Some(35.0),
            voltage_pu: Some(1.03),
            ..Default::default()
        };

        let findings = ComplianceChecker::check_all(&spec, &operating);
        // Should have: loading, thermal, voltage_deviation = 3 findings
        assert_eq!(findings.len(), 3);
        for f in &findings {
            assert_eq!(f.status, ComplianceStatus::Passed);
        }
    }
}
