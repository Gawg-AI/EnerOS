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

impl DrivenSchedule {
    /// 到期判定（唯一实现——主循环零手写周期算式，63.2 WP-63.2.2）。
    pub fn is_due(&self, tick: u64) -> bool {
        tick % self.period_ticks == self.offset_ticks
    }
}

pub struct ServiceWiring {
    pub name: &'static str,
    /// 心跳监控（WP-40.4 隔离面）：授予 `HEARTBEAT_NTFN_SLOT` + root 侧 poll。
    pub heartbeat: bool,
    /// virtio-net MMIO 槽（46.2；None = 无 NIC 面，如串口/纯服务）。
    pub net_slot: Option<u64>,
    /// 栈页覆盖（1 页 = 4 KiB；None = 默认 32 KiB/8 页——蓝图 68.1 起默认
    /// 翻倍，见 main.rs STACK_SIZE 注记。SPEC-E：svc-telemetry TSDB 磁盘写
    /// 链栈峰值 ~30 KiB，显式 16 页 = 64 KiB ≥2× 余量；68.3 持久 mount 后
    /// littlefs Allocation（~8.5KB cache）移栈入堆、写链栈峰值应降，但
    /// **不回落**（SPEC-E win1 栈溢出挂死前科——余量纪律优先）。
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
        // 68.3（WP-68.3.1 次生防护）：PAYLOAD_CAP 64→461 顶格扩容后
        // driver_service_main 栈帧 +~4.9KB（BoundedQueue 8×484B=3.9KB 在
        // 栈上 + raw/mrs/resp 数组），svc-mqtt 为最重 drv 服务（net +
        // plugin + relay 缓冲），32 KiB 默认栈在 QEMU 实弹（vol68-3 首轮
        // 窗 A）溢出 → loop 未进入即死 + root 阻塞 EXECUTE Call ⇒ 全系统
        // 静默冻结（68.1 已知失效形态复发）。16 页 = 64 KiB ≥2× 余量，
        // 同 svc-telemetry/SPEC-E 裕度哲学。
        stack_pages: Some(16),
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

/// 心跳缺失容忍轮次缺省值（`hello` ≈ 760 轮/s ⇒ 150 轮 ≈ 0.2s 真实）。
///
/// 语义（蓝图 57 WP-57.3 HangMonitor）：服务**处理任何一条消息前**先
/// signal 心跳 —— 该窗口度量的是"一条消息的处理时长"，不是调度延迟。
pub const HEARTBEAT_MISS_LIMIT_DEFAULT: i64 = 150;

/// 按服务校准的心跳缺失容忍轮次（蓝图 70.15 实弹定位的看门狗误杀面）。
///
/// **缺陷形态（2026-09-14 `VOL70.5-GATE` 窗实测）**：`svc-telemetry` 被
/// root 的 HangMonitor 连续 kill+restart（窗内 5 次：`HEARTBEAT-TIMEOUT:
/// svc-telemetry no progress 150 rounds since probe … — kill+restart`）。
/// 每次重启都重置事件面 ⇒ `SNAPSHOT-INPUT-REJECT reason=no-frozen-snapshot`
/// ×2978（且**在首个 `SNAP-INPUT-BOUND` 之后仍有 2223 次**，故非冷启动
/// 竞态）、EA 恒 `NOACTION` ⇒ 零 `dispatch=Committed` ⇒ 方向 1–5 的控制链
/// 在实弹窗里**整条停摆**。
///
/// **根因**：`svc-telemetry` 的**单条消息内**包含同步块 I/O（OBS 事件面
/// 追加 + 快照落盘 + TSDB 提交，`EVENT-STORE append` / `TSDB-DISK-COMMIT`
/// 逐批），其耗时受介质与模拟速度支配，远超缺省窗（0.2s 真实）；而心跳
/// 只在消息之间 signal，故"处理慢"被记成"卡死"。这与蓝图 67 的
/// 登记同族（"心跳 = 消息驱动进展，上游静默误判卡死"）。
///
/// **处置**：按服务的**最坏单条处理时长**校准容忍窗——不是取消看门狗
/// （真卡死仍在秒级内被发现），也不是改判据。IO 密集服务给 20× 缺省窗
/// （≈4s 真实，TCG 下数十秒），其余服务维持缺省。
pub fn heartbeat_miss_limit(name: &str) -> i64 {
    match name {
        // 事件面/快照面/TSDB 三条块 I/O 都在单条 OBS 批处理内完成。
        "svc-telemetry" => 3_000,
        // 其余在册服务：单条消息为纯计算 + 短 IPC（无同步块 I/O）。
        _ => HEARTBEAT_MISS_LIMIT_DEFAULT,
    }
}

/// 栈页覆盖（main.rs spawn 计划面；自本表推导）。
pub fn stack_pages_override(name: &str) -> Option<u64> {
    wiring_of(name).and_then(|w| w.stack_pages)
}

/// 协议 EXECUTE 驱动相关性 nonce（原 service.rs 内联
/// `(tick % 0xFFFF) as u32` 平移——`DriverRequest::new(corr, …)` 的
/// correlation 字段；随驱动表归 wiring 单源）。
pub fn execute_nonce(tick: u64) -> u32 {
    (tick % 0xFFFF) as u32
}

// ============================================================================
// root 主循环周期编排表（蓝图 63 卷 63.2 WP-63.2.2）
// ============================================================================

/// root 主循环周期任务（编排知识单源——此前七组周期常量硬编码在
/// run_ipc_loop 体内 `tick % X` / `tick == N` / `tick >= N`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PeriodicTask {
    /// 控制链回合（root 采集代理 INGEST + svc-energy-agent TICK）。
    ControlChain,
    /// 可观测上报（服务健康摘要入队 + FLUSH → MQTT 导出）。
    ObsReport,
    /// hang 注入（tick 200 一次性；svc-hang 仅测试装配 spawn——handle
    /// 缺失即自然 no-op，**非 cfg 门控**，与蓝图处方差异见执行卡）。
    HangInject,
    /// 存储落盘验证探针（tick ≥ 60 首达即触发，一次性）。
    StoragePersist,
    /// OTA download/verify/trial/commit 探针（tick ≥ 70 一次性）。
    OtaDownload,
    /// 周期故障注入（svc-fault ExecStack——supervisor restart 隔离证明）。
    FaultInject,
    /// 分层看门狗状态检查（诊断打印；原 `tick % 100` 块按职责命名）。
    WatchdogCheck,
    /// 蓝图 70.5（WP-70.5.1/70.5.4）：可信身份推送（一次性）——root 读
    /// boot-manager 写入的 IdentityCapsule（BOOT-007 窗口尾页）并经 OTA1
    /// 0x08 分发 svc-ota。capsule 缺失 → 发布装配不推送（OTA not-ready
    /// fail-closed）；qemu-test 装配（qemu-test-synth feature）以显式
    /// SYNTHETIC 标签构造测试身份。
    IdentityPush,
    /// 蓝图 69 卷 69.5（WP-69.5.1）：降级/学习**只读**观测探针——向
    /// svc-ai-worker 连发 5 次 `AI_DEGRADE_OBSERVE(0x22)`，`DEGRADE-OBS`
    /// 串口行由 svc-ai-worker 侧落（root 零新词形）；五连读同刻读数
    /// 逐字节一致 = 观测只读性实弹（Q5）。探针零决策面状态接触。
    DegradeObserve,
    /// 70.6（WP-70.6.5）：MGT1 管理动作链探针（qemu-test-synth 专属；
    /// Send 语义直驱 svc-mgmt，动作实效经串口词形取证）。
    MgtProbe,
    /// F70-03 §6.3.4 + recheck RS2：管理账本持久补写 tick——root 周期
    /// Send `MGT0\0` 帧到 svc-mgmt（生产闭包同跑；服务未 spawn 时自然
    /// no-op）。终态提交失败后的补写重试由此获得**独立、有界的周期
    /// 来源**，不依赖用户再发管理请求或有结束点的测试探针。
    MgmtPersistTick,
}

