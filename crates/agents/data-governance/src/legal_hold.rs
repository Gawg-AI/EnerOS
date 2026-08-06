//! 法定保留 (DATA-006).
//!
//! MASTER §21.17 / G8: "legal hold — 权限/审计".
//!
//! 法定保留（Legal Hold）防止受诉讼/监管约束的数据被删除或修改。
//! 每个保留项必须有权限审批（授权人 + 理由）和完整审计记录。
//!
//! # 证据推导
//!
//! 法定保留合规结论由证据推导：
//! 1. 每个 Active 保留项有授权人（authorized_by 非空）和理由
//! 2. 每个保留项至少有 Created 审计条目
//! 3. 受保留数据无删除记录（删除被拒绝）
//! 4. Released 保留项有 Released 审计条目
//! 5. 未过期的 Active 保留项仍在生效

use alloc::string::String;
use alloc::vec::Vec;

/// 保留状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldState {
    /// 生效中.
    Active,
    /// 已释放（经审批）.
    Released,
    /// 已过期（未续期）.
    Expired,
}

/// 保留权限来源.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldPermission {
    /// 法务部门.
    Legal,
    /// 合规官.
    ComplianceOfficer,
    /// 监管要求.
    Regulatory,
    /// 法院命令.
    CourtOrder,
}

impl HoldPermission {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Legal => "legal",
            Self::ComplianceOfficer => "compliance-officer",
            Self::Regulatory => "regulatory",
            Self::CourtOrder => "court-order",
        }
    }
}

/// 法定保留项.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldRecord {
    /// 保留 ID.
    pub hold_id: String,
    /// 受保留数据项 ID.
    pub data_id: String,
    /// 保留理由.
    pub reason: String,
    /// 授权来源.
    pub permission: HoldPermission,
    /// 授权人（非空才算有效审批）.
    pub authorized_by: String,
    /// 创建时间（Unix 秒）.
    pub created_at: u64,
    /// 过期时间（Unix 秒，0 = 无限期）.
    pub expires_at: u64,
    /// 当前状态.
    pub state: HoldState,
}

impl HoldRecord {
    /// 创建保留项.
    pub fn new(
        hold_id: &str,
        data_id: &str,
        reason: &str,
        permission: HoldPermission,
        authorized_by: &str,
        created_at: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            hold_id: String::from(hold_id),
            data_id: String::from(data_id),
            reason: String::from(reason),
            permission,
            authorized_by: String::from(authorized_by),
            created_at,
            expires_at,
            state: HoldState::Active,
        }
    }

    /// 是否有有效授权（授权人非空）.
    pub fn has_valid_authorization(&self) -> bool {
        !self.authorized_by.is_empty() && !self.reason.is_empty()
    }

    /// 在给定时间点是否仍生效.
    pub fn is_active_at(&self, now: u64) -> bool {
        if self.state != HoldState::Active {
            return false;
        }
        if self.expires_at == 0 {
            return true;
        }
        now < self.expires_at
    }

    /// 是否已过期（状态仍为 Active 但时间已过）.
    pub fn is_lapsed(&self, now: u64) -> bool {
        self.state == HoldState::Active && self.expires_at != 0 && now >= self.expires_at
    }
}

/// 审计动作.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldAuditAction {
    /// 保留创建.
    Created,
    /// 保留延期.
    Extended,
    /// 保留释放.
    Released,
    /// 受保留数据被访问（授权）.
    Accessed,
    /// 删除请求被拒绝.
    DeletionDenied,
}

/// 审计条目.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldAuditEntry {
    /// 关联的保留 ID.
    pub hold_id: String,
    /// 动作.
    pub action: HoldAuditAction,
    /// 操作者.
    pub actor: String,
    /// 时间（Unix 秒）.
    pub timestamp: u64,
    /// 备注.
    pub note: String,
}

impl HoldAuditEntry {
    pub fn new(
        hold_id: &str,
        action: HoldAuditAction,
        actor: &str,
        timestamp: u64,
        note: &str,
    ) -> Self {
        Self {
            hold_id: String::from(hold_id),
            action,
            actor: String::from(actor),
            timestamp,
            note: String::from(note),
        }
    }
}

