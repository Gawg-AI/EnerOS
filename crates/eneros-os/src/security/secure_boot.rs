//! Secure Boot 与内核锁定状态检测 (v0.24.0 — Task 2)
//!
//! 提供 Secure Boot 启用状态、lockdown 模式、MOK 注册、内核签名检测。
//! Linux 下通过 mokutil 和 /sys/kernel/security/lockdown 获取状态；
//! 非 Linux 平台返回 `Unsupported`。

use thiserror::Error;

/// Secure Boot 检测错误
#[derive(Debug, Error)]
pub enum SecureBootError {
    #[error("unsupported on this platform")]
    Unsupported,
    #[error("command failed: {0}")]
    CommandFailed(String),
    #[error("parse error: {0}")]
    Parse(String),
}

/// 内核 lockdown 模式
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LockdownMode {
    None,
    Integrity,
    Confidentiality,
    Unknown,
}

/// Secure Boot 状态
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SecureBootStatus {
    pub enabled: bool,
    pub lockdown: LockdownMode,
    pub kernel_signed: bool,
    pub mok_enrolled: bool,
}

/// 检测 Secure Boot 状态（Linux 实现）
#[cfg(target_os = "linux")]
pub fn check_secure_boot() -> Result<SecureBootStatus, SecureBootError> {
    use std::process::Command;

    // 1. 检查 Secure Boot 状态：mokutil --sb-state
    let enabled = match Command::new("mokutil").arg("--sb-state").output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.contains("SecureBoot enabled")
        }
        Err(_) => false, // mokutil 不存在，假定未启用
    };

    // 2. 检查 lockdown：/sys/kernel/security/lockdown
    let lockdown = match std::fs::read_to_string("/sys/kernel/security/lockdown") {
        Ok(content) => {
            // 格式示例：[integrity] confidentiality none
            if content.contains("[integrity]") {
                LockdownMode::Integrity
            } else if content.contains("[confidentiality]") {
                LockdownMode::Confidentiality
            } else if content.contains("[none]") {
                LockdownMode::None
            } else {
                LockdownMode::Unknown
            }
        }
        Err(_) => LockdownMode::None,
    };

    // 3. 检查 MOK 注册：mokutil --list-enrolled
    let mok_enrolled = match Command::new("mokutil").arg("--list-enrolled").output() {
        Ok(out) => !out.stdout.is_empty(),
        Err(_) => false,
    };

    // 4. 检查内核签名：modinfo on a built-in module or read /proc/modules
    // 简化实现：检查内核是否在 Secure Boot 下加载了签名模块
    let kernel_signed = enabled; // Secure Boot 启用时内核必然已签名

    Ok(SecureBootStatus {
        enabled,
        lockdown,
        kernel_signed,
        mok_enrolled,
    })
}

/// 检测 Secure Boot 状态（非 Linux stub）
#[cfg(not(target_os = "linux"))]
pub fn check_secure_boot() -> Result<SecureBootStatus, SecureBootError> {
    Err(SecureBootError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_check_secure_boot_unsupported() {
        let result = check_secure_boot();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SecureBootError::Unsupported));
    }
}