/// 到期语义（一次性任务首达即到期；fired 防重由分派侧既有 done 标志
/// 持有——表只承载"何时到期"）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScheduleKind {
    /// 周期重复：`tick % period == offset`。
    Repeat,
    /// 单次首达：`tick == period`。
    OnceAt,
    /// 单次首达：`tick >= period`。
    OnceFrom,
}

/// 周期编排表行：period/offset/kind/task 四元组。
pub struct PeriodicEntry {
    pub period: u16,
    pub offset: u16,
    pub kind: ScheduleKind,
    pub task: PeriodicTask,
}

impl PeriodicEntry {
    /// 到期判定（唯一实现——service.rs 零手写周期算式）。
    pub fn is_due(&self, tick: u64) -> bool {
        match self.kind {
            ScheduleKind::Repeat => tick % u64::from(self.period) == u64::from(self.offset),
            ScheduleKind::OnceAt => tick == u64::from(self.period),
            ScheduleKind::OnceFrom => tick >= u64::from(self.period),
        }
    }
}

/// root 主循环周期编排表。
///
/// 值自 service.rs 平移，零数字变化；ObsReport `offset=20` 是既有相位
/// （原 `tick % 25 == 20`），非新引入。分派侧在各周期块原位调
/// [`periodic_due`]——与 EXECUTE 驱动循环 / 心跳监控 / supervisor fault
/// poll（非表块）的既有交错不变，**执行顺序零变化**（QEMU 串口节奏即判据）。
pub const PERIODIC_SCHEDULE: &[PeriodicEntry] = &[
    PeriodicEntry {
        period: 5,
        offset: 0,
        kind: ScheduleKind::Repeat,
        task: PeriodicTask::ControlChain,
    },
    PeriodicEntry {
        period: 90,
        offset: 90,
        kind: ScheduleKind::OnceFrom,
        task: PeriodicTask::MgtProbe,
    },
    PeriodicEntry {
        period: 25,
        offset: 20,
        kind: ScheduleKind::Repeat,
        task: PeriodicTask::ObsReport,
    },
    PeriodicEntry {
        period: 200,
        offset: 200,
        kind: ScheduleKind::OnceAt,
        task: PeriodicTask::HangInject,
    },
    PeriodicEntry {
        period: 60,
        offset: 60,
        kind: ScheduleKind::OnceFrom,
        task: PeriodicTask::StoragePersist,
    },
    PeriodicEntry {
        period: 70,
        offset: 70,
        kind: ScheduleKind::OnceFrom,
        task: PeriodicTask::OtaDownload,
    },
    PeriodicEntry {
        period: 50,
        offset: 0,
        kind: ScheduleKind::Repeat,
        task: PeriodicTask::FaultInject,
    },
    PeriodicEntry {
        period: 100,
        offset: 0,
        kind: ScheduleKind::Repeat,
        task: PeriodicTask::WatchdogCheck,
    },
    // 69.5（WP-69.5.1）：观测探针 period 25 / offset 8——与 ControlChain
    // （5/0）、ObsReport（25/20）错峰；观测轮与决策轮不同 tick，五连读
    // 之间零 decide 交错（Q5 的同刻可比性由此承载）。
    PeriodicEntry {
        period: 25,
        offset: 8,
        kind: ScheduleKind::Repeat,
        task: PeriodicTask::DegradeObserve,
    },
    // 70.5（WP-70.5.1）：身份推送 55/55——先于 StoragePersist(60) 与
    // OtaDownload(70)（探针以身份在册为前提）。
    PeriodicEntry {
        period: 55,
        offset: 55,
        kind: ScheduleKind::OnceFrom,
        task: PeriodicTask::IdentityPush,
    },
    // recheck RS2：管理账本持久补写 25/17——与 DegradeObserve(25/8)、
    // ObsReport(25/20) 错峰；空闲 tick 在 svc-mgmt 侧为零盘写直答。
    PeriodicEntry {
        period: 25,
        offset: 17,
        kind: ScheduleKind::Repeat,
        task: PeriodicTask::MgmtPersistTick,
    },
];

