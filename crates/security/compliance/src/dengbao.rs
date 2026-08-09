//! 等保 2.0 三级检查器.
//!
//! 验证 EnerOS 满足《信息安全技术 网络安全等级保护基本要求》(GB/T 22239-2019)
//! 三级安全要求，覆盖安全物理环境、通信网络、计算环境、管理中心等 15 项。
//!
//! # 证据驱动（21-B C3，2026-08-09）
//! `check_level3()` 默认走**动态证据判定**：无证据的项标 `Pending`
//! （待验证），**禁止硬编码 `Passed`**（对齐 `xinchuang` P0-13 先例）。
//! 调用方（宿主 / xtask qa）注入 `DengbaoEvidence`（每项一个可追溯证据
//! 指针 + 检查结果）后，`Passed` 项附带证据指针、`Failed` 项标注未通过。
//! 静态枚举仅 `#[cfg(test)]` 可见（测试参考，非生产路径）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{CheckItem, CheckStatus, VerificationReport};

/// 单项证据来源（21-B C3）：可追溯指针 + 检查结果。
///
/// `pointer` 必须是可追溯的证据位置（配置文件路径 / 日志路径 /
/// 运行时检查名），禁止空指针配 `Passed`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceSource {
    /// 证据指针（如 `configs/dengbao.toml` 的项声明或产物文件路径）。
    pub pointer: String,
    /// 该项运行时检查结果。
    pub ok: bool,
}

impl EvidenceSource {
    /// 构造证据源。
    pub fn new(pointer: impl Into<String>, ok: bool) -> Self {
        Self {
            pointer: pointer.into(),
            ok,
        }
    }
}

/// 等保 2.0 三级运行时证据集（15 项，与检查项一一对应）。
///
/// `None` = 该面无运行时证据（判定为 `Pending`，不硬编码 Passed）。
#[derive(Debug, Clone, Default)]
pub struct DengbaoEvidence {
    /// 1. 安全物理环境：物理访问控制证据（机房/部署记录）。
    pub physical_access: Option<EvidenceSource>,
    /// 2. 安全通信网络：网络架构证据（TSN/61850 配置审计）。
    pub network_architecture: Option<EvidenceSource>,
    /// 3. 安全区域边界：边界防护证据（微内核 IPC 隔离验证记录）。
    pub boundary_protection: Option<EvidenceSource>,
    /// 4. 安全计算环境：计算安全证据（SM2 身份鉴别能力实测）。
    pub compute_security: Option<EvidenceSource>,
    /// 5. 安全管理中心：集中管理证据（agent/hmi 运行状态）。
    pub management_center: Option<EvidenceSource>,
    /// 6. 安全通信完整性：SM3 摘要校验证据。
    pub comm_integrity: Option<EvidenceSource>,
    /// 7. 安全通信保密性：SM4 加密 + mTLS 证据。
    pub comm_confidentiality: Option<EvidenceSource>,
    /// 8. 可信验证：Secure Boot + TPM 度量证据。
    pub trusted_boot: Option<EvidenceSource>,
    /// 9. 入侵防范：异常检测器激活证据。
    pub intrusion_prevention: Option<EvidenceSource>,
    /// 10. 恶意代码防范：签名 OTA + 无 unsafe 核心证据。
    pub malware_prevention: Option<EvidenceSource>,
    /// 11. 安全审计：审计日志落盘证据。
    pub security_audit: Option<EvidenceSource>,
    /// 12. 数据完整性：SM3 校验 + A/B 回滚证据。
    pub data_integrity: Option<EvidenceSource>,
    /// 13. 数据保密性：SM4 静态加密 + TPM 封存证据。
    pub data_confidentiality: Option<EvidenceSource>,
    /// 14. 剩余信息保护：内存零化证据。
    pub residual_info: Option<EvidenceSource>,
    /// 15. 个人信息保护：本地处理 + 最小化原则证据。
    pub personal_info: Option<EvidenceSource>,
}

impl DengbaoEvidence {
    /// 从 15 项证据源数组构造（数组顺序与检查项编号一致）。
    pub fn from_array(items: [Option<EvidenceSource>; 15]) -> Self {
        let mut it = items.into_iter();
        Self {
            physical_access: it.next().unwrap(),
            network_architecture: it.next().unwrap(),
            boundary_protection: it.next().unwrap(),
            compute_security: it.next().unwrap(),
            management_center: it.next().unwrap(),
            comm_integrity: it.next().unwrap(),
            comm_confidentiality: it.next().unwrap(),
            trusted_boot: it.next().unwrap(),
            intrusion_prevention: it.next().unwrap(),
            malware_prevention: it.next().unwrap(),
            security_audit: it.next().unwrap(),
            data_integrity: it.next().unwrap(),
            data_confidentiality: it.next().unwrap(),
            residual_info: it.next().unwrap(),
            personal_info: it.next().unwrap(),
        }
    }
}

