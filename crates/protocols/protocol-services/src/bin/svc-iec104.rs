//! svc-iec104 — IEC 60870-5-104 协议 EL0 服务（S1：装配阶梯单源
//! `protocol_ladder`，bin 纯接线；46.3 配置即 peer 唯一来源 + 配置回显；
//! **蓝图 61 v0.314.31 [07] §3.3：真实收包路径迁入 `IngestionModule` 深模块
//! （与 v0.314.30 Modbus 同构）**——总召/自发收包经 Iec104IngestAdapter
//! drain（CP56 时标/品质逐位映射/重复 ASDU 去重），来源认证、点映射、QoS、
//! 背压、事件日志全部在 module；事件 wire 帧为唯一上行）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate eneros_protocol_services;

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;

#[cfg(target_os = "none")]
use eneros_protocol_abstract::plugin::ProtocolPlugin;
#[cfg(target_os = "none")]
use eneros_protocol_abstract::PointRegistry;
#[cfg(target_os = "none")]
use eneros_protocol_services::protocol_ladder;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 45.5 + v0.314.31：点表驱动 + 事件平面接入的 iec104 I/O 服务。
///
/// `_start` 收到 root 移交的 virtio-net 设备 → 经阶梯构造 smoltcp TCP 传输
/// 并连接外部 IEC 104 从站 peer（peer 经 46.3 配置注入）；点表经 EIPC 从
/// svc-config 注入（**不自造地址**）。`EXECUTE`（非写载荷）触发采集轮：
/// `IngestionModule::tick` → adapter 驱动插件（STARTDT/总召唤调度/收帧）→
/// drain 数据面（去重/时标/品质）→ 原始值推送 module（映射/工程量/身份/
/// QoS/背压在 module 单点）→ 事件 wire 帧分批 INGEST 到 svc-telemetry。
///
/// 身份三事实全部来自签名配置（[07] §2.4）：source_id = `[[source]]` 表
/// iec104 行；source_epoch = registry `open_source`（floor = boot 单调时刻
/// ——持久 boot counter 缺口 = ADR-0047）；mapping_version = bundle
/// generation。
#[cfg(target_os = "none")]
struct Iec104IoService {
    /// 蓝图 61：接入深模块（调度/准入/背压/事件日志单点）。
    module: eneros_ingestion_core::IngestionModule,
    /// 本服务来源 ID（配置分配；重连登记用）。
    source_id: u32,
    /// 共享插件（收包=adapter drain；自检=启动期 PluginIoService 轮）。
    plugin: eneros_protocol_services::iec104_ingest::SharedIec104Plugin,
    /// 遥测上行端（交付半环单源——分批/chunk/解析/epoch 递增内化；
    /// WP-62.2.3）。
    uplink: eneros_protocol_services::ingestion_stage::TelemetryUplink<
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    >,
    transactions: u32,
}

#[cfg(target_os = "none")]
impl eneros_drv_svc::runtime::DriverService for Iec104IoService {
    fn capability(&self) -> Option<&'static str> {
        // 与 READY 行 `capability=` 词形同一事实（F-42 冻结）。
        Some("eneros_iec104_master::plugin::Iec104Plugin")
    }

    fn handle(
        &mut self,
        req: &eneros_drv_svc::runtime::DriverRequest,
        resp: &mut eneros_drv_svc::runtime::DriverResponse,
    ) {
        use eneros_control_gateway::wire::{OutcomeCode, OutcomeFrame};
        use eneros_drv_svc::runtime::*;
        match req.opcode {
            OP_EXECUTE => {
                self.transactions += 1;
                // 出站写载荷（17B 子码）？[07] §3.3：select/execute 控制不在
                // 本版本开放——fail-closed Denied（未来仍必须走
                // AuthorizedWriter，不旁路）。
                if eneros_protocol_abstract::write::decode_master_write_payload_v2(
                    &req.payload[..req.payload_len as usize],
                )
                .is_some()
                {
                    eneros_runtime::println!(
                        "[svc-iec104] MASTER-WRITE 不开放（[07] §3.3）—— Denied(2)"
                    );
                    resp.status = ST_OK;
                    resp.payload[..2]
                        .copy_from_slice(&OutcomeFrame::new(OutcomeCode::Denied, 2).encode());
                    resp.payload_len = 2;
                    return;
                }
                // 采集轮（交付半环单源——WP-62.2.3 IngestionStage）。
                let round_good = eneros_protocol_services::ingestion_stage::run_acq_round(
                    &mut self.module,
                    &mut self.uplink,
                    "svc-iec104",
                    eneros_drv_svc::now_ns(),
                );
                if round_good {
                    resp.status = ST_OK;
                } else {
                    // 本轮无任何成功收包（如拔 peer）→ fail-closed 指示。
                    resp.status = ST_UNAVAILABLE;
                }
            }
            _ => {
                resp.status = ST_INVALID;
            }
        }
    }
}