/// 周期任务到期判定（按任务查表；主循环唯一入口）。
pub fn periodic_due(task: PeriodicTask, tick: u64) -> bool {
    PERIODIC_SCHEDULE
        .iter()
        .find(|e| e.task == task)
        .is_some_and(|e| e.is_due(tick))
}

/// 显式不在接线表的服务名单（63.2 WP-63.2.3）：每个被 spawn 的服务必须
/// ∈ [`SERVICE_WIRING`] ∪ 本名单——"不在表"必须是有意识的装配决策且留
/// 理由（完备性测试 [`tests::every_spawned_service_is_wired_or_exempted`]
/// 直读 product/profiles/*.json 双向断言）。**不是杂物抽屉**：每项理由
/// 引用真实装配事实；名单与 profile spawn 清单一致（反方向断言）。
pub const EXPLICIT_UNWIRED: &[(&str, &str)] = &[
    // —— 驱动服务：句柄移交走 spawn.rs 专用面（UART_HANDOVER_TABLE /
    //    存储槽 / virtio-net 驱动装配），不属协议 EXECUTE 编排域 ——
    (
        "svc-drv-rng",
        "熵驱动：句柄移交（spawn 专用面），无 EXECUTE/心跳/网络槽装配语义",
    ),
    (
        "svc-drv-blk",
        "块驱动：句柄移交（spawn 专用面），无 EXECUTE/心跳/网络槽装配语义",
    ),
    (
        "svc-drv-net",
        "网络驱动：句柄移交（spawn 专用面），无 EXECUTE/心跳/网络槽装配语义",
    ),
    (
        "svc-drv-console",
        "控制台驱动：句柄移交（spawn 专用面），无 EXECUTE/心跳/网络槽装配语义",
    ),
    (
        "svc-storage",
        "块服务：STORAGE_MMIO_SLOT 专用移交（spawn.rs）；root 经 svc-ota PERSIST/LOAD-TRIAL 间接验证落盘——无 EXECUTE/心跳面",
    ),
    // —— 被动 EIPC 服务端（root 无周期驱动语义；62.2 装配现状）——
    (
        "svc-config",
        "配置面：init bundle 静态分发 + consumer 注册，被动响应",
    ),
    (
        "svc-ledger",
        "业务服务：被动 EIPC 服务端，无周期驱动/心跳装配（62.2 装配现状）",
    ),
    (
        "svc-market",
        "业务服务：被动 EIPC 服务端，无周期驱动/心跳装配（62.2 装配现状）",
    ),
    (
        "svc-cloud-sync",
        "业务服务：被动 EIPC 服务端，无周期驱动/心跳装配（62.2 装配现状）",
    ),
    (
        "svc-agent-bus",
        "业务服务：被动 EIPC 服务端，无周期驱动/心跳装配（62.2 装配现状）",
    ),
    (
        "svc-mgmt",
        "运营服务：被动 EIPC 服务端 + root 周期持久 tick（PeriodicTask::MgmtPersistTick 25/17，recheck RS2——终态补写不依赖新管理请求；协议 EXECUTE 编排域仍不适用）",
    ),
    (
        "svc-ota",
        "OTA 探针服务端：root 直连（ota_handle），被动响应——无 EXECUTE 驱动语义",
    ),
    (
        "svc-observability",
        "观测出口：OBS-REPORT 服务端，root 直连名单（service.rs DirectIndexes observability 槽）入队/FLUSH——无 EXECUTE 驱动语义",
    ),
    // —— Unassigned 能力壳协议服务（63.2.4 P1 在册）：无传输面/采集轮；
    //    接入真实协议时随 net_slot + driven 一并登记 ——
    (
        "svc-dlt645",
        "Unassigned 能力壳（63.2.4 P1 在册）：无传输面/采集轮",
    ),
    (
        "svc-iec101",
        "Unassigned 能力壳（63.2.4 P1 在册）：无传输面/采集轮",
    ),
    (
        "svc-iec103",
        "Unassigned 能力壳（63.2.4 P1 在册）：无传输面/采集轮",
    ),
    (
        "svc-gbt27930",
        "Unassigned 能力壳（63.2.4 P1 在册）：无传输面/采集轮",
    ),
    // —— 测试探针/装配验证服务（qemu-test 专属；root 主循环直连）——
    (
        "svc-hello",
        "测试探针（qemu-test）：hello 心跳锚点往返，root 主循环直连",
    ),
    (
        "svc-fault",
        "测试探针（qemu-test）：故障注入目标，root 主循环直连（fault ep poll）",
    ),
    (
        "svc-pi",
        "测试装配验证服务（qemu-test）：无编排语义",
    ),
    (
        "svc-hog",
        "测试装配验证服务（qemu-test）：无编排语义",
    ),
    (
        "svc-drv-fault",
        "测试装配验证服务（qemu-test）：无编排语义",
    ),
    (
        "svc-pie-a",
        "测试装配验证服务（qemu-test）：PIE 装配验证，无编排语义",
    ),
    (
        "svc-pie-b",
        "测试装配验证服务（qemu-test）：PIE 装配验证，无编排语义",
    ),
];

