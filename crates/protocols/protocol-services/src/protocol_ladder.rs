//! 协议 EL0 服务装配阶梯（SPEC-SERIES-2026-08-29/S1 WP-S1.1）。
//!
//! 历史病灶（第五轮体检 C1）：六个网络协议 bin（goose/sv/mms/iec104/
//! modbus-tcp/mqtt）的 `_start` 各持一份 ~90 行同构装配阶梯（传输构建 →
//! 配置取回 → 插件 start → READY/失败裁决），12 个 bin host 测试数全为 0
//! ——恰好整段压在 `cfg(target_os = "none")` 之下。本模块把阶梯下沉为
//! 单一内核：bin 退化为 [`ProtocolParts`] 声明 + [`print_descriptor`] +
//! [`run`] + 协议专属后处理；失败裁决与判定行词形在 host 逐态可达可测。
//!
//! 纯代码移动 + 槽位取法变更（按名解析取槽——agent-sdk `eipc_grants`
//! 单一事实源，bin 不再手数 `EIPC_EP_BASE_SLOT`）。生产字节不变（R2）：六句判定行/
//! 诊断行（`line_*`）与执行前逐字一致（冻结字面量对照测试锁定）；
//! seL4 叶子（ConfigGet 取数、传输构建）以 [`ServiceIo`] 单接缝注入。
//!
//! 判定顺序硬约束（F-36）：**transport 先于 config**——今日六个 bin 的
//! `match` 嵌套即此序；颠倒会让 `HandoverMissing` 与 `ConfigUnavailable`
//! 优先级互换（顺序锁定测试见 `transport_precedes_config`）。
//!
//! RTU 不在本阶梯（F-4：`RTU-UNAVAILABLE reason=<word>` 词形自成一家，
//! 保留其两态 `assemble()`；仅迁槽位取法与 ConfigGet 管道——R3）。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::config_gateway::{ConfigEntrySource, ConfigStep};
use eneros_config_bundle::points::PointRecord;
use eneros_protocol_abstract::plugin::ProtocolPlugin;
use eneros_protocol_abstract::{
    ProtocolConfig, ProtocolError, TransportFactory, TransportKind, VecPointRegistry,
};

/// 本卷唯一的新接缝：服务进程的 seL4 叶子。两个实现（target
/// [`Sel4ServiceIo`] / host 测试假件）。
pub trait ServiceIo {
    /// ConfigGet：节 id → 原始响应字节（含 4B 长度前缀，F-29）。
    /// `None` = 未授予 / 传输失败。target 实现内部按名取 svc-config 槽
    /// （agent-sdk `eipc_grants` 表 = 单一事实源）——bin 侧不接触槽位常量。
    fn config_entry(&mut self, id: &str) -> Option<Vec<u8>>;
    /// 蓝图 61：GET_GENERATION（观测映射世代；None = 不可得——fail-closed
    /// 降级为世代未定，观测不满足控制资格）。
    fn config_generation(&mut self) -> Option<u64> {
        None
    }
    /// 传输构建。**用 `Result` 而非 `Option`（F-33）**：今日
    /// `handover missing ({:?})` 行要打印失败原因，`build_net_transport`
    /// 的 `Err` 正是 `&'static str`；`Option` 会丢原因 → 串口行变样。
    fn transport(&mut self, kind: TransportKind)
        -> Result<Box<dyn TransportFactory>, &'static str>;
}

/// 一个协议服务的声明式构成（bin 侧唯一需要填写的东西）。
pub struct ProtocolParts {
    /// 判定行前缀，如 `svc-goose`。
    pub service: &'static str,
    /// 配置诊断行前缀，如 `goose-config`——**与 service 不同**（F-34）。
    pub config_client: &'static str,
    /// svc-config 节 id，如 `protocol-goose`。
    pub config_id: &'static str,
    /// READY 行 `capability=` 词形 + `PluginIoService` capability（同一串，
    /// F-42 冻结）。
    pub capability: &'static str,
    /// 点表策略（None = 空注册表；All = 全量；Filtered = 按协议过滤）。
    pub points: PointsPolicy,
    /// 配置回显（iec104 独有：逐键 `[iec104-config] k = v`）。
    pub config_dump: bool,
    /// 启动期自检轮数（0 = 不自检；本体留 bin [`run`] 之后的
    /// `PluginIoService::self_check_poll`——host 测试不可承受有界自旋，
    /// 裁决 6）。
    pub self_check_rounds: u32,
    /// 降级呈现族（S1 evidence M-6 登记）：今日两种真实串口形态——
    /// `Standard`（goose/sv/mms/iec104/mqtt：各失败态各自判定行）与
    /// `HandoverAlias`（modbus-tcp：try_new 全部失败统一映射
    /// `handover missing ("<reason>")`；config 失败先打 standard 行再打
    /// 别名行——try_new→_start 两段打印的逐字保真）。数据参数，零 hook。
    pub degraded: DegradedStyle,
}

