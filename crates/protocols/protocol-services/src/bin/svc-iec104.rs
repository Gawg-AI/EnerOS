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
use core::panic::PanicInfo;

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

/// OBS ingest OK 响应长度（[OK][accepted u16][rejected u16]）。
#[cfg(target_os = "none")]
const OBS_RESP_OK_LEN: usize = 5;

/// 单次 chunk 传输的 OBS 帧上限 = (1664B 逻辑上限 − 7B 头) / 128B 帧。
#[cfg(target_os = "none")]
const OBS_FRAMES_PER_TRANSFER: usize =
    (eneros_agent_sdk::chunked::CHUNK_COUNT_MAX as usize * eneros_agent_sdk::CHUNK_DATA_MAX - 7)
        / eneros_telemetry_model::OBSERVATION_WIRE_LEN;

#[cfg(target_os = "none")]
const _: () = assert!(OBS_FRAMES_PER_TRANSFER >= 1);

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
    /// svc-telemetry EIPC 客户端。
    telemetry: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 遥测分块 epoch（单调）。
    tel_epoch: u64,
    /// INGEST 累计接受计数（观测——真实采集链）。
    pub accepted_total: u64,
    transactions: u32,
}

#[cfg(target_os = "none")]
impl Iec104IoService {
    /// 采集轮（v0.314.31：module.tick 单点驱动——状态机/收包/去重/准入/
    /// 事件日志）。事件 wire 帧经 sink drain 分批上报（ObservationEvent =
    /// 唯一上行；事件身份/工程量/品质在 module 已定型）。ACQ 证据行从接受
    /// 事件解码打印（单一事实——与上行 wire 同源）。
    fn collect_and_report(&mut self, _now_ms: u64) -> bool {
        use eneros_ingestion_core::TickBudget;
        let now_ns = eneros_drv_svc::now_ns();
        // 1. 采集轮：周期调度 → 状态机/收包 drain → 准入 → 事件日志。
        let report = self.module.tick(
            now_ns,
            TickBudget {
                max_events: 256,
                max_transactions: 16,
            },
        );
        // 2. 对端重连登记（重连不换本地 source epoch——[07] §2.4）。
        self.module.take_reconnects(self.source_id);
        // 3. watermark 检查点（批量间隔持久化；save 失败显式——不静默）。
        if let Err(e) = self.module.checkpoint() {
            eneros_runtime::println!("[svc-iec104] OBS-CHECKPOINT FAIL: {e:?}");
        }
        // 4. 事件交付：drain → ACQ 证据行（同源 wire 解码）+ 分批上报。
        let frames = self.module.drain_frames();
        let mut any_good = false;
        let mut frame_buf: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        for wire in &frames {
            let Ok(ev) = eneros_telemetry_model::event::ObservationEvent::decode_v1(wire) else {
                continue;
            };
            let eng = match ev.value {
                eneros_telemetry_model::event::ObservationValue::F64(v) => v,
                eneros_telemetry_model::event::ObservationValue::I64(v) => v as f64,
                eneros_telemetry_model::event::ObservationValue::U64(v) => v as f64,
                _ => 0.0,
            };
            let bad = ev.quality_bits & eneros_telemetry_model::event::quality_bits::VALID == 0;
            if bad {
                eneros_runtime::println!(
                    "[svc-iec104] ACQ point={} q=bad (quality={:#06x})",
                    ev.point_id,
                    ev.quality_bits
                );
            } else {
                any_good = true;
                eneros_runtime::println!(
                    "[svc-iec104] ACQ point={} value={} q=good seq={}",
                    ev.point_id,
                    eng,
                    ev.sequence
                );
            }
            frame_buf.extend_from_slice(wire);
        }
        if report.rejected > 0 || report.backpressured > 0 {
            eneros_runtime::println!(
                "[svc-iec104] ACQ-ROUND rejected={} backpressured={} (rate-limited={})",
                report.rejected,
                report.backpressured,
                report.rate_limited
            );
            // 拒绝原因逐条可观测（[07] §2.5：一切丢弃可观测）。
            for (point, reason) in self.module.take_reject_log() {
                eneros_runtime::println!(
                    "[svc-iec104] OBS-REJECT point={} reason={:?}",
                    point,
                    reason
                );
            }
        }
        self.report_obs(&mut frame_buf);
        any_good
    }

