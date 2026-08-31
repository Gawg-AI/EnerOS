//! # 蓝图 61 §12 v0.314.48：DR restore（§3.4）
//!
//! 恢复顺序不变式：先 registry/snapshot watermarks，再 journal/audit，
//! 未完成前只允许 D4/D6（降级语义由上层状态机执行，本模块提供
//! [`RestorePlanner`] 顺序约束与 RPO/RTO 记账）。
//! 目标（§3.4）：intent/audit 已确认写 RPO=0；C2 站内控制服务
//! ≤60s 进入 D4、≤5min 恢复 D0（外部业务可更长）。

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

/// 恢复组件。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RestoreComponent {
    Registry,
    SnapshotWatermarks,
    Journal,
    AuditHead,
    EventLog,
    BlobIndex,
}

/// 恢复阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RestorePhase {
    /// 控制面临界组件（决策前置）。
    ControlCritical,
    /// 业务/历史组件（可滞后）。
    Business,
}

impl RestoreComponent {
    pub fn phase(self) -> RestorePhase {
        match self {
            Self::Registry | Self::SnapshotWatermarks | Self::Journal | Self::AuditHead => {
                RestorePhase::ControlCritical
            }
            Self::EventLog | Self::BlobIndex => RestorePhase::Business,
        }
    }
}

/// 恢复计划：控制临界组件先恢复（顺序编排），业务组件随后。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestorePlanner {
    control_first: Vec<RestoreComponent>,
    business_after: Vec<RestoreComponent>,
}

impl RestorePlanner {
    pub fn new() -> Self {
        Self {
            control_first: vec![
                RestoreComponent::Registry,
                RestoreComponent::SnapshotWatermarks,
                RestoreComponent::Journal,
                RestoreComponent::AuditHead,
            ],
            business_after: vec![RestoreComponent::EventLog, RestoreComponent::BlobIndex],
        }
    }

    pub fn control_first(&self) -> &[RestoreComponent] {
        &self.control_first
    }

    pub fn business_after(&self) -> &[RestoreComponent] {
        &self.business_after
    }

    /// 完整顺序。
    pub fn full_order(&self) -> Vec<RestoreComponent> {
        self.control_first
            .iter()
            .chain(self.business_after.iter())
            .copied()
            .collect()
    }
}

impl Default for RestorePlanner {
    fn default() -> Self {
        Self::new()
    }
}

/// RPO/RTO 记账（目标侧实测入口；host 侧模拟用注入时钟）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryMetrics {
    /// 已确认写（intent/audit）在故障前的最大丢量（RPO=0 目标）。
    pub rpo_lost_records: u64,
    /// 恢复开始到可服务（进入 D4）耗时 ms。
    pub rto_to_d4_ms: u64,
    /// 恢复到完全正常（D0）耗时 ms。
    pub rto_to_d0_ms: u64,
}

/// RTO 上限（§3.4 参考）。
pub const RTO_TO_D4_MS: u64 = 60_000;
pub const RTO_TO_D0_MS: u64 = 300_000;

impl RecoveryMetrics {
    pub fn within_target(&self) -> bool {
        self.rpo_lost_records == 0
            && self.rto_to_d4_ms <= RTO_TO_D4_MS
            && self.rto_to_d0_ms <= RTO_TO_D0_MS
    }
}

/// 恢复进度跟踪：组件恢复状态机（幂等：重复标记无害）。
/// `started_ms` 供上层 RTO 记账（已确认写 RPO=0 由 [`RecoveryMetrics`] 承担）。
#[derive(Clone, Debug, Default)]
pub struct RestoreProgress {
    done: BTreeSet<RestoreComponent>,
    started_ms: u64,
}

impl RestoreProgress {
    pub fn new(started_ms: u64) -> Self {
        Self {
            done: BTreeSet::new(),
            started_ms,
        }
    }

    pub fn started_ms(&self) -> u64 {
        self.started_ms
    }

    pub fn mark_done(&mut self, c: RestoreComponent) {
        self.done.insert(c);
    }

    pub fn is_done(&self, c: RestoreComponent) -> bool {
        self.done.contains(&c)
    }

    /// 控制临界组件全部恢复 = 可进入 D4。
    pub fn control_ready(&self) -> bool {
        [
            RestoreComponent::Registry,
            RestoreComponent::SnapshotWatermarks,
            RestoreComponent::Journal,
            RestoreComponent::AuditHead,
        ]
        .iter()
        .all(|c| self.done.contains(c))
    }

    /// 全部恢复 = D0。
    pub fn fully_restored(&self) -> bool {
        self.control_ready()
            && self.done.contains(&RestoreComponent::EventLog)
            && self.done.contains(&RestoreComponent::BlobIndex)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    #[test]
    fn planner_orders_control_first() {
        let p = RestorePlanner::new();
        let order = p.full_order();
        let d4_pos = order
            .iter()
            .position(|c| *c == RestoreComponent::AuditHead)
            .expect("audit");
        let biz_pos = order
            .iter()
            .position(|c| *c == RestoreComponent::EventLog)
            .expect("event");
        assert!(d4_pos < biz_pos, "控制临界组件必须先于业务组件恢复");
    }

    #[test]
    fn progress_gates_d4_and_d0() {
        let mut pr = RestoreProgress::new(0);
        assert!(!pr.control_ready());
        pr.mark_done(RestoreComponent::Registry);
        pr.mark_done(RestoreComponent::SnapshotWatermarks);
        pr.mark_done(RestoreComponent::Journal);
        assert!(!pr.control_ready(), "audit head 未恢复不可进 D4");
        pr.mark_done(RestoreComponent::AuditHead);
        assert!(pr.control_ready());
        assert!(!pr.fully_restored());
        pr.mark_done(RestoreComponent::EventLog);
        pr.mark_done(RestoreComponent::BlobIndex);
        assert!(pr.fully_restored());
    }

    #[test]
    fn metrics_within_target() {
        let m = RecoveryMetrics {
            rpo_lost_records: 0,
            rto_to_d4_ms: 30_000,
            rto_to_d0_ms: 240_000,
        };
        assert!(m.within_target());
        let bad = RecoveryMetrics {
            rpo_lost_records: 3,
            rto_to_d4_ms: 30_000,
            rto_to_d0_ms: 240_000,
        };
        assert!(!bad.within_target(), "RPO 非零即不达标");
        let slow = RecoveryMetrics {
            rpo_lost_records: 0,
            rto_to_d4_ms: 90_000,
            rto_to_d0_ms: 400_000,
        };
        assert!(!slow.within_target(), "超过 RTO 上限不达标");
    }
}
