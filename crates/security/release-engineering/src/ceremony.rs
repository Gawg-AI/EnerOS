//! HSM 离线签名仪式 (REL-010).
//!
//! REL-010: HSM/offline signing ceremony — 演练与恢复

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// HSM 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HsmStatus {
    /// HSM 就绪。
    Ready,
    /// HSM 离线（需要物理接入）。
    Offline,
    /// HSM 故障。
    Faulted,
    /// 密钥已锁定（需要多人解锁）。
    Locked,
}

impl HsmStatus {
    pub fn label(self) -> &'static str {
        match self {
            HsmStatus::Ready => "READY",
            HsmStatus::Offline => "OFFLINE",
            HsmStatus::Faulted => "FAULTED",
            HsmStatus::Locked => "LOCKED",
        }
    }
}

/// 仪式步骤。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeremonyStep {
    /// 仪式发起。
    Initiate,
    /// 第一见证人到场。
    Witness1Present,
    /// 第二见证人到场。
    Witness2Present,
    /// HSM 解锁。
    HsmUnlock,
    /// 密钥导入/生成。
    KeyProvision,
    /// 签名执行。
    Sign,
    /// 签名验证。
    VerifySignature,
    /// 仪式记录归档。
    Archive,
    /// HSM 重新锁定。
    HsmLock,
}

impl CeremonyStep {
    pub fn label(self) -> &'static str {
        match self {
            CeremonyStep::Initiate => "INITIATE",
            CeremonyStep::Witness1Present => "WITNESS1_PRESENT",
            CeremonyStep::Witness2Present => "WITNESS2_PRESENT",
            CeremonyStep::HsmUnlock => "HSM_UNLOCK",
            CeremonyStep::KeyProvision => "KEY_PROVISION",
            CeremonyStep::Sign => "SIGN",
            CeremonyStep::VerifySignature => "VERIFY_SIGNATURE",
            CeremonyStep::Archive => "ARCHIVE",
            CeremonyStep::HsmLock => "HSM_LOCK",
        }
    }

    pub fn all() -> &'static [CeremonyStep] {
        &[
            CeremonyStep::Initiate,
            CeremonyStep::Witness1Present,
            CeremonyStep::Witness2Present,
            CeremonyStep::HsmUnlock,
            CeremonyStep::KeyProvision,
            CeremonyStep::Sign,
            CeremonyStep::VerifySignature,
            CeremonyStep::Archive,
            CeremonyStep::HsmLock,
        ]
    }
}

/// 仪式结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeremonyResult {
    pub step: CeremonyStep,
    pub completed: bool,
    pub operator: String,
    pub timestamp: u64,
    pub evidence: String,
}

impl CeremonyResult {
    pub fn done(step: CeremonyStep, operator: &str, timestamp: u64, evidence: &str) -> Self {
        Self {
            step,
            completed: true,
            operator: String::from(operator),
            timestamp,
            evidence: String::from(evidence),
        }
    }

    pub fn failed(step: CeremonyStep, operator: &str, timestamp: u64, evidence: &str) -> Self {
        Self {
            step,
            completed: false,
            operator: String::from(operator),
            timestamp,
            evidence: String::from(evidence),
        }
    }
}

/// 签名仪式。
pub struct SigningCeremony {
    /// 仪式 ID。
    ceremony_id: String,
    /// HSM 状态。
    hsm_status: HsmStatus,
    /// 已完成的步骤。
    completed_steps: Vec<CeremonyResult>,
    /// 产物签名（SM2 hex）。
    artifact_signature: Option<String>,
}

impl SigningCeremony {
    pub fn new(ceremony_id: &str) -> Self {
        Self {
            ceremony_id: String::from(ceremony_id),
            hsm_status: HsmStatus::Offline,
            completed_steps: Vec::new(),
            artifact_signature: None,
        }
    }

    /// 执行步骤。
    pub fn execute_step(&mut self, result: CeremonyResult) -> Result<(), &'static str> {
        // 检查顺序
        let step_idx = CeremonyStep::all()
            .iter()
            .position(|s| *s == result.step)
            .ok_or("unknown step")?;

        let expected_count = step_idx;
        if self.completed_steps.len() != expected_count {
            return Err("steps must be executed in order");
        }