/// 法定保留管理器.
///
/// 汇总保留项与审计记录，计算合规结论。结论**由证据推导**。
pub struct LegalHoldManager {
    holds: Vec<HoldRecord>,
    audit: Vec<HoldAuditEntry>,
}

impl LegalHoldManager {
    pub fn new(holds: Vec<HoldRecord>, audit: Vec<HoldAuditEntry>) -> Self {
        Self { holds, audit }
    }

    /// 添加保留项.
    pub fn add_hold(&mut self, hold: HoldRecord) {
        self.holds.push(hold);
    }

    /// 添加审计条目.
    pub fn add_audit(&mut self, entry: HoldAuditEntry) {
        self.audit.push(entry);
    }

    /// 缺失 Created 审计的保留项.
    pub fn holds_without_create_audit(&self) -> Vec<&String> {
        self.holds
            .iter()
            .filter(|h| {
                !self
                    .audit
                    .iter()
                    .any(|a| a.hold_id == h.hold_id && a.action == HoldAuditAction::Created)
            })
            .map(|h| &h.hold_id)
            .collect()
    }

    /// 缺失 Released 审计的已释放保留项.
    pub fn released_without_release_audit(&self) -> Vec<&String> {
        self.holds
            .iter()
            .filter(|h| {
                h.state == HoldState::Released
                    && !self
                        .audit
                        .iter()
                        .any(|a| a.hold_id == h.hold_id && a.action == HoldAuditAction::Released)
            })
            .map(|h| &h.hold_id)
            .collect()
    }

    /// 缺失授权的保留项.
    pub fn holds_without_authorization(&self) -> Vec<&String> {
        self.holds
            .iter()
            .filter(|h| !h.has_valid_authorization())
            .map(|h| &h.hold_id)
            .collect()
    }

    /// 已过期但仍标记 Active 的保留项（未及时处置）.
    pub fn lapsed_holds(&self, now: u64) -> Vec<&String> {
        self.holds
            .iter()
            .filter(|h| h.is_lapsed(now))
            .map(|h| &h.hold_id)
            .collect()
    }

    /// 合规结论（由证据推导）.
    ///
    /// 条件：
    /// 1. 非空（至少有保留项）
    /// 2. 所有保留项有有效授权
    /// 3. 所有保留项有 Created 审计
    /// 4. 所有 Released 保留项有 Released 审计
    /// 5. 无过期未处置的保留项
    pub fn is_compliant(&self, now: u64) -> bool {
        if self.holds.is_empty() {
            return false;
        }
        self.holds_without_authorization().is_empty()
            && self.holds_without_create_audit().is_empty()
            && self.released_without_release_audit().is_empty()
            && self.lapsed_holds(now).is_empty()
    }
}