/// 控制链回合周期（相位格判据基准——service.rs 原常量平移，值不变）。
pub const CONTROL_CHAIN_PERIOD_TICKS: u16 = 5;

/// 相位格静态断言（const fn，编译期锁）：全部协议 EXECUTE 的周期与相位
/// 必须落在控制链相位格（`% CONTROL_CHAIN_PERIOD == 0`）——"ACQ 保鲜 →
/// 链回合"的编排关系由表结构承载，原 wiring 注释自认"只剩 QEMU 门能抓"
/// 的相位对齐自此有编译期守卫。
const fn execute_phase_grid_aligned() -> bool {
    let mut i = 0;
    while i < SERVICE_WIRING.len() {
        if let Some(d) = &SERVICE_WIRING[i].driven {
            let p = CONTROL_CHAIN_PERIOD_TICKS as u64;
            if d.period_ticks % p != 0 || d.offset_ticks % p != 0 {
                return false;
            }
        }
        i += 1;
    }
    true
}
const _: () = assert!(execute_phase_grid_aligned());

// ============================================================================
// 结构断言（WP-62.2.1 红测；host 单测）
// ============================================================================

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    /// recheck RS2：MgmtPersistTick 必须以 **Repeat** 形态在周期表在册
    /// （svc-mgmt 终态补写的生产独立周期来源——不得退化为一次性探针）。
    #[test]
    fn mgmt_persist_tick_is_a_repeating_production_periodic() {
        let e = PERIODIC_SCHEDULE
            .iter()
            .find(|e| e.task == PeriodicTask::MgmtPersistTick)
            .expect("MgmtPersistTick 必须在周期编排表在册");
        assert_eq!(e.kind, ScheduleKind::Repeat, "持续周期（无结束点）");
        assert!(e.period > 0);
        assert!(e.offset < e.period);
        assert!(!periodic_due(PeriodicTask::MgmtPersistTick, 0));
        assert!(periodic_due(
            PeriodicTask::MgmtPersistTick,
            u64::from(e.offset)
        ));
        assert!(
            periodic_due(
                PeriodicTask::MgmtPersistTick,
                u64::from(e.offset) + u64::from(e.period)
            ),
            "Repeat：第二个周期同样到期"
        );
    }

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

    /// 周期编排表绝对基线锁（63.2 WP-63.2.2）：七组周期任务
    /// （5/25/200/60/70/50/100 + ObsReport offset 20）自 service.rs 平移，
    /// 逐行锁定 (period, offset, kind, task)——任何周期数字/相位/到期
    /// 语义漂移即红。改动必须同步 QEMU 串口编排证据（VOL63.2-GATE）。
    /// 69.5 追记：DegradeObserve（25/8，只读观测探针）为蓝图 69 卷 69.5
    /// 有意新增行（WP-69.5.1；与 ControlChain 5/0、ObsReport 25/20 错峰
    /// ——观测轮与决策轮不同 tick），既有七行数字零变化。
    #[test]
    fn periodic_schedule_absolute_baseline_locked() {
        let rows: Vec<(u16, u16, ScheduleKind, PeriodicTask)> = PERIODIC_SCHEDULE
            .iter()
            .map(|e| (e.period, e.offset, e.kind, e.task))
            .collect();
        assert_eq!(
            rows,
            vec![
                (5, 0, ScheduleKind::Repeat, PeriodicTask::ControlChain),
                // 70.6 新增（WP-70.6.5）：MGT1 探针（qemu-test-synth 装配
                // 专属；release 闭包 step 体 cfg 隔离零路径）——非平移行。
                (90, 90, ScheduleKind::OnceFrom, PeriodicTask::MgtProbe),
                (25, 20, ScheduleKind::Repeat, PeriodicTask::ObsReport),
                (200, 200, ScheduleKind::OnceAt, PeriodicTask::HangInject),
                (60, 60, ScheduleKind::OnceFrom, PeriodicTask::StoragePersist),
                (70, 70, ScheduleKind::OnceFrom, PeriodicTask::OtaDownload),
                (50, 0, ScheduleKind::Repeat, PeriodicTask::FaultInject),
                (100, 0, ScheduleKind::Repeat, PeriodicTask::WatchdogCheck),
                // 69.5 新增（WP-69.5.1）：只读观测探针——非平移行，有意扩展。
                (25, 8, ScheduleKind::Repeat, PeriodicTask::DegradeObserve),
                // 70.5 新增（WP-70.5.1）：可信身份推送——非平移行，有意扩展。
                (55, 55, ScheduleKind::OnceFrom, PeriodicTask::IdentityPush),
                // recheck RS2 新增（F70-03 §6.3.4）：管理账本持久补写
                // tick——非平移行，有意扩展（生产闭包同跑的持续周期）。
                (25, 17, ScheduleKind::Repeat, PeriodicTask::MgmtPersistTick),
            ],
            "周期编排表绝对基线漂移（service.rs 平移值必须逐字节保持）"
        );
    }

    /// 到期判定语义锁（63.2 WP-63.2.2）：Repeat 的 `% period == offset`
    /// 与 Once 的首达语义逐点验证（与 service.rs 原手写算式等价）。
    #[test]
    fn periodic_due_semantics_match_original_predicates() {
        assert!(periodic_due(PeriodicTask::ControlChain, 5));
        assert!(periodic_due(PeriodicTask::ControlChain, 10));
        assert!(!periodic_due(PeriodicTask::ControlChain, 7));
        // ObsReport 原算式 `tick % 25 == 20`（非零 offset）。
        assert!(periodic_due(PeriodicTask::ObsReport, 20));
        assert!(periodic_due(PeriodicTask::ObsReport, 45));
        assert!(!periodic_due(PeriodicTask::ObsReport, 25));
        // HangInject 原算式 `tick == 200`（一次性首达）。
        assert!(periodic_due(PeriodicTask::HangInject, 200));
        assert!(!periodic_due(PeriodicTask::HangInject, 400));
        // StoragePersist 原算式 `tick >= 60`。
        assert!(!periodic_due(PeriodicTask::StoragePersist, 59));
        assert!(periodic_due(PeriodicTask::StoragePersist, 60));
        assert!(periodic_due(PeriodicTask::StoragePersist, 61));
        // FaultInject 原算式 `fault_tick % 50 == 0`（fault_tick ≡ tick，
        // 两计数器每轮各 +1 且主循环无 continue——等价性见执行卡）。
        assert!(periodic_due(PeriodicTask::FaultInject, 50));
        assert!(!periodic_due(PeriodicTask::FaultInject, 55));
        assert!(periodic_due(PeriodicTask::WatchdogCheck, 100));
        assert!(!periodic_due(PeriodicTask::WatchdogCheck, 99));
    }

    /// EXECUTE 相位格断言（运行时镜像——编译期 const 断言的失败消息
    /// 版本）：全部 driven 周期/相位落在控制链相位格（%5==0）。
    #[test]
    fn execute_phase_grid_aligned_with_control_chain() {
        for w in SERVICE_WIRING {
            if let Some(d) = &w.driven {
                assert_eq!(
                    d.period_ticks % u64::from(CONTROL_CHAIN_PERIOD_TICKS),
                    0,
                    "{} EXECUTE 周期不在控制链相位格",
                    w.name
                );
                assert_eq!(
                    d.offset_ticks % u64::from(CONTROL_CHAIN_PERIOD_TICKS),
                    0,
                    "{} EXECUTE 相位不在控制链相位格",
                    w.name
                );
            }
        }
    }

    /// profile JSON → 服务 id 名单（spawn 名单事实源直读；WP-36.4 禁
    /// 另处硬编码名单，故测试不复制 roster——解析失败/ roster 空即 panic）。
    #[cfg(unix)]
    #[allow(clippy::disallowed_macros)] // 测试模块 panic! 豁免（clippy.toml 约定）
    fn profile_service_ids(raw: &str, label: &str) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("{label} profile JSON 解析失败: {e}"));
        let services = v
            .get("services")
            .and_then(|s| s.as_array())
            .unwrap_or_else(|| panic!("{label} profile 无 services 数组（schema 漂移？）"));
        assert!(
            services.len() >= 20,
            "{label} services 条目过少（{}）——profile 解析面漂移",
            services.len()
        );
        services
            .iter()
            .map(|s| {
                s.get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or_else(|| panic!("{label} service 条目缺 id"))
                    .to_string()
            })
            .collect()
    }

    /// 接线表完备性锁（63.2 WP-63.2.3）：每个被 spawn 的服务必须 ∈
    /// SERVICE_WIRING ∪ EXPLICIT_UNWIRED（后者逐项理由见常量表）。
    ///
    /// spawn.rs `proto_net_slot` None 臂的静默缺省 = 编译不拦运行时静默
    /// ——D-1 同型缺陷（接线未登记）此前无 host 红测能抓；本测试把
    /// "该登记未登记"提前到 host。双向：EXPLICIT_UNWIRED 每项必须是
    /// 真实 spawn 名单成员（防杂物抽屉），且不得与接线表重叠。
    #[test]
    #[cfg(unix)]
    fn every_spawned_service_is_wired_or_exempted() {
        let qemu_prod = profile_service_ids(
            include_str!("../../../../product/profiles/qemu-prod.json"),
            "qemu-prod",
        );
        let board_prod = profile_service_ids(
            include_str!("../../../../product/profiles/board-prod.json"),
            "board-prod",
        );
        let qemu_test = profile_service_ids(
            include_str!("../../../../product/profiles/qemu-test.json"),
            "qemu-test",
        );
        let wired: Vec<&str> = SERVICE_WIRING.iter().map(|w| w.name).collect();
        let exempted: Vec<&str> = EXPLICIT_UNWIRED.iter().map(|(n, _)| *n).collect();

        // 正向：每个被 spawn 的服务（root-task 本体除外）必须登记或豁免。
        for roster in [&qemu_prod, &board_prod, &qemu_test] {
            for id in roster {
                if id == "root-task" {
                    continue;
                }
                assert!(
                    wired.contains(&id.as_str()) || exempted.contains(&id.as_str()),
                    "服务 {id} 已 spawn 但既不在 SERVICE_WIRING 也不在 \
                     EXPLICIT_UNWIRED——表外静默缺省（D-1 同型），必须登记 \
                     接线属性或以真实理由豁免",
                );
            }
        }
        // 反向（防杂物抽屉）：EXPLICIT_UNWIRED 每项必须是真实 spawn 名单成员。
        let all: Vec<&String> = qemu_prod
            .iter()
            .chain(&board_prod)
            .chain(&qemu_test)
            .collect();
        for name in &exempted {
            assert!(
                all.iter().any(|s| *s == name),
                "EXPLICIT_UNWIRED 的 {name} 不在任何 profile spawn 名单——\
                 豁免项必须对应真实装配，不是杂物抽屉"
            );
        }
        // 豁免与接线表互斥。
        for name in &exempted {
            assert!(
                !wired.contains(name),
                "{name} 同时在 SERVICE_WIRING 与 EXPLICIT_UNWIRED——装配决策矛盾"
            );
        }
    }
}
