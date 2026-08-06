//! 安全擦除与退役 (OPS-013).
//!
//! MASTER §21.14 OPS-013: "secure erase/decommission — cert revoke + report"
//!
//! 原则：设备退役前必须执行安全擦除：
//! 1. 吊销设备证书（防止身份冒用）。
//! 2. 销毁所有密钥（会话密钥、设备密钥、备份密钥）。
//! 3. 擦除存储介质（crypto erase 或 deep overwrite）。
//! 4. 生成签名擦除报告（证明数据已不可恢复）。

use alloc::string::String;
use alloc::vec::Vec;

/// 擦除级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EraseLevel {
    /// 快速：仅销毁密钥（数据加密后不可解密）。
    Quick,
    /// 完整：加密擦除 + 元数据清除。
    Full,
    /// 深度：多次覆写存储介质（DoD 5220.22-M 等效）。
    Deep,
}

impl EraseLevel {
    pub fn label(self) -> &'static str {
        match self {
            EraseLevel::Quick => "QUICK",
            EraseLevel::Full => "FULL",
            EraseLevel::Deep => "DEEP",
        }
    }
}

/// 擦除条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraseItem {
    /// 擦除的存储区域。
    pub target: String,
    /// 擦除级别。
    pub level: EraseLevel,
    /// 是否已验证擦除成功。
    pub verified: bool,
    /// 验证方法。
    pub verification_method: String,
}

impl EraseItem {
    pub fn new(target: &str, level: EraseLevel, verified: bool, method: &str) -> Self {
        Self {
            target: String::from(target),
            level,
            verified,
            verification_method: String::from(method),
        }
    }
}

/// 擦除报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraseReport {
    /// 报告 ID。
    pub report_id: String,
    /// 设备 ID。
    pub device_id: String,
    /// 擦除时间（Unix 秒）。
    pub timestamp: u64,
    /// 擦除条目。
    pub items: Vec<EraseItem>,
    /// 设备证书是否已吊销。
    pub cert_revoked: bool,
    /// 设备密钥是否已销毁。
    pub keys_destroyed: bool,
    /// 是否全部验证通过。
    pub all_verified: bool,
    /// 报告签名（SM2）。
    pub signature: String,
}

impl EraseReport {
    /// 从实测数据生成报告（禁止硬编码 all_verified = true）。
    pub fn from_results(
        report_id: &str,
        device_id: &str,
        timestamp: u64,
        items: Vec<EraseItem>,
        cert_revoked: bool,
        keys_destroyed: bool,
        signature: &str,
    ) -> Result<Self, &'static str> {
        if report_id.is_empty() {
            return Err("report_id is empty");
        }
        if device_id.is_empty() {
            return Err("device_id is empty");
        }
        if signature.is_empty() {
            return Err("signature is empty");
        }
        if items.is_empty() {
            return Err("no erase items");
        }

        let all_verified = items.iter().all(|i| i.verified) && cert_revoked && keys_destroyed;

        Ok(Self {
            report_id: String::from(report_id),
            device_id: String::from(device_id),
            timestamp,
            items,
            cert_revoked,
            keys_destroyed,
            all_verified,
            signature: String::from(signature),
        })
    }
}

/// 安全擦除执行器。
pub struct SecureErase {
    device_id: String,
    /// 是否已吊销证书。
    cert_revoked: bool,
    /// 是否已销毁密钥。
    keys_destroyed: bool,
    /// 擦除条目。
    items: Vec<EraseItem>,
}

impl SecureErase {
    pub fn new(device_id: &str) -> Self {
        Self {
            device_id: String::from(device_id),
            cert_revoked: false,
            keys_destroyed: false,
            items: Vec::new(),
        }
    }

    /// 吊销设备证书。
    pub fn revoke_certificate(&mut self) {
        self.cert_revoked = true;
    }

    /// 销毁设备密钥。
    pub fn destroy_keys(&mut self) {
        self.keys_destroyed = true;
    }

    /// 擦除存储区域。
    pub fn erase(&mut self, target: &str, level: EraseLevel, verified: bool, method: &str) {
        self.items
            .push(EraseItem::new(target, level, verified, method));
    }

