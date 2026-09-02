//! root-task 服务装配接线表（蓝图 62 卷 62.2 WP-62.2.1，S1 + D-1）。
//!
//! 单一事实源：每个服务的全部装配属性（心跳 / 网络槽 / 栈覆盖 / 编排驱动）
//! 只在本表登记一次；`spawn.rs`（网络槽 + 心跳授予）、`main.rs`（栈页
//! 覆盖）、`service.rs`（协议 EXECUTE 驱动循环）四处此前各自持有按名
//! 表/内联名单（S1：四张按名表），现全部从本表推导。
//!
//! D-1（第六轮体检坐实）：`svc-iec104` 有完整 `OP_EXECUTE →
//! collect_and_report` 采集路径且已分配网络槽 9，但旧驱动内联名单只有
//! modbus-tcp/mqtt 两项——默认编排下除启动自检一轮外永不采集上报。本表以 `driven` 登记收口：**凡分配网络槽的协议服务必须
//! 显式决定驱动与否**（结构断言强制，见本模块测试）。
//!
//! host 可测：本模块不依赖 seL4 FFI，全部断言在 `cargo test -p
//! eneros-root-task` 运行。

/// 编排驱动登记：`tick % period_ticks == offset_ticks` 时由 root 主循环
/// 发 `OP_EXECUTE`（47 WP-47.1 信封语义）。
pub struct DrivenSchedule {
    pub period_ticks: u64,
    pub offset_ticks: u64,
}

pub struct ServiceWiring {
    pub name: &'static str,
    /// 心跳监控（WP-40.4 隔离面）：授予 `HEARTBEAT_NTFN_SLOT` + root 侧 poll。
    pub heartbeat: bool,
    /// virtio-net MMIO 槽（46.2；None = 无 NIC 面，如串口/纯服务）。
    pub net_slot: Option<u64>,
    /// 栈页覆盖（1 页 = 4 KiB；None = 默认 16 KiB/4 页。SPEC-E：svc-telemetry
    /// TSDB 磁盘写链栈峰值 ~30 KiB，16 页 = 64 KiB ≥2× 余量）。
    pub stack_pages: Option<u64>,
    /// 编排驱动登记（Some = 周期驱动；None + [`ServiceWiring::
    /// driven_none_reason`] = 显式不驱动 + 理由）。
    pub driven: Option<DrivenSchedule>,
    /// `driven: None` 的显式理由——对有网络槽的协议服务为必填（结构断言
    /// (a)），表达"接了线但决定不驱动"是有意识的装配决策，不是漏登。
    pub driven_none_reason: Option<&'static str>,
}

