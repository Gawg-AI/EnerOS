//! 核心数据结构

use alloc::string::String;
use alloc::vec::Vec;

/// 应用认证类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppCertCategory {
    /// SDK 兼容性
    SdkCompat,
    /// 安全性
    Security,
    /// 性能
    Performance,
    /// 资源限制
    ResourceLimit,
    /// 行为审计
    BehaviorAudit,
}

/// 应用认证项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCertItem {
    /// 认证项唯一标识
    pub id: String,
    /// 所属认证类别
    pub category: AppCertCategory,
    /// 是否为强制项
    pub mandatory: bool,
}

/// 应用认证规范
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCertSpec {
    /// 规范版本号
    pub version: String,
    /// 认证项列表
    pub items: Vec<AppCertItem>,
}

/// 应用包信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPackage {
    /// 应用 ID
    pub app_id: u64,
    /// 应用名称
    pub name: String,
    /// 应用版本
    pub version: String,
    /// 签名是否有效
    pub signature_valid: bool,
    /// 二进制哈希 (SHA-256)
    pub binary_hash: [u8; 32],
}

/// 测试用例结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 通过
    Pass,
    /// 失败（附原因）
    Fail(String),
    /// 跳过
    Skip,
}

/// 单个测试用例结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCaseResult {
    /// 用例标识
    pub case_id: String,
    /// 测试结果
    pub outcome: Outcome,
}

/// 应用认证测试报告
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCertReport {
    /// 总用例数
    pub total: u32,
    /// 通过数
    pub passed: u32,
    /// 失败数
    pub failed: u32,
    /// 各用例结果
    pub results: Vec<TestCaseResult>,
}

/// 应用认证证书
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCertificate {
    /// 证书编号
    pub cert_id: u64,
    /// 应用 ID
    pub app_id: u64,
    /// 签发时间 (Unix epoch seconds)
    pub issued_at: u64,
    /// 过期时间 (Unix epoch seconds)
    pub expires_at: u64,
}

/// 设备信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// 厂商
    pub vendor: String,
    /// 型号
    pub model: String,
    /// CPU 架构
    pub cpu_arch: String,
    /// BSP 是否已认证
    pub bsp_certified: bool,
}

/// 设备兼容性报告
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceCompatReport {
    /// 设备厂商
    pub device_vendor: String,
    /// 兼容性评分 (0.0 ~ 1.0)
    pub compat_score: f64,
    /// 问题列表
    pub issues: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_cert_category_eq() {
        assert_eq!(AppCertCategory::SdkCompat, AppCertCategory::SdkCompat);
        assert_ne!(AppCertCategory::Security, AppCertCategory::Performance);
    }

    #[test]
    fn test_app_cert_item_creation() {
        let item = AppCertItem {
            id: String::from("sec-001"),
            category: AppCertCategory::Security,
            mandatory: true,
        };
        assert_eq!(item.id, "sec-001");
        assert_eq!(item.category, AppCertCategory::Security);
        assert!(item.mandatory);
    }

    #[test]
    fn test_app_cert_spec_creation() {
        let spec = AppCertSpec {
            version: String::from("1.0.0"),
            items: Vec::new(),
        };
        assert_eq!(spec.version, "1.0.0");
        assert!(spec.items.is_empty());
    }

    #[test]
    fn test_app_package_creation() {
        let app = AppPackage {
            app_id: 100,
            name: String::from("test-app"),
            version: String::from("0.1.0"),
            signature_valid: true,
            binary_hash: [0u8; 32],
        };
        assert_eq!(app.app_id, 100);
        assert!(app.signature_valid);
    }

    #[test]
    fn test_outcome_variants() {
        let pass = Outcome::Pass;
        let fail = Outcome::Fail(String::from("signature invalid"));
        let skip = Outcome::Skip;
        assert_eq!(pass, Outcome::Pass);
        assert_eq!(fail, Outcome::Fail(String::from("signature invalid")));
        assert_eq!(skip, Outcome::Skip);
    }

    #[test]
    fn test_test_case_result() {
        let r = TestCaseResult {
            case_id: String::from("tc-01"),
            outcome: Outcome::Pass,
        };
        assert_eq!(r.case_id, "tc-01");
        assert_eq!(r.outcome, Outcome::Pass);
    }

    #[test]
    fn test_app_cert_report_counts() {
        let report = AppCertReport {
            total: 5,
            passed: 4,
            failed: 1,
            results: Vec::new(),
        };
        assert_eq!(report.total, 5);
        assert_eq!(report.passed + report.failed, report.total);
    }

    #[test]
    fn test_app_certificate() {
        let cert = AppCertificate {
            cert_id: 2001,
            app_id: 100,
            issued_at: 1700000000,
            expires_at: 1731536000,
        };
        assert_eq!(cert.cert_id, 2001);
        assert!(cert.expires_at > cert.issued_at);
    }

    #[test]
    fn test_device_info() {
        let dev = DeviceInfo {
            vendor: String::from("HiSilicon"),
            model: String::from("Kunpeng-920"),
            cpu_arch: String::from("aarch64"),
            bsp_certified: true,
        };
        assert_eq!(dev.vendor, "HiSilicon");
        assert!(dev.bsp_certified);
    }

    #[test]
    fn test_device_compat_report() {
        let report = DeviceCompatReport {
            device_vendor: String::from("Phytium"),
            compat_score: 1.0,
            issues: Vec::new(),
        };
        assert_eq!(report.device_vendor, "Phytium");
        assert!((report.compat_score - 1.0).abs() < f64::EPSILON);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn test_clone_equality() {
        let dev = DeviceInfo {
            vendor: String::from("Vendor"),
            model: String::from("Model"),
            cpu_arch: String::from("riscv64"),
            bsp_certified: false,
        };
        let dev2 = dev.clone();
        assert_eq!(dev, dev2);
    }
}
