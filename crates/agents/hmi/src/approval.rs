//! 审批状态机 — 运维操作的审批流程管理（v0.42.1）
//!
//! 提供手动操作的审批流程：提交 → 审批/拒绝 → 执行 → 完成。
//! 所有状态存储在内存中（D14），重启后丢失。
//!
//! # 状态转换图
//!
//! ```text
//! Pending ──approve──→ Approved ──execute──→ Executed
//!    │                    │
//!    │                 reject
//!    │                    ↓
//!    └──────────────→ Rejected
//!    │
//!  expire
//!    ↓
//! Expired
//! ```
//!
//! # 偏差声明
//!
//! - **D14**: 审批状态机为内存实现（`BTreeMap`），无持久化
//!   （蓝图未要求持久化，MVP 阶段内存足够）。
//!
//! # no_std 合规
//!
//! 仅使用 `alloc::*` / `core::*`，无 `std::*`，无 `panic!`/`todo!`/`unimplemented!`。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{ApprovalId, HmiError, ManualAction};

/// 审批状态（5 种）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApprovalState {
    /// 待审批（初始状态）
    Pending,
    /// 已批准
    Approved,
    /// 已拒绝
    Rejected,
    /// 已执行（终态）
    Executed,
    /// 已过期（终态）
    Expired,
}

/// 待审批项
#[derive(Clone, Debug, PartialEq)]
pub struct PendingApproval {
    /// 审批 ID
    pub id: ApprovalId,
    /// 待审批的手动操作
    pub action: ManualAction,
    /// 请求者名称
    pub requester: String,
    /// 提交时间戳（ms）
    pub timestamp: u64,
    /// 当前审批状态
    pub state: ApprovalState,
}

/// 审批管理器（D14：内存状态机）
///
/// 管理审批的生命周期。所有状态存储在 `BTreeMap<ApprovalId, PendingApproval>` 中。
#[derive(Debug, Clone, Default)]
pub struct ApprovalManager {
    /// 审批项存储
    approvals: BTreeMap<ApprovalId, PendingApproval>,
    /// 下一个审批 ID（自增）
    next_id: u64,
}

impl ApprovalManager {
    /// 创建空审批管理器
    pub fn new() -> Self {
        Self::default()
    }

    /// 提交审批请求
    ///
    /// 创建一个新的 Pending 状态审批项，返回其 ID。
    pub fn submit(&mut self, action: ManualAction, requester: &str, now: u64) -> ApprovalId {
        let id = ApprovalId(self.next_id);
        self.next_id += 1;
        let approval = PendingApproval {
            id,
            action,
            requester: String::from(requester),
            timestamp: now,
            state: ApprovalState::Pending,
        };
        self.approvals.insert(id, approval);
        id
    }

    /// 批准审批
    ///
    /// 仅 Pending 状态可批准。转换：Pending → Approved
    pub fn approve(&mut self, id: ApprovalId) -> Result<(), HmiError> {
        let approval = self
            .approvals
            .get_mut(&id)
            .ok_or(HmiError::ApprovalNotFound(id))?;
        if approval.state != ApprovalState::Pending {
            return Err(HmiError::InvalidStateTransition {
                from: approval.state,
                to: ApprovalState::Approved,
            });
        }
        approval.state = ApprovalState::Approved;
        Ok(())
    }

    /// 拒绝审批
    ///
    /// 仅 Pending 状态可拒绝。转换：Pending → Rejected
    pub fn reject(&mut self, id: ApprovalId) -> Result<(), HmiError> {
        let approval = self
            .approvals
            .get_mut(&id)
            .ok_or(HmiError::ApprovalNotFound(id))?;
        if approval.state != ApprovalState::Pending {
            return Err(HmiError::InvalidStateTransition {
                from: approval.state,
                to: ApprovalState::Rejected,
            });
        }
        approval.state = ApprovalState::Rejected;
        Ok(())
    }

    /// 执行已批准的操作
    ///
    /// 仅 Approved 状态可执行。转换：Approved → Executed
    /// 返回待执行的操作（调用方负责实际执行）。
    pub fn execute(&mut self, id: ApprovalId) -> Result<ManualAction, HmiError> {
        let approval = self
            .approvals
            .get_mut(&id)
            .ok_or(HmiError::ApprovalNotFound(id))?;
        if approval.state != ApprovalState::Approved {
            return Err(HmiError::InvalidStateTransition {
                from: approval.state,
                to: ApprovalState::Executed,
            });
        }
        approval.state = ApprovalState::Executed;
        Ok(approval.action.clone())
    }

    /// 使审批过期
    ///
    /// 仅 Pending 状态可过期。转换：Pending → Expired
    pub fn expire(&mut self, id: ApprovalId) -> Result<(), HmiError> {
        let approval = self
            .approvals
            .get_mut(&id)
            .ok_or(HmiError::ApprovalNotFound(id))?;
        if approval.state != ApprovalState::Pending {
            return Err(HmiError::InvalidStateTransition {
                from: approval.state,
                to: ApprovalState::Expired,
            });
        }
        approval.state = ApprovalState::Expired;
        Ok(())
    }