/// 降级判定行呈现族（见 [`ProtocolParts::degraded`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradedStyle {
    /// 各失败态各自判定行（F-42 词形族）。
    Standard,
    /// 失败态统一映射 `handover missing ("<reason>")`（modbus-tcp 族）。
    HandoverAlias,
}

/// 点表策略与取回时机（S1 evidence M-2 登记）：今日两族真实串口行序——
/// iec104/mms 在配置取回后取点表，modbus-tcp 在传输构建**前**取。R2
/// 逐字节保真要求两序皆可表达，故时机入策略。
pub enum PointsPolicy {
    /// 无点表（goose/sv/mqtt——空注册表；`EmptyPointRegistry` → 空
    /// `VecPointRegistry` 行为等价，DQ-9/M-5 微偏差登记）。
    None,
    /// 全量（iec104：`|_| true`），配置取回后取。
    All,
    /// 按协议过滤（点记录谓词；fn 指针——过滤均为纯函数），配置取回后取。
    Filtered(fn(&PointRecord) -> bool),
    /// 传输构建前取（modbus-tcp 现行行序：点表行先于传输阶段）。
    Eager(fn(&PointRecord) -> bool),
}

/// 阶梯裁决出口。bin 据此做协议专属后处理（mms `drive_mms_read` /
/// modbus-tcp `ModbusTcpIoService` / mqtt 端点解析）。
pub enum Ladder<P> {
    /// 装配成功：`plugin.start` 已消费配置/传输/点表；READY 行已打印。
    Ready {
        plugin: P,
        registry: VecPointRegistry,
        /// 蓝图 61：来源注册表（`[[source]]`；旧 bundle = 空）。
        sources: alloc::vec::Vec<eneros_config_bundle::points::SourceRecord>,
        /// 映射世代（active bundle generation；不可得 → 0）。
        generation: u64,
    },
    /// fail-closed：判定行已打印，bin 落 [`capability_shell`]。
    Degraded(LadderReason),
}

/// 阶梯失败原因（host 测试可区分；`ConfigUnavailable` 与
/// `ConfigDecodeFailed` 今日共用同一句词形，拆分仅为可测——打印词形
/// 保持同一句，不新增串口行，R2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderReason {
    HandoverMissing,
    ConfigUnavailable,
    ConfigDecodeFailed,
    PluginStartFailed,
}

// ============================================================================
// 判定行 / 诊断行词形构造器（单源；测试以冻结字面量对照，不得以 helper
// 作期望侧——F-37：helper 与冻结常量分离）
// ============================================================================

/// `[svc-goose] NET-TRANSPORT READY capability=…`
pub fn line_ready(service: &str, capability: &str) -> String {
    alloc::format!("[{service}] NET-TRANSPORT READY capability={capability}")
}

/// `[svc-goose] handover missing ({reason:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）`
///
/// `reason` 以 `{:?}` 呈现（`&'static str` → 带引号，与今日 bin 逐字一致）。
pub fn line_handover_missing(service: &str, reason: &str) -> String {
    alloc::format!(
        "[{service}] handover missing ({reason:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）"
    )
}

/// `[svc-goose] protocol config unavailable —— capability 壳（EXECUTE ST_UNAVAILABLE）`
pub fn line_config_unavailable(service: &str) -> String {
    alloc::format!(
        "[{service}] protocol config unavailable —— capability 壳（EXECUTE ST_UNAVAILABLE）"
    )
}

