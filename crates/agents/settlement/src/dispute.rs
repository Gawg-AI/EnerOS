//! 争议处理模块：管理交易争议的创建、升级与解决。

use alloc::string::ToString;
use alloc::vec::Vec;

use crate::types::{DisputeCase, DisputeState};
use crate::SettlementError;

/// 争议管理器。
#[derive(Debug)]
pub struct DisputeManager {
    /// 所有争议案例。
    pub cases: Vec<DisputeCase>,
    /// 下一个争议 ID。
    next_id: u64,
}

impl Default for DisputeManager {
    fn default() -> Self {
        Self {
            cases: Vec::new(),
            next_id: 1,
        }
    }
}

impl DisputeManager {
    /// 创建争议管理器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 发起争议。
    ///
    /// 同一 deal_id 不允许存在未解决（Open/Arbitrating）的重复争议。
    pub fn open_dispute(
        &mut self,
        deal_id: u64,
        reason: &str,
        now: u64,
    ) -> Result<DisputeCase, SettlementError> {
        // 检查是否已存在未解决的争议
        let has_active = self
            .cases
            .iter()
            .any(|c| c.deal_id == deal_id && c.state != DisputeState::Resolved);
        if has_active {
            return Err(SettlementError::DuplicateDispute);
        }

        let case = DisputeCase {
            case_id: self.next_id,
            deal_id,
            reason: reason.to_string(),
            state: DisputeState::Open,
            opened_at: now,
        };
        self.next_id += 1;
        self.cases.push(case.clone());
        Ok(case)
    }

    /// 升级争议：Open → Arbitrating。
    pub fn escalate(&mut self, case_id: u64) -> Result<(), SettlementError> {
        let case = self
            .cases
            .iter_mut()
            .find(|c| c.case_id == case_id)
            .ok_or(SettlementError::NotFound)?;

        if case.state != DisputeState::Open {
            return Err(SettlementError::InvalidTransition);
        }
        case.state = DisputeState::Arbitrating;
        Ok(())
    }

    /// 解决争议：Arbitrating → Resolved。
    pub fn resolve(&mut self, case_id: u64) -> Result<(), SettlementError> {
        let case = self
            .cases
            .iter_mut()
            .find(|c| c.case_id == case_id)
            .ok_or(SettlementError::NotFound)?;

        if case.state != DisputeState::Arbitrating {
            return Err(SettlementError::InvalidTransition);
        }
        case.state = DisputeState::Resolved;
        Ok(())
    }

    /// 按 ID 查询争议案例。
    pub fn get_case(&self, case_id: u64) -> Option<&DisputeCase> {
        self.cases.iter().find(|c| c.case_id == case_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_dispute() {
        let mut mgr = DisputeManager::new();
        let case = mgr.open_dispute(100, "金额不符", 1000).unwrap();

        assert_eq!(case.case_id, 1);
        assert_eq!(case.deal_id, 100);
        assert_eq!(case.reason, "金额不符");
        assert_eq!(case.state, DisputeState::Open);
        assert_eq!(case.opened_at, 1000);
    }

    #[test]
    fn test_escalate() {
        let mut mgr = DisputeManager::new();
        mgr.open_dispute(100, "reason", 1000).unwrap();

        mgr.escalate(1).unwrap();
        let case = mgr.get_case(1).unwrap();
        assert_eq!(case.state, DisputeState::Arbitrating);
    }

    #[test]
    fn test_resolve() {
        let mut mgr = DisputeManager::new();
        mgr.open_dispute(100, "reason", 1000).unwrap();
        mgr.escalate(1).unwrap();

        mgr.resolve(1).unwrap();
        let case = mgr.get_case(1).unwrap();
        assert_eq!(case.state, DisputeState::Resolved);
    }

    #[test]
    fn test_escalate_not_open_fails() {
        let mut mgr = DisputeManager::new();
        mgr.open_dispute(100, "reason", 1000).unwrap();
        mgr.escalate(1).unwrap();

        // 已经是 Arbitrating，再次 escalate 应失败
        let result = mgr.escalate(1);
        assert_eq!(result.unwrap_err(), SettlementError::InvalidTransition);
    }

    #[test]
    fn test_resolve_not_arbitrating_fails() {
        let mut mgr = DisputeManager::new();
        mgr.open_dispute(100, "reason", 1000).unwrap();

        // 还是 Open 状态，直接 resolve 应失败
        let result = mgr.resolve(1);
        assert_eq!(result.unwrap_err(), SettlementError::InvalidTransition);
    }

    #[test]
    fn test_not_found() {
        let mut mgr = DisputeManager::new();
        let result = mgr.escalate(999);
        assert_eq!(result.unwrap_err(), SettlementError::NotFound);

        let result = mgr.resolve(999);
        assert_eq!(result.unwrap_err(), SettlementError::NotFound);
    }

    #[test]
    fn test_duplicate_dispute_same_deal() {
        let mut mgr = DisputeManager::new();
        mgr.open_dispute(100, "first", 1000).unwrap();

        // 同一 deal_id 再次发起争议应失败
        let result = mgr.open_dispute(100, "second", 2000);
        assert_eq!(result.unwrap_err(), SettlementError::DuplicateDispute);
    }

    #[test]
    fn test_dispute_after_resolved_allowed() {
        let mut mgr = DisputeManager::new();
        mgr.open_dispute(100, "first", 1000).unwrap();
        mgr.escalate(1).unwrap();
        mgr.resolve(1).unwrap();

        // 已解决后可以对同一 deal 再次发起争议
        let case = mgr.open_dispute(100, "second", 3000).unwrap();
        assert_eq!(case.case_id, 2);
        assert_eq!(case.state, DisputeState::Open);
    }

    #[test]
    fn test_get_case_none() {
        let mgr = DisputeManager::new();
        assert!(mgr.get_case(1).is_none());
    }
}
