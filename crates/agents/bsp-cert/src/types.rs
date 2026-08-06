//! 核心数据结构

use alloc::string::String;
use alloc::vec::Vec;

/// BSP 硬件类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BspCategory {
    /// 中央处理器
    Cpu,
    /// 内存
    Mem,
    /// 时钟
    Clock,
    /// 中断
    Irq,
    /// 串口
    Serial,
    /// 网络
    Net,
    /// 存储
    Storage,
    /// 加密
    Crypto,
    /// 电源管理
    Power,
}

/// BSP 认证项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BspItem {
    /// 认证项唯一标识
    pub id: String,
    /// 所属硬件类别
    pub category: BspCategory,
    /// 是否为强制项
    pub mandatory: bool,
}

/// BSP 认证规范
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BspSpec {
    /// 规范版本号
    pub version: String,
    /// 认证项列表
    pub items: Vec<BspItem>,
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
    /// 执行耗时 (毫秒)
    pub duration_ms: u64,
}

/// BSP 测试报告
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BspTestReport {
    /// 总用例数
    pub total: u32,
    /// 通过数
    pub passed: u32,
    /// 失败数
    pub failed: u32,
    /// 跳过数
    pub skipped: u32,
    /// 各用例结果
    pub results: Vec<TestCaseResult>,
}

/// 认证流程阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertPhase {
    /// 已提交
    Submitted,
    /// 测试中
    Testing,
    /// 报告已生成
    Report,
    /// 已认证
    Certified,
    /// 已拒绝
    Rejected,
}

/// 硬件信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareInfo {
    /// 厂商
    pub vendor: String,
    /// 型号
    pub model: String,
    /// CPU 架构
    pub cpu_arch: String,
}

/// BSP 认证证书
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    /// 证书编号
    pub cert_id: u64,
    /// 硬件厂商
    pub hw_vendor: String,
    /// 硬件型号
    pub hw_model: String,
    /// 规范版本
    pub spec_version: String,
    /// 签发时间 (Unix epoch seconds)
    pub issued_at: u64,
    /// 过期时间 (Unix epoch seconds)
    pub expires_at: u64,
}

#[cfg(test)]
mod tests {
    use alloc::format;

    use super::*;

    #[test]
    fn test_bsp_category_eq() {
        assert_eq!(BspCategory::Cpu, BspCategory::Cpu);
        assert_ne!(BspCategory::Cpu, BspCategory::Mem);
    }

    #[test]
    fn test_bsp_item_creation() {
        let item = BspItem {
            id: String::from("cpu-001"),
            category: BspCategory::Cpu,
            mandatory: true,
        };
        assert_eq!(item.id, "cpu-001");
        assert_eq!(item.category, BspCategory::Cpu);
        assert!(item.mandatory);
    }

    #[test]
    fn test_bsp_spec_creation() {
        let spec = BspSpec {
            version: String::from("1.0.0"),
            items: Vec::new(),
        };
        assert_eq!(spec.version, "1.0.0");
        assert!(spec.items.is_empty());
    }

    #[test]
    fn test_outcome_variants() {
        let pass = Outcome::Pass;
        let fail = Outcome::Fail(String::from("timeout"));
        let skip = Outcome::Skip;
        assert_eq!(pass, Outcome::Pass);
        assert_eq!(fail, Outcome::Fail(String::from("timeout")));
        assert_eq!(skip, Outcome::Skip);
    }

    #[test]
    fn test_test_case_result() {
        let r = TestCaseResult {
            case_id: String::from("tc-01"),
            outcome: Outcome::Pass,
            duration_ms: 42,
        };
        assert_eq!(r.case_id, "tc-01");
        assert_eq!(r.duration_ms, 42);
    }

    #[test]
    fn test_bsp_test_report_counts() {
        let report = BspTestReport {
            total: 3,
            passed: 2,
            failed: 1,
            skipped: 0,
            results: Vec::new(),
        };
        assert_eq!(report.total, 3);
        assert_eq!(report.passed + report.failed + report.skipped, report.total);
    }

    #[test]
    fn test_cert_phase_transitions() {
        let phases = [
            CertPhase::Submitted,
            CertPhase::Testing,
            CertPhase::Report,
            CertPhase::Certified,
        ];
        assert_eq!(phases[0], CertPhase::Submitted);
        assert_ne!(phases[3], CertPhase::Rejected);
    }

    #[test]
    fn test_hardware_info() {
        let hw = HardwareInfo {
            vendor: String::from("HiSilicon"),
            model: String::from("Kunpeng-920"),
            cpu_arch: String::from("aarch64"),
        };
        assert_eq!(hw.vendor, "HiSilicon");
        assert_eq!(hw.cpu_arch, "aarch64");
    }

    #[test]
    fn test_certificate() {
        let cert = Certificate {
            cert_id: 1001,
            hw_vendor: String::from("Phytium"),
            hw_model: String::from("D2000"),
            spec_version: String::from("1.0.0"),
            issued_at: 1700000000,
            expires_at: 1731536000,
        };
        assert_eq!(cert.cert_id, 1001);
        assert!(cert.expires_at > cert.issued_at);
    }

    #[test]
    fn test_clone_equality() {
        let hw = HardwareInfo {
            vendor: String::from("Vendor"),
            model: String::from("Model"),
            cpu_arch: String::from("riscv64"),
        };
        let hw2 = hw.clone();
        assert_eq!(hw, hw2);
    }

    #[test]
    fn test_outcome_fail_message() {
        let outcome = Outcome::Fail(format!("IRQ {} not responding", 7));
        assert!(matches!(outcome, Outcome::Fail(_)));
        if let Outcome::Fail(msg) = outcome {
            assert!(msg.contains("IRQ"));
        }
    }
}
