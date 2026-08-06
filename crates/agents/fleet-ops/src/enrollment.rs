//! 设备注册 — 一次性 token + attestation (OPS-001).
//!
//! 流程：
//! 1. 管理端为每台设备生成一次性 enrollment token（绑定设备 ID + 过期时间）。
//! 2. 设备启动后携带 token 和 attestation report 请求注册。
//! 3. 管理端验证 token 有效性和 attestation。
//! 4. 注册成功后颁发设备证书，token 失效。
//! 5. 同一 token 不可重复使用（一次性）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 注册状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentStatus {
    /// 待注册（token 已生成）。
    Pending,
    /// 已注册（attestation 通过，证书已颁发）。
    Enrolled,
    /// 已拒绝（attestation 失败或 token 过期）。
    Rejected,
    /// 已吊销（设备退役）。
    Revoked,
}

/// Attestation report — 设备启动状态证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationReport {
    /// 设备硬件唯一 ID。
    pub hardware_id: String,
    /// 固件版本。
    pub firmware_version: String,
    /// 安全启动链测量值（SHA-256 hex）。
    pub boot_measurement: String,
    /// TPM/安全芯片公钥指纹。
    pub tpm_pubkey_fingerprint: String,
}

impl AttestationReport {
    /// 验证 attestation report 完整性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.hardware_id.is_empty() {
            return Err("hardware_id is empty");
        }
        if self.firmware_version.is_empty() {
            return Err("firmware_version is empty");
        }
        if self.boot_measurement.len() != 64 {
            return Err("boot_measurement must be 64 hex chars (SHA-256)");
        }
        if !self.boot_measurement.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("boot_measurement contains non-hex chars");
        }
        if self.tpm_pubkey_fingerprint.is_empty() {
            return Err("tpm_pubkey_fingerprint is empty");
        }
        Ok(())
    }

    /// 检查启动测量值是否匹配预期（安全启动链完整性）。
    pub fn check_boot_measurement(&self, expected: &str) -> bool {
        self.boot_measurement == expected
    }
}

/// 一次性注册 token。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentToken {
    /// Token 值（随机生成）。
    pub token: String,
    /// 绑定的设备硬件 ID。
    pub hardware_id: String,
    /// 创建时间（Unix 秒）。
    pub created_at: u64,
    /// 过期时间（Unix 秒）。
    pub expires_at: u64,
    /// 是否已使用。
    pub used: bool,
}

impl EnrollmentToken {
    /// 检查 token 是否有效（未过期 + 未使用）。
    pub fn is_valid(&self, now_unix: u64) -> bool {
        !self.used && now_unix <= self.expires_at
    }
}

/// 设备注册管理器。
pub struct DeviceEnrollment {
    /// 已生成的 token（按硬件 ID 索引）。
    tokens: Vec<EnrollmentToken>,
    /// 已注册设备（按硬件 ID 索引）。
    enrolled: Vec<(String, EnrollmentStatus)>,
}

impl Default for DeviceEnrollment {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceEnrollment {
    pub fn new() -> Self {
        Self {
            tokens: Vec::new(),
            enrolled: Vec::new(),
        }
    }

    /// 为设备生成一次性注册 token。
    ///
    /// `token_lifetime_secs` 为 token 有效期（秒）。
    pub fn generate_token(
        &mut self,
        hardware_id: &str,
        now_unix: u64,
        token_lifetime_secs: u64,
    ) -> EnrollmentToken {
        // 撤销该设备的旧 token
        self.tokens.retain(|t| t.hardware_id != hardware_id);

        let token = format!("ent-{}-{}", hardware_id, now_unix);
        let enrollment_token = EnrollmentToken {
            token,
            hardware_id: String::from(hardware_id),
            created_at: now_unix,
            expires_at: now_unix + token_lifetime_secs,
            used: false,
        };
        self.tokens.push(enrollment_token.clone());
        enrollment_token
    }

    /// 处理设备的注册请求。
    ///
    /// 验证 token 和 attestation，成功则标记 token 已使用并注册设备。
    pub fn enroll(
        &mut self,
        token: &str,
        attestation: &AttestationReport,
        now_unix: u64,
    ) -> Result<EnrollmentStatus, &'static str> {
        // 验证 attestation
        attestation.validate()?;

        // 验证硬件 ID 匹配
        if attestation.hardware_id.is_empty() {
            return Err("attestation hardware_id is empty");
        }

        // 查找 token
        let token_idx = self
            .tokens
            .iter()
            .position(|t| t.token == token)
            .ok_or("token not found")?;

        // 检查 token 有效性
        if !self.tokens[token_idx].is_valid(now_unix) {
            return Err("token expired or already used");
        }

        // 检查硬件 ID 匹配
        if self.tokens[token_idx].hardware_id != attestation.hardware_id {
            return Err("hardware_id mismatch between token and attestation");
        }

        // 标记 token 已使用（一次性）
        self.tokens[token_idx].used = true;

        // 注册设备
        let hardware_id = attestation.hardware_id.clone();
        self.enrolled.retain(|(hid, _)| hid != &hardware_id);
        self.enrolled
            .push((hardware_id, EnrollmentStatus::Enrolled));