        if !result.completed {
            // 失败步骤终止仪式
            self.completed_steps.push(result);
            return Ok(());
        }

        // 更新 HSM 状态
        match result.step {
            CeremonyStep::HsmUnlock => self.hsm_status = HsmStatus::Ready,
            CeremonyStep::HsmLock => self.hsm_status = HsmStatus::Locked,
            _ => {}
        }

        self.completed_steps.push(result);
        Ok(())
    }

    /// 设置产物签名。
    pub fn set_signature(&mut self, sig: &str) {
        self.artifact_signature = Some(String::from(sig));
    }

    /// 是否完成（所有步骤通过 + 签名已设置）。
    pub fn is_complete(&self) -> bool {
        self.completed_steps.len() == CeremonyStep::all().len()
            && self.completed_steps.iter().all(|r| r.completed)
            && self.artifact_signature.is_some()
    }

    /// HSM 状态。
    pub fn hsm_status(&self) -> HsmStatus {
        self.hsm_status
    }

    /// 仪式 ID。
    pub fn ceremony_id(&self) -> &str {
        &self.ceremony_id
    }

    /// 已完成步骤。
    pub fn completed_steps(&self) -> &[CeremonyResult] {
        &self.completed_steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_ceremony_success() {
        let mut ceremony = SigningCeremony::new("cer-001");
        ceremony.hsm_status = HsmStatus::Offline;

        let steps = CeremonyStep::all();
        for (i, step) in steps.iter().enumerate() {
            let result = CeremonyResult::done(*step, "operator-a", 1000 + i as u64, "verified");
            ceremony.execute_step(result).unwrap();
        }
        ceremony.set_signature("sm2-sig-abc");

        assert!(ceremony.is_complete());
        assert_eq!(ceremony.hsm_status(), HsmStatus::Locked);
    }

    #[test]
    fn out_of_order_step_rejected() {
        let mut ceremony = SigningCeremony::new("cer-002");
        // Skip Initiate, try Witness1Present first
        let result = CeremonyResult::done(CeremonyStep::Witness1Present, "op", 1000, "ok");
        assert!(ceremony.execute_step(result).is_err());
    }

    #[test]
    fn hsm_unlock_changes_status() {
        let mut ceremony = SigningCeremony::new("cer-003");
        ceremony.execute_step(CeremonyResult::done(CeremonyStep::Initiate, "op", 1000, "ok")).unwrap();
        ceremony.execute_step(CeremonyResult::done(CeremonyStep::Witness1Present, "op", 1001, "ok")).unwrap();
        ceremony.execute_step(CeremonyResult::done(CeremonyStep::Witness2Present, "op", 1002, "ok")).unwrap();
        ceremony.execute_step(CeremonyResult::done(CeremonyStep::HsmUnlock, "op", 1003, "HSM unlocked")).unwrap();

        assert_eq!(ceremony.hsm_status(), HsmStatus::Ready);
    }

    #[test]
    fn incomplete_ceremony_not_complete() {
        let mut ceremony = SigningCeremony::new("cer-004");
        ceremony.execute_step(CeremonyResult::done(CeremonyStep::Initiate, "op", 1000, "ok")).unwrap();
        assert!(!ceremony.is_complete());
    }

    #[test]
    fn ceremony_without_signature_not_complete() {
        let mut ceremony = SigningCeremony::new("cer-005");
        for (i, step) in CeremonyStep::all().iter().enumerate() {
            ceremony.execute_step(CeremonyResult::done(*step, "op", 1000 + i as u64, "ok")).unwrap();
        }
        // No signature set
        assert!(!ceremony.is_complete());
    }

    #[test]
    fn failed_step_recorded() {
        let mut ceremony = SigningCeremony::new("cer-006");
        ceremony.execute_step(CeremonyResult::done(CeremonyStep::Initiate, "op", 1000, "ok")).unwrap();
        ceremony.execute_step(CeremonyResult::failed(CeremonyStep::Witness1Present, "op", 1001, "no witness")).unwrap();

        assert!(!ceremony.is_complete());
        assert!(!ceremony.completed_steps()[1].completed);
    }

    #[test]
    fn nine_steps_total() {
        assert_eq!(CeremonyStep::all().len(), 9);
    }
}