/// 服务装配接线表。
///
/// 表序即驱动编排序：同相位（`offset_ticks` 相同）的服务按表序依次 EXECUTE
/// ——47 WP-47.1 的"先 modbus-tcp ACQ 保鲜（意图授权前遥测新鲜），后
/// mqtt 下行写命令轮询 → 意图转发 dispatch；同轮顺序使 intent 的 SNAPSHOT
/// 与 ACQ 相隔毫秒"编排语义由本表序承载，改动表序即改动编排，须过 QEMU 门。
pub const SERVICE_WIRING: &[ServiceWiring] = &[
    // —— 协议服务（47 编排：modbus-tcp → mqtt 同相位；iec104 错峰）——
    ServiceWiring {
        name: "svc-modbus-tcp",
        heartbeat: false,
        net_slot: Some(8),
        stack_pages: None,
        driven: Some(DrivenSchedule {
            period_ticks: 15,
            offset_ticks: 10,
        }),
        driven_none_reason: None,
    },
    ServiceWiring {
        name: "svc-mqtt",
        heartbeat: false,
        net_slot: Some(10),
        stack_pages: None,
        driven: Some(DrivenSchedule {
            period_ticks: 15,
            offset_ticks: 10,
        }),
        driven_none_reason: None,
    },
    // D-1 修复：svc-iec104 采集轮接入默认编排。周期 15 与 modbus-tcp/mqtt
    // 同（`PROTO_EXECUTE_PERIOD` 既有值 15 迁入表，值不变）；offset 取 5
    // ——与 10 错峰（避免三采集服务同 tick IPC 突发），且 5/10 同落
    // CONTROL_CHAIN_PERIOD=5 相位集合，保持"ACQ 保鲜→链回合"的编排关系。
    ServiceWiring {
        name: "svc-iec104",
        heartbeat: false,
        net_slot: Some(9),
        stack_pages: None,
        driven: Some(DrivenSchedule {
            period_ticks: 15,
            offset_ticks: 5,
        }),
        driven_none_reason: None,
    },
    ServiceWiring {
        name: "svc-mms",
        heartbeat: false,
        net_slot: Some(11),
        stack_pages: None,
        driven: None,
        driven_none_reason: Some(
            "MMS 走 protocol_ladder 通用服务面，无自定义 OP_EXECUTE 采集路径；驱动接线待后续卷裁决",
        ),
    },
    ServiceWiring {
        name: "svc-goose",
        heartbeat: false,
        net_slot: Some(1),
        stack_pages: None,
        driven: None,
        driven_none_reason: Some(
            "46.5 平台限制：GOOSE 收归既有 G3 通道（bus.1 socket 5577），无独立 EXECUTE 采集轮",
        ),
    },
    ServiceWiring {
        name: "svc-sv",
        heartbeat: false,
        net_slot: Some(13),
        stack_pages: None,
        driven: None,
        driven_none_reason: Some("SV 采样值流为被动订阅面，无轮询采集轮"),
    },
    ServiceWiring {
        name: "svc-modbus-rtu",
        heartbeat: false,
        net_slot: None,
        stack_pages: None,
        driven: None,
        driven_none_reason: Some(
            "46.1 决策 b：QEMU 无第二串口——Unassigned 壳服务（SPEC-D UART X7 移交窗口按需驱动）",
        ),
    },
    // —— 平台服务（WP-40.4 心跳监控面）——
    ServiceWiring {
        name: "svc-telemetry",
        heartbeat: true,
        net_slot: None,
        stack_pages: Some(16),
        driven: None,
        driven_none_reason: None,
    },
    ServiceWiring {
        name: "svc-energy-agent",
        heartbeat: true,
        net_slot: None,
        stack_pages: None,
        driven: None,
        driven_none_reason: None,
    },
    ServiceWiring {
        name: "svc-ai-worker",
        heartbeat: true,
        net_slot: None,
        stack_pages: None,
        driven: None,
        driven_none_reason: None,
    },
    ServiceWiring {
        name: "svc-control-dispatch",
        heartbeat: true,
        net_slot: None,
        stack_pages: None,
        driven: None,
        driven_none_reason: None,
    },
    ServiceWiring {
        name: "svc-device-agent",
        heartbeat: true,
        net_slot: None,
        stack_pages: None,
        driven: None,
        driven_none_reason: None,
    },
    ServiceWiring {
        name: "svc-hang",
        heartbeat: true,
        net_slot: None,
        stack_pages: None,
        driven: None,
        driven_none_reason: None,
    },
];

/// 按名查登记项（服务名 = bundle 模块名 = spawn 计划名，同一命名空间）。
pub fn wiring_of(name: &str) -> Option<&'static ServiceWiring> {
    SERVICE_WIRING.iter().find(|w| w.name == name)
}

/// virtio-net MMIO 槽（spawn.rs 46.2 资源占用面；自本表推导）。
pub fn proto_net_slot(name: &str) -> Option<u64> {
    wiring_of(name).and_then(|w| w.net_slot)
}

/// 心跳监控在册判定（spawn 心跳授予面；自本表推导）。
pub fn is_heartbeat(name: &str) -> bool {
    wiring_of(name).is_some_and(|w| w.heartbeat)
}

/// 栈页覆盖（main.rs spawn 计划面；自本表推导）。
pub fn stack_pages_override(name: &str) -> Option<u64> {
    wiring_of(name).and_then(|w| w.stack_pages)
}