/// `[svc-goose] plugin start FAILED {e:?} —— capability 壳（EXECUTE ST_UNAVAILABLE）`
pub fn line_plugin_start_failed(service: &str, e: &ProtocolError) -> String {
    alloc::format!(
        "[{service}] plugin start FAILED {e:?} —— capability 壳（EXECUTE ST_UNAVAILABLE）"
    )
}

/// `[goose-config] config response too short`（F-35：今日 service_cfg 内两抄收编）
pub fn line_config_too_short(client: &str) -> String {
    alloc::format!("[{client}] config response too short")
}

/// `[goose-config] config length prefix malformed`（F-35：同上）
pub fn line_config_bad_prefix(client: &str) -> String {
    alloc::format!("[{client}] config length prefix malformed")
}

/// descriptor 证据行（`[protocol-service] goose plugin=… role=… interop=…`，
/// 词形单源）。
///
/// bin 在 [`run`] 前调用——mqtt（SECURITY-CHECK）与 modbus-tcp（telemetry
/// 绑定行）需在 descriptor 行与传输阶段之间插入服务专属步骤（S1 evidence
/// M-1/M-4 登记）。
pub fn print_descriptor<P: ProtocolPlugin>(service: &str, plugin: &P) {
    let d = plugin.descriptor();
    eneros_runtime::println!(
        "[protocol-service] {} plugin={} role={:?} interop={:?}",
        short_name(service),
        d.id,
        d.role,
        d.interop_level
    );
}

/// 能力壳构造单点（fail-closed 终态：EXECUTE `ST_UNAVAILABLE`）。bin 不
/// 直呼 `UnassignedProtocolService`（C1 阶梯归零面）。
pub fn capability_shell() -> Box<dyn eneros_drv_svc::runtime::DriverService> {
    Box::new(crate::service::UnassignedProtocolService)
}

/// 能力壳 bin 装配单点（蓝图 63 卷 63.2 WP-63.2.4 P1 收编）：descriptor
/// 证据行 + [`capability_shell`] + `driver_service_main`——SpecBlocked
/// 协议 bin（svc-dlt645/iec101/iec103/gbt27930）共享。bin 直呼
/// `UnassignedProtocolService` 违纪（C1 阶梯归零面）自此消除；串口词形
/// 逐字节不变（descriptor 行经 [`print_descriptor`] 单源）。
#[cfg(target_os = "none")]
pub fn run_capability_shell<P: ProtocolPlugin>(
    ep: u64,
    service: &'static str,
    ready_ntfn: u64,
    plugin: P,
) -> ! {
    print_descriptor(service, &plugin);
    let mut shell = capability_shell();
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        short_name(service),
        ready_ntfn,
        crate::now_ms,
        crate::instance_epoch(),
        &mut *shell,
    )
}

/// 判定行前缀 `svc-<name>` → `<name>`（wall_epoch / descriptor 行用短名；
/// 与今日 bin 字面量逐字一致）。
fn short_name(service: &str) -> &str {
    service.strip_prefix("svc-").unwrap_or(service)
}

/// `&mut dyn ServiceIo` → `&mut dyn ConfigEntrySource` 适配（同一方法
/// 签名；避免 trait upcast 的 no_std 工具链依赖）。
pub struct ServiceIoAsSource<'a>(pub &'a mut dyn ServiceIo);

impl ConfigEntrySource for ServiceIoAsSource<'_> {
    fn config_entry(&mut self, id: &str) -> Option<Vec<u8>> {
        self.0.config_entry(id)
    }

    /// 蓝图 61：GET_GENERATION 转发（观测映射世代单源）。
    fn config_generation(&mut self) -> Option<u64> {
        self.0.config_generation()
    }
}

