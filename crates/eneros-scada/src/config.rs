use std::time::Duration;

use eneros_core::ElementId;
use serde::{Deserialize, Serialize};

/// 连接池配置（T029-14）
///
/// 用于 SCADA / Modbus / IEC 61850 协议连接池化。
/// 在 TOML 配置文件中通过 `[pool]` 段指定。
///
/// # 示例
///
/// ```toml
/// [pool]
/// max_size = 16
/// idle_timeout_ms = 30000
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// 最大连接数（活跃 + 空闲）。默认 16。
    #[serde(default = "default_pool_max_size")]
    pub max_size: usize,
    /// 空闲连接超时时间（毫秒）。默认 30000（30 秒）。
    #[serde(default = "default_pool_idle_timeout_ms")]
    pub idle_timeout_ms: u64,
}

fn default_pool_max_size() -> usize {
    16
}

fn default_pool_idle_timeout_ms() -> u64 {
    30000
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: default_pool_max_size(),
            idle_timeout_ms: default_pool_idle_timeout_ms(),
        }
    }
}

impl PoolConfig {
    /// 将配置转换为 `pool::PoolConfig`（使用 `Duration` 类型）。
    pub fn to_pool_config(&self) -> crate::pool::PoolConfig {
        crate::pool::PoolConfig::new(
            self.max_size,
            Duration::from_millis(self.idle_timeout_ms),
        )
    }
}

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
    /// 连接池配置（T029-14）
    #[serde(default)]
    pub pool: PoolConfig,
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
            pool: PoolConfig::default(),
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
        assert_eq!(config.pool.max_size, 16);
        assert_eq!(config.pool.idle_timeout_ms, 30000);
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
            pool: PoolConfig::default(),
        };
        assert_eq!(config.points.len(), 1);
        assert_eq!(config.default_scan_rate_ms, 500);
        assert!(!config.enable_quality_check);
    }

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.max_size, 16);
        assert_eq!(config.idle_timeout_ms, 30000);
    }

    #[test]
    fn test_pool_config_to_pool_config() {
        let config = PoolConfig {
            max_size: 32,
            idle_timeout_ms: 60000,
        };
        let pool_config = config.to_pool_config();
        assert_eq!(pool_config.max_size, 32);
        assert_eq!(pool_config.idle_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_pool_config_serde() {
        let toml_str = r#"
max_size = 8
idle_timeout_ms = 10000
"#;
        let config: PoolConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.max_size, 8);
        assert_eq!(config.idle_timeout_ms, 10000);
    }

    #[test]
    fn test_pool_config_serde_defaults() {
        let toml_str = "";
        let config: PoolConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.max_size, 16);
        assert_eq!(config.idle_timeout_ms, 30000);
    }
}
