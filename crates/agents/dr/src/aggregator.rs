//! 负荷聚合与调度

use alloc::vec::Vec;

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
            reducible_kw += p.base_kw - p.min_kw;
            increasable_kw += p.max_kw - p.base_kw;
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

        // 根据信号类型确定每个参与者的可用容量
        let capacities: Vec<f64> = available
            .iter()
            .map(|p| match sig.signal_type {
                DrSignalType::LoadReduce | DrSignalType::Curtailment => p.base_kw - p.min_kw,
                DrSignalType::LoadIncrease => p.max_kw - p.base_kw,
            })
            .collect();

        let total_capacity: f64 = capacities.iter().sum();
        if total_capacity <= 0.0 {
            return Err(DrError::CapacityInsufficient);
        }

        if sig.target_kw > total_capacity {
            return Err(DrError::CapacityInsufficient);
        }

        // 按比例分配，并钳位到参与者限制
        let assignments: Vec<DrAssignment> = available
            .iter()
            .zip(capacities.iter())
            .map(|(p, &cap)| {
                let raw = (cap / total_capacity) * sig.target_kw;
                // 钳位：不超过参与者自身容量
                let assign_kw = raw.min(cap).max(0.0);
                DrAssignment {
                    agent_id: p.agent_id,
                    assign_kw,
                    acknowledged: false,
                }
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
}