/// 阶梯本体：wall_epoch → 传输 → 配置 →（回显）→ 点表 → `plugin.start` →
/// READY/失败裁决。除 time_init（target-only）外全程 host 可达。
///
/// `self_check_poll` 不在本阶梯（裁决 6）：有界自旋 + 真实 `plugin.poll`
/// 留在 bin 的 Ready 臂，轮数由 [`ProtocolParts::self_check_rounds`] 单点。
pub fn run<P: ProtocolPlugin>(
    wall_epoch: u64,
    parts: &ProtocolParts,
    mut plugin: P,
    io: &mut dyn ServiceIo,
) -> Ladder<P> {
    // smoltcp 传输用 eneros_time 单调时钟推进；EL0 服务进程内初始化。
    #[cfg(target_os = "none")]
    eneros_time::time_init(eneros_hal::arm64::timer::clock(), 0);
    // SPEC-B WP-B.1：X6 墙钟 epoch 秒（0=缺失显式降级）。
    crate::apply_wall_epoch(short_name(parts.service), wall_epoch);

    // Eager 点表（modbus-tcp 现行串口行序：点表行先于传输阶段——R2 保序；
    // S1 evidence M-2 登记）。
    // 蓝图 61：取数单源 = fetch_points（注册表 + 来源表 + 映射世代）。
    let (mut registry, mut obs_sources, mut obs_generation) = match parts.points {
        PointsPolicy::Eager(keep) => {
            let f = crate::service_cfg::fetch_points(
                &mut ServiceIoAsSource(&mut *io),
                parts.config_client,
                keep,
            );
            (f.registry, f.sources, f.generation)
        }
        _ => (VecPointRegistry::new(Vec::new()), Vec::new(), 0),
    };

    // 判定顺序硬约束（F-36）：transport 先于 config。
    let mut factory = match io.transport(TransportKind::Tcp) {
        Ok(factory) => factory,
        Err(e) => {
            eneros_runtime::println!("{}", line_handover_missing(parts.service, e));
            return Ladder::Degraded(LadderReason::HandoverMissing);
        }
    };
    let cfg = match crate::service_cfg::fetch_protocol_config(
        &mut ServiceIoAsSource(&mut *io),
        parts.config_client,
        parts.config_id,
    ) {
        Ok(cfg) => cfg,
        Err(step) => {
            eneros_runtime::println!("{}", line_config_unavailable(parts.service));
            if parts.degraded == DegradedStyle::HandoverAlias {
                // modbus-tcp 族：try_new 的 Err 再经 _start 判定行（M-6）。
                eneros_runtime::println!(
                    "{}",
                    line_handover_missing(parts.service, "protocol-config-missing")
                );
            }
            return Ladder::Degraded(match step {
                ConfigStep::Unavailable => LadderReason::ConfigUnavailable,
                // TooShort/BadPrefix/DecodeFailed：今日同落 "protocol config
                // unavailable" 终态；细分 reason 仅供 host 测试区分（§2）。
                _ => LadderReason::ConfigDecodeFailed,
            });
        }
    };
    // 46.5 诊断：落盘实际配置值（iec104 独有；poll_interval 是否 5000）。
    if parts.config_dump {
        for (k, v) in cfg.entries.iter() {
            eneros_runtime::println!("[{}] {k} = {v}", parts.config_client);
        }
    }
    // 点表注入（46.5）：All/Filtered 在配置取回后取（iec104/mms 现行行序）。
    let keep: Option<fn(&PointRecord) -> bool> = match parts.points {
        PointsPolicy::None => None,
        PointsPolicy::All => Some(|_| true),
        PointsPolicy::Filtered(keep) => Some(keep),
        PointsPolicy::Eager(_) => None,
    };
    if let Some(keep) = keep {
        let fetched = crate::service_cfg::fetch_points(
            &mut ServiceIoAsSource(&mut *io),
            parts.config_client,
            keep,
        );
        registry = fetched.registry;
        obs_sources = fetched.sources;
        obs_generation = fetched.generation;
    }
    // 39：插件 start 消费配置/传输/点表（签名不动，F-12——插件一行不改）。
    match plugin.start(&cfg, factory.as_mut(), &mut registry) {
        Ok(()) => {
            eneros_runtime::println!("{}", line_ready(parts.service, parts.capability));
            Ladder::Ready {
                plugin,
                registry,
                sources: obs_sources,
                generation: obs_generation,
            }
        }
        Err(e) => {
            if parts.degraded == DegradedStyle::Standard {
                eneros_runtime::println!("{}", line_plugin_start_failed(parts.service, &e));
            } else {
                // modbus-tcp 族：无 plugin start FAILED 行，统一 handover
                // 别名（M-6——try_new map_err 单词原因的逐字保真）。
                eneros_runtime::println!(
                    "{}",
                    line_handover_missing(parts.service, "plugin-start-failed")
                );
            }
            Ladder::Degraded(LadderReason::PluginStartFailed)
        }
    }
}

