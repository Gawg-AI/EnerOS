//! 负荷聚合与调度

use alloc::vec::Vec;

use eneros_allocation::{proportional, sanitize};

use crate::types::{AggregatedCapacity, DrAssignment, DrParticipant, DrSignal, DrSignalType};
use crate::DrError;

/// DR 聚合器 trait
///
/// 抽象需求响应聚合器的核心行为。
pub trait DrAggregator {
    /// 聚合所有可用参与者的容量
    fn aggregate_capacity(&self) -> AggregatedCapacity;

    /// 根据 DR 信号按比例分配负荷
    fn dispatch(&self, sig: &DrSignal) -> Result<Vec<DrAssignment>, DrError>;
}

/// DR 聚合器实现
#[derive(Debug, Clone, Default)]
pub struct DrAggregatorImpl {
    /// 参与者列表
    participants: Vec<DrParticipant>,
}

impl DrAggregatorImpl {
    /// 创建空聚合器
    pub fn new() -> Self {
        Self {
            participants: Vec::new(),
        }
    }

    /// 添加参与者
    pub fn add_participant(&mut self, p: DrParticipant) {
        self.participants.push(p);
    }

    /// 移除参与者
    pub fn remove_participant(&mut self, agent_id: u64) {
        self.participants.retain(|p| p.agent_id != agent_id);
    }
}

impl DrAggregator for DrAggregatorImpl {
    fn aggregate_capacity(&self) -> AggregatedCapacity {
        let mut reducible_kw = 0.0;
        let mut increasable_kw = 0.0;
        let mut participants = 0usize;

        for p in self.participants.iter().filter(|p| p.available) {
            // D-4 修复（kernel sanitize 单源）：非有限贡献按 0 计——
            // 聚合恒有限；available 计数语义不动。
            reducible_kw += sanitize(p.base_kw - p.min_kw).unwrap_or(0.0);
            increasable_kw += sanitize(p.max_kw - p.base_kw).unwrap_or(0.0);
            participants += 1;
        }

        AggregatedCapacity {
            reducible_kw,
            increasable_kw,
            participants,
        }
    }

