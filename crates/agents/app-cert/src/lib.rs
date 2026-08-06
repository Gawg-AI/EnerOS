//! EnerOS Agent OS 认证体系 (workspace v0.192.0)
//!
//! 提供应用认证与设备兼容性认证能力，包括认证规范定义、测试套件执行、
//! 设备兼容性检查与认证流程管理。
//!
//! # 模块
//! - [`types`][]: 核心数据结构（AppCertCategory / AppCertSpec / AppPackage / AppCertificate）
//! - [`spec`][]: 认证规范管理（默认规范生成）
//! - [`testkit`][]: 测试套件（AppTestKit trait / MockAppTestKit）
//! - [`device_compat`][]: 设备兼容性检查（DeviceCompatChecker trait / CompatChecker）

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod device_compat;
pub mod spec;
pub mod testkit;
pub mod types;

pub use device_compat::{CompatChecker, DeviceCompatChecker};
pub use spec::default_app_spec;
pub use testkit::{AppTestKit, MockAppTestKit};
pub use types::{
    AppCertCategory, AppCertItem, AppCertReport, AppCertSpec, AppCertificate, AppPackage,
    DeviceCompatReport, DeviceInfo, Outcome, TestCaseResult,
};

/// 应用认证错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCertError {
    /// 应用不存在
    AppNotFound,
    /// 签名无效
    SignatureInvalid,
    /// 强制项测试未通过
    MandatoryFailed,
    /// 设备不兼容
    DeviceIncompatible,
    /// 已认证，不可重复操作
    AlreadyCertified,
}

/// 应用认证 trait
pub trait AppCertification {
    /// 对应用执行认证，返回证书或错误
    fn certify_app(&self, app: &AppPackage) -> Result<AppCertificate, AppCertError>;
    /// 检查设备兼容性，返回兼容性报告或错误
    fn check_device_compat(&self, device: &DeviceInfo) -> Result<DeviceCompatReport, AppCertError>;
}

/// 应用认证引擎
///
/// 整合测试套件与设备兼容性检查，提供完整认证流程。
pub struct AppCertEngine {
    test_kit: MockAppTestKit,
    compat_checker: CompatChecker,
    next_cert_id: u64,
}

impl AppCertEngine {
    /// 创建新的认证引擎实例
    pub fn new() -> Self {
        Self {
            test_kit: MockAppTestKit,
            compat_checker: CompatChecker,
            next_cert_id: 1,
        }
    }
}

impl Default for AppCertEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AppCertification for AppCertEngine {
    fn certify_app(&self, app: &AppPackage) -> Result<AppCertificate, AppCertError> {
        if !app.signature_valid {
            return Err(AppCertError::SignatureInvalid);
        }

        let spec = default_app_spec();
        let report = self.test_kit.run(app, &spec);

        // 检查强制项是否全部通过
        for (i, item) in spec.items.iter().enumerate() {
            if item.mandatory {
                if let Some(result) = report.results.get(i) {
                    if let Outcome::Fail(_) = result.outcome {
                        return Err(AppCertError::MandatoryFailed);
                    }
                }
            }
        }

        Ok(AppCertificate {
            cert_id: self.next_cert_id,
            app_id: app.app_id,
            issued_at: 1700000000,
            expires_at: 1731536000,
        })
    }

    fn check_device_compat(&self, device: &DeviceInfo) -> Result<DeviceCompatReport, AppCertError> {
        let report = self.compat_checker.check(device);

        if report.compat_score < 1.0 {
            return Err(AppCertError::DeviceIncompatible);
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::*;

    fn make_valid_app() -> AppPackage {
        AppPackage {
            app_id: 42,
            name: String::from("energy-agent"),
            version: String::from("1.0.0"),
            signature_valid: true,
            binary_hash: [0xCD; 32],
        }
    }

    fn make_invalid_app() -> AppPackage {
        AppPackage {
            app_id: 43,
            name: String::from("bad-agent"),
            version: String::from("0.0.1"),
            signature_valid: false,
            binary_hash: [0x00; 32],
        }
    }

    #[test]
    fn test_certify_valid_app() {
        let engine = AppCertEngine::new();
        let app = make_valid_app();
        let result = engine.certify_app(&app);
        assert!(result.is_ok());

        let cert = result.unwrap();
        assert_eq!(cert.app_id, 42);
        assert!(cert.expires_at > cert.issued_at);
    }

    #[test]
    fn test_certify_invalid_signature() {
        let engine = AppCertEngine::new();
        let app = make_invalid_app();
        let result = engine.certify_app(&app);
        assert_eq!(result, Err(AppCertError::SignatureInvalid));
    }

    #[test]
    fn test_check_device_compat_certified() {
        let engine = AppCertEngine::new();
        let device = DeviceInfo {
            vendor: String::from("HiSilicon"),
            model: String::from("Kunpeng-920"),
            cpu_arch: String::from("aarch64"),
            bsp_certified: true,
        };
        let result = engine.check_device_compat(&device);
        assert!(result.is_ok());

        let report = result.unwrap();
        assert!((report.compat_score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_check_device_compat_uncertified() {
        let engine = AppCertEngine::new();
        let device = DeviceInfo {
            vendor: String::from("Unknown"),
            model: String::from("X1"),
            cpu_arch: String::from("x86_64"),
            bsp_certified: false,
        };
        let result = engine.check_device_compat(&device);
        assert_eq!(result, Err(AppCertError::DeviceIncompatible));
    }

    #[test]
    fn test_engine_default() {
        let engine = AppCertEngine::default();
        let app = make_valid_app();
        assert!(engine.certify_app(&app).is_ok());
    }
}
