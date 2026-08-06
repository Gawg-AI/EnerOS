//! 远程吊销与离线重连传播 (OPS-007).
//!
//! MASTER §21.14 OPS-007: "remote package/cert revoke — 离线后重连生效"
//!
//! 原则：吊销命令必须对离线设备生效。设备离线时吊销进入 pending 队列，
//! 设备重连后立即传播并执行吊销。绝不允许离线设备因"错过通知"而继续使用
//! 已吊销的证书/包/Agent/模型。
//!
//! 吊销是即时的：运行中的 Agent 访问权限立即撤销，旧 lease 不可恢复
//!（project_memory 硬约束：Capability revocation must immediately revoke
//! access for running Agents; old leases cannot be recovered）。

use alloc::string::String;
use alloc::vec::Vec;

/// 吊销目标类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RevokeTarget {
    /// 证书（按序列号或指纹）。
    Certificate,
    /// 软件包（按包名+版本）。
    Package,
    /// Agent 二进制（按 Agent ID）。
    Agent,
    /// AI 模型（按模型指纹）。
    Model,
    /// 配置版本。
    Config,
}

impl RevokeTarget {
    pub fn label(self) -> &'static str {
        match self {
            RevokeTarget::Certificate => "CERTIFICATE",
            RevokeTarget::Package => "PACKAGE",
            RevokeTarget::Agent => "AGENT",
            RevokeTarget::Model => "MODEL",
            RevokeTarget::Config => "CONFIG",
        }
    }
}

/// 吊销状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeStatus {
    /// 已入队，等待设备重连。
    Pending,
    /// 已传播到目标设备。
    Propagated,
    /// 已确认执行（设备已应用吊销）。
    Confirmed,
    /// 传播失败（设备超时未确认）。
    Failed,
}

impl RevokeStatus {
    pub fn label(self) -> &'static str {
        match self {
            RevokeStatus::Pending => "PENDING",
            RevokeStatus::Propagated => "PROPAGATED",
            RevokeStatus::Confirmed => "CONFIRMED",
            RevokeStatus::Failed => "FAILED",
        }
    }

    /// 是否处于终态。
    pub fn is_terminal(self) -> bool {
        matches!(self, RevokeStatus::Confirmed | RevokeStatus::Failed)
    }
}

/// 吊销记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationRecord {
    /// 吊销 ID（唯一）。
    pub revoke_id: String,
    /// 吊销目标类型。
    pub target_type: RevokeTarget,
    /// 目标标识（证书序列号 / 包名+版本 / Agent ID / 模型指纹）。
    pub target_id: String,
    /// 吊销原因。
    pub reason: String,
    /// 吊销发起时间（Unix 秒）。
    pub issued_at: u64,
    /// 吊销生效时间（Unix 秒）。
    pub effective_at: u64,
    /// 发起人。
    pub issued_by: String,
    /// 目标设备列表（空 = 全 fleet）。
    pub target_devices: Vec<String>,
    /// 当前状态（按设备 ID 索引）。
    pub device_status: Vec<(String, RevokeStatus)>,
}

impl RevocationRecord {
    /// 创建吊销记录。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        revoke_id: &str,
        target_type: RevokeTarget,
        target_id: &str,
        reason: &str,
        issued_at: u64,
        effective_at: u64,
        issued_by: &str,
        target_devices: Vec<String>,
    ) -> Result<Self, &'static str> {
        if revoke_id.is_empty() {
            return Err("revoke_id is empty");
        }
        if target_id.is_empty() {
            return Err("target_id is empty");
        }
        if reason.is_empty() {
            return Err("reason is empty");
        }
        if issued_by.is_empty() {
            return Err("issued_by is empty");
        }
        if effective_at < issued_at {
            return Err("effective_at must be >= issued_at");
        }

        let device_status = target_devices
            .iter()
            .map(|d| (d.clone(), RevokeStatus::Pending))
            .collect();

        Ok(Self {
            revoke_id: String::from(revoke_id),
            target_type,
            target_id: String::from(target_id),
            reason: String::from(reason),
            issued_at,
            effective_at,
            issued_by: String::from(issued_by),
            target_devices,
            device_status,
        })
    }

    /// 是否针对全 fleet（无特定设备）。
    pub fn is_fleet_wide(&self) -> bool {
        self.target_devices.is_empty()
    }

    /// 返回待传播的设备列表。
    pub fn pending_devices(&self) -> Vec<&str> {
        self.device_status
            .iter()
            .filter(|(_, s)| *s == RevokeStatus::Pending)
            .map(|(d, _)| d.as_str())
            .collect()
    }

    /// 返回已确认的设备数。
    pub fn confirmed_count(&self) -> usize {
        self.device_status
            .iter()
            .filter(|(_, s)| *s == RevokeStatus::Confirmed)
            .count()
    }

    /// 是否全部设备已确认。
    pub fn all_confirmed(&self) -> bool {
        !self.device_status.is_empty()
            && self
                .device_status
                .iter()
                .all(|(_, s)| *s == RevokeStatus::Confirmed)
    }
}