// ============================================================================
// target 侧接缝实现（seL4 叶子单点）
// ============================================================================

/// target 侧 [`ServiceIo`]：ConfigGet 按名解析取 svc-config 槽
/// （agent-sdk 授予表单源；F-8 七处手抄收编；F-9 逐条对表槽值不变）；
/// 传输经 net_transport virtio-net 移交构建（X3/X4/X5 原样透传）。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub struct Sel4ServiceIo {
    service: &'static str,
    config_client: &'static str,
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl Sel4ServiceIo {
    pub fn new(
        service: &'static str,
        config_client: &'static str,
        mmio_vaddr: u64,
        dma_vaddr: u64,
        dma_phys: u64,
    ) -> Self {
        Self {
            service,
            config_client,
            mmio_vaddr,
            dma_vaddr,
            dma_phys,
        }
    }
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl ServiceIo for Sel4ServiceIo {
    fn config_entry(&mut self, id: &str) -> Option<Vec<u8>> {
        use eneros_agent_sdk::config_gateway::ConfigEntrySource as _;
        // 蓝图 62 卷 62.2（WP-62.2.4）：EIPC 建端单源 ChunkedEndpoint::bind。
        let mut ep = eneros_agent_sdk::endpoint::ChunkedEndpoint::bind(
            self.service,
            eneros_agent_sdk::config_client::CONFIG_ENDPOINT,
        )?;
        ep.client_mut().config_entry(id)
    }

    /// 蓝图 61：GET_GENERATION（观测映射世代；经同一 config 槽）。
    fn config_generation(&mut self) -> Option<u64> {
        use eneros_agent_sdk::config_gateway::ConfigEntrySource as _;
        let mut ep = eneros_agent_sdk::endpoint::ChunkedEndpoint::bind(
            self.service,
            eneros_agent_sdk::config_client::CONFIG_ENDPOINT,
        )?;
        ep.client_mut().config_generation()
    }

    fn transport(
        &mut self,
        _kind: TransportKind,
    ) -> Result<Box<dyn TransportFactory>, &'static str> {
        let stack = crate::net_transport::build_net_transport(
            self.mmio_vaddr,
            self.dma_vaddr,
            self.dma_phys,
        )?;
        Ok(Box::new(crate::net_transport::NetTransportFactory::new(
            stack,
        )))
    }
}

