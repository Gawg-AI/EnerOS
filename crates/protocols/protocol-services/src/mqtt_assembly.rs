//! C9（2026-09-24 架构走查收口）：svc-mqtt 三臂 SECURITY-CHECK 的**判定
//! 逻辑与观测词形**单源——原全部住在 bin 的 `cfg(target_os = "none")`
//! 区（分支接线、X6 墙钟诚实打印、材料缺失 fail-closed 词形只在 QEMU
//! 可见，host 结构性不可测）。本模块纯函数收编：
//!
//! - 臂选择（qemu-test-synth 优先，与 bin 原 cfg 臂互斥序一致）；
//! - 判定（`assembly_allows` 单源调用不变；材料在位性）；
//! - 观测行词形（**逐字节冻结**——QEMU 门按词形 grep，任何变化 = 验收
//!   契约变化，须与 tools/ 判据同步）；
//! - 待执行动作（注入材料 / 进能力壳）。
//!
//! bin 的 cfg(none) 区只剩：取材料（feature 门控）、调 gate、逐行打印、
//! 注入/拨号——四段 fail-closed `driver_service_main` 收一段。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_protocol_abstract::channel::{
    assembly_allows, AssemblyMode, ChannelPolicy, SecureChannelRegistry,
};
use eneros_protocol_abstract::plugin::SecurityProfile;

/// 服务前缀（本服务安全观测行共用）。
pub const PREFIX: &str = "[svc-mqtt] ";

/// SECURITY-CHECK 裁决：Proceed = 继续装配（打印 lines 后按需注入）；
/// Shell = fail-closed（打印 lines 后进能力壳——调用方不再返回）。
pub enum MqttGateVerdict {
    /// 装配允许。`inject_tls_material` = true 时调用方必须注入插件 TLS
    /// 材料（ReleaseTls 臂；材料本体留在调用方侧取用——本模块不依赖
    /// feature 门控的 eneros-tls；QemuTest 臂恒 false——显式明文）。
    Proceed {
        /// SECURITY-CHECK 观测行（按序逐行打印；词形冻结）。
        lines: Vec<String>,
        inject_tls_material: bool,
    },
    /// 装配拒绝（fail-closed 能力壳）。
    Shell { lines: Vec<String> },
}