    fn dispatch(&self, sig: &DrSignal) -> Result<Vec<DrAssignment>, DrError> {
        let available: Vec<&DrParticipant> =
            self.participants.iter().filter(|p| p.available).collect();

        if available.is_empty() {
            return Err(DrError::NoParticipants);
        }

        // 有限性门（kernel sanitize 单源，D-4）：非有限 target 显式拒。
        let target = match sanitize(sig.target_kw) {
            Some(t) => t,
            None => return Err(DrError::InvalidSignal),
        };

        // 根据信号类型确定每个参与者的可用容量（非有限贡献按 0 计——
        // 聚合器吞错政策；输出形状不变：每可用参与者一条）。
        let capacities: Vec<(u64, f64)> = available
            .iter()
            .map(|p| {
                let cap = match sig.signal_type {
                    DrSignalType::LoadReduce | DrSignalType::Curtailment => p.base_kw - p.min_kw,
                    DrSignalType::LoadIncrease => p.max_kw - p.base_kw,
                };
                (p.agent_id, sanitize(cap).unwrap_or(0.0))
            })
            .collect();

        // 比例分配 + 钳位单源（kernel；Σcap ≤ 0 / target > Σcap →
        // 容量不足——判拒语义保留在本聚合器）。
        let shares = proportional(target, &capacities).ok_or(DrError::CapacityInsufficient)?;

        let assignments: Vec<DrAssignment> = shares
            .into_iter()
            .map(|(agent_id, assign_kw)| DrAssignment {
                agent_id,
                assign_kw,
                acknowledged: false,
            })
            .collect();

        Ok(assignments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DrSignalType;

    fn make_participant(id: u64, base: f64, min: f64, max: f64, available: bool) -> DrParticipant {
        DrParticipant {
            agent_id: id,
            base_kw: base,
            min_kw: min,
            max_kw: max,
            available,
        }
    }

    fn make_signal(signal_type: DrSignalType, target_kw: f64) -> DrSignal {
        DrSignal {
            event_id: 1,
            signal_type,
            start_at: 1000,
            end_at: 2000,
            target_kw,
            incentive_yuan: 500.0,
        }
    }

    #[test]
    fn test_aggregate_capacity_basic() {
        let mut agg = DrAggregatorImpl::new();
        agg.add_participant(make_participant(1, 200.0, 50.0, 400.0, true));
        agg.add_participant(make_participant(2, 300.0, 100.0, 500.0, true));

        let cap = agg.aggregate_capacity();
        assert_eq!(cap.participants, 2);
        // reducible: (200-50) + (300-100) = 150 + 200 = 350
        assert!((cap.reducible_kw - 350.0).abs() < 1e-9);
        // increasable: (400-200) + (500-300) = 200 + 200 = 400
        assert!((cap.increasable_kw - 400.0).abs() < 1e-9);
    }

    #[test]
    fn test_aggregate_capacity_skips_unavailable() {
        let mut agg = DrAggregatorImpl::new();
        agg.add_participant(make_participant(1, 200.0, 50.0, 400.0, true));
        agg.add_participant(make_participant(2, 300.0, 100.0, 500.0, false));

        let cap = agg.aggregate_capacity();
        assert_eq!(cap.participants, 1);
        assert!((cap.reducible_kw - 150.0).abs() < 1e-9);
        assert!((cap.increasable_kw - 200.0).abs() < 1e-9);
    }

    #[test]
    fn test_dispatch_proportional_load_reduce() {
        let mut agg = DrAggregatorImpl::new();
        // p1: reducible = 150, p2: reducible = 200, total = 350
        agg.add_participant(make_participant(1, 200.0, 50.0, 400.0, true));
        agg.add_participant(make_participant(2, 300.0, 100.0, 500.0, true));

        let sig = make_signal(DrSignalType::LoadReduce, 175.0);
        let assignments = agg.dispatch(&sig).unwrap();

        assert_eq!(assignments.len(), 2);
        // p1: (150/350)*175 = 75
        assert!((assignments[0].assign_kw - 75.0).abs() < 1e-9);
        // p2: (200/350)*175 = 100
        assert!((assignments[1].assign_kw - 100.0).abs() < 1e-9);
        assert!(!assignments[0].acknowledged);
    }

    #[test]
    fn test_dispatch_proportional_load_increase() {
        let mut agg = DrAggregatorImpl::new();
        // p1: increasable = 200, p2: increasable = 200, total = 400
        agg.add_participant(make_participant(1, 200.0, 50.0, 400.0, true));
        agg.add_participant(make_participant(2, 300.0, 100.0, 500.0, true));

        let sig = make_signal(DrSignalType::LoadIncrease, 200.0);
        let assignments = agg.dispatch(&sig).unwrap();

        assert_eq!(assignments.len(), 2);
        // p1: (200/400)*200 = 100
        assert!((assignments[0].assign_kw - 100.0).abs() < 1e-9);
        // p2: (200/400)*200 = 100
        assert!((assignments[1].assign_kw - 100.0).abs() < 1e-9);
    }

    #[test]
    fn test_dispatch_clamping() {
        let mut agg = DrAggregatorImpl::new();
        // p1: reducible = 10, p2: reducible = 1000, total = 1010
        agg.add_participant(make_participant(1, 100.0, 90.0, 200.0, true));
        agg.add_participant(make_participant(2, 1000.0, 0.0, 2000.0, true));

        // target = 1010, p1 raw = (10/1010)*1010 = 10 (clamped to cap 10)
        let sig = make_signal(DrSignalType::LoadReduce, 1010.0);
        let assignments = agg.dispatch(&sig).unwrap();

        assert!((assignments[0].assign_kw - 10.0).abs() < 1e-9);
        assert!((assignments[1].assign_kw - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn test_dispatch_no_available_participants() {
        let mut agg = DrAggregatorImpl::new();
        agg.add_participant(make_participant(1, 200.0, 50.0, 400.0, false));

        let sig = make_signal(DrSignalType::LoadReduce, 100.0);
        assert_eq!(agg.dispatch(&sig), Err(DrError::NoParticipants));
    }

    #[test]
    fn test_dispatch_capacity_insufficient() {
        let mut agg = DrAggregatorImpl::new();
        agg.add_participant(make_participant(1, 200.0, 50.0, 400.0, true));

        // reducible = 150, target = 200 > 150
        let sig = make_signal(DrSignalType::LoadReduce, 200.0);
        assert_eq!(agg.dispatch(&sig), Err(DrError::CapacityInsufficient));
    }

    #[test]
    fn test_remove_participant() {
        let mut agg = DrAggregatorImpl::new();
        agg.add_participant(make_participant(1, 200.0, 50.0, 400.0, true));
        agg.add_participant(make_participant(2, 300.0, 100.0, 500.0, true));
        agg.remove_participant(1);

        let cap = agg.aggregate_capacity();
        assert_eq!(cap.participants, 1);
        assert!((cap.reducible_kw - 200.0).abs() < 1e-9);
    }

    // =========================================================================
    // D-4（蓝图 62 卷 62.3 WP-62.3.2）：非有限容量零防御 → sanitize 纪律。
    // participant 是全 pub f64 字段直构（types.rs 无校验构造器）——聚合器
    // 是 NaN 进系统的最后闸门。
    // =========================================================================

    /// D-4-①：participant 携带 f64::NAN 容量时 aggregate_capacity 必须
    /// 不产出 NaN（非有限贡献按 0 计——输出恒有限）。
    #[test]
    fn nan_capacity_never_produces_nan_aggregate() {
        let mut agg = DrAggregatorImpl::new();
        agg.add_participant(make_participant(1, f64::NAN, 50.0, 400.0, true));
        agg.add_participant(make_participant(2, 300.0, 100.0, 500.0, true));

        let cap = agg.aggregate_capacity();
        assert!(
            cap.reducible_kw.is_finite(),
            "reducible 恒有限: {}",
            cap.reducible_kw
        );
        assert!(
            cap.increasable_kw.is_finite(),
            "increasable 恒有限: {}",
            cap.increasable_kw
        );
        // 非有限贡献按 0 计：reducible = 0 + (300-100) = 200。
        assert!((cap.reducible_kw - 200.0).abs() < 1e-9);
        assert_eq!(
            cap.participants, 2,
            "available 语义不动（计数仍含该参与者）"
        );
    }

    /// D-4-②：非有限 target → InvalidSignal；非有限参与者容量 → 分配
    /// 恒有限且 Σ ≤ target。
    #[test]
    fn non_finite_rejected_or_finite_in_dispatch() {
        let mut agg = DrAggregatorImpl::new();
        agg.add_participant(make_participant(1, 200.0, 50.0, 400.0, true));

        let sig = make_signal(DrSignalType::LoadReduce, f64::NAN);
        assert_eq!(agg.dispatch(&sig), Err(DrError::InvalidSignal));
        let sig = make_signal(DrSignalType::LoadReduce, f64::INFINITY);
        assert_eq!(agg.dispatch(&sig), Err(DrError::InvalidSignal));
        let sig = make_signal(DrSignalType::LoadReduce, f64::NEG_INFINITY);
        assert_eq!(agg.dispatch(&sig), Err(DrError::InvalidSignal));

        // 非有限参与者容量混入：分配恒有限（该贡献按 0 计）。
        agg.add_participant(make_participant(2, f64::NAN, 0.0, f64::NAN, true));
        let sig = make_signal(DrSignalType::LoadReduce, 100.0);
        let assignments = agg.dispatch(&sig).expect("finite target dispatches");
        assert_eq!(assignments.len(), 2, "输出形状不变（每可用参与者一条）");
        assert!(assignments.iter().all(|a| a.assign_kw.is_finite()));
        let total: f64 = assignments.iter().map(|a| a.assign_kw).sum();
        assert!(total <= 100.0 + 1e-9, "Σ assignment ≤ target: {total}");
    }
}