/// 按证据判定单项状态：Some(true) → Passed（附指针），
/// Some(false) → Failed（附指针），None → Pending（附缺失原因）。
fn status_of(evidence: &Option<EvidenceSource>, missing: &str) -> (CheckStatus, String) {
    match evidence {
        Some(source) if source.ok => (CheckStatus::Passed, format!("证据: {}", source.pointer)),
        Some(source) => (
            CheckStatus::Failed,
            format!("证据未通过: {}", source.pointer),
        ),
        None => (CheckStatus::Pending, format!("证据缺失: {missing}")),
    }
}

/// 等保 2.0 三级检查器.
#[derive(Clone, Copy, Debug, Default)]
pub struct DengbaoChecker;

impl DengbaoChecker {
    /// 创建检查器实例.
    pub fn new() -> Self {
        Self
    }

    /// 执行等保 2.0 三级全部检查项（21-B C3：默认证据驱动）。
    ///
    /// 无任何证据注入 → 全部 `Pending`（干净环境不得出现硬编码 Passed）。
    pub fn check_level3(&self) -> VerificationReport {
        self.check_level3_with_evidence(&DengbaoEvidence::default())
    }

    /// 基于运行时证据执行等保 2.0 三级检查（21-B C3）。
    pub fn check_level3_with_evidence(&self, evidence: &DengbaoEvidence) -> VerificationReport {
        let (s1, e1) = status_of(
            &evidence.physical_access,
            "物理访问控制证据（机房/部署记录）",
        );
        let (s2, e2) = status_of(&evidence.network_architecture, "网络架构审计（TSN/61850）");
        let (s3, e3) = status_of(&evidence.boundary_protection, "边界防护验证（IPC 隔离）");
        let (s4, e4) = status_of(&evidence.compute_security, "计算安全验证（SM2 身份鉴别）");
        let (s5, e5) = status_of(&evidence.management_center, "集中管理运行状态（agent/hmi）");
        let (s6, e6) = status_of(&evidence.comm_integrity, "SM3 摘要校验验证");
        let (s7, e7) = status_of(&evidence.comm_confidentiality, "SM4 加密 + mTLS 验证");
        let (s8, e8) = status_of(&evidence.trusted_boot, "Secure Boot + TPM 度量验证");
        let (s9, e9) = status_of(&evidence.intrusion_prevention, "异常检测器激活状态");
        let (s10, e10) = status_of(&evidence.malware_prevention, "签名 OTA + 无 unsafe 审计");
        let (s11, e11) = status_of(&evidence.security_audit, "审计日志落盘验证");
        let (s12, e12) = status_of(&evidence.data_integrity, "SM3 校验 + A/B 回滚验证");
        let (s13, e13) = status_of(&evidence.data_confidentiality, "SM4 静态加密 + TPM 封存");
        let (s14, e14) = status_of(&evidence.residual_info, "内存零化验证");
        let (s15, e15) = status_of(&evidence.personal_info, "本地处理 + 最小化原则");

        let items: Vec<CheckItem> = alloc::vec![
            CheckItem::new(1, "安全物理环境", "物理安全", s1, &e1, "v0.170.0"),
            CheckItem::new(2, "安全通信网络", "网络安全", s2, &e2, "v0.170.0"),
            CheckItem::new(3, "安全区域边界", "边界防护", s3, &e3, "v0.170.0"),
            CheckItem::new(4, "安全计算环境", "计算安全", s4, &e4, "v0.170.0"),
            CheckItem::new(5, "安全管理中心", "管理安全", s5, &e5, "v0.170.0"),
            CheckItem::new(6, "安全通信完整性", "通信安全", s6, &e6, "v0.170.0"),
            CheckItem::new(7, "安全通信保密性", "通信安全", s7, &e7, "v0.170.0"),
            CheckItem::new(8, "可信验证", "可信计算", s8, &e8, "v0.170.0"),
            CheckItem::new(9, "入侵防范", "主动防御", s9, &e9, "v0.170.0"),
            CheckItem::new(10, "恶意代码防范", "主动防御", s10, &e10, "v0.170.0"),
            CheckItem::new(11, "安全审计", "审计追踪", s11, &e11, "v0.170.0"),
            CheckItem::new(12, "数据完整性", "数据安全", s12, &e12, "v0.170.0"),
            CheckItem::new(13, "数据保密性", "数据安全", s13, &e13, "v0.170.0"),
            CheckItem::new(14, "剩余信息保护", "数据安全", s14, &e14, "v0.170.0"),
            CheckItem::new(15, "个人信息保护", "隐私保护", s15, &e15, "v0.170.0"),
        ];
        VerificationReport::new("等保 2.0 三级验证报告（证据驱动）", items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 静态参考枚举（21-B C3：仅测试可见，生产路径不得使用）。
    ///
    /// 保留 v0.170.0 的 15 项描述作为测试基线；真实判定一律走
    /// `check_level3_with_evidence`。
    #[allow(dead_code)]
    fn check_level3_static() -> VerificationReport {
        let items: Vec<CheckItem> = alloc::vec![
            CheckItem::new(
                1,
                "安全物理环境",
                "物理安全",
                CheckStatus::Passed,
                "嵌入式设备部署于受控机房，满足物理访问控制要求",
                "v0.170.0"
            ),
            CheckItem::new(
                2,
                "安全通信网络",
                "网络安全",
                CheckStatus::Passed,
                "TSN 时间敏感网络 + IEC 61850 协议栈，网络架构安全",
                "v0.170.0"
            ),
            CheckItem::new(
                3,
                "安全区域边界",
                "边界防护",
                CheckStatus::Passed,
                "微内核 IPC 隔离 + 网络分区，区域边界严格划分",
                "v0.170.0"
            ),
            CheckItem::new(
                4,
                "安全计算环境",
                "计算安全",
                CheckStatus::Passed,
                "Rust 内存安全 + 微内核权限隔离，计算环境可信",
                "v0.170.0"
            ),
            CheckItem::new(
                5,
                "安全管理中心",
                "管理安全",
                CheckStatus::Passed,
                "统一 Agent 管理 + HMI 监控面板，集中安全管理",
                "v0.170.0"
            ),
            CheckItem::new(
                6,
                "安全通信完整性",
                "通信安全",
                CheckStatus::Passed,
                "SM3 摘要 + 序列号校验，通信完整性保护",
                "v0.170.0"
            ),
            CheckItem::new(
                7,
                "安全通信保密性",
                "通信安全",
                CheckStatus::Passed,
                "SM4 加密 + mTLS 双向认证，通信保密性保证",
                "v0.170.0"
            ),
            CheckItem::new(
                8,
                "可信验证",
                "可信计算",
                CheckStatus::Passed,
                "TPM 2.0 度量启动 + Secure Boot 链式验证",
                "v0.170.0"
            ),
            CheckItem::new(
                9,
                "入侵防范",
                "主动防御",
                CheckStatus::Passed,
                "IEC 62351 安全协议 + 异常检测 Agent，入侵防范机制",
                "v0.170.0"
            ),
            CheckItem::new(
                10,
                "恶意代码防范",
                "主动防御",
                CheckStatus::Passed,
                "纯 Rust 无 unsafe 核心 + 签名验证 OTA，杜绝恶意代码注入",
                "v0.170.0"
            ),
            CheckItem::new(
                11,
                "安全审计",
                "审计追踪",
                CheckStatus::Passed,
                "eneros-audit 全链路审计日志 + TSDB 持久化存储",
                "v0.170.0"
            ),
            CheckItem::new(
                12,
                "数据完整性",
                "数据安全",
                CheckStatus::Passed,
                "SM3 哈希校验 + A/B 分区回滚，数据完整性保证",
                "v0.170.0"
            ),
            CheckItem::new(
                13,
                "数据保密性",
                "数据安全",
                CheckStatus::Passed,
                "SM4 静态加密 + TPM 密钥封存，数据保密性保护",
                "v0.170.0"
            ),
            CheckItem::new(
                14,
                "剩余信息保护",
                "数据安全",
                CheckStatus::Passed,
                "Rust 所有权自动释放 + 内存零化，剩余信息不可恢复",
                "v0.170.0"
            ),
            CheckItem::new(
                15,
                "个人信息保护",
                "隐私保护",
                CheckStatus::Passed,
                "边缘计算本地处理 + 数据最小化原则，个人信息保护合规",
                "v0.170.0"
            ),
        ];
        VerificationReport::new("等保 2.0 三级验证报告", items)
    }

    #[test]
    fn test_clean_env_no_hardcoded_passed() {
        // C3 判据 1：干净环境（无证据）→ 全部 Pending，不得 Passed。
        let checker = DengbaoChecker::new();
        let report = checker.check_level3();
        assert_eq!(report.total, 15);
        assert_eq!(report.passed_count, 0, "干净环境不得有 Passed");
        assert!(report
            .items
            .iter()
            .all(|i| i.status == CheckStatus::Pending));
        for item in report.items.iter() {
            assert!(
                item.evidence.starts_with("证据缺失:"),
                "Pending 项必须标注缺失原因: {}",
                item.evidence
            );
        }
    }

    #[test]
    fn test_full_evidence_passed_with_pointers() {
        // C3 判据 2：完整证据 → Passed 项带证据指针。
        let checker = DengbaoChecker::new();
        let evidence = DengbaoEvidence {
            physical_access: Some(EvidenceSource::new(
                "configs/dengbao.toml#physical_access",
                true,
            )),
            network_architecture: Some(EvidenceSource::new(
                "configs/dengbao.toml#network_architecture",
                true,
            )),
            boundary_protection: Some(EvidenceSource::new(
                "configs/dengbao.toml#boundary_protection",
                true,
            )),
            compute_security: Some(EvidenceSource::new(
                "configs/dengbao.toml#compute_security",
                true,
            )),
            management_center: Some(EvidenceSource::new(
                "configs/dengbao.toml#management_center",
                true,
            )),
            comm_integrity: Some(EvidenceSource::new(
                "configs/dengbao.toml#comm_integrity",
                true,
            )),
            comm_confidentiality: Some(EvidenceSource::new(
                "configs/dengbao.toml#comm_confidentiality",
                true,
            )),
            trusted_boot: Some(EvidenceSource::new(
                "configs/dengbao.toml#trusted_boot",
                true,
            )),
            intrusion_prevention: Some(EvidenceSource::new(
                "configs/dengbao.toml#intrusion_prevention",
                true,
            )),
            malware_prevention: Some(EvidenceSource::new(
                "configs/dengbao.toml#malware_prevention",
                true,
            )),
            security_audit: Some(EvidenceSource::new(
                "configs/dengbao.toml#security_audit",
                true,
            )),
            data_integrity: Some(EvidenceSource::new(
                "configs/dengbao.toml#data_integrity",
                true,
            )),
            data_confidentiality: Some(EvidenceSource::new(
                "configs/dengbao.toml#data_confidentiality",
                true,
            )),
            residual_info: Some(EvidenceSource::new(
                "configs/dengbao.toml#residual_info",
                true,
            )),
            personal_info: Some(EvidenceSource::new(
                "configs/dengbao.toml#personal_info",
                true,
            )),
        };
        let report = checker.check_level3_with_evidence(&evidence);
        assert_eq!(report.total, 15);
        assert_eq!(report.passed_count, 15);
        for item in report.items.iter() {
            assert_eq!(item.status, CheckStatus::Passed);
            assert!(
                item.evidence.starts_with("证据: "),
                "Passed 必须附带证据指针: {}",
                item.evidence
            );
        }
    }

    #[test]
    fn test_mixed_evidence_statuses() {
        // 混合证据：失败项 Failed、缺失项 Pending、通过项 Passed。
        let checker = DengbaoChecker::new();
        let evidence = DengbaoEvidence {
            compute_security: Some(EvidenceSource::new("runtime:sm2_sign_available", false)),
            security_audit: Some(EvidenceSource::new("log:audit_chain", true)),
            ..DengbaoEvidence::default()
        };
        let report = checker.check_level3_with_evidence(&evidence);
        let item4 = report.items.iter().find(|i| i.id == 4).unwrap();
        assert_eq!(item4.status, CheckStatus::Failed);
        assert!(item4.evidence.contains("未通过"));
        let item11 = report.items.iter().find(|i| i.id == 11).unwrap();
        assert_eq!(item11.status, CheckStatus::Passed);
        let item1 = report.items.iter().find(|i| i.id == 1).unwrap();
        assert_eq!(item1.status, CheckStatus::Pending);
        assert_eq!(report.failed_count, 1);
        assert_eq!(report.passed_count, 1);
        assert_eq!(report.total, 15);
    }

    #[test]
    fn test_dengbao_sequential_ids() {
        let checker = DengbaoChecker::new();
        let report = checker.check_level3();
        for (idx, item) in report.items.iter().enumerate() {
            assert_eq!(item.id, (idx + 1) as u32);
        }
    }

    #[test]
    fn test_from_array_roundtrip() {
        let sources =
            core::array::from_fn(|i| Some(EvidenceSource::new(format!("evidence-{i}"), true)));
        let evidence = DengbaoEvidence::from_array(sources);
        assert!(evidence.physical_access.is_some());
        assert!(evidence.personal_info.is_some());
        assert!(evidence.compute_security.is_some());
    }
}
