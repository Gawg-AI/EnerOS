//! 撤回通道与产物不可变性 (REL-015~016).
//!
//! REL-015: rollback/withdraw channel — 撤回演练
//! REL-016: artifact repository immutability — digest/signature retention

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// 撤回状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawStatus {
    /// 活跃（未撤回）。
    Active,
    /// 撤回中（通知传播）。
    Withdrawing,
    /// 已撤回。
    Withdrawn,
    /// 撤回失败。
    Failed,
}

impl WithdrawStatus {
    pub fn label(self) -> &'static str {
        match self {
            WithdrawStatus::Active => "ACTIVE",
            WithdrawStatus::Withdrawing => "WITHDRAWING",
            WithdrawStatus::Withdrawn => "WITHDRAWN",
            WithdrawStatus::Failed => "FAILED",
        }
    }
}

/// 撤回通道。
pub struct WithdrawChannel {
    /// 版本号。
    version: String,
    /// 当前状态。
    status: WithdrawStatus,
    /// 撤回原因。
    reason: Option<String>,
    /// 撤回时间。
    withdrawn_at: Option<u64>,
    /// 已通知的设备数。
    notified_devices: u32,
    /// 已确认回滚的设备数。
    confirmed_rolled_back: u32,
}

impl WithdrawChannel {
    pub fn new(version: &str) -> Self {
        Self {
            version: String::from(version),
            status: WithdrawStatus::Active,
            reason: None,
            withdrawn_at: None,
            notified_devices: 0,
            confirmed_rolled_back: 0,
        }
    }

    /// 发起撤回。
    pub fn withdraw(&mut self, reason: &str, timestamp: u64) -> Result<(), &'static str> {
        if self.status != WithdrawStatus::Active {
            return Err("version not active");
        }
        if reason.is_empty() {
            return Err("reason is empty");
        }
        self.reason = Some(String::from(reason));
        self.withdrawn_at = Some(timestamp);
        self.status = WithdrawStatus::Withdrawing;
        Ok(())
    }

    /// 记录设备通知。
    pub fn notify_device(&mut self) {
        self.notified_devices += 1;
    }

    /// 记录设备确认回滚。
    pub fn confirm_rollback(&mut self) {
        self.confirmed_rolled_back += 1;
    }

    /// 完成撤回（所有通知的设备已确认）。
    pub fn complete(&mut self) -> Result<(), &'static str> {
        if self.status != WithdrawStatus::Withdrawing {
            return Err("not in withdrawing state");
        }
        if self.notified_devices == 0 {
            return Err("no devices notified");
        }
        if self.confirmed_rolled_back < self.notified_devices {
            return Err("not all devices confirmed rollback");
        }
        self.status = WithdrawStatus::Withdrawn;
        Ok(())
    }

    pub fn status(&self) -> WithdrawStatus {
        self.status
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// 不可变产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmutableArtifact {
    /// 产物名称。
    pub name: String,
    /// 版本。
    pub version: String,
    /// SHA-256 摘要。
    pub sha256: String,
    /// 签名（SM2）。
    pub signature: String,
    /// 上传时间。
    pub uploaded_at: u64,
    /// 是否已锁定（不可删除/覆盖）。
    pub locked: bool,
}

impl ImmutableArtifact {
    pub fn new(name: &str, version: &str, sha256: &str, signature: &str, uploaded_at: u64) -> Self {
        Self {
            name: String::from(name),
            version: String::from(version),
            sha256: String::from(sha256),
            signature: String::from(signature),
            uploaded_at,
            locked: false,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.is_empty() {
            return Err("name is empty");
        }
        if self.sha256.len() != 64 {
            return Err("sha256 must be 64 hex chars");
        }
        if self.signature.is_empty() {
            return Err("signature is empty");
        }
        Ok(())
    }
}

/// 产物仓库不可变性管理器。
pub struct ArtifactImmutability {
    artifacts: Vec<ImmutableArtifact>,
}

impl Default for ArtifactImmutability {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtifactImmutability {
    pub fn new() -> Self {
        Self { artifacts: Vec::new() }
    }

    /// 上传产物（新条目）。
    pub fn upload(&mut self, artifact: ImmutableArtifact) -> Result<(), &'static str> {
        artifact.validate()?;
        // 检查是否已存在（同名同版本不可重复上传）
        if self.artifacts.iter().any(|a| a.name == artifact.name && a.version == artifact.version) {
            return Err("artifact already exists (immutable)");
        }
        // 检查 digest 冲突（同名不同版本但相同 digest = 异常）
        if self.artifacts.iter().any(|a| a.name == artifact.name && a.sha256 == artifact.sha256) {
            return Err("digest collision: same content, different version");
        }
        self.artifacts.push(artifact);
        Ok(())
    }

