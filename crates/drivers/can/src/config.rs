//! CAN 配置（v0.47.0）.
//!
//! 定义 CAN 驱动配置结构、工作模式与控制器类型标识。
//!
//! # 偏差声明
//! - D2: `CanControllerType` 枚举仅作配置标识（MCP2515/Internal/SJA1000），
//!   不实现具体寄存器级操作。具体寄存器操作由 `CanController` trait 的实现负责。

use alloc::vec::Vec;

use crate::filter::CanFilter;

/// CAN 工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanMode {
    /// 正常模式（收发均可）
    Normal,
    /// 仅监听模式（只收不发，不影响总线）
    ListenOnly,
    /// 环回模式（自发自收，用于测试）
    Loopback,
}

/// CAN 控制器类型标识（D2 偏差：仅作配置标识，无寄存器级操作）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanControllerType {
    /// MCP2515 独立 CAN 控制器（SPI 接口）
    MCP2515,
    /// SoC 内部 CAN 控制器
    Internal,
    /// SJA1000 兼容 CAN 控制器
    SJA1000,
}

impl CanControllerType {
    /// 返回类型对应的名称字符串（用于驱动命名）
    pub fn as_str(&self) -> &'static str {
        match self {
            CanControllerType::MCP2515 => "mcp2515",
            CanControllerType::Internal => "internal",
            CanControllerType::SJA1000 => "sja1000",
        }
    }
}

/// CAN 驱动配置
#[derive(Debug, Clone)]
pub struct CanConfig {
    /// 控制器类型标识（D2）
    pub controller_type: CanControllerType,
    /// 波特率（bps，默认 500_000）
    pub baud_rate: u32,
    /// 工作模式
    pub mode: CanMode,
    /// 软件过滤器列表（空表示接收所有）
    pub filters: Vec<CanFilter>,
    /// 是否启用自动重传
    pub auto_retransmit: bool,
}

impl Default for CanConfig {
    fn default() -> Self {
        Self {
            controller_type: CanControllerType::Internal,
            baud_rate: 500_000,
            mode: CanMode::Normal,
            filters: Vec::new(),
            auto_retransmit: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_mode_variants() {
        let modes = [CanMode::Normal, CanMode::ListenOnly, CanMode::Loopback];
        // 3 个变体两两不相等
        for i in 0..modes.len() {
            for j in (i + 1)..modes.len() {
                assert_ne!(modes[i], modes[j]);
            }
        }
        // Copy 语义
        let m = CanMode::Normal;
        let m_copy = m;
        assert_eq!(m, m_copy);
    }

    #[test]
    fn test_controller_type_variants() {
        let types = [
            CanControllerType::MCP2515,
            CanControllerType::Internal,
            CanControllerType::SJA1000,
        ];
        for i in 0..types.len() {
            for j in (i + 1)..types.len() {
                assert_ne!(types[i], types[j]);
            }
        }
    }

    #[test]
    fn test_controller_type_as_str() {
        assert_eq!(CanControllerType::MCP2515.as_str(), "mcp2515");
        assert_eq!(CanControllerType::Internal.as_str(), "internal");
        assert_eq!(CanControllerType::SJA1000.as_str(), "sja1000");
    }

    #[test]
    fn test_default_config_values() {
        let cfg = CanConfig::default();
        assert_eq!(cfg.controller_type, CanControllerType::Internal);
        assert_eq!(cfg.baud_rate, 500_000);
        assert_eq!(cfg.mode, CanMode::Normal);
        assert!(cfg.filters.is_empty());
        assert!(cfg.auto_retransmit);
    }

    #[test]
    fn test_config_field_access() {
        let cfg = CanConfig {
            controller_type: CanControllerType::MCP2515,
            baud_rate: 250_000,
            mode: CanMode::Loopback,
            filters: Vec::new(),
            auto_retransmit: false,
        };
        assert_eq!(cfg.controller_type, CanControllerType::MCP2515);
        assert_eq!(cfg.baud_rate, 250_000);
        assert_eq!(cfg.mode, CanMode::Loopback);
        assert!(!cfg.auto_retransmit);
    }

    #[test]
    fn test_config_with_filters() {
        let cfg = CanConfig {
            filters: alloc::vec![CanFilter::match_exact(0x123, false)],
            ..Default::default()
        };
        assert_eq!(cfg.filters.len(), 1);
    }

    #[test]
    fn test_config_clone() {
        let cfg = CanConfig::default();
        let cfg_clone = cfg.clone();
        assert_eq!(cfg.baud_rate, cfg_clone.baud_rate);
        assert_eq!(cfg.controller_type, cfg_clone.controller_type);
    }
}