/// 装配接入深模块（[07] §3.6.5：接 profile/source descriptors；与
/// svc-modbus-tcp `build_module` 同构）。
///
/// 蓝图 64 卷 64.2（WP-64.2.2）：64 行点计划孪生收编 lib
/// `plans_from_records`；本 bin 只保留协议差异点——Iec104 地址反构造闭包
/// （mappings 表经 on_planned 在配对成功后构建）+ period 0 → 1s drain
/// 节奏缺省（[07] §2.3 事件驱动在调度面不可达）。
#[cfg(target_os = "none")]
fn build_module(
    sources: &[(u32, eneros_ingestion_core::SourceDescriptor)],
    own_source_id: u32,
    registry: &eneros_protocol_abstract::VecPointRegistry,
    records: &[eneros_config_bundle::points::PointRecord],
    plugin: eneros_protocol_services::iec104_ingest::SharedIec104Plugin,
) -> Option<eneros_ingestion_core::IngestionModule> {
    use eneros_protocol_services::iec104_ingest::IecMapping;
    use eneros_protocol_services::ingestion_stage::{new_reference_module, plans_from_records};
    let mut module = new_reference_module(sources, own_source_id)?;
    // 地址反构造：Iec104 → (reg_addr=ioa, 余 0)；mappings 表经 on_planned
    // 在**点表记录配对成功后**构建（与原 bin 一致——addr_of 保持纯查询，
    // "地址匹配但缺记录"的点不产生 mapping 行，64.2 复查硬伤修正）。
    // 非 Iec104 地址的映射点跳过（原 bin 过滤语义）。
    let mappings = core::cell::RefCell::new(alloc::vec::Vec::<IecMapping>::new());
    let plans = plans_from_records(
        records,
        own_source_id,
        registry,
        |m| match m.protocol_addr {
            eneros_protocol_abstract::ProtocolAddress::Iec104 { ioa, .. } => {
                Some(eneros_protocol_services::ingestion_stage::PlanAddr {
                    slave_addr: 0,
                    reg_addr: ioa,
                    func_code: 0,
                })
            }
            _ => None,
        },
        // [07] §2.3：事件驱动（period_ms=0）在 module 调度面不可达，以点表
        // period 为 drain 节奏（0 → 1s 缺省）；协议本体仍事件驱动。
        1_000,
        |m, _addr| {
            if let eneros_protocol_abstract::ProtocolAddress::Iec104 {
                common_addr, ioa, ..
            } = m.protocol_addr
            {
                mappings.borrow_mut().push(IecMapping {
                    common_addr,
                    ioa: u32::from(ioa),
                    point_id: m.point_id,
                    device_id: m.device_id,
                });
            }
        },
    );
    module.install_points(1, plans).ok()?;
    let adapter = Box::new(
        eneros_protocol_services::iec104_ingest::Iec104IngestAdapter::new(
            own_source_id,
            plugin,
            mappings.into_inner(),
        ),
    );
    module.attach(adapter).ok()?;
    Some(module)
}

