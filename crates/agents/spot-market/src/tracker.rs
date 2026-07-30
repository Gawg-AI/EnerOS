//! 执行状态跟踪

use crate::types::ExecStatus;
use crate::MarketError;

/// 偏差阈值：5%
const DEVIATION_THRESHOLD: f64 = 0.05;

/// 执行跟踪器
///
/// 跟踪单个报价的执行状态，包含计划电量与实际电量的偏差检测。
#[derive(Debug, Clone)]
pub struct ExecTracker {
    /// 报价 ID
    pub bid_id: u64,
    /// 当前状态
    pub state: ExecStatus,
    /// 计划电量 (kWh)
    pub planned_kwh: f64,
    /// 实际电量 (kWh)
    pub actual_kwh: f64,
}

impl ExecTracker {
    /// 创建执行跟踪器（初始状态为 Submitted）
    pub fn new(bid_id: u64, planned_kwh: f64) -> Self {
        Self {
            bid_id,
            state: ExecStatus::Submitted,
            planned_kwh,
            actual_kwh: 0.0,
        }
    }

    /// 状态转换
    ///
    /// 合法转换路径：
    /// - Submitted -> Cleared | Failed
    /// - Cleared -> Dispatching | Failed
    /// - Dispatching -> Settled | Deviated | Failed
    /// - Deviated -> Settled | Failed
    pub fn transition(&mut self, new_state: ExecStatus) -> Result<(), MarketError> {
        let valid = matches!(
            (self.state, new_state),
            (ExecStatus::Submitted, ExecStatus::Cleared)
                | (ExecStatus::Submitted, ExecStatus::Failed)
                | (ExecStatus::Cleared, ExecStatus::Dispatching)
                | (ExecStatus::Cleared, ExecStatus::Failed)
                | (ExecStatus::Dispatching, ExecStatus::Settled)
                | (ExecStatus::Dispatching, ExecStatus::Deviated)
                | (ExecStatus::Dispatching, ExecStatus::Failed)
                | (ExecStatus::Deviated, ExecStatus::Settled)
                | (ExecStatus::Deviated, ExecStatus::Failed)
        );
        if valid {
            self.state = new_state;
            Ok(())
        } else {
            Err(MarketError::InvalidStateTransition)
        }
    }

    /// 更新实际电量并检测偏差
    ///
    /// 如果偏差超过 5%，自动将状态转为 Deviated。
    pub fn update_actual(&mut self, kwh: f64) -> Result<(), MarketError> {
        self.actual_kwh = kwh;
        if self.planned_kwh > 0.0 {
            let deviation = ((self.actual_kwh - self.planned_kwh) / self.planned_kwh).abs();
            if deviation > DEVIATION_THRESHOLD
                && (self.state == ExecStatus::Dispatching || self.state == ExecStatus::Cleared)
            {
                self.state = ExecStatus::Deviated;
            }
        }
        Ok(())
    }

    /// 获取当前偏差率
    pub fn deviation_ratio(&self) -> f64 {
        if self.planned_kwh <= 0.0 {
            return 0.0;
        }
        ((self.actual_kwh - self.planned_kwh) / self.planned_kwh).abs()
    }

    /// 是否已偏差
    pub fn is_deviated(&self) -> bool {
        self.state == ExecStatus::Deviated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_lifecycle() {
        let mut t = ExecTracker::new(1, 100.0);
        assert_eq!(t.state, ExecStatus::Submitted);

        t.transition(ExecStatus::Cleared).unwrap();
        assert_eq!(t.state, ExecStatus::Cleared);

        t.transition(ExecStatus::Dispatching).unwrap();
        assert_eq!(t.state, ExecStatus::Dispatching);

        t.transition(ExecStatus::Settled).unwrap();
        assert_eq!(t.state, ExecStatus::Settled);
    }

    #[test]
    fn test_invalid_transition() {
        let mut t = ExecTracker::new(1, 100.0);
        // Submitted -> Settled 非法
        assert_eq!(
            t.transition(ExecStatus::Settled),
            Err(MarketError::InvalidStateTransition)
        );
        // Submitted -> Dispatching 非法
        assert_eq!(
            t.transition(ExecStatus::Dispatching),
            Err(MarketError::InvalidStateTransition)
        );
    }

    #[test]
    fn test_fail_from_any_active_state() {
        let mut t = ExecTracker::new(1, 100.0);
        t.transition(ExecStatus::Failed).unwrap();
        assert_eq!(t.state, ExecStatus::Failed);

        let mut t2 = ExecTracker::new(2, 100.0);
        t2.transition(ExecStatus::Cleared).unwrap();
        t2.transition(ExecStatus::Failed).unwrap();
        assert_eq!(t2.state, ExecStatus::Failed);
    }

    #[test]
    fn test_deviation_detection() {
        let mut t = ExecTracker::new(1, 100.0);
        t.transition(ExecStatus::Cleared).unwrap();
        t.transition(ExecStatus::Dispatching).unwrap();

        // 偏差 10% > 5%
        t.update_actual(110.0).unwrap();
        assert!(t.is_deviated());
        assert_eq!(t.state, ExecStatus::Deviated);
    }

    #[test]
    fn test_no_deviation_within_threshold() {
        let mut t = ExecTracker::new(1, 100.0);
        t.transition(ExecStatus::Cleared).unwrap();
        t.transition(ExecStatus::Dispatching).unwrap();

        // 偏差 3% < 5%
        t.update_actual(103.0).unwrap();
        assert!(!t.is_deviated());
        assert_eq!(t.state, ExecStatus::Dispatching);
    }

    #[test]
    fn test_deviation_ratio() {
        let mut t = ExecTracker::new(1, 200.0);
        t.update_actual(220.0).unwrap();
        let ratio = t.deviation_ratio();
        assert!((ratio - 0.1).abs() < 1e-9);
    }

    #[test]
    fn test_deviated_to_settled() {
        let mut t = ExecTracker::new(1, 100.0);
        t.transition(ExecStatus::Cleared).unwrap();
        t.transition(ExecStatus::Dispatching).unwrap();
        t.update_actual(120.0).unwrap();
        assert_eq!(t.state, ExecStatus::Deviated);
        t.transition(ExecStatus::Settled).unwrap();
        assert_eq!(t.state, ExecStatus::Settled);
    }

    #[test]
    fn test_settled_is_terminal() {
        let mut t = ExecTracker::new(1, 100.0);
        t.transition(ExecStatus::Cleared).unwrap();
        t.transition(ExecStatus::Dispatching).unwrap();
        t.transition(ExecStatus::Settled).unwrap();
        // 从 Settled 不能再转换
        assert_eq!(
            t.transition(ExecStatus::Failed),
            Err(MarketError::InvalidStateTransition)
        );
    }
}
