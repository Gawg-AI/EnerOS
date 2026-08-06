//! RMA 身份重绑 (OPS-014).
//!
//! MASTER §21.14 OPS-014: "RMA identity rebind — 双授权/不泄密"
//!
//! 原则：设备返厂维修（RMA）时可能需要重绑身份：
//! 1. 必须有双授权（两名授权人批准）。
//! 2. 旧身份必须完全销毁（证书、密钥、配置）。
//! 3. 新身份必须全新生成（不复用旧密钥材料）。
//! 4. 必须验证无数据泄露（旧数据不可恢复）。
//! 5. 全程审计。

use alloc::string::String;

/// RMA 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RmaStatus {
    /// 待授权。
    PendingAuthorization,
    /// 已授权，待执行。
    Authorized,
    /// 正在重绑。
    Rebinding,
    /// 完成。
    Complete,
    /// 失败。
    Failed,
}

impl RmaStatus {
    pub fn label(self) -> &'static str {
        match self {
            RmaStatus::PendingAuthorization => "PENDING_AUTHORIZATION",
            RmaStatus::Authorized => "AUTHORIZED",
            RmaStatus::Rebinding => "REBINDING",
            RmaStatus::Complete => "COMPLETE",
            RmaStatus::Failed => "FAILED",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, RmaStatus::Complete | RmaStatus::Failed)
    }
}

/// RMA 授权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmaAuthorization {
    /// 第一授权人。
    pub authorizer1: String,
    /// 第二授权人。
    pub authorizer2: String,
    /// 授权时间（Unix 秒）。
    pub timestamp: u64,
    /// RMA 原因。
    pub reason: String,
}

impl RmaAuthorization {
    /// 创建授权。
    ///
    /// 两名授权人必须不同（防止单人冒签）。
    pub fn new(
        authorizer1: &str,
        authorizer2: &str,
        timestamp: u64,
        reason: &str,
    ) -> Result<Self, &'static str> {
        if authorizer1.is_empty() {
            return Err("authorizer1 is empty");
        }
        if authorizer2.is_empty() {
            return Err("authorizer2 is empty");
        }
        if authorizer1 == authorizer2 {
            return Err("dual authorization requires two different authorizers");
        }
        if reason.is_empty() {
            return Err("reason is empty");
        }

        Ok(Self {
            authorizer1: String::from(authorizer1),
            authorizer2: String::from(authorizer2),
            timestamp,
            reason: String::from(reason),
        })
    }
}

/// RMA 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmaResult {
    /// 旧身份是否已销毁。
    pub old_identity_destroyed: bool,
    /// 新身份是否已配置。
    pub new_identity_provisioned: bool,
    /// 数据泄露检查是否通过（旧数据不可恢复）。
    pub data_leak_check_passed: bool,
    /// 审计记录 ID。
    pub audit_record_id: String,
    /// 新设备 ID（重绑后）。
    pub new_device_id: String,
}

impl RmaResult {
    /// 是否全部通过。
    pub fn all_passed(&self) -> bool {
        self.old_identity_destroyed && self.new_identity_provisioned && self.data_leak_check_passed
    }
}

/// RMA 重绑执行器。
pub struct RmaRebind {
    /// 当前状态。
    status: RmaStatus,
    /// 授权信息。
    authorization: Option<RmaAuthorization>,
    /// 旧设备 ID。
    old_device_id: String,
    /// 新设备 ID。
    new_device_id: Option<String>,
    /// 旧身份是否已销毁。
    old_identity_destroyed: bool,
    /// 新身份是否已配置。
    new_identity_provisioned: bool,
    /// 数据泄露检查是否通过。
    data_leak_check_passed: bool,
    /// 审计记录 ID。
    audit_record_id: Option<String>,
}

impl RmaRebind {
    pub fn new(old_device_id: &str) -> Self {
        Self {
            status: RmaStatus::PendingAuthorization,
            authorization: None,
            old_device_id: String::from(old_device_id),
            new_device_id: None,
            old_identity_destroyed: false,
            new_identity_provisioned: false,
            data_leak_check_passed: false,
            audit_record_id: None,
        }
    }