/// 三臂安全门（判定 + 词形单源）。
///
/// - `qemu_test_synth` / `mqtt_tls` = feature 探针（`cfg!(feature = …)`
///   ——与 bin 原 cfg 臂同一互斥优先序：qemu-test 优先）；
/// - `policy` = 装配策略（生产 bin 传 [`ChannelPolicy::default_dev`]——
///   HEAD 语义；参数化保留 REJECTED 臂的可测性）；
/// - `material_present` = TLS 嵌入材料在位性（bin 侧经 feature 门控的
///   `tls_material::embedded_material(wall_epoch).is_some()` 取得；非
///   mqtt-tls 构建恒 false。材料本体由调用方在 Proceed 时自行取用注入
///   ——`embedded_material` 是种子派生的纯函数，探测与注入两次调用
///   逐字节等值）。
pub fn security_gate(
    qemu_test_synth: bool,
    mqtt_tls: bool,
    wall_epoch: u64,
    profile: SecurityProfile,
    plugin_id: &'static str,
    policy: &ChannelPolicy,
    material_present: bool,
) -> MqttGateVerdict {
    if qemu_test_synth {
        // 46.5/47：AssemblyMode::QemuTest 显式明文（InsecureStream 永不
        // 冒充）。registry 查询与 HEAD 同型（新注册表 + 插件 id）。
        let registry = SecureChannelRegistry::new();
        let has = registry.has(plugin_id);
        let ok = assembly_allows(AssemblyMode::QemuTest, profile, has, policy);
        let head = alloc::format!(
            "{PREFIX}SECURITY-CHECK mode=QemuTest profile={profile:?} factory={has} assembly={}",
            if ok {
                "allowed（显式明文）"
            } else {
                "REJECTED"
            }
        );
        if ok {
            return MqttGateVerdict::Proceed {
                lines: Vec::from([head]),
                inject_tls_material: false,
            };
        }
        return MqttGateVerdict::Shell {
            lines: Vec::from([
                head,
                String::from("[svc-mqtt] policy/实现不一致 —— capability 壳（fail-closed）"),
            ]),
        };
    }
    if mqtt_tls {
        // 70.5（WP-70.5.6）：ReleaseTls——材料在位 → 注入 + 真实 TLS 1.3
        // 握手；缺失 → fail-closed 能力壳（不降级明文）。
        return if material_present {
            let mut lines = Vec::new();
            if wall_epoch == 0 {
                // X6 墙钟缺失：证书有效期以 0 为基准 → 全部
                // CertNotYetValid，握手必败（fail-closed 方向正确）——
                // 诚实打印，不让下一行 "allowed" 误导观测（70.5 复查）。
                lines.push(String::from(
                        "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: X6 wall epoch missing (0) —— TLS 握手将全部失败（证书 not-yet-valid）直至时间可得",
                    ));
            }
            lines.push(String::from(
                    "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls profile=Tls13 factory=true assembly=allowed（材料嵌入在位，握手时验证）",
                ));
            MqttGateVerdict::Proceed {
                lines,
                inject_tls_material: true,
            }
        } else {
            MqttGateVerdict::Shell {
                    lines: Vec::from([String::from(
                        "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: 材料缺失 —— capability 壳（fail-closed，不降级明文）",
                    )]),
                }
        };
    }
    // 无 mqtt-tls 的 release 构建 = 装配面缺 TLS 支持——fail-closed
    //（能力壳；70.5 起明文 broker 不允许出现在 release 闭包）。
    MqttGateVerdict::Shell {
        lines: Vec::from([String::from(
            "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: TLS 支持未编译 —— capability 壳（fail-closed）",
        )]),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_protocol_abstract::channel::ChannelPolicy;

    use super::*;

    /// 词形自检：SecurityProfile Debug 形态冻结（观测行内插值依赖它——
    /// derive 变更会静默改词形，此处显式钉住）。
    #[test]
    fn security_profile_debug_shape_frozen() {
        assert_eq!(alloc::format!("{:?}", SecurityProfile::Tls13), "Tls13");
        assert_eq!(alloc::format!("{:?}", SecurityProfile::None), "None");
    }

    /// 臂 1（qemu-test-synth）+ default_dev：恒放行（allow_plain=true）；
    /// 观测行与 HEAD bin 字面逐字节一致（含 factory 值插值）。
    #[test]
    fn arm1_qemu_test_allowed_word_form() {
        let v = security_gate(
            true,
            false,
            1_758_000_000,
            SecurityProfile::Tls13,
            "protocol-mqtt",
            &ChannelPolicy::default_dev(),
            false,
        );
        match v {
            MqttGateVerdict::Proceed {
                lines,
                inject_tls_material,
            } => {
                assert_eq!(lines.len(), 1);
                assert_eq!(
                    lines[0],
                    "[svc-mqtt] SECURITY-CHECK mode=QemuTest profile=Tls13 factory=false assembly=allowed（显式明文）"
                );
                assert!(!inject_tls_material, "QemuTest 臂显式明文，无材料注入");
            }
            _ => panic!("arm1 default_dev 必须放行"),
        }
    }

    /// 臂 1 拒绝臂（策略禁明文且无工厂）→ Shell，词形两行（头行 REJECTED
    /// + 壳说明）。
    #[test]
    fn arm1_policy_mismatch_shell() {
        let v = security_gate(
            true,
            false,
            1_758_000_000,
            SecurityProfile::Tls13,
            "protocol-mqtt",
            &ChannelPolicy::default_release(),
            false,
        );
        match v {
            MqttGateVerdict::Shell { lines } => {
                assert_eq!(lines.len(), 2);
                assert_eq!(
                    lines[0],
                    "[svc-mqtt] SECURITY-CHECK mode=QemuTest profile=Tls13 factory=false assembly=REJECTED"
                );
                assert_eq!(
                    lines[1],
                    "[svc-mqtt] policy/实现不一致 —— capability 壳（fail-closed）"
                );
            }
            _ => panic!("禁明文策略 + 无工厂必须 Shell"),
        }
    }

    /// 臂 2 材料在位 + X6 墙钟缺失（wall_epoch=0）→ Proceed 注入材料，
    /// 两行（诚实打印在前、allowed 在后——顺序冻结）。
    #[test]
    fn arm2_tls_allowed_with_epoch_zero_honest_line() {
        let v = security_gate(
            false,
            true,
            0,
            SecurityProfile::Tls13,
            "protocol-mqtt",
            &ChannelPolicy::default_dev(),
            true,
        );
        match v {
            MqttGateVerdict::Proceed {
                lines,
                inject_tls_material,
            } => {
                assert_eq!(lines.len(), 2);
                assert_eq!(
                    lines[0],
                    "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: X6 wall epoch missing (0) —— TLS 握手将全部失败（证书 not-yet-valid）直至时间可得"
                );
                assert_eq!(
                    lines[1],
                    "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls profile=Tls13 factory=true assembly=allowed（材料嵌入在位，握手时验证）"
                );
                assert!(inject_tls_material, "调用方必须注入插件 TLS 材料");
            }
            _ => panic!("材料在位必须 Proceed"),
        }
    }

    /// 臂 2 材料在位 + 墙钟正常 → 单行 allowed。
    #[test]
    fn arm2_tls_allowed_normal_epoch_single_line() {
        let v = security_gate(
            false,
            true,
            1_758_000_000,
            SecurityProfile::Tls13,
            "protocol-mqtt",
            &ChannelPolicy::default_dev(),
            true,
        );
        match v {
            MqttGateVerdict::Proceed { lines, .. } => {
                assert_eq!(lines.len(), 1);
                assert_eq!(
                    lines[0],
                    "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls profile=Tls13 factory=true assembly=allowed（材料嵌入在位，握手时验证）"
                );
            }
            _ => panic!("材料在位必须 Proceed"),
        }
    }

    /// 臂 2 材料缺失 → Shell（fail-closed，不降级明文）。
    #[test]
    fn arm2_material_missing_shell() {
        let v = security_gate(
            false,
            true,
            1_758_000_000,
            SecurityProfile::Tls13,
            "protocol-mqtt",
            &ChannelPolicy::default_dev(),
            false,
        );
        match v {
            MqttGateVerdict::Shell { lines } => {
                assert_eq!(lines.len(), 1);
                assert_eq!(
                    lines[0],
                    "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: 材料缺失 —— capability 壳（fail-closed，不降级明文）"
                );
            }
            _ => panic!("材料缺失必须 Shell"),
        }
    }

    /// 臂 3（两 feature 皆无的 release 构建）→ Shell。
    #[test]
    fn arm3_tls_unsupported_shell() {
        let v = security_gate(
            false,
            false,
            1_758_000_000,
            SecurityProfile::Tls13,
            "protocol-mqtt",
            &ChannelPolicy::default_dev(),
            false,
        );
        match v {
            MqttGateVerdict::Shell { lines } => {
                assert_eq!(lines.len(), 1);
                assert_eq!(
                    lines[0],
                    "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: TLS 支持未编译 —— capability 壳（fail-closed）"
                );
            }
            _ => panic!("无 TLS 支持必须 Shell"),
        }
    }

    /// 互斥优先序：qemu-test 优先于 mqtt-tls（与 bin 原 cfg 臂序一致——
    /// 两 feature 同enable 时 qemu-test 臂胜）。
    #[test]
    fn arm_priority_qemu_test_wins() {
        let v = security_gate(
            true,
            true,
            0,
            SecurityProfile::Tls13,
            "protocol-mqtt",
            &ChannelPolicy::default_dev(),
            true,
        );
        match v {
            MqttGateVerdict::Proceed { lines, .. } => {
                assert!(lines[0].contains("mode=QemuTest"), "qemu-test 臂优先");
            }
            _ => panic!("qemu-test + default_dev 必须放行"),
        }
    }
}