    /// 列出所有 Pending 状态的审批
    pub fn list_pending(&self) -> Vec<&PendingApproval> {
        self.approvals
            .values()
            .filter(|a| a.state == ApprovalState::Pending)
            .collect()
    }

    /// 获取审批项
    pub fn get(&self, id: ApprovalId) -> Option<&PendingApproval> {
        self.approvals.get(&id)
    }

    /// 获取所有审批项
    pub fn list_all(&self) -> Vec<&PendingApproval> {
        self.approvals.values().collect()
    }

    /// 获取审批项数量
    pub fn count(&self) -> usize {
        self.approvals.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualAction;

    fn make_action(id: u64) -> ManualAction {
        ManualAction {
            id,
            action_type: String::from("restart_agent"),
            target_agent: None,
            params: String::from("{}"),
        }
    }

    #[test]
    fn test_submit_creates_pending_approval() {
        let mut mgr = ApprovalManager::new();
        let id = mgr.submit(make_action(1), "operator", 1000);
        let approval = mgr.get(id).unwrap();
        assert_eq!(approval.state, ApprovalState::Pending);
        assert_eq!(approval.requester, "operator");
        assert_eq!(approval.timestamp, 1000);
    }

    #[test]
    fn test_approve_pending() {
        let mut mgr = ApprovalManager::new();
        let id = mgr.submit(make_action(1), "op", 1000);
        assert!(mgr.approve(id).is_ok());
        assert_eq!(mgr.get(id).unwrap().state, ApprovalState::Approved);
    }

    #[test]
    fn test_reject_pending() {
        let mut mgr = ApprovalManager::new();
        let id = mgr.submit(make_action(1), "op", 1000);
        assert!(mgr.reject(id).is_ok());
        assert_eq!(mgr.get(id).unwrap().state, ApprovalState::Rejected);
    }

    #[test]
    fn test_execute_approved() {
        let mut mgr = ApprovalManager::new();
        let id = mgr.submit(make_action(1), "op", 1000);
        mgr.approve(id).unwrap();
        let action = mgr.execute(id).unwrap();
        assert_eq!(action.id, 1);
        assert_eq!(mgr.get(id).unwrap().state, ApprovalState::Executed);
    }

    #[test]
    fn test_expire_pending() {
        let mut mgr = ApprovalManager::new();
        let id = mgr.submit(make_action(1), "op", 1000);
        assert!(mgr.expire(id).is_ok());
        assert_eq!(mgr.get(id).unwrap().state, ApprovalState::Expired);
    }

    #[test]
    fn test_invalid_state_transition() {
        let mut mgr = ApprovalManager::new();
        let id = mgr.submit(make_action(1), "op", 1000);
        mgr.approve(id).unwrap();
        // Cannot approve again
        let err = mgr.approve(id).unwrap_err();
        assert!(matches!(
            err,
            HmiError::InvalidStateTransition {
                from: ApprovalState::Approved,
                to: ApprovalState::Approved
            }
        ));
        // Cannot reject an approved item
        let err = mgr.reject(id).unwrap_err();
        assert!(matches!(
            err,
            HmiError::InvalidStateTransition {
                from: ApprovalState::Approved,
                to: ApprovalState::Rejected
            }
        ));
    }

    #[test]
    fn test_approval_not_found() {
        let mut mgr = ApprovalManager::new();
        let err = mgr.approve(ApprovalId(999)).unwrap_err();
        assert!(matches!(err, HmiError::ApprovalNotFound(ApprovalId(999))));
    }

    #[test]
    fn test_list_pending() {
        let mut mgr = ApprovalManager::new();
        let id1 = mgr.submit(make_action(1), "op", 1000);
        let id2 = mgr.submit(make_action(2), "op", 1000);
        let id3 = mgr.submit(make_action(3), "op", 1000);
        mgr.approve(id1).unwrap(); // id1 no longer pending
        mgr.reject(id2).unwrap(); // id2 no longer pending
                                  // Only id3 should be pending
        let pending = mgr.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id3);
    }

    #[test]
    fn test_multiple_submits_increment_id() {
        let mut mgr = ApprovalManager::new();
        let id1 = mgr.submit(make_action(1), "op", 1000);
        let id2 = mgr.submit(make_action(2), "op", 1000);
        let id3 = mgr.submit(make_action(3), "op", 1000);
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_execute_not_approved_fails() {
        let mut mgr = ApprovalManager::new();
        let id = mgr.submit(make_action(1), "op", 1000);
        // Cannot execute a Pending item
        let err = mgr.execute(id).unwrap_err();
        assert!(matches!(
            err,
            HmiError::InvalidStateTransition {
                from: ApprovalState::Pending,
                to: ApprovalState::Executed
            }
        ));
    }

    #[test]
    fn test_count_and_list_all() {
        let mut mgr = ApprovalManager::new();
        mgr.submit(make_action(1), "op", 1000);
        mgr.submit(make_action(2), "op", 1000);
        assert_eq!(mgr.count(), 2);
        assert_eq!(mgr.list_all().len(), 2);
    }
}
