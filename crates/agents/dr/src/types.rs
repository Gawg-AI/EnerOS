//! 需求响应数据类型定义

/// DR 信号类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrSignalType {
    /// 削负荷
    #[default]
    LoadReduce,
    /// 增负荷
    LoadIncrease,
    /// 限电
    Curtailment,
}

/// DR 事件信号（ARC-005 统一量纲：target_kw/kW、start_at·end_at/unix 秒、
/// incentive_yuan/元——全仓唯一定义，energy-market-agent re-export）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DrSignal {
    /// 事件 ID
    pub event_id: u64,
    /// 信号类型
    pub signal_type: DrSignalType,
    /// 事件开始时间 (unix timestamp)
    pub start_at: u64,
    /// 事件结束时间 (unix timestamp)
    pub end_at: u64,
    /// 目标功率 (kW)
    pub target_kw: f64,
    /// 激励金额 (元)
    pub incentive_yuan: f64,
}

/// DR 参与者
#[derive(Debug, Clone, PartialEq)]
pub struct DrParticipant {
    /// 代理 ID
    pub agent_id: u64,
    /// 基线负荷 (kW)
    pub base_kw: f64,
    /// 最小负荷 (kW)
    pub min_kw: f64,
    /// 最大负荷 (kW)
    pub max_kw: f64,
    /// 是否可用
    pub available: bool,
}

/// DR 分配结果
#[derive(Debug, Clone, PartialEq)]
pub struct DrAssignment {
    /// 代理 ID
    pub agent_id: u64,
    /// 分配功率 (kW)
    pub assign_kw: f64,
    /// 是否已确认
    pub acknowledged: bool,
}

/// 聚合容量
#[derive(Debug, Clone, PartialEq)]
pub struct AggregatedCapacity {
    /// 可削减容量 (kW)
    pub reducible_kw: f64,
    /// 可增加容量 (kW)
    pub increasable_kw: f64,
    /// 参与者数量
    pub participants: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_type_eq() {
        assert_eq!(DrSignalType::LoadReduce, DrSignalType::LoadReduce);
        assert_ne!(DrSignalType::LoadReduce, DrSignalType::LoadIncrease);
        assert_ne!(DrSignalType::Curtailment, DrSignalType::LoadIncrease);
    }

    #[test]
    fn test_dr_signal_creation() {
        let sig = DrSignal {
            event_id: 1,
            signal_type: DrSignalType::LoadReduce,
            start_at: 1000,
            end_at: 2000,
            target_kw: 500.0,
            incentive_yuan: 1000.0,
        };
        assert_eq!(sig.event_id, 1);
        assert_eq!(sig.signal_type, DrSignalType::LoadReduce);
        assert_eq!(sig.start_at, 1000);
        assert_eq!(sig.end_at, 2000);
        assert!((sig.target_kw - 500.0).abs() < f64::EPSILON);
        assert!((sig.incentive_yuan - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_dr_participant_creation() {
        let p = DrParticipant {
            agent_id: 10,
            base_kw: 200.0,
            min_kw: 50.0,
            max_kw: 400.0,
            available: true,
        };
        assert_eq!(p.agent_id, 10);
        assert!(p.available);
        assert!((p.base_kw - 200.0).abs() < f64::EPSILON);
        assert!((p.min_kw - 50.0).abs() < f64::EPSILON);
        assert!((p.max_kw - 400.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_dr_assignment_creation() {
        let a = DrAssignment {
            agent_id: 10,
            assign_kw: 100.0,
            acknowledged: false,
        };
        assert_eq!(a.agent_id, 10);
        assert!(!a.acknowledged);
    }

    #[test]
    fn test_aggregated_capacity_creation() {
        let cap = AggregatedCapacity {
            reducible_kw: 300.0,
            increasable_kw: 200.0,
            participants: 5,
        };
        assert_eq!(cap.participants, 5);
        assert!((cap.reducible_kw - 300.0).abs() < f64::EPSILON);
        assert!((cap.increasable_kw - 200.0).abs() < f64::EPSILON);
    }
}