// ============================================================================
// host 测试（六臂逐态可达 + 词形冻结对照 + 判定顺序锁定）
// ============================================================================

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use eneros_protocol_abstract::plugin::{
        InteropLevel, ProtocolDescriptor, ProtocolRole, ResourceBudget, SecurityProfile, WorkBudget,
    };
    use eneros_protocol_abstract::test_support::EchoFactory;
    use eneros_protocol_abstract::{AdapterState, ProtocolAddress};
    use eneros_protocol_abstract::{PointRegistry, Progress, ProtocolHealth};

    use super::*;

    /// 冻结字面量（F-42：逐字取自执行前 svc-goose.rs:76/91/99/107 与
    /// service_cfg.rs:38/42）。**期望侧禁止引用 `line_*` helper**（F-37：
    /// 改一词形必须变红——C5 反证判据）。
    const LINE_READY_EXPECTED: &str =
        "[svc-goose] NET-TRANSPORT READY capability=eneros_iec61850_goose::plugin::GoosePlugin";
    const LINE_HANDOVER_MISSING_EXPECTED: &str =
        "[svc-goose] handover missing (\"no-dma\") —— capability 壳（EXECUTE ST_UNAVAILABLE）";
    const LINE_CONFIG_UNAVAILABLE_EXPECTED: &str =
        "[svc-goose] protocol config unavailable —— capability 壳（EXECUTE ST_UNAVAILABLE）";
    const LINE_PLUGIN_START_FAILED_EXPECTED: &str =
        "[svc-goose] plugin start FAILED InvalidConfig —— capability 壳（EXECUTE ST_UNAVAILABLE）";
    const LINE_CONFIG_TOO_SHORT_EXPECTED: &str = "[goose-config] config response too short";
    const LINE_CONFIG_BAD_PREFIX_EXPECTED: &str = "[goose-config] config length prefix malformed";

    /// protocol-* canonical KV 负载（`encode_protocol` 同格式；agent-sdk
    /// 测试同款构造，以真 `decode_protocol` 验证）。
    fn proto_payload(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut out = alloc::vec![entries.len() as u8];
        for (k, v) in entries {
            out.push(k.len() as u8);
            out.extend_from_slice(k.as_bytes());
            out.extend_from_slice(&(v.len() as u16).to_le_bytes());
            out.extend_from_slice(v.as_bytes());
        }
        out
    }

    /// 4B LE 长度前缀 + payload（GET_ENTRY 原始响应形态）。
    fn framed(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// 一条 modbus-tcp Analog 点的 `FIELD_POINTS` 完整 payload
    /// （schema::points 自描述子格式；record 布局 = points.rs 解码序）。
    fn points_payload_one() -> Vec<u8> {
        use eneros_config_bundle::schema::points::{FIELD_POINTS, PROTO_MODBUS_TCP};
        let mut rec = Vec::new();
        rec.extend_from_slice(&1001u32.to_le_bytes()); // point_id
        rec.extend_from_slice(&1u16.to_le_bytes()); // device_id
        rec.push(PROTO_MODBUS_TCP);
        rec.push(0); // PointType::Analog ordinal
        rec.extend_from_slice(&0.1f64.to_le_bytes()); // scale
        rec.extend_from_slice(&(-40.0f64).to_le_bytes()); // offset
        rec.extend_from_slice(&5000u32.to_le_bytes()); // period_ms
        rec.extend_from_slice(&0.0f64.to_le_bytes()); // deadband
        rec.push(0); // no range
        rec.extend_from_slice(&4u16.to_le_bytes()); // addr_len
        rec.push(1); // slave
        rec.extend_from_slice(&4097u16.to_le_bytes()); // reg
        rec.push(3); // func
        let mut table = Vec::new();
        table.extend_from_slice(&1u32.to_le_bytes()); // count
        table.extend_from_slice(&rec);
        let mut raw = alloc::vec![FIELD_POINTS];
        raw.extend_from_slice(&(table.len() as u16).to_le_bytes());
        raw.extend_from_slice(&table);
        raw
    }

    /// 内存假件：按节 id 回吐预置响应；传输成败/调用序可注入。
    struct FakeIo {
        entries: Vec<(&'static str, Option<Vec<u8>>)>,
        transport: Result<(), &'static str>,
        calls: Vec<&'static str>,
    }

    impl FakeIo {
        fn ok(entries: Vec<(&'static str, Option<Vec<u8>>)>) -> Self {
            Self {
                entries,
                transport: Ok(()),
                calls: Vec::new(),
            }
        }
        fn parts(points: PointsPolicy) -> ProtocolParts {
            ProtocolParts {
                service: "svc-goose",
                config_client: "goose-config",
                config_id: "protocol-goose",
                capability: "eneros_iec61850_goose::plugin::GoosePlugin",
                points,
                config_dump: false,
                self_check_rounds: 0,
                degraded: DegradedStyle::Standard,
            }
        }
    }

    impl ServiceIo for FakeIo {
        fn config_entry(&mut self, id: &str) -> Option<Vec<u8>> {
            self.calls.push("config");
            self.entries
                .iter()
                .find(|(k, _)| *k == id)
                .and_then(|(_, v)| v.clone())
        }
        fn transport(
            &mut self,
            _kind: TransportKind,
        ) -> Result<Box<dyn TransportFactory>, &'static str> {
            self.calls.push("transport");
            match self.transport {
                Ok(()) => Ok(Box::new(EchoFactory)),
                Err(e) => Err(e),
            }
        }
    }

    /// 测试假插件（F-24 六必填方法；记录 start 消费的点表规模）。
    struct FakePlugin {
        fail_start: bool,
        started: bool,
        started_point_ids: usize,
    }

    impl FakePlugin {
        fn new(fail_start: bool) -> Self {
            Self {
                fail_start,
                started: false,
                started_point_ids: 0,
            }
        }
    }

    impl ProtocolPlugin for FakePlugin {
        fn descriptor(&self) -> &ProtocolDescriptor {
            static D: ProtocolDescriptor = ProtocolDescriptor {
                id: "fake",
                version: "0",
                role: ProtocolRole::Master,
                transport_kinds: &[TransportKind::Tcp],
                security_profile: SecurityProfile::None,
                point_types: &[],
                resource_budget: ResourceBudget {
                    max_connections: 1,
                    max_poll_ms: 50,
                    max_frame: 64,
                },
                interop_level: InteropLevel::SpecBlocked,
            };
            &D
        }
        fn validate_config(&self, _cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
            Ok(())
        }
        fn start(
            &mut self,
            _cfg: &ProtocolConfig,
            _transports: &mut dyn TransportFactory,
            points: &mut dyn PointRegistry,
        ) -> Result<(), ProtocolError> {
            if self.fail_start {
                return Err(ProtocolError::InvalidConfig);
            }
            self.started = true;
            let mut ids = Vec::new();
            points.collect_ids(&mut ids);
            self.started_point_ids = ids.len();
            Ok(())
        }
        fn poll(&mut self, _now_ms: u64, _budget: WorkBudget) -> Result<Progress, ProtocolError> {
            Ok(Progress::Idle)
        }
        fn health(&self) -> ProtocolHealth {
            ProtocolHealth {
                state: AdapterState::Stopped,
                last_poll_ms: 0,
                error_count: 0,
                last_error: None,
                last_progress: Progress::Idle,
            }
        }
        fn stop(&mut self) -> Result<(), ProtocolError> {
            Ok(())
        }
    }

    fn valid_protocol_entry() -> Vec<(&'static str, Option<Vec<u8>>)> {
        alloc::vec![(
            "protocol-goose",
            Some(framed(&proto_payload(&[("peer", "1.2.3.4:5")])))
        )]
    }

    /// C4 臂 1：Ready——插件 start 消费点表（All 策略经真 decode_points）。
    #[test]
    fn ready_with_points_loaded_and_plugin_started() {
        let mut entries = valid_protocol_entry();
        entries.push(("points", Some(framed(&points_payload_one()))));
        let mut io = FakeIo::ok(entries);
        let r = run(
            0,
            &FakeIo::parts(PointsPolicy::All),
            FakePlugin::new(false),
            &mut io,
        );
        match r {
            Ladder::Ready {
                plugin, registry, ..
            } => {
                assert!(plugin.started);
                assert_eq!(plugin.started_point_ids, 1);
                assert_eq!(registry.len(), 1);
            }
            Ladder::Degraded(_) => panic!("有效配置 + All 点表应 Ready"),
        }
    }

    /// C4 臂 2：传输缺席 → HandoverMissing（真实 reason 词形不变）。
    #[test]
    fn handover_missing_when_transport_denied() {
        let mut io = FakeIo::ok(valid_protocol_entry());
        io.transport = Err("no-dma");
        let r = run(
            0,
            &FakeIo::parts(PointsPolicy::None),
            FakePlugin::new(false),
            &mut io,
        );
        assert!(matches!(r, Ladder::Degraded(LadderReason::HandoverMissing)));
    }

    /// C4 臂 3：ConfigGet 未授予 → ConfigUnavailable。
    #[test]
    fn config_unavailable_when_entry_not_granted() {
        let mut io = FakeIo::ok(Vec::new());
        let r = run(
            0,
            &FakeIo::parts(PointsPolicy::None),
            FakePlugin::new(false),
            &mut io,
        );
        assert!(matches!(
            r,
            Ladder::Degraded(LadderReason::ConfigUnavailable)
        ));
    }

    /// C4 臂 4：配置体解码失败 → ConfigDecodeFailed（同句词形，细分可测）。
    #[test]
    fn config_decode_failed_when_payload_junk() {
        let mut io = FakeIo::ok(alloc::vec![(
            "protocol-goose",
            Some(framed(b"\x01\x05junk"))
        )]);
        let r = run(
            0,
            &FakeIo::parts(PointsPolicy::None),
            FakePlugin::new(false),
            &mut io,
        );
        assert!(matches!(
            r,
            Ladder::Degraded(LadderReason::ConfigDecodeFailed)
        ));
    }

    /// C4 臂 5：插件 start 失败 → PluginStartFailed。
    #[test]
    fn plugin_start_failed() {
        let mut io = FakeIo::ok(valid_protocol_entry());
        let r = run(
            0,
            &FakeIo::parts(PointsPolicy::None),
            FakePlugin::new(true),
            &mut io,
        );
        assert!(matches!(
            r,
            Ladder::Degraded(LadderReason::PluginStartFailed)
        ));
    }

    /// C4 臂 6：PointsEmpty——None 策略 → 空注册表照常 start（DQ-9/M-5：
    /// 与今日 EmptyPointRegistry 行为等价——插件观察 0 点）。
    #[test]
    fn points_empty_when_policy_none() {
        let mut io = FakeIo::ok(valid_protocol_entry());
        let r = run(
            0,
            &FakeIo::parts(PointsPolicy::None),
            FakePlugin::new(false),
            &mut io,
        );
        match r {
            Ladder::Ready {
                plugin, registry, ..
            } => {
                assert!(plugin.started);
                assert_eq!(plugin.started_point_ids, 0);
                assert_eq!(registry.len(), 0);
            }
            Ladder::Degraded(_) => panic!("None 策略应 Ready"),
        }
    }

    /// C4 词形对照：六句构造器 vs 冻结字面量（F-37 分离——期望侧为
    /// 手抄常量，改实现词形必须变红）。
    #[test]
    fn wordforms_match_frozen_literals() {
        assert_eq!(
            line_ready("svc-goose", "eneros_iec61850_goose::plugin::GoosePlugin"),
            LINE_READY_EXPECTED
        );
        assert_eq!(
            line_handover_missing("svc-goose", "no-dma"),
            LINE_HANDOVER_MISSING_EXPECTED
        );
        assert_eq!(
            line_config_unavailable("svc-goose"),
            LINE_CONFIG_UNAVAILABLE_EXPECTED
        );
        assert_eq!(
            line_plugin_start_failed("svc-goose", &ProtocolError::InvalidConfig),
            LINE_PLUGIN_START_FAILED_EXPECTED
        );
        assert_eq!(
            line_config_too_short("goose-config"),
            LINE_CONFIG_TOO_SHORT_EXPECTED
        );
        assert_eq!(
            line_config_bad_prefix("goose-config"),
            LINE_CONFIG_BAD_PREFIX_EXPECTED
        );
    }

    /// F-36 判定顺序硬约束：transport 先于 config——传输失败时配置节
    /// 不得被触达（否则 HandoverMissing/ConfigUnavailable 优先级互换）。
    #[test]
    fn transport_precedes_config() {
        let mut io = FakeIo::ok(valid_protocol_entry());
        io.transport = Err("handover-absent");
        let r = run(
            0,
            &FakeIo::parts(PointsPolicy::None),
            FakePlugin::new(false),
            &mut io,
        );
        assert!(matches!(r, Ladder::Degraded(LadderReason::HandoverMissing)));
        assert_eq!(
            io.calls,
            alloc::vec!["transport"],
            "config 不得先于 transport 被触达"
        );
    }

    /// M-6（modbus-tcp 族）：HandoverAlias 降级呈现——失败 reason 分类
    /// 不受呈现族影响（判定行词形由冻结对照测试锁定）。
    #[test]
    fn handover_alias_style_keeps_reason_classification() {
        let mut io = FakeIo::ok(valid_protocol_entry());
        let mut parts = FakeIo::parts(PointsPolicy::Eager(|_| false));
        parts.degraded = DegradedStyle::HandoverAlias;
        let r = run(0, &parts, FakePlugin::new(true), &mut io);
        assert!(matches!(
            r,
            Ladder::Degraded(LadderReason::PluginStartFailed)
        ));
        // Eager 点表先于传输被取（调用序：config("points") 在 transport 前）。
        assert_eq!(
            io.calls,
            alloc::vec!["config", "transport", "config"],
            "Eager points → transport → protocol config"
        );
    }

    /// ProtocolAddress 引用保温（假件夹具域；防未用导入漂移）。
    #[test]
    fn fixture_domain_imports_wired() {
        let addr = ProtocolAddress::Modbus {
            slave_addr: 1,
            reg_addr: 4097,
            func_code: 3,
        };
        assert!(matches!(addr, ProtocolAddress::Modbus { .. }));
    }
}