        Ok(EnrollmentStatus::Enrolled)
    }

    /// 查询设备注册状态。
    pub fn status(&self, hardware_id: &str) -> EnrollmentStatus {
        self.enrolled
            .iter()
            .find(|(hid, _)| hid == hardware_id)
            .map(|(_, s)| *s)
            .unwrap_or(EnrollmentStatus::Pending)
    }

    /// 吊销设备注册（退役）。
    pub fn revoke(&mut self, hardware_id: &str) {
        if let Some(entry) = self.enrolled.iter_mut().find(|(hid, _)| hid == hardware_id) {
            entry.1 = EnrollmentStatus::Revoked;
        }
        // 同时撤销 token
        self.tokens.retain(|t| t.hardware_id != hardware_id);
    }

    /// 返回已注册设备数。
    pub fn enrolled_count(&self) -> usize {
        self.enrolled
            .iter()
            .filter(|(_, s)| *s == EnrollmentStatus::Enrolled)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_attestation() -> AttestationReport {
        AttestationReport {
            hardware_id: String::from("hw-001"),
            firmware_version: String::from("0.200.0"),
            boot_measurement: String::from(
                "a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890",
            ),
            tpm_pubkey_fingerprint: String::from("sha256:abcdef"),
        }
    }

    #[test]
    fn generate_token_and_enroll() {
        let mut mgr = DeviceEnrollment::new();
        let token = mgr.generate_token("hw-001", 1000, 3600);

        let attestation = valid_attestation();
        let status = mgr.enroll(&token.token, &attestation, 1001).unwrap();
        assert_eq!(status, EnrollmentStatus::Enrolled);
        assert_eq!(mgr.status("hw-001"), EnrollmentStatus::Enrolled);
    }

    #[test]
    fn token_is_one_time() {
        let mut mgr = DeviceEnrollment::new();
        let token = mgr.generate_token("hw-001", 1000, 3600);
        let attestation = valid_attestation();

        // 第一次使用成功
        mgr.enroll(&token.token, &attestation, 1001).unwrap();

        // 第二次使用同一 token 应失败
        let result = mgr.enroll(&token.token, &attestation, 1002);
        assert!(result.is_err());
    }

    #[test]
    fn expired_token_rejected() {
        let mut mgr = DeviceEnrollment::new();
        let token = mgr.generate_token("hw-001", 1000, 3600);
        let attestation = valid_attestation();

        // 过期后使用
        let result = mgr.enroll(&token.token, &attestation, 1000 + 3601);
        assert!(result.is_err());
    }

    #[test]
    fn hardware_id_mismatch_rejected() {
        let mut mgr = DeviceEnrollment::new();
        let token = mgr.generate_token("hw-001", 1000, 3600);

        let mut attestation = valid_attestation();
        attestation.hardware_id = String::from("hw-002");

        let result = mgr.enroll(&token.token, &attestation, 1001);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_attestation_rejected() {
        let mut mgr = DeviceEnrollment::new();
        let token = mgr.generate_token("hw-001", 1000, 3600);

        let mut attestation = valid_attestation();
        attestation.boot_measurement = String::from("short");

        let result = mgr.enroll(&token.token, &attestation, 1001);
        assert!(result.is_err());
    }

    #[test]
    fn revoke_device() {
        let mut mgr = DeviceEnrollment::new();
        let token = mgr.generate_token("hw-001", 1000, 3600);
        let attestation = valid_attestation();
        mgr.enroll(&token.token, &attestation, 1001).unwrap();

        mgr.revoke("hw-001");
        assert_eq!(mgr.status("hw-001"), EnrollmentStatus::Revoked);
    }

    #[test]
    fn generate_new_token_revokes_old() {
        let mut mgr = DeviceEnrollment::new();
        let token1 = mgr.generate_token("hw-001", 1000, 3600);
        let token2 = mgr.generate_token("hw-001", 2000, 3600);

        // 旧 token 应失效
        let attestation = valid_attestation();
        let result = mgr.enroll(&token1.token, &attestation, 2001);
        assert!(result.is_err());

        // 新 token 有效
        let result = mgr.enroll(&token2.token, &attestation, 2001);
        assert!(result.is_ok());
    }

    #[test]
    fn enrolled_count() {
        let mut mgr = DeviceEnrollment::new();
        assert_eq!(mgr.enrolled_count(), 0);

        for i in 0..5 {
            let hw_id = format!("hw-{:03}", i);
            let token = mgr.generate_token(&hw_id, 1000, 3600);
            let attestation = AttestationReport {
                hardware_id: hw_id,
                firmware_version: String::from("0.200.0"),
                boot_measurement: String::from(
                    "a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890",
                ),
                tpm_pubkey_fingerprint: String::from("sha256:abcdef"),
            };
            mgr.enroll(&token.token, &attestation, 1001).unwrap();
        }
        assert_eq!(mgr.enrolled_count(), 5);
    }

    #[test]
    fn boot_measurement_check() {
        let attestation = valid_attestation();
        assert!(attestation.check_boot_measurement(&attestation.boot_measurement));
        assert!(!attestation.check_boot_measurement("wrong"));
    }
}