/// 兼容别名.
pub type LegalHold = LegalHoldManager;

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_hold(id: &str, data: &str, auth: &str, expires: u64) -> HoldRecord {
        HoldRecord::new(
            id,
            data,
            "litigation-2026",
            HoldPermission::Legal,
            auth,
            1_700_000_000,
            expires,
        )
    }

    #[test]
    fn t1_compliant_with_full_evidence() {
        let h1 = mk_hold("h1", "d1", "legal-lead", 0);
        let h2 = HoldRecord::new(
            "h2",
            "d2",
            "audit-investigation",
            HoldPermission::ComplianceOfficer,
            "compliance",
            1_700_000_000,
            1_700_010_000,
        );
        let audit = vec![
            HoldAuditEntry::new(
                "h1",
                HoldAuditAction::Created,
                "legal-lead",
                1_700_000_000,
                "",
            ),
            HoldAuditEntry::new(
                "h2",
                HoldAuditAction::Created,
                "compliance",
                1_700_000_000,
                "",
            ),
        ];
        let mgr = LegalHoldManager::new(vec![h1, h2], audit);
        assert!(mgr.is_compliant(1_700_005_000));
    }

    #[test]
    fn t2_fails_without_authorization() {
        let h = mk_hold("h1", "d1", "", 0); // 空 authorized_by
        let audit = vec![HoldAuditEntry::new(
            "h1",
            HoldAuditAction::Created,
            "x",
            1_700_000_000,
            "",
        )];
        let mgr = LegalHoldManager::new(vec![h], audit);
        assert!(!mgr.is_compliant(1_700_000_000));
    }

    #[test]
    fn t3_fails_without_create_audit() {
        let h = mk_hold("h1", "d1", "legal-lead", 0);
        let mgr = LegalHoldManager::new(vec![h], Vec::new());
        assert!(!mgr.is_compliant(1_700_000_000));
        assert_eq!(mgr.holds_without_create_audit().len(), 1);
    }

    #[test]
    fn t4_released_needs_release_audit() {
        let mut h = mk_hold("h1", "d1", "legal-lead", 0);
        h.state = HoldState::Released;
        let audit = vec![HoldAuditEntry::new(
            "h1",
            HoldAuditAction::Created,
            "legal-lead",
            1_700_000_000,
            "",
        )];
        let mgr = LegalHoldManager::new(vec![h], audit);
        assert!(!mgr.is_compliant(1_700_000_000));
        assert_eq!(mgr.released_without_release_audit().len(), 1);
    }

    #[test]
    fn t5_released_with_release_audit_compliant() {
        let mut h = mk_hold("h1", "d1", "legal-lead", 0);
        h.state = HoldState::Released;
        let audit = vec![
            HoldAuditEntry::new(
                "h1",
                HoldAuditAction::Created,
                "legal-lead",
                1_700_000_000,
                "",
            ),
            HoldAuditEntry::new(
                "h1",
                HoldAuditAction::Released,
                "legal-lead",
                1_700_001_000,
                "",
            ),
        ];
        let mgr = LegalHoldManager::new(vec![h], audit);
        assert!(mgr.is_compliant(1_700_002_000));
    }

    #[test]
    fn t6_lapsed_hold_blocks_compliance() {
        let h = mk_hold("h1", "d1", "legal-lead", 1_700_001_000);
        let audit = vec![HoldAuditEntry::new(
            "h1",
            HoldAuditAction::Created,
            "legal-lead",
            1_700_000_000,
            "",
        )];
        let mgr = LegalHoldManager::new(vec![h], audit);
        // 过期时间已过
        assert!(!mgr.is_compliant(1_700_002_000));
        assert_eq!(mgr.lapsed_holds(1_700_002_000).len(), 1);
    }

    #[test]
    fn t7_empty_manager_fails() {
        let mgr = LegalHoldManager::new(Vec::new(), Vec::new());
        assert!(!mgr.is_compliant(1_700_000_000));
    }

    #[test]
    fn t8_deletion_denied_audit_recorded() {
        let h = mk_hold("h1", "d1", "legal-lead", 0);
        let audit = vec![
            HoldAuditEntry::new(
                "h1",
                HoldAuditAction::Created,
                "legal-lead",
                1_700_000_000,
                "",
            ),
            HoldAuditEntry::new(
                "h1",
                HoldAuditAction::DeletionDenied,
                "system",
                1_700_000_500,
                "deletion blocked by hold",
            ),
        ];
        let mgr = LegalHoldManager::new(vec![h], audit);
        assert!(mgr.is_compliant(1_700_001_000));
    }

    #[test]
    fn t9_court_order_permission() {
        let h = HoldRecord::new(
            "h1",
            "d1",
            "court-order-2026-001",
            HoldPermission::CourtOrder,
            "judge-smith",
            1_700_000_000,
            0,
        );
        let audit = vec![HoldAuditEntry::new(
            "h1",
            HoldAuditAction::Created,
            "judge-smith",
            1_700_000_000,
            "",
        )];
        let mgr = LegalHoldManager::new(vec![h], audit);
        assert!(mgr.is_compliant(1_700_000_000));
    }

    #[test]
    fn t10_extend_updates_expiry() {
        let mut h = mk_hold("h1", "d1", "legal-lead", 1_700_001_000);
        h.expires_at = 1_700_020_000; // 延期
        let audit = vec![
            HoldAuditEntry::new(
                "h1",
                HoldAuditAction::Created,
                "legal-lead",
                1_700_000_000,
                "",
            ),
            HoldAuditEntry::new(
                "h1",
                HoldAuditAction::Extended,
                "legal-lead",
                1_700_000_500,
                "",
            ),
        ];
        let mgr = LegalHoldManager::new(vec![h], audit);
        assert!(mgr.is_compliant(1_700_010_000));
    }
}
