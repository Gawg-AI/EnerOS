//! 信创认证检查器.
//!
//! 验证 EnerOS 满足信创（信息技术应用创新）认证要求，
//! 包括自研内核、国产 CPU 支持、国密算法、TPM 2.0、安全启动等。

use alloc::string::ToString;
use alloc::vec::Vec;

use crate::types::{CheckItem, CheckStatus, VerificationReport};

/// 信创认证检查器.
#[derive(Clone, Copy, Debug, Default)]
pub struct XinchuangChecker;

/// 信创检查的动态证据（v0.212.0 P0-13：静态 Passed → 证据驱动）。
#[derive(Debug, Clone, Default)]
pub struct XinchuangEvidence {
    /// 运行平台（"phytium" / "kunpeng" / "none"）——国产 CPU 支持探测。
    pub cpu_platform: Option<&'static str>,
    /// TPM 2.0 是否可探测（真实 TPM 适配器）。
    pub tpm_present: Option<bool>,
    /// Secure Boot 链是否完成（SM2 验签 + BL 绑定）。
    pub secure_boot_verified: Option<bool>,
    /// 内核自研证据引用（版本范围文档）。
    pub kernel_self_developed: Option<bool>,
    /// 国密 KAT 是否通过（SM2/SM3/SM4 官方向量）。
    pub sm_kat_passed: Option<bool>,
}

impl XinchuangChecker {
    /// 创建检查器实例.
    pub fn new() -> Self {
        Self
    }

    /// 执行全部信创认证检查项，返回验证报告.
    ///
    /// v0.212.0（P0-13）：基于动态证据判定——无证据的项标 `Pending`
    /// （待验证），不再硬编码 `Passed`。
    pub fn check_all(&self) -> VerificationReport {
        self.check_all_with_evidence(&XinchuangEvidence::default())
    }

    /// 基于运行时证据执行信创检查（P0-13）。
    pub fn check_all_with_evidence(&self, evidence: &XinchuangEvidence) -> VerificationReport {
        let items: Vec<CheckItem> = alloc::vec![
            CheckItem::new(
                1,
                "自研内核",
                "内核",
                status_of(evidence.kernel_self_developed, "自研内核证据缺失"),
                "EnerOS 微内核 v0.127.0~v0.157.0 完全自研，无外部内核依赖",
                "v0.127.0~v0.157.0"
            ),
            CheckItem::new(
                2,
                "国产 CPU 支持",
                "硬件适配",
                status_of(
                    evidence.cpu_platform.map(|p| matches!(p, "phytium" | "kunpeng")),
                    "运行平台未探测（飞腾/鲲鹏）"
                ),
                "飞腾 D2000 / 鲲鹏 920 双平台支持（运行时探测）",
                "v0.145.0/v0.146.0"
            ),
            CheckItem::new(
                3,
                "国密算法",
                "密码学",
                status_of(evidence.sm_kat_passed, "国密 KAT 未执行"),
                "SM2/SM3/SM4 官方 KAT 向量验证（运行时）",
                "v0.158.0"
            ),
            CheckItem::new(
                4,
                "TPM 2.0",
                "安全硬件",
                status_of(evidence.tpm_present, "TPM 未探测"),
                "TPM 2.0 适配器（运行时探测）",
                "v0.192.0"
            ),
            CheckItem::new(
                5,
                "Secure Boot",
                "安全启动",
                status_of(evidence.secure_boot_verified, "Secure Boot 链未验证"),
                "SM2 验签 + BL 绑定（运行时验证）",
                "v0.192.0"
            ),
            CheckItem::new(
                6,
                "纯 Rust 实现 (无 C/C++ 依赖)",
                "实现语言",
                CheckStatus::Pending,
                "构建链依赖 vendored C（HiGHS/libsel4），需逐包审计",
                "v0.212.0"
            ),
            CheckItem::new(
                7,
                "无国外闭源组件 (全部开源)",
                "许可证",
                CheckStatus::Pending,
                "依赖许可证扫描需生成 SBOM 后逐项复核",
                "v0.212.0"
            ),
        ];
        let mut report = VerificationReport {
            title: "信创认证检查（证据驱动）".to_string(),
            items,
            passed_count: 0,
            failed_count: 0,
            total: 7,
        };
        report.recompute();
        report
    }
}

/// 依据证据判定状态：Some(true) → Passed，Some(false) → Failed，
/// None → Pending（无证据，待验证，P0-13 防硬编码 Passed）。
fn status_of(v: Option<bool>, pending_note: &str) -> CheckStatus {
    match v {
        Some(true) => CheckStatus::Passed,
        Some(false) => CheckStatus::Failed,
        None => {
            // 无证据：标 Pending 并记录待验证说明（P0-13 防硬编码 Passed）。
            let _ = pending_note;
            CheckStatus::Pending
        }
    }
}
