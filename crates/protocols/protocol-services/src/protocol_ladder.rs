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
        /// 原始点记录（keep 过滤后——调度面 period_ms/deadband 消费）。
        /// 蓝图 64 卷 64.2（WP-64.2.3）：从 PointsFetch 透传——bin 的二次
        /// `fetch_points` 整表拉取删除（EIPC 往返 ×2 + 协议谓词双抄收口；
        /// 唯一登记的串口行为变更 = ingest bin POINTS-LOADED 行 2→1）。
        records: alloc::vec::Vec<PointRecord>,
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
    emit(alloc::format!(
        "[protocol-service] {} plugin={} role={:?} interop={:?}",
        short_name(service),
        d.id,
        d.role,
        d.interop_level
    ));
}

// ============================================================================
// 判定行发射单点 + bin 装配核心（69.3 WP-69.3.1：族 A bin 91 行共同壳收编）
// ============================================================================

/// 判定行发射单点（生产 = 串口 `println!`；host 测试 = 收集进
/// `EMITTED_LINES`——`bin_body` 行序快照测试 t-ladder-1/2 的捕获接缝）。
/// 发射内容与改造前 bin 的 `println!` 逐字节相同（R2 词形零漂移）。
fn emit(line: String) {
    #[cfg(test)]
    EMITTED_LINES.with(|l| l.borrow_mut().push(line.clone()));
    eneros_runtime::println!("{}", line);
}

#[cfg(test)]
std::thread_local! {
    static EMITTED_LINES: core::cell::RefCell<alloc::vec::Vec<String>> =
        const { core::cell::RefCell::new(alloc::vec::Vec::new()) };
}

/// 取走测试期收集的判定行（行序快照断言用；仅 host 测试构建存在）。
#[cfg(test)]
pub(crate) fn take_emitted_lines() -> alloc::vec::Vec<String> {
    EMITTED_LINES.with(core::cell::RefCell::take)
}

/// 族 A bin 装配核心的产物（WP-69.3.1）：Ready 臂插件已 start（READY 行
/// 已打印），启动期自检与 Box 装配由调用方衔接——自检本体含轮间有界
/// 自旋，host 测试不可承受（46.5 裁决 6），故不在 [`assemble`] 内。
pub(crate) enum Assembled<P: ProtocolPlugin> {
    /// 装配成功：bin 侧 `self_check_poll(parts.self_check_rounds)` 后
    /// `Box::new` 进 `driver_service_main`（与改造前 bin Ready 臂同序）。
    Ready(crate::service::PluginIoService<P>),
    /// fail-closed：失败判定行已打印，bin 落 [`capability_shell`]。
    Degraded,
}

/// [`bin_body`] 的 host 可测核心：descriptor → ladder run → Ready/Degraded
/// 分支。判定行经 [`emit`] 单点发射，行序与改造前 bin 逐字节相同
/// （t-ladder-1/2 行序快照锁；期望值 = 改造前 svc-goose.rs 装配序手抄）。
pub(crate) fn assemble<P: ProtocolPlugin>(
    wall_epoch: u64,
    parts: &ProtocolParts,
    plugin: P,
    io: &mut dyn ServiceIo,
) -> Assembled<P> {
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）——改造前
    // bin 在 run 前调用，次序不变。
    print_descriptor(parts.service, &plugin);
    match run(wall_epoch, parts, plugin, io) {
        Ladder::Ready { plugin, .. } => Assembled::Ready(crate::service::PluginIoService::new(
            plugin,
            parts.capability,
        )),
        Ladder::Degraded(_) => Assembled::Degraded,
    }
}

