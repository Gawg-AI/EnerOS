//! 22 §2.W5.3：安全目录（safety catalog）——学习参数硬界。
//!
//! 所有在线学习输出（阈值自适应/预测修正/钳位值）的**唯一权威边界**：
//! 越界 = 拒绝（用旧值），"学坏不致灾"。学习器不得自行放宽本目录。
//!
//! 边界来源：电气安全规则（PCS 额定、电池 SOC 界、电压界）+ 运维保守
//! 默认；生产部署可经管理通道（W4.2）覆写（仍须满足目录语义）。

/// 安全目录：学习/控制参数硬界（不可越界）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SafetyCatalog {
    /// 遥测死区硬界 `[deadband_min, deadband_max]`（W2.5 阈值自适应）。
    pub deadband_min: f64,
    pub deadband_max: f64,
    /// 预测修正系数上限（±；不许修正超过该比例）。
    pub max_correction: f64,
    /// 功率钳位硬界（kW；安全钳位值松弛/收紧不得越此目录）。
    pub clamp_power_min_kw: f64,
    pub clamp_power_max_kw: f64,
}

impl SafetyCatalog {
    /// 默认目录（保守可用：死区 [0.1, 10.0]、修正 ±50%、
    /// 钳位 ±PCS 额定 100kW——与 `ScheduleConfig::default()` 对齐）。
    pub fn default_eps() -> Self {
        Self {
            deadband_min: 0.1,
            deadband_max: 10.0,
            max_correction: 0.5,
            clamp_power_min_kw: -100.0,
            clamp_power_max_kw: 100.0,
        }
    }

    /// 校验死区值（有限 + 界内）。
    pub fn check_deadband(&self, v: f64) -> bool {
        v.is_finite() && v >= self.deadband_min && v <= self.deadband_max
    }

    /// 校验预测修正系数（有限 + |v| ≤ 上限）。
    pub fn check_correction(&self, v: f64) -> bool {
        v.is_finite() && v.abs() <= self.max_correction
    }

    /// 校验功率钳位值（有限 + 界内）。
    pub fn check_clamp_power(&self, v: f64) -> bool {
        v.is_finite() && v >= self.clamp_power_min_kw && v <= self.clamp_power_max_kw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目录边界语义：界内放行、越界拒绝、NaN 拒绝（fail-closed）。
    #[test]
    fn test_catalog_boundary_semantics() {
        let c = SafetyCatalog::default_eps();
        assert!(c.check_deadband(5.0));
        assert!(!c.check_deadband(10.5), "超上界拒绝");
        assert!(!c.check_deadband(0.05), "低于下界拒绝");
        assert!(!c.check_deadband(f64::NAN), "NaN 拒绝");
        assert!(c.check_correction(0.3));
        assert!(!c.check_correction(0.7), "修正超限拒绝");
        assert!(c.check_clamp_power(50.0));
        assert!(!c.check_clamp_power(101.0), "钳位越界拒绝");
        assert!(!c.check_clamp_power(f64::INFINITY));
    }
}