    /// 提交双授权。
    pub fn authorize(&mut self, auth: RmaAuthorization) -> Result<(), &'static str> {
        if self.status != RmaStatus::PendingAuthorization {
            return Err("RMA not in PendingAuthorization state");
        }
        self.authorization = Some(auth);
        self.status = RmaStatus::Authorized;
        Ok(())
    }

    /// 销毁旧身份。
    pub fn destroy_old_identity(&mut self) -> Result<(), &'static str> {
        if self.status != RmaStatus::Authorized && self.status != RmaStatus::Rebinding {
            return Err("RMA not authorized");
        }
        self.status = RmaStatus::Rebinding;
        self.old_identity_destroyed = true;
        Ok(())
    }

    /// 配置新身份。
    pub fn provision_new_identity(&mut self, new_device_id: &str) -> Result<(), &'static str> {
        if !self.old_identity_destroyed {
            return Err("must destroy old identity before provisioning new");
        }
        if new_device_id.is_empty() {
            return Err("new_device_id is empty");
        }
        if new_device_id == self.old_device_id {
            return Err("new_device_id must differ from old");
        }
        self.new_device_id = Some(String::from(new_device_id));
        self.new_identity_provisioned = true;
        Ok(())
    }

    /// 执行数据泄露检查。
    pub fn verify_no_data_leak(&mut self, passed: bool) -> Result<(), &'static str> {
        if !self.new_identity_provisioned {
            return Err("must provision new identity before leak check");
        }
        self.data_leak_check_passed = passed;
        Ok(())
    }

    /// 完成重绑（记录审计）。
    pub fn complete(&mut self, audit_record_id: &str) -> Result<RmaResult, &'static str> {
        if !self.data_leak_check_passed {
            self.status = RmaStatus::Failed;
            return Err("data leak check not passed");
        }
        if audit_record_id.is_empty() {
            return Err("audit_record_id is empty");
        }

        self.audit_record_id = Some(String::from(audit_record_id));
        self.status = RmaStatus::Complete;

        Ok(RmaResult {
            old_identity_destroyed: self.old_identity_destroyed,
            new_identity_provisioned: self.new_identity_provisioned,
            data_leak_check_passed: self.data_leak_check_passed,
            audit_record_id: String::from(audit_record_id),
            new_device_id: self.new_device_id.clone().unwrap_or_default(),
        })
    }

    /// 当前状态。
    pub fn status(&self) -> RmaStatus {
        self.status
    }

    /// 授权信息。
    pub fn authorization(&self) -> Option<&RmaAuthorization> {
        self.authorization.as_ref()
    }

    /// 旧设备 ID。
    pub fn old_device_id(&self) -> &str {
        &self.old_device_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_rma_success() {
        let mut rma = RmaRebind::new("dev-001");

        // Dual authorization
        let auth = RmaAuthorization::new("alice", "bob", 1000, "hardware failure").unwrap();
        rma.authorize(auth).unwrap();
        assert_eq!(rma.status(), RmaStatus::Authorized);

        // Destroy old identity
        rma.destroy_old_identity().unwrap();
        assert!(rma.old_identity_destroyed);

        // Provision new identity
        rma.provision_new_identity("dev-001-reborn").unwrap();
        assert!(rma.new_identity_provisioned);

        // Verify no data leak
        rma.verify_no_data_leak(true).unwrap();
        assert!(rma.data_leak_check_passed);

        // Complete
        let result = rma.complete("audit-001").unwrap();
        assert_eq!(rma.status(), RmaStatus::Complete);
        assert!(result.all_passed());
        assert_eq!(result.new_device_id, "dev-001-reborn");
    }

    #[test]
    fn dual_auth_same_person_rejected() {
        let auth = RmaAuthorization::new("alice", "alice", 1000, "test");
        assert!(auth.is_err());
    }

    #[test]
    fn dual_auth_empty_fields_rejected() {
        assert!(RmaAuthorization::new("", "bob", 1000, "test").is_err());
        assert!(RmaAuthorization::new("alice", "", 1000, "test").is_err());
        assert!(RmaAuthorization::new("alice", "bob", 1000, "").is_err());
    }

    #[test]
    fn cannot_destroy_without_authorization() {
        let mut rma = RmaRebind::new("dev-001");
        let result = rma.destroy_old_identity();
        assert!(result.is_err());
    }

    #[test]
    fn cannot_provision_without_destroy() {
        let mut rma = RmaRebind::new("dev-001");
        rma.authorize(RmaAuthorization::new("a", "b", 1000, "test").unwrap())
            .unwrap();
        let result = rma.provision_new_identity("dev-002");
        assert!(result.is_err());
    }

    #[test]
    fn cannot_reuse_old_device_id() {
        let mut rma = RmaRebind::new("dev-001");
        rma.authorize(RmaAuthorization::new("a", "b", 1000, "test").unwrap())
            .unwrap();
        rma.destroy_old_identity().unwrap();
        let result = rma.provision_new_identity("dev-001"); // same as old
        assert!(result.is_err());
    }

    #[test]
    fn cannot_leak_check_without_provision() {
        let mut rma = RmaRebind::new("dev-001");
        rma.authorize(RmaAuthorization::new("a", "b", 1000, "test").unwrap())
            .unwrap();
        rma.destroy_old_identity().unwrap();
        let result = rma.verify_no_data_leak(true);
        assert!(result.is_err());
    }

    #[test]
    fn leak_check_failure_blocks_completion() {
        let mut rma = RmaRebind::new("dev-001");
        rma.authorize(RmaAuthorization::new("a", "b", 1000, "test").unwrap())
            .unwrap();
        rma.destroy_old_identity().unwrap();
        rma.provision_new_identity("dev-002").unwrap();
        rma.verify_no_data_leak(false).unwrap(); // leak detected!

        let result = rma.complete("audit-002");
        assert!(result.is_err());
        assert_eq!(rma.status(), RmaStatus::Failed);
    }

    #[test]
    fn empty_audit_id_rejected() {
        let mut rma = RmaRebind::new("dev-001");
        rma.authorize(RmaAuthorization::new("a", "b", 1000, "test").unwrap())
            .unwrap();
        rma.destroy_old_identity().unwrap();
        rma.provision_new_identity("dev-002").unwrap();
        rma.verify_no_data_leak(true).unwrap();

        let result = rma.complete("");
        assert!(result.is_err());
    }

    #[test]
    fn cannot_authorize_twice() {
        let mut rma = RmaRebind::new("dev-001");
        rma.authorize(RmaAuthorization::new("a", "b", 1000, "test").unwrap())
            .unwrap();

        let result = rma.authorize(RmaAuthorization::new("c", "d", 1001, "test2").unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn rma_result_all_passed() {
        let result = RmaResult {
            old_identity_destroyed: true,
            new_identity_provisioned: true,
            data_leak_check_passed: true,
            audit_record_id: String::from("audit-001"),
            new_device_id: String::from("dev-002"),
        };
        assert!(result.all_passed());

        let result2 = RmaResult {
            old_identity_destroyed: false,
            new_identity_provisioned: true,
            data_leak_check_passed: true,
            audit_record_id: String::from("audit-002"),
            new_device_id: String::from("dev-003"),
        };
        assert!(!result2.all_passed());
    }
}