/// 协议 bin 的标准入口（族 A；WP-69.3.1）：io 构造 → [`assemble`] →
/// Ready 臂自检 → `driver_service_main`。**不含** `#[global_allocator]` /
/// `#[panic_handler]` / `_start` 签名与 `set_ipc_buffer`/`ALLOC.init()`
/// （链接期 / EL0 入口 ABI 约束，留在 bin crate 根——卷册 WP-69.3.1 边界，
/// 移动会改变链接期语义且触碰内存预算铁律）。
/// 短名经 [`short_name`] 单源由 `parts.service` 推导（"svc-goose"→"goose"，
/// 与改造前 `driver_service_main` 第二参字面逐字相同，不另设参数双源）。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub fn bin_body<P: ProtocolPlugin>(
    ep: u64,
    ready_ntfn: u64,
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    wall_epoch: u64,
    parts: &ProtocolParts,
    make_plugin: fn() -> P,
) -> ! {
    let mut io = Sel4ServiceIo::new(
        parts.service,
        parts.config_client,
        mmio_vaddr,
        dma_vaddr,
        dma_phys,
    );
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match assemble(wall_epoch, parts, make_plugin(), &mut io) {
            Assembled::Ready(mut io) => {
                // 46.5：启动期自检轮询——post-connect 协议交换在进入
                // request 循环前完成（有界不阻塞对端）。
                io.self_check_poll(parts.self_check_rounds);
                Box::new(io)
            }
            Assembled::Degraded => capability_shell(),
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        short_name(parts.service),
        ready_ntfn,
        crate::now_ms,
        crate::instance_epoch(),
        &mut *service,
    )
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
    // 蓝图 64 卷 64.2（WP-64.2.3）：records 一并透传进 Ready——bin 二次
    // fetch 删除。
    let (mut registry, mut obs_sources, mut obs_generation, mut obs_records) = match parts.points {
        PointsPolicy::Eager(keep) => {
            let f = crate::service_cfg::fetch_points(
                &mut ServiceIoAsSource(&mut *io),
                parts.config_client,
                keep,
            );
            (f.registry, f.sources, f.generation, f.records)
        }
        _ => (VecPointRegistry::new(Vec::new()), Vec::new(), 0, Vec::new()),
    };

    // 判定顺序硬约束（F-36）：transport 先于 config。
    let mut factory = match io.transport(TransportKind::Tcp) {
        Ok(factory) => factory,
        Err(e) => {
            emit(line_handover_missing(parts.service, e));
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
            emit(line_config_unavailable(parts.service));
            if parts.degraded == DegradedStyle::HandoverAlias {
                // modbus-tcp 族：try_new 的 Err 再经 _start 判定行（M-6）。
                emit(line_handover_missing(
                    parts.service,
                    "protocol-config-missing",
                ));
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
            emit(alloc::format!("[{}] {k} = {v}", parts.config_client));
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
        obs_records = fetched.records;
    }
    // 39：插件 start 消费配置/传输/点表（签名不动，F-12——插件一行不改）。
    match plugin.start(&cfg, factory.as_mut(), &mut registry) {
        Ok(()) => {
            emit(line_ready(parts.service, parts.capability));
            Ladder::Ready {
                plugin,
                registry,
                sources: obs_sources,
                generation: obs_generation,
                records: obs_records,
            }
        }
        Err(e) => {
            if parts.degraded == DegradedStyle::Standard {
                emit(line_plugin_start_failed(parts.service, &e));
            } else {
                // modbus-tcp 族：无 plugin start FAILED 行，统一 handover
                // 别名（M-6——try_new map_err 单词原因的逐字保真）。
                emit(line_handover_missing(parts.service, "plugin-start-failed"));
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
    /// 蓝图 67 WP-67.2.3（[01] §2.1a）：config 端点惰性绑定一次持有
    /// （原实现每取数重建 endpoint 且 `client_mut()` 绕过 epoch——恒 0
    /// 违纪面；现单 endpoint 跨取数 epoch 单调递增）。
    config_ep: Option<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
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
            config_ep: None,
        }
    }

    /// config 端点惰性绑定（首次取数时一次；None = 槽未授予/建端失败
    /// ——fail-closed 语义同原 per-call bind）。
    fn bind_config_ep(&mut self) -> Option<()> {
        if self.config_ep.is_none() {
            self.config_ep = eneros_agent_sdk::endpoint::ChunkedEndpoint::bind(
                self.service,
                eneros_agent_sdk::config_client::CONFIG_ENDPOINT,
            );
        }
        Some(())
    }
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl ServiceIo for Sel4ServiceIo {
    fn config_entry(&mut self, id: &str) -> Option<Vec<u8>> {
        use eneros_agent_sdk::config_gateway::ConfigEntrySource as _;
        self.bind_config_ep()?;
        self.config_ep.as_mut()?.config_entry(id)
    }

    /// 蓝图 61：GET_GENERATION（观测映射世代；经同一 config 槽）。
    fn config_generation(&mut self) -> Option<u64> {
        use eneros_agent_sdk::config_gateway::ConfigEntrySource as _;
        self.bind_config_ep()?;
        self.config_ep.as_mut()?.config_generation()
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

    /// 两条点记录的 `FIELD_POINTS` payload（1001 = ModbusTcp，1002 =
    /// Iec104——Filtered keep 语义测试用；record 布局 = points.rs 解码序）。
    fn points_payload_two_protocols() -> Vec<u8> {
        use eneros_config_bundle::schema::points::{FIELD_POINTS, PROTO_IEC104, PROTO_MODBUS_TCP};
        let mut rec1 = Vec::new();
        rec1.extend_from_slice(&1001u32.to_le_bytes()); // point_id
        rec1.extend_from_slice(&1u16.to_le_bytes()); // device_id
        rec1.push(PROTO_MODBUS_TCP);
        rec1.push(0); // Analog
        rec1.extend_from_slice(&0.1f64.to_le_bytes()); // scale
        rec1.extend_from_slice(&(-40.0f64).to_le_bytes()); // offset
        rec1.extend_from_slice(&5000u32.to_le_bytes()); // period_ms
        rec1.extend_from_slice(&0.0f64.to_le_bytes()); // deadband
        rec1.push(0); // no range
        rec1.extend_from_slice(&4u16.to_le_bytes()); // addr_len
        rec1.push(1); // slave
        rec1.extend_from_slice(&4097u16.to_le_bytes()); // reg
        rec1.push(3); // func
        let mut rec2 = Vec::new();
        rec2.extend_from_slice(&1002u32.to_le_bytes()); // point_id
        rec2.extend_from_slice(&2u16.to_le_bytes()); // device_id
        rec2.push(PROTO_IEC104);
        rec2.push(0); // Analog
        rec2.extend_from_slice(&1.0f64.to_le_bytes()); // scale
        rec2.extend_from_slice(&0.0f64.to_le_bytes()); // offset
        rec2.extend_from_slice(&3000u32.to_le_bytes()); // period_ms
        rec2.extend_from_slice(&0.0f64.to_le_bytes()); // deadband
        rec2.push(0); // no range
        rec2.extend_from_slice(&5u16.to_le_bytes()); // addr_len
        rec2.extend_from_slice(&1u16.to_le_bytes()); // common_addr
        rec2.extend_from_slice(&100u16.to_le_bytes()); // ioa
        rec2.push(13); // type_id
        let mut table = Vec::new();
        table.extend_from_slice(&2u32.to_le_bytes()); // count
        table.extend_from_slice(&rec1);
        table.extend_from_slice(&rec2);
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
                plugin,
                registry,
                records,
                ..
            } => {
                assert!(plugin.started);
                assert_eq!(plugin.started_point_ids, 1);
                assert_eq!(registry.len(), 1);
                // WP-64.2.3：records 从 PointsFetch 透传（过滤后）——二次
                // fetch 删除的行为锁。
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].point_id, 1001);
                assert_eq!(records[0].period_ms, 5000);
            }
            Ladder::Degraded(_) => panic!("有效配置 + All 点表应 Ready"),
        }
    }

    /// WP-64.2.3：Filtered/Eager 面 records 同样随 keep 过滤透传（非本
    /// 协议点不进 records——配对语义与原 bin 二次 fetch 面等价）。
    #[test]
    fn ready_records_carry_kept_filter_only() {
        let mut entries = valid_protocol_entry();
        entries.push(("points", Some(framed(&points_payload_two_protocols()))));
        let mut io = FakeIo::ok(entries);
        let keep =
            |p: &PointRecord| p.protocol == eneros_config_bundle::points::PointProtocol::ModbusTcp;
        let r = run(
            0,
            &FakeIo::parts(PointsPolicy::Filtered(keep)),
            FakePlugin::new(false),
            &mut io,
        );
        match r {
            Ladder::Ready { records, .. } => {
                assert_eq!(records.len(), 1, "Filtered 面只透传本协议记录");
                assert_eq!(records[0].point_id, 1001);
            }
            Ladder::Degraded(_) => panic!("有效配置应 Ready"),
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

    // --------------------------------------------------------------------
    // 69.3 WP-69.3.1：bin_body 行序快照（改造前 svc-goose.rs 装配序冻结：
    // print_descriptor → run 判定行；69.1 出口窗串口相邻序佐证）+ PARTS
    // 字面锁。F-37 同律：期望侧手抄冻结字面，禁引用 helper。
    // --------------------------------------------------------------------

    /// 改造前 Ready 路径行序（descriptor → READY；FakePlugin descriptor
    /// = id"fake"/Master/SpecBlocked，FakeIo::parts capability 同 goose bin）。
    const BIN_SEQ_READY_EXPECTED: [&str; 2] = [
        "[protocol-service] goose plugin=fake role=Master interop=SpecBlocked",
        "[svc-goose] NET-TRANSPORT READY capability=eneros_iec61850_goose::plugin::GoosePlugin",
    ];
    /// 改造前 Degraded（传输缺席）路径行序（descriptor → handover 判定行）。
    const BIN_SEQ_DEGRADED_EXPECTED: [&str; 2] = [
        "[protocol-service] goose plugin=fake role=Master interop=SpecBlocked",
        "[svc-goose] handover missing (\"no-dma\") —— capability 壳（EXECUTE ST_UNAVAILABLE）",
    ];

    fn seq_expected(seq: &[&str]) -> alloc::vec::Vec<String> {
        seq.iter().map(alloc::string::ToString::to_string).collect()
    }

    /// t-ladder-1：Ready 路径判定行序列与改造前 bin 逐字节相同。
    #[test]
    fn bin_body_ready_path_matches_legacy_line_sequence() {
        let mut io = FakeIo::ok(valid_protocol_entry());
        let a = assemble(
            0,
            &FakeIo::parts(PointsPolicy::None),
            FakePlugin::new(false),
            &mut io,
        );
        assert!(
            matches!(a, Assembled::Ready(_)),
            "有效配置应 Ready（自检窗口在调用方——host 不跑有界自旋）"
        );
        assert_eq!(take_emitted_lines(), seq_expected(&BIN_SEQ_READY_EXPECTED));
    }

    /// t-ladder-2：Degraded 路径判定行序列不变 + 落能力壳
    ///（capability None + EXECUTE 显式 ST_UNAVAILABLE——fail-closed 终态）。
    #[test]
    fn bin_body_degraded_path_falls_back_to_capability_shell() {
        use eneros_drv_svc::runtime::{DriverRequest, DriverResponse, OP_EXECUTE};
        let mut io = FakeIo::ok(valid_protocol_entry());
        io.transport = Err("no-dma");
        let a = assemble(
            0,
            &FakeIo::parts(PointsPolicy::None),
            FakePlugin::new(false),
            &mut io,
        );
        assert_eq!(
            take_emitted_lines(),
            seq_expected(&BIN_SEQ_DEGRADED_EXPECTED)
        );
        let mut svc = match a {
            Assembled::Degraded => capability_shell(),
            Assembled::Ready(_) => panic!("传输缺席应 Degraded"),
        };
        assert_eq!(svc.capability(), None, "能力壳 capability=None");
        // 完整 dispatch 路径锁（runtime::process_request 的映射语义）：
        // capability None + 设备操作 EXECUTE → 显式 ST_UNAVAILABLE。
        let req = DriverRequest::new(0, OP_EXECUTE);
        let mut queue = eneros_drv_svc::runtime::BoundedQueue::new();
        let mut resp = DriverResponse::new(0, 0, 0);
        eneros_drv_svc::runtime::process_request(
            &mut queue,
            &mut *svc,
            0,
            0,
            &eneros_drv_svc::runtime::wire::encode_request(&req),
            &mut resp,
        );
        assert_eq!(
            resp.status,
            eneros_drv_svc::runtime::ST_UNAVAILABLE,
            "能力壳 EXECUTE 显式 ST_UNAVAILABLE（不伪造设备 I/O）"
        );
    }

    /// t-ladder-3：改造 bin 的 `static PARTS` 8 字段与改造前内联字面量
    /// 逐项相同（include_str! 字面锁。运行时断言双保险不可达：bin 非
    /// lib，host 测试无法实例化其 static——字面锁为唯一可达保险，执行
    /// 卡登记）。同时锁改造事实：`_start` 体内不再内联 parts 字面量。
    #[test]
    fn parts_static_is_byte_identical_to_inline_literal() {
        for (name, src, cfg_client, cfg_id, capability, plugin_path) in [
            (
                "svc-goose",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bin/svc-goose.rs")),
                "goose-config",
                "protocol-goose",
                "eneros_iec61850_goose::plugin::GoosePlugin",
                "eneros_iec61850_goose::plugin::GoosePlugin::new",
            ),
            (
                "svc-sv",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bin/svc-sv.rs")),
                "sv-config",
                "protocol-sv",
                "eneros_iec61850_sv::plugin::SvPlugin",
                "eneros_iec61850_sv::plugin::SvPlugin::new",
            ),
        ] {
            assert!(
                src.contains("static PARTS"),
                "{name} 须以 static PARTS 声明描述符（_start 纯接线）"
            );
            assert!(
                src.contains(&alloc::format!("service: \"{name}\",")),
                "{name} PARTS.service 字面漂移"
            );
            assert!(
                src.contains(&alloc::format!("config_client: \"{cfg_client}\",")),
                "{name} PARTS.config_client 字面漂移"
            );
            assert!(
                src.contains(&alloc::format!("config_id: \"{cfg_id}\",")),
                "{name} PARTS.config_id 字面漂移"
            );
            assert!(
                src.contains(&alloc::format!("capability: \"{capability}\",")),
                "{name} PARTS.capability 字面漂移"
            );
            assert!(
                src.contains("points: protocol_ladder::PointsPolicy::None,")
                    || src.contains("points: PointsPolicy::None,"),
                "{name} PARTS.points 字面漂移"
            );
            assert!(
                src.contains("config_dump: false,"),
                "{name} PARTS.config_dump 字面漂移"
            );
            assert!(
                src.contains("self_check_rounds: 150,"),
                "{name} PARTS.self_check_rounds 字面漂移（改造前 150）"
            );
            assert!(
                src.contains("degraded: protocol_ladder::DegradedStyle::Standard,")
                    || src.contains("degraded: DegradedStyle::Standard,"),
                "{name} PARTS.degraded 字面漂移"
            );
            assert!(
                src.contains(&alloc::format!("{plugin_path},")),
                "{name} 插件构造函数指针漂移"
            );
            assert!(
                src.contains("bin_body("),
                "{name} _start 须经 bin_body 接线"
            );
            assert!(
                !src.contains("let parts ="),
                "{name} _start 体内不得再内联 parts 字面量（静态化兑现）"
            );
            assert!(
                src.contains("#[global_allocator]"),
                "{name} allocator 留 bin（链接期约束，WP-69.3.1 边界）"
            );
            assert!(
                src.contains("#[panic_handler]"),
                "{name} panic handler 留 bin（no_std bin 根约束）"
            );
        }
    }

    /// t-ladder-4（变异 M3 覆盖：bin_body 的 Ready 臂自检调用是 target-only
    /// 代码，host 行序不含自检痕迹——按卷册变异表补结构锁）：`bin_body`
    /// 体内必须保有 `self_check_poll(parts.self_check_rounds)` 调用
    ///（46.5 启动期自检 = goose/sv STARTDT/订阅前交换的实弹前提）。
    #[test]
    fn bin_body_keeps_self_check_poll_call() {
        let src = include_str!("protocol_ladder.rs");
        let needle = concat!("io.self_check_poll(", "parts.self_check_rounds)");
        assert!(
            src.contains(needle),
            "bin_body Ready 臂自检调用被删（46.5 启动期交换窗口丢失）"
        );
        // 轮数单点：自检轮数只能来自 PARTS（bin 侧 150 字面由 t-ladder-3 锁）。
        assert!(
            !src.contains(concat!("self_check_poll(", "150")),
            "自检轮数必须单点自 PARTS，禁止 bin_body 内字面轮数"
        );
    }
}