/// 吊销管理器。
pub struct RevokeManager {
    /// 已签发的吊销记录。
    records: Vec<RevocationRecord>,
    /// 已吊销目标的全局集合（即时查询）。
    revoked_set: Vec<(RevokeTarget, String)>,
}

impl Default for RevokeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RevokeManager {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            revoked_set: Vec::new(),
        }
    }

    /// 签发吊销命令。
    ///
    /// 吊销立即生效（加入全局 revoked_set），但传播到离线设备需等待重连。
    pub fn issue(&mut self, record: RevocationRecord) -> Result<&RevocationRecord, &'static str> {
        // 检查重复
        if self.records.iter().any(|r| r.revoke_id == record.revoke_id) {
            return Err("revoke_id already exists");
        }

        // 立即加入全局吊销集合（运行中 Agent 立即失去访问权）
        let key = (record.target_type, record.target_id.clone());
        if !self.revoked_set.iter().any(|k| k == &key) {
            self.revoked_set.push(key);
        }

        self.records.push(record);
        Ok(self.records.last().unwrap())
    }

    /// 检查目标是否已被吊销。
    ///
    /// **即时查询**：不依赖设备状态。即使设备离线未确认，吊销已全局生效。
    pub fn is_revoked(&self, target_type: RevokeTarget, target_id: &str) -> bool {
        self.revoked_set
            .iter()
            .any(|(t, id)| *t == target_type && id == target_id)
    }

    /// 设备重连时传播 pending 吊销。
    ///
    /// 返回需要传播到该设备的吊销记录 ID 列表。
    pub fn propagate_on_reconnect(&mut self, device_id: &str) -> Vec<String> {
        let mut propagated = Vec::new();
        for record in &mut self.records {
            // 全 fleet 或指定设备
            let applies =
                record.is_fleet_wide() || record.target_devices.iter().any(|d| d == device_id);

            if !applies {
                continue;
            }

            // 若设备在记录中且为 Pending，标记为 Propagated
            if let Some(entry) = record
                .device_status
                .iter_mut()
                .find(|(d, _)| d == device_id)
            {
                if entry.1 == RevokeStatus::Pending {
                    entry.1 = RevokeStatus::Propagated;
                    propagated.push(record.revoke_id.clone());
                }
            } else if record.is_fleet_wide() {
                // 全 fleet 吊销：设备首次出现，加入并标记 Propagated
                record
                    .device_status
                    .push((String::from(device_id), RevokeStatus::Propagated));
                propagated.push(record.revoke_id.clone());
            }
        }
        propagated
    }

    /// 设备确认已执行吊销。
    pub fn confirm(&mut self, device_id: &str, revoke_id: &str) -> Result<(), &'static str> {
        let record = self
            .records
            .iter_mut()
            .find(|r| r.revoke_id == revoke_id)
            .ok_or("revoke_id not found")?;

        let entry = record
            .device_status
            .iter_mut()
            .find(|(d, _)| d == device_id)
            .ok_or("device not in target list")?;

        if entry.1 != RevokeStatus::Propagated {
            return Err("device status is not Propagated");
        }
        entry.1 = RevokeStatus::Confirmed;
        Ok(())
    }

    /// 标记传播失败（设备超时未确认）。
    pub fn mark_failed(&mut self, device_id: &str, revoke_id: &str) -> Result<(), &'static str> {
        let record = self
            .records
            .iter_mut()
            .find(|r| r.revoke_id == revoke_id)
            .ok_or("revoke_id not found")?;

        let entry = record
            .device_status
            .iter_mut()
            .find(|(d, _)| d == device_id)
            .ok_or("device not in target list")?;

        entry.1 = RevokeStatus::Failed;
        Ok(())
    }

    /// 返回 pending 吊销数（等待设备重连）。
    pub fn pending_count(&self) -> usize {
        self.records
            .iter()
            .map(|r| {
                r.device_status
                    .iter()
                    .filter(|(_, s)| *s == RevokeStatus::Pending)
                    .count()
            })
            .sum()
    }

    /// 返回所有吊销记录。
    pub fn records(&self) -> &[RevocationRecord] {
        &self.records
    }

    /// 按 ID 查找吊销记录。
    pub fn find(&self, revoke_id: &str) -> Option<&RevocationRecord> {
        self.records.iter().find(|r| r.revoke_id == revoke_id)
    }

    /// 返回已吊销目标数。
    pub fn revoked_target_count(&self) -> usize {
        self.revoked_set.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(devices: &[&str]) -> RevocationRecord {
        RevocationRecord::new(
            "rev-001",
            RevokeTarget::Certificate,
            "cert-sn-12345",
            "key compromise",
            1000,
            1000,
            "security-officer",
            devices.iter().map(|s| String::from(*s)).collect(),
        )
        .unwrap()
    }

    #[test]
    fn issue_immediately_marks_revoked() {
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001", "dev-002"]);
        mgr.issue(record).unwrap();

        // Even though devices haven't confirmed, target is globally revoked
        assert!(mgr.is_revoked(RevokeTarget::Certificate, "cert-sn-12345"));
        assert!(!mgr.is_revoked(RevokeTarget::Certificate, "cert-other"));
    }

    #[test]
    fn duplicate_revoke_id_rejected() {
        let mut mgr = RevokeManager::new();
        let r1 = sample_record(&["dev-001"]);
        mgr.issue(r1).unwrap();

        let r2 = RevocationRecord::new(
            "rev-001", // same ID
            RevokeTarget::Agent,
            "agent-xyz",
            "malicious",
            2000,
            2000,
            "admin",
            Vec::new(),
        )
        .unwrap();
        assert!(mgr.issue(r2).is_err());
    }

    #[test]
    fn offline_device_receives_revoke_on_reconnect() {
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001", "dev-002"]);
        mgr.issue(record).unwrap();

        // dev-001 is offline, dev-002 reconnects first
        let propagated = mgr.propagate_on_reconnect("dev-002");
        assert_eq!(propagated, vec!["rev-001"]);

        // dev-001 reconnects later
        let propagated = mgr.propagate_on_reconnect("dev-001");
        assert_eq!(propagated, vec!["rev-001"]);

        // Both devices should now be Propagated
        let record = mgr.find("rev-001").unwrap();
        let dev1_status = record
            .device_status
            .iter()
            .find(|(d, _)| d == "dev-001")
            .unwrap();
        assert_eq!(dev1_status.1, RevokeStatus::Propagated);
    }

    #[test]
    fn confirm_marks_device_confirmed() {
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001"]);
        mgr.issue(record).unwrap();

        mgr.propagate_on_reconnect("dev-001");
        mgr.confirm("dev-001", "rev-001").unwrap();

        let record = mgr.find("rev-001").unwrap();
        assert_eq!(record.confirmed_count(), 1);
        assert!(record.all_confirmed());
    }

    #[test]
    fn confirm_without_propagation_rejected() {
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001"]);
        mgr.issue(record).unwrap();

        // Try to confirm without propagating first
        let result = mgr.confirm("dev-001", "rev-001");
        assert!(result.is_err());
    }

    #[test]
    fn fleet_wide_revocation() {
        let mut mgr = RevokeManager::new();
        let record = RevocationRecord::new(
            "rev-002",
            RevokeTarget::Model,
            "llm-7b-v4-fingerprint",
            "adversarial vulnerability",
            1000,
            1000,
            "security-team",
            Vec::new(), // fleet-wide
        )
        .unwrap();
        mgr.issue(record).unwrap();

        // Any device reconnecting gets the revocation
        let propagated = mgr.propagate_on_reconnect("dev-100");
        assert_eq!(propagated, vec!["rev-002"]);

        let propagated = mgr.propagate_on_reconnect("dev-200");
        assert_eq!(propagated, vec!["rev-002"]);
    }

    #[test]
    fn mark_failed_on_timeout() {
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001"]);
        mgr.issue(record).unwrap();
        mgr.propagate_on_reconnect("dev-001");

        // Device doesn't confirm in time
        mgr.mark_failed("dev-001", "rev-001").unwrap();

        let record = mgr.find("rev-001").unwrap();
        let status = record
            .device_status
            .iter()
            .find(|(d, _)| d == "dev-001")
            .unwrap();
        assert_eq!(status.1, RevokeStatus::Failed);
    }

    #[test]
    fn pending_count_tracks_offline() {
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001", "dev-002", "dev-003"]);
        mgr.issue(record).unwrap();

        assert_eq!(mgr.pending_count(), 3);

        mgr.propagate_on_reconnect("dev-001");
        assert_eq!(mgr.pending_count(), 2);

        mgr.propagate_on_reconnect("dev-002");
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn revocation_is_immediate_for_running_agents() {
        // project_memory 硬约束：吊销必须立即对运行中 Agent 生效
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001"]);
        mgr.issue(record).unwrap();

        // Even before propagation, is_revoked returns true
        assert!(mgr.is_revoked(RevokeTarget::Certificate, "cert-sn-12345"));

        // Agent checking access is denied immediately
        let agent_cert = "cert-sn-12345";
        assert!(
            mgr.is_revoked(RevokeTarget::Certificate, agent_cert),
            "revoked cert must be immediately visible"
        );
    }

    #[test]
    fn record_validation_rejects_bad() {
        // Empty revoke_id
        let r = RevocationRecord::new(
            "",
            RevokeTarget::Certificate,
            "cert-123",
            "reason",
            1000,
            1000,
            "admin",
            Vec::new(),
        );
        assert!(r.is_err());

        // effective before issued
        let r = RevocationRecord::new(
            "rev-003",
            RevokeTarget::Certificate,
            "cert-123",
            "reason",
            2000,
            1000, // before issued
            "admin",
            Vec::new(),
        );
        assert!(r.is_err());

        // Empty reason
        let r = RevocationRecord::new(
            "rev-004",
            RevokeTarget::Certificate,
            "cert-123",
            "",
            1000,
            1000,
            "admin",
            Vec::new(),
        );
        assert!(r.is_err());
    }

    #[test]
    fn multiple_revocation_types() {
        let mut mgr = RevokeManager::new();

        let _ = mgr.issue(
            RevocationRecord::new(
                "rev-cert",
                RevokeTarget::Certificate,
                "cert-sn-1",
                "compromised",
                1000,
                1000,
                "officer",
                Vec::new(),
            )
            .unwrap(),
        );

        let _ = mgr.issue(
            RevocationRecord::new(
                "rev-agent",
                RevokeTarget::Agent,
                "agent-malicious-1",
                "misbehavior",
                1000,
                1000,
                "officer",
                Vec::new(),
            )
            .unwrap(),
        );

        let _ = mgr.issue(
            RevocationRecord::new(
                "rev-model",
                RevokeTarget::Model,
                "model-fp-abc",
                "adversarial",
                1000,
                1000,
                "officer",
                Vec::new(),
            )
            .unwrap(),
        );

        assert!(mgr.is_revoked(RevokeTarget::Certificate, "cert-sn-1"));
        assert!(mgr.is_revoked(RevokeTarget::Agent, "agent-malicious-1"));
        assert!(mgr.is_revoked(RevokeTarget::Model, "model-fp-abc"));
        assert!(!mgr.is_revoked(RevokeTarget::Package, "some-package"));

        assert_eq!(mgr.revoked_target_count(), 3);
    }

    #[test]
    fn propagate_only_for_targeted_devices() {
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001", "dev-002"]);
        mgr.issue(record).unwrap();

        // dev-003 is not in target list
        let propagated = mgr.propagate_on_reconnect("dev-003");
        assert!(propagated.is_empty());
    }

    #[test]
    fn reconfirm_rejected() {
        let mut mgr = RevokeManager::new();
        let record = sample_record(&["dev-001"]);
        mgr.issue(record).unwrap();
        mgr.propagate_on_reconnect("dev-001");
        mgr.confirm("dev-001", "rev-001").unwrap();

        // Second confirm should fail (already Confirmed)
        let result = mgr.confirm("dev-001", "rev-001");
        assert!(result.is_err());
    }
}