    /// 锁定产物（永久不可删除）。
    pub fn lock(&mut self, name: &str, version: &str) -> Result<(), &'static str> {
        let artifact = self
            .artifacts
            .iter_mut()
            .find(|a| a.name == name && a.version == version)
            .ok_or("artifact not found")?;
        artifact.locked = true;
        Ok(())
    }

    /// 尝试删除产物（锁定产物不可删除）。
    pub fn delete(&mut self, name: &str, version: &str) -> Result<(), &'static str> {
        let artifact = self
            .artifacts
            .iter()
            .find(|a| a.name == name && a.version == version)
            .ok_or("artifact not found")?;
        if artifact.locked {
            return Err("artifact is locked (immutable)");
        }
        self.artifacts.retain(|a| !(a.name == name && a.version == version));
        Ok(())
    }

    /// 查找产物。
    pub fn find(&self, name: &str, version: &str) -> Option<&ImmutableArtifact> {
        self.artifacts.iter().find(|a| a.name == name && a.version == version)
    }

    /// 产物数。
    pub fn count(&self) -> usize {
        self.artifacts.len()
    }

    /// 验证所有产物签名和 digest 保留完整。
    pub fn verify_all(&self) -> bool {
        self.artifacts.iter().all(|a| a.validate().is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_artifact(name: &str, version: &str) -> ImmutableArtifact {
        ImmutableArtifact::new(
            name,
            version,
            "a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890",
            "sm2-sig-abc",
            1000,
        )
    }

    #[test]
    fn withdraw_full_lifecycle() {
        let mut channel = WithdrawChannel::new("0.200.0");
        channel.withdraw("critical security issue", 1000).unwrap();
        assert_eq!(channel.status(), WithdrawStatus::Withdrawing);

        // Notify 3 devices
        for _ in 0..3 {
            channel.notify_device();
            channel.confirm_rollback();
        }

        channel.complete().unwrap();
        assert_eq!(channel.status(), WithdrawStatus::Withdrawn);
    }

    #[test]
    fn withdraw_not_active_rejected() {
        let mut channel = WithdrawChannel::new("0.200.0");
        channel.withdraw("test", 1000).unwrap();
        let result = channel.withdraw("again", 1001);
        assert!(result.is_err());
    }

    #[test]
    fn withdraw_empty_reason_rejected() {
        let mut channel = WithdrawChannel::new("0.200.0");
        assert!(channel.withdraw("", 1000).is_err());
    }

    #[test]
    fn withdraw_complete_without_notifications_fails() {
        let mut channel = WithdrawChannel::new("0.200.0");
        channel.withdraw("test", 1000).unwrap();
        assert!(channel.complete().is_err());
    }

    #[test]
    fn withdraw_partial_confirm_fails() {
        let mut channel = WithdrawChannel::new("0.200.0");
        channel.withdraw("test", 1000).unwrap();
        channel.notify_device();
        channel.notify_device();
        channel.confirm_rollback(); // only 1 of 2
        assert!(channel.complete().is_err());
    }

    #[test]
    fn artifact_upload_and_find() {
        let mut repo = ArtifactImmutability::new();
        repo.upload(sample_artifact("eneros-kernel", "0.200.0")).unwrap();
        assert_eq!(repo.count(), 1);
        assert!(repo.find("eneros-kernel", "0.200.0").is_some());
    }

    #[test]
    fn duplicate_upload_rejected() {
        let mut repo = ArtifactImmutability::new();
        repo.upload(sample_artifact("eneros-kernel", "0.200.0")).unwrap();
        let result = repo.upload(sample_artifact("eneros-kernel", "0.200.0"));
        assert!(result.is_err());
    }

    #[test]
    fn digest_collision_rejected() {
        let mut repo = ArtifactImmutability::new();
        repo.upload(sample_artifact("eneros-kernel", "0.200.0")).unwrap();
        // Same name, same digest, different version
        let result = repo.upload(sample_artifact("eneros-kernel", "0.201.0"));
        assert!(result.is_err());
    }

    #[test]
    fn locked_artifact_cannot_delete() {
        let mut repo = ArtifactImmutability::new();
        repo.upload(sample_artifact("eneros-kernel", "0.200.0")).unwrap();
        repo.lock("eneros-kernel", "0.200.0").unwrap();
        let result = repo.delete("eneros-kernel", "0.200.0");
        assert!(result.is_err());
    }

    #[test]
    fn unlocked_artifact_can_delete() {
        let mut repo = ArtifactImmutability::new();
        repo.upload(sample_artifact("eneros-kernel", "0.200.0")).unwrap();
        repo.delete("eneros-kernel", "0.200.0").unwrap();
        assert_eq!(repo.count(), 0);
    }

    #[test]
    fn verify_all_artifacts() {
        let mut repo = ArtifactImmutability::new();
        repo.upload(sample_artifact("eneros-kernel", "0.200.0")).unwrap();
        repo.upload(sample_artifact("eneros-agent", "0.200.0")).unwrap();
        assert!(repo.verify_all());
    }
}