    /// 生成擦除报告。
    pub fn generate_report(
        &self,
        report_id: &str,
        timestamp: u64,
        signature: &str,
    ) -> Result<EraseReport, &'static str> {
        EraseReport::from_results(
            report_id,
            &self.device_id,
            timestamp,
            self.items.clone(),
            self.cert_revoked,
            self.keys_destroyed,
            signature,
        )
    }

    /// 是否就绪（证书吊销 + 密钥销毁 + 至少一项擦除）。
    pub fn is_ready(&self) -> bool {
        self.cert_revoked && self.keys_destroyed && !self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_secure_erase_success() {
        let mut erase = SecureErase::new("dev-001");
        erase.revoke_certificate();
        erase.destroy_keys();
        erase.erase(
            "user-config-partition",
            EraseLevel::Full,
            true,
            "readback all zeros",
        );
        erase.erase(
            "business-data-partition",
            EraseLevel::Deep,
            true,
            "DoD 5220.22-M 3-pass verified",
        );
        erase.erase(
            "ai-model-cache",
            EraseLevel::Quick,
            true,
            "key destroyed, ciphertext unreadable",
        );

        assert!(erase.is_ready());

        let report = erase
            .generate_report("erase-001", 1000, "sm2-sig-abc")
            .unwrap();

        assert!(report.cert_revoked);
        assert!(report.keys_destroyed);
        assert!(report.all_verified);
        assert_eq!(report.items.len(), 3);
    }

    #[test]
    fn erase_without_cert_revoke_fails() {
        let mut erase = SecureErase::new("dev-001");
        // forgot to revoke cert
        erase.destroy_keys();
        erase.erase("data", EraseLevel::Full, true, "verified");

        let report = erase.generate_report("erase-002", 1000, "sig").unwrap();
        assert!(!report.all_verified);
        assert!(!report.cert_revoked);
    }

    #[test]
    fn erase_without_key_destroy_fails() {
        let mut erase = SecureErase::new("dev-001");
        erase.revoke_certificate();
        // forgot to destroy keys
        erase.erase("data", EraseLevel::Full, true, "verified");

        let report = erase.generate_report("erase-003", 1000, "sig").unwrap();
        assert!(!report.all_verified);
        assert!(!report.keys_destroyed);
    }

    #[test]
    fn erase_with_unverified_item_fails() {
        let mut erase = SecureErase::new("dev-001");
        erase.revoke_certificate();
        erase.destroy_keys();
        erase.erase("data", EraseLevel::Full, false, "verification pending");

        let report = erase.generate_report("erase-004", 1000, "sig").unwrap();
        assert!(!report.all_verified);
    }

    #[test]
    fn empty_erase_items_rejected() {
        let erase = SecureErase::new("dev-001");
        let result = erase.generate_report("erase-005", 1000, "sig");
        assert!(result.is_err());
    }

    #[test]
    fn missing_signature_rejected() {
        let mut erase = SecureErase::new("dev-001");
        erase.revoke_certificate();
        erase.destroy_keys();
        erase.erase("data", EraseLevel::Full, true, "ok");

        let result = erase.generate_report("erase-006", 1000, "");
        assert!(result.is_err());
    }

    #[test]
    fn missing_device_id_rejected() {
        let mut erase = SecureErase::new("dev-001");
        erase.revoke_certificate();
        erase.destroy_keys();
        erase.erase("data", EraseLevel::Full, true, "ok");

        let result =
            EraseReport::from_results("rpt", "", 1000, erase.items.clone(), true, true, "sig");
        assert!(result.is_err());
    }

    #[test]
    fn deep_erase_higher_than_full() {
        assert!(EraseLevel::Deep > EraseLevel::Full);
        assert!(EraseLevel::Full > EraseLevel::Quick);
    }

    #[test]
    fn is_ready_requires_all_three() {
        let mut erase = SecureErase::new("dev-001");
        assert!(!erase.is_ready());

        erase.revoke_certificate();
        assert!(!erase.is_ready()); // still missing keys + erase

        erase.destroy_keys();
        assert!(!erase.is_ready()); // still missing erase

        erase.erase("data", EraseLevel::Quick, true, "ok");
        assert!(erase.is_ready());
    }
}
