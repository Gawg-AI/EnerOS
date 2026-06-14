use eneros_core::ElementId;
use serde::{Deserialize, Serialize};

/// Configuration for a single SCADA data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScadaPoint {
    /// Element ID this point is associated with
    pub element_id: ElementId,
    /// Parameter name (e.g., "voltage_pu", "active_power_mw")
    pub parameter: String,
    /// Scan rate in milliseconds
    pub scan_rate_ms: u64,
    /// Deadband for change detection
    pub deadband: f64,
    /// Minimum valid value (optional)
    pub min_value: Option<f64>,
    /// Maximum valid value (optional)
    pub max_value: Option<f64>,
}

/// SCADA system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScadaConfig {
    /// Data points to collect
    pub points: Vec<ScadaPoint>,
    /// Default scan rate in milliseconds
    #[serde(default = "default_scan_rate")]
    pub default_scan_rate_ms: u64,
    /// Read timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Whether to enable quality checks
    #[serde(default = "default_enable_quality")]
    pub enable_quality_check: bool,
}

fn default_scan_rate() -> u64 {
    1000
}

fn default_timeout() -> u64 {
    5000
}

fn default_enable_quality() -> bool {
    true
}

impl Default for ScadaConfig {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            default_scan_rate_ms: 1000,
            timeout_ms: 5000,
            enable_quality_check: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scada_point_creation() {
        let point = ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: Some(0.8),
            max_value: Some(1.2),
        };
        assert_eq!(point.element_id, 1);
        assert_eq!(point.parameter, "voltage_pu");
        assert_eq!(point.scan_rate_ms, 500);
        assert_eq!(point.deadband, 0.01);
        assert_eq!(point.min_value, Some(0.8));
        assert_eq!(point.max_value, Some(1.2));
    }

    #[test]
    fn test_scada_point_no_limits() {
        let point = ScadaPoint {
            element_id: 2,
            parameter: "frequency_hz".to_string(),
            scan_rate_ms: 1000,
            deadband: 0.0,
            min_value: None,
            max_value: None,
        };
        assert_eq!(point.min_value, None);
        assert_eq!(point.max_value, None);
    }

    #[test]
    fn test_scada_config_default() {
        let config = ScadaConfig::default();
        assert!(config.points.is_empty());
        assert_eq!(config.default_scan_rate_ms, 1000);
        assert_eq!(config.timeout_ms, 5000);
        assert!(config.enable_quality_check);
    }

    #[test]
    fn test_scada_config_with_points() {
        let config = ScadaConfig {
            points: vec![
                ScadaPoint {
                    element_id: 1,
                    parameter: "voltage_pu".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.01,
                    min_value: Some(0.8),
                    max_value: Some(1.2),
                },
            ],
            default_scan_rate_ms: 500,
            timeout_ms: 3000,
            enable_quality_check: false,
        };
        assert_eq!(config.points.len(), 1);
        assert_eq!(config.default_scan_rate_ms, 500);
        assert!(!config.enable_quality_check);
    }
}