// ============================================================================
// 结构断言（WP-62.2.1 红测；host 单测）
// ============================================================================

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    /// (a) 完备性：凡登记网络槽的服务必须显式决定驱动与否（Some(driven)
    /// 或显式 `driven_none_reason`）。**D-1 红点**：表初落时 svc-iec104
    /// 有槽 9 而无驱动登记 → 本测试红；登记 `driven` 后转绿。
    #[test]
    fn net_slotted_services_have_explicit_drive_decision() {
        for w in SERVICE_WIRING {
            if w.net_slot.is_some() {
                assert!(
                    w.driven.is_some() || w.driven_none_reason.is_some(),
                    "{} 有网络槽（{:?}）但既无 driven 登记也无显式不驱动理由——装配决策缺失（WP-62.2.1 断言 a）",
                    w.name,
                    w.net_slot
                );
            }
        }
    }

    /// D-1 修复锁：svc-iec104 有完整 `OP_EXECUTE → collect_and_report`
    /// 采集路径（svc-iec104.rs handle 臂），必须被默认编排周期驱动。
    #[test]
    fn svc_iec104_acq_round_is_driven() {
        let w = wiring_of("svc-iec104").expect("svc-iec104 必须登记");
        let d = w
            .driven
            .as_ref()
            .expect("svc-iec104 必须登记 driven（D-1：否则默认编排下永不采集）");
        assert!(d.period_ticks > 0, "驱动周期必须为正");
        assert!(
            d.offset_ticks < d.period_ticks,
            "驱动 offset 必须落在周期相位内"
        );
    }

    /// (b) 唯一性：表内 net_slot 互异（槽冲突即装配失败——原
    /// proto_net_slot 单测语义随表收编）。
    #[test]
    fn wiring_net_slots_unique() {
        let mut slots: Vec<u64> = SERVICE_WIRING.iter().filter_map(|w| w.net_slot).collect();
        slots.sort_unstable();
        let len = slots.len();
        slots.dedup();
        assert_eq!(slots.len(), len, "网络槽冲突：{slots:?} 存在重复——装配失败");
    }

    /// 心跳集合锁：表中 heartbeat 项必须与 WP-40.4 既有 6 服务名单一致
    /// （防漏登/多登——卷步骤 4）。
    #[test]
    fn heartbeat_registry_matches_wp404_baseline() {
        let mut got: Vec<&str> = SERVICE_WIRING
            .iter()
            .filter(|w| w.heartbeat)
            .map(|w| w.name)
            .collect();
        got.sort_unstable();
        assert_eq!(
            got,
            vec![
                "svc-ai-worker",
                "svc-control-dispatch",
                "svc-device-agent",
                "svc-energy-agent",
                "svc-hang",
                "svc-telemetry"
            ],
            "心跳监控名单漂移——与 WP-40.4 基线不一致"
        );
    }

    /// 47 编排语义锁：同相位（offset 相同）时表序保证 modbus-tcp 先于
    /// mqtt（ACQ 保鲜先于下行轮询）；iec104 与二者错峰（不同 offset）。
    #[test]
    fn driven_phase_ordering_preserves_wp471_choreography() {
        let sched = |name: &str| {
            wiring_of(name)
                .and_then(|w| w.driven.as_ref())
                .expect("协议驱动服务必须登记 driven")
        };
        let (mb, mqtt, iec) = (
            sched("svc-modbus-tcp"),
            sched("svc-mqtt"),
            sched("svc-iec104"),
        );
        assert_eq!(
            (mb.period_ticks, mb.offset_ticks),
            (mqtt.period_ticks, mqtt.offset_ticks),
            "modbus-tcp/mqtt 同相位（47 编排：同轮相隔毫秒）"
        );
        assert_ne!(
            iec.offset_ticks, mb.offset_ticks,
            "iec104 必须与 modbus-tcp 错峰（避免同 tick 采集 IPC 突发）"
        );
        let mb_idx = SERVICE_WIRING
            .iter()
            .position(|w| w.name == "svc-modbus-tcp");
        let mqtt_idx = SERVICE_WIRING.iter().position(|w| w.name == "svc-mqtt");
        assert!(
            mb_idx < mqtt_idx,
            "表序承载 47 编排：modbus-tcp 必须先于 mqtt"
        );
    }

    /// 驱动绝对基线锁（62 复查补充）：表数据化后相对断言（同相位/错峰）
    /// 锁不住绝对值漂移——若 15/10/5 被误改（如 16/14/6），相对测试全绿
    /// 只剩 QEMU 门能抓。绝对基线 = WP-47.1 既有 `PROTO_EXECUTE_PERIOD`=15
    /// + offset 10（modbus/mqtt，值迁表不变）+ D-1 错峰 offset 5（iec104）。
    ///
    /// 改动必须同步 QEMU 串口编排证据。
    #[test]
    fn driven_schedule_absolute_baseline_locked() {
        let sched = |name: &str| {
            wiring_of(name)
                .and_then(|w| w.driven.as_ref())
                .expect("协议驱动服务必须登记 driven")
        };
        let (mb, mqtt, iec) = (
            sched("svc-modbus-tcp"),
            sched("svc-mqtt"),
            sched("svc-iec104"),
        );
        assert_eq!(
            (mb.period_ticks, mb.offset_ticks),
            (15, 10),
            "svc-modbus-tcp 驱动基线漂移（WP-47.1 PROTO_EXECUTE_PERIOD=15/offset=10）"
        );
        assert_eq!(
            (mqtt.period_ticks, mqtt.offset_ticks),
            (15, 10),
            "svc-mqtt 驱动基线漂移（与 modbus-tcp 同相位）"
        );
        assert_eq!(
            (iec.period_ticks, iec.offset_ticks),
            (15, 5),
            "svc-iec104 驱动基线漂移（D-1 错峰 offset=5）"
        );
    }
}
