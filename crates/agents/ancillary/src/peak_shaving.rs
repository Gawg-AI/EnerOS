//! 调峰（削峰）控制
//!
//! 根据负荷阈值和 SOC 约束决定放电策略。

use crate::types::PeakShavePlan;
use crate::AncillaryError;

/// 削峰控制器
#[derive(Debug, Clone)]
pub struct PeakShaveController {
    /// 削峰计划参数
    pub plan: PeakShavePlan,
}

impl PeakShaveController {
    /// 创建削峰控制器
    pub fn new(plan: PeakShavePlan) -> Self {
        Self { plan }
    }

    /// 判断是否需要放电（负荷超过阈值）
    pub fn should_discharge(&self, current_load_kw: f64) -> bool {
        current_load_kw > self.plan.threshold_kw
    }

    /// 计算需要削减的功率 (kW)
    ///
    /// 若负荷未超阈值则返回 0.0。
    pub fn calculate_discharge_kw(&self, current_load_kw: f64) -> f64 {
        if current_load_kw <= self.plan.threshold_kw {
            return 0.0;
        }
        current_load_kw - self.plan.threshold_kw
    }

    /// 检查 SOC 是否满足最低要求
    pub fn check_soc(&self) -> Result<(), AncillaryError> {
        if self.plan.soc_current < self.plan.soc_min {
            return Err(AncillaryError::SocTooLow);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> PeakShavePlan {
        PeakShavePlan {
            threshold_kw: 800.0,
            discharge_limit_kwh: 2000.0,
            soc_min: 0.2,
            soc_current: 0.8,
        }
    }

    #[test]
    fn test_should_discharge_above_threshold() {
        let ctrl = PeakShaveController::new(sample_plan());
        assert!(ctrl.should_discharge(900.0));
        assert!(ctrl.should_discharge(801.0));
    }

    #[test]
    fn test_should_not_discharge_below_threshold() {
        let ctrl = PeakShaveController::new(sample_plan());
        assert!(!ctrl.should_discharge(800.0));
        assert!(!ctrl.should_discharge(500.0));
    }

    #[test]
    fn test_calculate_discharge_above() {
        let ctrl = PeakShaveController::new(sample_plan());
        let kw = ctrl.calculate_discharge_kw(1000.0);
        assert!((kw - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_discharge_below() {
        let ctrl = PeakShaveController::new(sample_plan());
        let kw = ctrl.calculate_discharge_kw(700.0);
        assert!((kw - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_check_soc_ok() {
        let ctrl = PeakShaveController::new(sample_plan());
        assert!(ctrl.check_soc().is_ok());
    }

    #[test]
    fn test_check_soc_too_low() {
        let plan = PeakShavePlan {
            soc_current: 0.1,
            ..sample_plan()
        };
        let ctrl = PeakShaveController::new(plan);
        assert_eq!(ctrl.check_soc().unwrap_err(), AncillaryError::SocTooLow);
    }

    #[test]
    fn test_check_soc_at_boundary() {
        let plan = PeakShavePlan {
            soc_current: 0.2,
            ..sample_plan()
        };
        let ctrl = PeakShaveController::new(plan);
        assert!(ctrl.check_soc().is_ok());
    }
}
