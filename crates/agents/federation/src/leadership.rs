//! # Phase 7 协调器领导权策略（蓝图 61 §12 §2.4）
//!
//! 「两节点自动共识不足以安全选主」：自动协调器 HA 要 3 个 voter 或独立
//! 硬件 witness；两节点只允许主备 + 人工 failover。
//!
//! 本模块把该约束做成构建/运行时可验证的策略：
//!
//! - [`LeadPolicy::validate`]：`voters < 3` 且 `auto_failover` → 拒绝
//!   （被 [`CoordinatorNegotiator`] 装配端与 svc-federation 启动时调用）；
//! - [`EpochRegulator`]：协调器纪元单调递增器——重启/接管时 +1，保证
//!   陈旧 leader 的消息落在接收方窗口外（配合 negotiation 的 epoch 窗口
//!   实现「旧 leader 即使网络恢复也不能主导」）。
//! - 人工 failover 只能由带外审批提供（本模块只提供 gate，不提供自动
//!   提拔路径）；真实 quorum/lease/fencing 机制在 v0.314.48（Phase 7
//!   第 2 版本）的 HA 模块扩展。
//!
//! 2 节点主备的活性由上层心跳/看门狗驱动（本项目 supervisor 既有心跳
//! 机制），本模块不重复实现心跳。

/// 领导权策略校验错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeadError {
    /// voter < 3 时请求自动 failover。
    TwoNodeAutoFailoverForbidden,
    /// voter 数非法（0）。
    NoVoters,
}

/// 协调器 HA 装配策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeadPolicy {
    /// 法定投票节点数（不包含本节点）。
    pub voters: u32,
    /// 是否允许自动故障转移（选举）。
    pub auto_failover: bool,
    /// 是否有独立硬件 witness（与 voters 二选一满足自动 HA）。
    pub hardware_witness: bool,
}

impl LeadPolicy {
    /// 组装约束：两节点（voters<3 且无 witness）只允许主备 + 人工
    /// failover；自动选主必须 3 voter 或硬件 witness。
    pub fn validate(&self) -> Result<(), LeadError> {
        if self.voters == 0 && !self.hardware_witness {
            return Err(LeadError::NoVoters);
        }
        if self.auto_failover && self.voters < 3 && !self.hardware_witness {
            return Err(LeadError::TwoNodeAutoFailoverForbidden);
        }
        Ok(())
    }

    /// 两节点安全配置（主备 + 人工 failover 的合法形态）。
    pub fn two_node_active_standby() -> Self {
        Self {
            voters: 2,
            auto_failover: false,
            hardware_witness: false,
        }
    }

    /// 三 voter 自动 HA 合法形态。
    pub fn three_voter_auto() -> Self {
        Self {
            voters: 3,
            auto_failover: true,
            hardware_witness: false,
        }
    }
}

/// 纪元调节器：协调器/站点重启后纪元 +1（持久化纪元 +1 → 新 leader
/// 身份；陈旧 leader 的重放因 epoch 不符被接收方窗口拒绝）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpochRegulator {
    last_epoch: u64,
}

impl EpochRegulator {
    pub fn new(last_persisted_epoch: u64) -> Self {
        Self {
            last_epoch: last_persisted_epoch,
        }
    }

    /// 本实例当前应使用的纪元（启动 = 持久化纪元 + 1）。
    pub fn current(&self) -> u64 {
        self.last_epoch + 1
    }

    /// 一次接管（人工 failover 或 quorum 选举后）→ 再 +1。
    pub fn take_over(&mut self) -> u64 {
        self.last_epoch += 1;
        self.current()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    #[test]
    fn two_node_auto_failover_forbidden() {
        let p = LeadPolicy {
            voters: 2,
            auto_failover: true,
            hardware_witness: false,
        };
        assert_eq!(p.validate(), Err(LeadError::TwoNodeAutoFailoverForbidden));
        // 主备 + 人工 = 合法。
        assert_eq!(LeadPolicy::two_node_active_standby().validate(), Ok(()));
        // 2 节点 + 硬件 witness + 自动 = 合法。
        let p2 = LeadPolicy {
            voters: 2,
            auto_failover: true,
            hardware_witness: true,
        };
        assert_eq!(p2.validate(), Ok(()));
    }

    #[test]
    fn three_voter_auto_ok() {
        assert_eq!(LeadPolicy::three_voter_auto().validate(), Ok(()));
        let p = LeadPolicy {
            voters: 5,
            auto_failover: true,
            hardware_witness: false,
        };
        assert_eq!(p.validate(), Ok(()));
    }

    #[test]
    fn no_voters_rejected() {
        let p = LeadPolicy {
            voters: 0,
            auto_failover: false,
            hardware_witness: false,
        };
        assert_eq!(p.validate(), Err(LeadError::NoVoters));
    }

    #[test]
    fn epoch_regulator_monotonic() {
        let mut r = EpochRegulator::new(41);
        assert_eq!(r.current(), 42);
        assert_eq!(r.take_over(), 43);
        assert_eq!(r.take_over(), 44);
    }
}