    /// OBS 批次上报（[4][source u32][count u16][n × 128B]；分批同 [06]
    /// §4.2——单次 chunk 传输上限 1664B，超限轮次自动分批）。
    fn report_obs(&mut self, frame_buf: &mut alloc::vec::Vec<u8>) {
        if frame_buf.is_empty() {
            return;
        }
        let Some(client) = &mut self.telemetry else {
            eneros_runtime::println!(
                "[svc-iec104] telemetry EP not granted —— 事件未上报（fail-closed）"
            );
            return;
        };
        let frames = frame_buf.as_slice();
        let total = frames.len() / eneros_telemetry_model::OBSERVATION_WIRE_LEN;
        let mut accepted_total: u32 = 0;
        let mut all_ok = true;
        for batch in
            frames.chunks(eneros_telemetry_model::OBSERVATION_WIRE_LEN * OBS_FRAMES_PER_TRANSFER)
        {
            let count = (batch.len() / eneros_telemetry_model::OBSERVATION_WIRE_LEN) as u16;
            let mut req: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(7 + batch.len());
            req.push(4u8); // TEL_INGEST_OBS
            req.extend_from_slice(&self.source_id.to_le_bytes());
            req.extend_from_slice(&count.to_le_bytes());
            req.extend_from_slice(batch);
            let mut transfer =
                eneros_agent_sdk::ChunkedClient::new(&mut *client, self.tel_epoch, 5_000);
            self.tel_epoch = self.tel_epoch.saturating_add(1);
            match transfer.transfer("svc-telemetry", &req) {
                Ok(resp) if resp.len() >= OBS_RESP_OK_LEN && resp[0] == 0 => {
                    accepted_total += u32::from(u16::from_le_bytes([resp[1], resp[2]]));
                }
                Ok(resp) => {
                    all_ok = false;
                    eneros_runtime::println!(
                        "[svc-iec104] OBS-ROUND rejected {:02x?}",
                        &resp[..resp.len().min(8)]
                    );
                    break;
                }
                Err(e) => {
                    all_ok = false;
                    eneros_runtime::println!("[svc-iec104] OBS-ROUND eipc={:?}", e);
                    break;
                }
            }
        }
        if all_ok {
            self.accepted_total = self
                .accepted_total
                .saturating_add(u64::from(accepted_total));
            eneros_runtime::println!(
                "[svc-iec104] OBS-ROUND sent={} accepted={}",
                total,
                accepted_total
            );
        }
    }
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
                // 采集轮（module.tick 单点驱动）。
                let round_good = self.collect_and_report(eneros_protocol_services::now_ms());
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
/// 点计划从点表 `ProtocolAddress::Iec104` 行构建（映射/身份两路同源——同一
/// 次 bundle 解码）；adapter 映射表同步构建。协议侧功能码/寄存器/从站字段
/// 填 0（仅 Modbus 组读用；iec104 adapter 用自己的 mappings 表）。
#[cfg(target_os = "none")]
fn build_module(
    sources: &[(u32, eneros_ingestion_core::SourceDescriptor)],
    own_source_id: u32,
    registry: &eneros_protocol_abstract::VecPointRegistry,
    records: &[eneros_config_bundle::points::PointRecord],
    plugin: eneros_protocol_services::iec104_ingest::SharedIec104Plugin,
) -> Option<eneros_ingestion_core::IngestionModule> {
    use eneros_ingestion_core::{
        EventQos, InMemorySink, IngestionModule, MemEpochStore, OperationalProfile, PointPlan,
    };
    use eneros_protocol_services::iec104_ingest::{Iec104IngestAdapter, IecMapping};
    let mut module = IngestionModule::new(
        Box::new(InMemorySink::new(1024)),
        Box::new(MemEpochStore::default()),
        OperationalProfile::reference(),
    );
    for (_, d) in sources {
        module.register(d.clone());
    }
    if !sources.iter().any(|(id, _)| *id == own_source_id) {
        return None;
    }
    let mut plans: alloc::vec::Vec<PointPlan> = alloc::vec::Vec::new();
    let mut mappings: alloc::vec::Vec<IecMapping> = alloc::vec::Vec::new();
    let mut ids: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    registry.collect_ids(&mut ids);
    for id in ids {
        let Some(m) = registry.resolve(id) else {
            continue;
        };
        // 遥测点（analog/digital）才进采集调度；控制/设点/计数点不采集。
        match m.data_type {
            eneros_upa_model::PointType::Analog | eneros_upa_model::PointType::Digital => {}
            _ => continue,
        }
        let (common_addr, ioa) = match m.protocol_addr {
            eneros_protocol_abstract::ProtocolAddress::Iec104 {
                common_addr, ioa, ..
            } => (common_addr, ioa),
            _ => continue,
        };
        // 调度字段来自点表记录（缺记录 fail-closed：无调度的点不进计划）。
        let Some(rec) = records.iter().find(|r| r.point_id == m.point_id) else {
            continue;
        };
        // [07] §2.3 事件驱动（period_ns=0）在 module 调度面不可达（tick 的
        // collect_due 只调度 period>0 点；module 公共 API 不动——[07] §3.1
        // 纪律），故以点表 period_ms 为 drain 节奏（0 → 1s 缺省）；协议本体
        // 仍事件驱动（总召/自发上送，drain 去重保证不重复发射）。
        let period_ms = if rec.period_ms == 0 {
            1_000
        } else {
            rec.period_ms
        };
        let period_ns = u64::from(period_ms) * 1_000_000;
        mappings.push(IecMapping {
            common_addr,
            ioa: u32::from(ioa),
            point_id: m.point_id,
            device_id: m.device_id,
        });
        plans.push(PointPlan {
            source_id: own_source_id,
            point_id: m.point_id,
            device_id: m.device_id,
            period_ns,
            phase_offset_ns: 0,
            deadline_ns: period_ns.saturating_mul(2),
            // 参考闭环 QoS：Q1 + 不可抑制（IEC104 品质变化/总召全量上报，
            // 与 modbus bin 同判；死区/QoS 分级在配置面扩展后生效）。
            qos: EventQos::Q1,
            suppressible: false,
            deadband_abs: rec.deadband,
            deadband_rel: 0.0,
            timeout_ns: 1_000_000_000,
            max_retries: 2,
            schema_id: m.identity.schema_id,
            schema_version: m.identity.schema_version,
            unit_code: m.identity.unit_code,
            asset_id: m.identity.asset_id,
            mapping_version: m.identity.mapping_generation,
            scale: m.scale,
            offset: m.offset,
            // 协议组计划字段仅 Modbus 消费——iec104 用 adapter mappings 表。
            func_code: 0,
            reg_addr: ioa,
            slave_addr: 0,
        });
    }
    module.install_points(1, plans).ok()?;
    let adapter = Box::new(Iec104IngestAdapter::new(own_source_id, plugin, mappings));
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
    let telemetry =
        eneros_agent_sdk::eipc_resolve("svc-iec104", "svc-telemetry").and_then(|(_, slot)| {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "iec104-telemetry",
                slot,
                eneros_drv_svc::now_ns,
            )
            .ok()
        });
    if telemetry.is_some() {
        eneros_runtime::println!("[svc-iec104] telemetry EIPC bound (svc-telemetry)");
    } else {
        eneros_runtime::println!("[svc-iec104] telemetry EIPC NOT granted");
    }
    // 43：设备移交（virtio-net 槽）→ 真实 I/O 服务；未移交 → 能力壳。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match protocol_ladder::run(wall_epoch, &parts, plugin, &mut io) {
            protocol_ladder::Ladder::Ready {
                plugin,
                registry,
                sources,
                generation,
            } => {
                // 启动期自检（own plugin——传输建立/STARTDT 窗口）；完成
                // 后取回所有权包共享句柄（收包=adapter；见 _start 装配序）。
                let mut check = eneros_protocol_services::service::PluginIoService::new(
                    plugin,
                    parts.capability,
                );
                check.self_check_poll(parts.self_check_rounds);
                let plugin = alloc::rc::Rc::new(core::cell::RefCell::new(check.into_plugin()));
                // 调度字段取数（period_ms/deadband；Iec104 过滤——二次
                // fetch 先例 = modbus bin）。
                let fetched = eneros_protocol_services::service_cfg::fetch_points(
                    &mut protocol_ladder::ServiceIoAsSource(&mut io),
                    parts.config_client,
                    |p| p.protocol == PointProtocol::Iec104,
                );
                let records = fetched.records;
                // 来源 descriptor（iec104 行 = 本适配器身份；[07] §2.4）。
                let mut descriptors: alloc::vec::Vec<(
                    u32,
                    eneros_ingestion_core::SourceDescriptor,
                )> = alloc::vec::Vec::new();
                let mut own_source_id: Option<u32> = None;
                for src in &sources {
                    if src.protocol == PointProtocol::Iec104 {
                        descriptors.push((
                        src.source_id,
                        eneros_ingestion_core::SourceDescriptor {
                            source_id: src.source_id,
                            tenant_id: src.tenant_id,
                            site_id: src.site_id,
                            allow_control: src.allow_control,
                            require_integrity: src.require_integrity,
                            point_range: (src.point_min, src.point_max),
                            max_events_per_s: src.max_events_per_s,
                            allowed_schemas: {
                                // allowed_schemas 从点表 identity 收集
                                //（仅 Iec104 行——本来源授权面）。
                                let mut ids = alloc::vec::Vec::new();
                                registry.collect_ids(&mut ids);
                                let mut v = alloc::vec::Vec::new();
                                for id in ids {
                                    if let Some(m) = registry.resolve(id) {
                                        let is_iec104 = matches!(
                                            m.protocol_addr,
                                            eneros_protocol_abstract::ProtocolAddress::Iec104 { .. }
                                        );
                                        if is_iec104
                                            && m.identity.schema_id != 0
                                            && !v.contains(&m.identity.schema_id)
                                        {
                                            v.push(m.identity.schema_id);
                                        }
                                    }
                                }
                                v
                            },
                            clock_id: 1,
                            enabled: true,
                        },
                    ));
                        if own_source_id.is_none() {
                            own_source_id = Some(src.source_id);
                        }
                    }
                }
                // 装配序列（任一失败 → 能力壳 fail-closed；labeled block
                // 汇聚到统一的 Box<dyn DriverService> 臂值）。
                let built: Option<Box<dyn eneros_drv_svc::runtime::DriverService>> = 'build: {
                    let Some(own) = own_source_id else {
                        eneros_runtime::println!(
                            "[svc-iec104] OBS-SOURCE disabled (no iec104 [[source]] row)"
                        );
                        break 'build None;
                    };
                    let Some(mut module) =
                        build_module(&descriptors, own, &registry, &records, plugin.clone())
                    else {
                        eneros_runtime::println!("[svc-iec104] OBS-MODULE build FAIL");
                        break 'build None;
                    };
                    // 开源（floor = boot 单调时刻；持久 boot counter = ADR-0047）。
                    let epoch_floor = eneros_protocol_services::now_ms();
                    match module.open(own, epoch_floor) {
                        Ok(epoch) => {
                            eneros_runtime::println!(
                                "[svc-iec104] OBS-SOURCE id={} epoch={} (config-allocated)",
                                own,
                                epoch
                            );
                        }
                        Err(e) => {
                            eneros_runtime::println!("[svc-iec104] OBS-SOURCE open FAIL: {e:?}");
                            break 'build None;
                        }
                    }
                    let _ = generation; // generation 已注入 mapping.identity
                    let mut io = Iec104IoService {
                        module,
                        source_id: own,
                        plugin,
                        telemetry,
                        tel_epoch: 0,
                        accepted_total: 0,
                        transactions: 0,
                    };
                    // 启动期自检：一轮真实采集（外部 peer 记录事务；串口
                    // 打印 = L2 互操作/46 采集证据行）。
                    io.collect_and_report(eneros_protocol_services::now_ms());
                    break 'build Some(Box::new(io));
                };
                built.unwrap_or_else(protocol_ladder::capability_shell)
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

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
