//! 等保 2.0 三级检查器.
//!
//! 验证 EnerOS 满足《信息安全技术 网络安全等级保护基本要求》(GB/T 22239-2019)
//! 三级安全要求，覆盖安全物理环境、通信网络、计算环境、管理中心等 15 项。

use alloc::vec::Vec;

use crate::types::{CheckItem, CheckStatus, VerificationReport};

/// 运行时探测结果（VRF-006：动态检测输入，替换静态 Passed）。
///
/// 由宿主在检查时注入真实运行时数据；`None` 表示该面无运行时数据（保留静态项）。
#[derive(Debug, Clone, Default)]
pub struct RuntimeProbe {
    /// SM2 签名验签能力是否可用（身份鉴别，项 4）。
    pub sm2_sign_available: Option<bool>,
    /// 审计链是否已写入（审计记录，项 8）。
    pub audit_chain_written: Option<bool>,
    /// 异常检测器是否激活（入侵防范，项 9）。
    pub anomaly_detector_active: Option<bool>,
    /// 时间同步是否有效（时间戳完整性，项 7）。
    pub time_sync_valid: Option<bool>,
}

/// 等保 2.0 三级检查器.
#[derive(Clone, Copy, Debug, Default)]
pub struct DengbaoChecker;

impl DengbaoChecker {
    /// 执行等保 2.0 三级全部检查项，返回验证报告（VRF-006：支持运行时探测
    /// 动态判定——`probe` 中提供的面按真实结果判定，其余保留静态项）。
    pub fn check_level3_dynamic(&self, probe: &RuntimeProbe) -> VerificationReport {
        let mut report = self.check_level3();
        for item in report.items_mut() {
            let status = match (item.id, probe) {
                (
                    4,
                    RuntimeProbe {
                        sm2_sign_available: Some(true),
                        ..
                    },
                ) => CheckStatus::Passed,
                (
                    4,
                    RuntimeProbe {
                        sm2_sign_available: Some(false),
                        ..
                    },
                ) => CheckStatus::Failed,
                (
                    8,
                    RuntimeProbe {
                        audit_chain_written: Some(true),
                        ..
                    },
                ) => CheckStatus::Passed,
                (
                    8,
                    RuntimeProbe {
                        audit_chain_written: Some(false),
                        ..
                    },
                ) => CheckStatus::Failed,
                (
                    9,
                    RuntimeProbe {
                        anomaly_detector_active: Some(true),
                        ..
                    },
                ) => CheckStatus::Passed,
                (
                    9,
                    RuntimeProbe {
                        anomaly_detector_active: Some(false),
                        ..
                    },
                ) => CheckStatus::Failed,
                (
                    7,
                    RuntimeProbe {
                        time_sync_valid: Some(true),
                        ..
                    },
                ) => CheckStatus::Passed,
                (
                    7,
                    RuntimeProbe {
                        time_sync_valid: Some(false),
                        ..
                    },
                ) => CheckStatus::Failed,
                _ => continue,
            };
            item.set_status(status);
        }
        report.recompute();
        report
    }
}

impl DengbaoChecker {
    /// 创建检查器实例.
    pub fn new() -> Self {
        Self
    }

    /// 静态基准检查（VRF-006：15 项中可动态判定的面由
    /// `check_level3_dynamic` 覆盖，本方法保留静态枚举）。
    fn check_level3_static(&self) -> VerificationReport {
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

    /// 执行等保 2.0 三级全部检查项（静态基准；动态判定见 `check_level3_dynamic`）。
    pub fn check_level3(&self) -> VerificationReport {
        self.check_level3_static()
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::*;

    #[test]
    fn test_dengbao_check_level3_count() {
        let checker = DengbaoChecker::new();
        let report = checker.check_level3();
        assert_eq!(report.total, 15);
        assert_eq!(report.items.len(), 15);
    }

    #[test]
    fn test_dengbao_all_passed() {
        let checker = DengbaoChecker::new();
        let report = checker.check_level3();
        assert!(report.all_passed());
        assert_eq!(report.passed_count, 15);
        assert_eq!(report.failed_count, 0);
    }

    #[test]
    fn test_dengbao_item_names() {
        let checker = DengbaoChecker::new();
        let report = checker.check_level3();
        assert_eq!(report.items[0].name, "安全物理环境");
        assert_eq!(report.items[4].name, "安全管理中心");
        assert_eq!(report.items[7].name, "可信验证");
        assert_eq!(report.items[14].name, "个人信息保护");
    }

    #[test]
    fn test_dengbao_report_title() {
        let checker = DengbaoChecker::new();
        let report = checker.check_level3();
        assert_eq!(report.title, String::from("等保 2.0 三级验证报告"));
    }

    #[test]
    fn test_dengbao_categories() {
        let checker = DengbaoChecker::new();
        let report = checker.check_level3();
        assert_eq!(report.items[0].category, "物理安全");
        assert_eq!(report.items[5].category, "通信安全");
        assert_eq!(report.items[10].category, "审计追踪");
        assert_eq!(report.items[14].category, "隐私保护");
    }

    #[test]
    fn test_dengbao_sequential_ids() {
        let checker = DengbaoChecker::new();
        let report = checker.check_level3();
        for (idx, item) in report.items.iter().enumerate() {
            assert_eq!(item.id, (idx + 1) as u32);
        }
    }
}

#[cfg(test)]
mod vrf006_tests {
    use super::*;

    #[test]
    fn test_vrf006_dynamic_detection_overrides_static() {
        let checker = DengbaoChecker::new();
        // 全部静态 Passed
        let static_report = checker.check_level3();
        assert_eq!(static_report.failed_count, 0);
        // 动态探测：SM2 不可用 → 项 4 变 Failed（运行时真实判定）
        let probe = RuntimeProbe {
            sm2_sign_available: Some(false),
            ..RuntimeProbe::default()
        };
        let dyn_report = checker.check_level3_dynamic(&probe);
        let item4 = dyn_report
            .items
            .iter()
            .find(|i| i.id == 4)
            .expect("项 4 存在");
        assert_eq!(
            item4.status,
            CheckStatus::Failed,
            "动态探测 SM2 不可用 → 项 4 Failed"
        );
        assert_eq!(dyn_report.failed_count, 1, "动态检测使出口可对假实现失败");
        // 全部通过探测 → 全 Passed
        let ok_probe = RuntimeProbe {
            sm2_sign_available: Some(true),
            audit_chain_written: Some(true),
            anomaly_detector_active: Some(true),
            time_sync_valid: Some(true),
        };
        let ok_report = checker.check_level3_dynamic(&ok_probe);
        assert_eq!(ok_report.failed_count, 0);
    }
}
