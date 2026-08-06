//! 控制点规格 (CTL-002)。
//!
//! [`PointSpec`] 描述一个控制点的物理单位、安全范围、控制权限与控制方向。独立安全
//! 校验器 (CTL-008) 据此重算命令的物理可行性，且不依赖生成命令的代码路径——满足
//! MASTER §9.5 "safety validator 与被监控逻辑共享同一失败点"的禁止捷径。

use eneros_upa_model::{DeviceId, PointId};

use crate::unit::PhysicalUnit;

/// 控制方向：命令对受控量的预期作用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlDirection {
    /// 增加受控量（如充电 = SOC 上升）。
    Increase,
    /// 减少受控量（如放电 = SOC 下降）。
    Decrease,
    /// 双态切换（分/合），无连续方向。
    Discrete,
}

/// 控制点规格。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointSpec {
    pub point_id: PointId,
    pub device_id: DeviceId,
    pub unit: PhysicalUnit,
    /// 安全下限（命令不得使受控量低于此值）。
    pub safe_min: f64,
    /// 安全上限（命令不得使受控量高于此值）。
    pub safe_max: f64,
    /// 最大允许变化率（单位/秒）；`None` 表示不强制。
    pub max_ramp_per_sec: Option<f64>,
    pub direction: ControlDirection,
    /// 是否允许远程控制。`false` 的点只能本地操作（manual override）。
    pub remote_controllable: bool,
}

impl PointSpec {
    /// 校验单个值是否落在安全范围。
    pub fn is_within_safe_range(&self, v: f64) -> bool {
        v >= self.safe_min && v <= self.safe_max
    }

    /// 校验从 `from` 到 `to` 的变化是否满足斜率约束（`dt_secs` 为时间间隔）。
    /// 返回 `Ok(())` 或拒绝原因。
    pub fn check_ramp(&self, from: f64, to: f64, dt_secs: f64) -> Result<(), RampViolation> {
        if dt_secs <= 0.0 {
            return Err(RampViolation::NonPositiveDt);
        }
        let delta = (to - from).abs();
        if let Some(max_ramp) = self.max_ramp_per_sec {
            let allowed = max_ramp * dt_secs;
            if delta > allowed {
                return Err(RampViolation::Exceeded {
                    requested: delta / dt_secs,
                    limit: max_ramp,
                });
            }
        }
        Ok(())
    }

    /// 单位是否与另一规格兼容（防止点表映射错误）。
    pub fn unit_compatible_with(&self, other: &PointSpec) -> bool {
        self.unit.compatible_with(other.unit)
    }
}

/// 斜率校验失败原因。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RampViolation {
    NonPositiveDt,
    Exceeded { requested: f64, limit: f64 },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn soc_spec() -> PointSpec {
        PointSpec {
            point_id: 1,
            device_id: 7,
            unit: PhysicalUnit::Percent,
            safe_min: 10.0,
            safe_max: 90.0,
            max_ramp_per_sec: Some(1.0), // 1%/s
            direction: ControlDirection::Increase,
            remote_controllable: true,
        }
    }

    #[test]
    fn safe_range_check() {
        let s = soc_spec();
        assert!(s.is_within_safe_range(50.0));
        assert!(!s.is_within_safe_range(95.0));
        assert!(!s.is_within_safe_range(5.0));
    }

    #[test]
    fn ramp_within_limit_ok() {
        let s = soc_spec();
        assert!(s.check_ramp(50.0, 55.0, 10.0).is_ok()); // 0.5 %/s < 1
    }

    #[test]
    fn ramp_exceeded_rejected() {
        let s = soc_spec();
        let err = s.check_ramp(50.0, 70.0, 5.0).unwrap_err(); // 4 %/s > 1
        assert!(matches!(err, RampViolation::Exceeded { .. }));
    }

    #[test]
    fn ramp_nonpositive_dt_rejected() {
        let s = soc_spec();
        assert!(matches!(
            s.check_ramp(50.0, 55.0, 0.0),
            Err(RampViolation::NonPositiveDt)
        ));
    }
}
