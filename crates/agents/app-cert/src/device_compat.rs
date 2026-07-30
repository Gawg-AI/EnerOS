//! 设备兼容性检查

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{DeviceCompatReport, DeviceInfo};

/// 设备兼容性检查 trait
pub trait DeviceCompatChecker {
    /// 检查设备兼容性，返回兼容性报告
    fn check(&self, device: &DeviceInfo) -> DeviceCompatReport;
}

/// 标准兼容性检查器
///
/// 规则：检查 `bsp_certified` 标志；
/// - 已认证 → score = 1.0，无问题
/// - 未认证 → score = 0.5，issues 说明原因
pub struct CompatChecker;

impl DeviceCompatChecker for CompatChecker {
    fn check(&self, device: &DeviceInfo) -> DeviceCompatReport {
        let mut issues: Vec<String> = Vec::new();

        let compat_score = if device.bsp_certified {
            1.0
        } else {
            issues.push(String::from("BSP not certified"));
            0.5
        };

        DeviceCompatReport {
            device_vendor: device.vendor.clone(),
            compat_score,
            issues,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_device(bsp_certified: bool) -> DeviceInfo {
        DeviceInfo {
            vendor: String::from("HiSilicon"),
            model: String::from("Kunpeng-920"),
            cpu_arch: String::from("aarch64"),
            bsp_certified,
        }
    }

    #[test]
    fn test_certified_device_full_score() {
        let checker = CompatChecker;
        let device = make_device(true);
        let report = checker.check(&device);

        assert_eq!(report.device_vendor, "HiSilicon");
        assert!((report.compat_score - 1.0).abs() < f64::EPSILON);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn test_uncertified_device_half_score() {
        let checker = CompatChecker;
        let device = make_device(false);
        let report = checker.check(&device);

        assert_eq!(report.device_vendor, "HiSilicon");
        assert!((report.compat_score - 0.5).abs() < f64::EPSILON);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0], "BSP not certified");
    }

    #[test]
    fn test_uncertified_device_has_issue() {
        let checker = CompatChecker;
        let device = DeviceInfo {
            vendor: String::from("Phytium"),
            model: String::from("D2000"),
            cpu_arch: String::from("aarch64"),
            bsp_certified: false,
        };
        let report = checker.check(&device);

        assert_eq!(report.device_vendor, "Phytium");
        assert!(!report.issues.is_empty());
    }
}
