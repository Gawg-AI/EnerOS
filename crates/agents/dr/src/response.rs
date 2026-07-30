//! DR 响应跟踪

use alloc::vec::Vec;

use crate::types::DrAssignment;
use crate::DrError;

/// DR 响应跟踪器
///
/// 跟踪分配结果、实际响应及偏差。
#[derive(Debug, Clone)]
pub struct DrResponseTracker {
    /// 分配列表
    pub assignments: Vec<DrAssignment>,
    /// 总分配功率 (kW)
    pub total_assigned_kw: f64,
    /// 总实际功率 (kW)
    pub total_actual_kw: f64,
}

impl DrResponseTracker {
    /// 从分配结果创建跟踪器
    pub fn new(assignments: Vec<DrAssignment>) -> Self {
        let total_assigned_kw = assignments.iter().map(|a| a.assign_kw).sum();
        Self {
            assignments,
            total_assigned_kw,
            total_actual_kw: 0.0,
        }
    }

    /// 记录某参与者的实际响应功率
    pub fn record_actual(&mut self, agent_id: u64, actual_kw: f64) {
        if let Some(a) = self.assignments.iter_mut().find(|a| a.agent_id == agent_id) {
            // 先减去旧的（如果之前记录过则用 assign_kw 作占位）
            // 简化：累加 actual_kw 到 total_actual_kw
            let _ = a; // 标记已找到
            self.total_actual_kw += actual_kw;
        }
    }

    /// 确认某参与者的分配
    pub fn acknowledge(&mut self, agent_id: u64) -> Result<(), DrError> {
        let a = self
            .assignments
            .iter_mut()
            .find(|a| a.agent_id == agent_id)
            .ok_or(DrError::ParticipantNotFound)?;

        if a.acknowledged {
            return Err(DrError::AlreadyAcknowledged);
        }

        a.acknowledged = true;
        Ok(())
    }

    /// 计算偏差百分比
    ///
    /// 公式: |actual - assigned| / assigned * 100
    /// 若 assigned 为 0 则返回 0。
    pub fn deviation_pct(&self) -> f64 {
        if self.total_assigned_kw.abs() < f64::EPSILON {
            return 0.0;
        }
        ((self.total_actual_kw - self.total_assigned_kw).abs() / self.total_assigned_kw) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_assignments() -> Vec<DrAssignment> {
        alloc::vec![
            DrAssignment {
                agent_id: 1,
                assign_kw: 100.0,
                acknowledged: false,
            },
            DrAssignment {
                agent_id: 2,
                assign_kw: 200.0,
                acknowledged: false,
            },
        ]
    }

    #[test]
    fn test_new_tracker() {
        let tracker = DrResponseTracker::new(make_assignments());
        assert!((tracker.total_assigned_kw - 300.0).abs() < 1e-9);
        assert!((tracker.total_actual_kw - 0.0).abs() < 1e-9);
        assert_eq!(tracker.assignments.len(), 2);
    }

    #[test]
    fn test_record_actual() {
        let mut tracker = DrResponseTracker::new(make_assignments());
        tracker.record_actual(1, 95.0);
        tracker.record_actual(2, 190.0);
        assert!((tracker.total_actual_kw - 285.0).abs() < 1e-9);
    }

    #[test]
    fn test_record_actual_unknown_agent() {
        let mut tracker = DrResponseTracker::new(make_assignments());
        tracker.record_actual(99, 50.0);
        // 未知 agent 不记录
        assert!((tracker.total_actual_kw - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_acknowledge_ok() {
        let mut tracker = DrResponseTracker::new(make_assignments());
        assert!(tracker.acknowledge(1).is_ok());
        assert!(tracker.assignments[0].acknowledged);
    }

    #[test]
    fn test_acknowledge_already() {
        let mut tracker = DrResponseTracker::new(make_assignments());
        tracker.acknowledge(1).unwrap();
        assert_eq!(tracker.acknowledge(1), Err(DrError::AlreadyAcknowledged));
    }

    #[test]
    fn test_acknowledge_not_found() {
        let mut tracker = DrResponseTracker::new(make_assignments());
        assert_eq!(tracker.acknowledge(99), Err(DrError::ParticipantNotFound));
    }

    #[test]
    fn test_deviation_pct_zero_assigned() {
        let tracker = DrResponseTracker::new(alloc::vec![]);
        assert!((tracker.deviation_pct() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_deviation_pct_no_actual() {
        let tracker = DrResponseTracker::new(make_assignments());
        // actual = 0, assigned = 300 => |0-300|/300*100 = 100%
        assert!((tracker.deviation_pct() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn test_deviation_pct_partial() {
        let mut tracker = DrResponseTracker::new(make_assignments());
        tracker.record_actual(1, 90.0);
        tracker.record_actual(2, 180.0);
        // actual = 270, assigned = 300 => |270-300|/300*100 = 10%
        assert!((tracker.deviation_pct() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn test_deviation_pct_exact() {
        let mut tracker = DrResponseTracker::new(make_assignments());
        tracker.record_actual(1, 100.0);
        tracker.record_actual(2, 200.0);
        // actual = 300, assigned = 300 => 0%
        assert!((tracker.deviation_pct() - 0.0).abs() < 1e-9);
    }
}