/// 46.2/46.3 + S1：手柄移交（virtio-net 槽）→ 统一传输工厂 → 配置装配
/// 插件；未移交/配置不可得/装配失败 → 能力壳（fail-closed）。
///
/// 蓝图 61 v0.314.31 装配序（最小侵入登记）：Ready（插件已 start/传输
/// 建立）→ `PluginIoService::self_check_poll`（46.5 启动期自检窗口，own
/// plugin——覆盖 STARTDT→总召唤启动交换）→ `into_plugin` 包
/// `Rc<RefCell>` 共享句柄 → descriptors/点表 → `build_module`（挂
/// Iec104IngestAdapter）→ open → 启动期一轮真实采集 → EXECUTE 轮
/// `module.tick` 单点驱动。
#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    wall_epoch: u64,
) -> ! {
    use eneros_config_bundle::points::PointProtocol;
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    let parts = protocol_ladder::ProtocolParts {
        service: "svc-iec104",
        config_client: "iec104-config",
        config_id: "protocol-iec104",
        capability: "eneros_iec104_master::plugin::Iec104Plugin",
        // 46.5：点表经 svc-config 注入——104 设备映射 → STARTDT/总召唤驱动
        // （全量：插件内部按 ProtocolAddress::Iec104 过滤；本 bin 二次
        // fetch 时按 Iec104 过滤——与 modbus bin 二次 fetch 同先例）。
        points: protocol_ladder::PointsPolicy::All,
        // 46.5 诊断：回显实际配置值（poll_interval 是否 5000）。
        config_dump: true,
        // 46.5：自检轮询窗口须覆盖 STARTDT→总召唤启动交换（QEMU 实弹判据
        // ——300 轮≈6s 使首个召唤落在窗口内）。
        self_check_rounds: 300,
        degraded: protocol_ladder::DegradedStyle::Standard,
    };
    let mut io = protocol_ladder::Sel4ServiceIo::new(
        parts.service,
        parts.config_client,
        mmio_vaddr,
        dma_vaddr,
        dma_phys,
    );
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_iec104_master::plugin::Iec104Plugin::new();
    protocol_ladder::print_descriptor(parts.service, &plugin);
    // 遥测上行端装配单源（WP-64.2.3：bound/NOT-granted 词形归模块；
    // 绑定时机在阶梯 run 前 = 串口行序不变）。
    let telemetry = eneros_protocol_services::ingestion_stage::bind_telemetry("svc-iec104");
    // 43：设备移交（virtio-net 槽）→ 真实 I/O 服务；未移交 → 能力壳。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match protocol_ladder::run(wall_epoch, &parts, plugin, &mut io) {
            protocol_ladder::Ladder::Ready {
                plugin,
                registry,
                sources,
                generation,
                records,
            } => {
                // 启动期自检（own plugin——传输建立/STARTDT 窗口）；完成
                // 后取回所有权包共享句柄（收包=adapter；见 _start 装配序）。
                let mut check = eneros_protocol_services::service::PluginIoService::new(
                    plugin,
                    parts.capability,
                );
                check.self_check_poll(parts.self_check_rounds);
                let plugin = alloc::rc::Rc::new(core::cell::RefCell::new(check.into_plugin()));
                let _ = generation; // generation 已注入 mapping.identity
                                    // 装配序列单源（蓝图 63 卷 63.2 WP-63.2.4）：descriptor
                                    // 循环→build→open→uplink→自检→壳兜底进 lib；本 bin 只保留
                                    // 协议差异点。schema 谓词 = **协议地址谓词**（仅 Iec104
                                    // 地址的映射点——与 modbus 的范围口径差异参数化保留）。
                                    // records 由 Ready 携带（WP-64.2.3：二次 fetch 删除——
                                    // POINTS-LOADED 2→1 已登记；All 面记录未过滤，配对按
                                    // point_id 与原 Iec104 过滤面等价）。
                eneros_protocol_services::ingestion_stage::build_ingested_service(
                    "svc-iec104",
                    "no iec104 [[source]] row",
                    &sources,
                    PointProtocol::Iec104,
                    registry,
                    |reg, id| {
                        reg.resolve(id).and_then(|m| {
                            let is_iec104 = matches!(
                                m.protocol_addr,
                                eneros_protocol_abstract::ProtocolAddress::Iec104 { .. }
                            );
                            if is_iec104 {
                                Some(m.identity.schema_id)
                            } else {
                                None
                            }
                        })
                    },
                    |reg, descriptors, own| {
                        build_module(descriptors, own, reg, &records, plugin.clone())
                    },
                    telemetry,
                    |module, own, uplink, _registry| {
                        Box::new(Iec104IoService {
                            module,
                            source_id: own,
                            plugin: plugin.clone(),
                            uplink,
                            transactions: 0,
                        })
                    },
                )
            }
            protocol_ladder::Ladder::Degraded(_) => protocol_ladder::capability_shell(),
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "iec104",
        ready_ntfn,
        eneros_protocol_services::now_ms,
        eneros_protocol_services::instance_epoch(),
        &mut *service,
    )
}

eneros_runtime::svc_panic!("svc-iec104");

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
