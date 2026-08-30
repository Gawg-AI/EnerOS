//! svc-modbus-tcp — Modbus TCP 协议 EL0 服务（蓝图 39 WP-39.12；43 L2
//! 互操作接线；S1 装配阶梯；**蓝图 61 v0.314.30 [07] §2：接入改走
//! `IngestionModule` 深模块**——来源认证、点映射、周期调度、QoS、背压、
//! 事件日志全部在 module；adapter 只做协议状态机与原始值。legacy 58B
//! telemetry INGEST wire 自本卷起不再由生产协议代码拼装）。

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

/// 47：读回容差（kW）——值单源 = control-gateway
/// `PCS_SETPOINT_READBACK_TOL_KW`（与签发侧 ControlContract.tolerance_abs
/// 同一事实；历史手抄常量退役）。
pub use eneros_control_gateway::PCS_SETPOINT_READBACK_TOL_KW as MASTER_READBACK_TOL_KW;

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

/// 45.5 + v0.314.30：点表驱动 + 事件平面接入的 modbus-tcp I/O 服务。
///
/// `_start` 收到 root 移交的 virtio-net 设备 → 经阶梯构造 smoltcp TCP 传输
/// 并连接外部 Modbus-TCP 从站 peer；点表经 EIPC 从 svc-config 注入（**不自
/// 造地址**）。`EXECUTE`（非写载荷）触发采集轮：`IngestionModule::tick` 按
/// 点表周期调度到期点 → adapter 组读（相邻寄存器按设备/功能码/连续地址合
/// 并）→ 原始值推送 module（映射/工程量/身份/去重/QoS/背压在 module 单点）
/// → 事件 wire 帧分批 INGEST 到 svc-telemetry（ ObservationEvent = 唯一上
/// 行；legacy 58B INGEST wire 已退役）。
///
/// 身份三事实全部来自签名配置（[07] §2.4）：source_id = `[[source]]` 表
/// modbus-tcp 行；source_epoch = registry `open_source`（floor = boot 单调
/// 时刻——持久 boot counter 缺口 = ADR-0047）；mapping_version = bundle
/// generation。
#[cfg(target_os = "none")]
struct ModbusTcpIoService {
    /// 蓝图 61：接入深模块（调度/准入/死区/背压/事件日志单点）。
    module: eneros_ingestion_core::IngestionModule,
    /// 本服务来源 ID（配置分配；写路径与重连登记用）。
    source_id: u32,
    /// 共享插件（读=adapter 组读；写=出站写）。
    plugin: eneros_protocol_services::modbus_ingest::SharedPlugin,
    /// 点表（写路径解析 + readback）。
    registry: eneros_protocol_abstract::VecPointRegistry,
    /// svc-telemetry EIPC 客户端。
    telemetry: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 遥测分块 epoch（单调）。
    tel_epoch: u64,
    /// INGEST 累计接受计数（观测——真实采集链）。
    pub accepted_total: u64,
    transactions: u32,
}

#[cfg(target_os = "none")]
impl ModbusTcpIoService {
    /// 采集轮（v0.314.30：module.tick 单点驱动——调度/组读/准入/事件）。
    ///
    /// 事件 wire 帧经 sink drain 分批上报（ObservationEvent = 唯一上行；
    /// 事件身份/工程量/品质在 module 已定型）。ACQ 证据行从接受事件解码
    /// 打印（单一事实——与上行 wire 同源）。
    fn collect_and_report(&mut self, _now_ms: u64) -> bool {
        use eneros_ingestion_core::TickBudget;
        let now_ns = eneros_drv_svc::now_ns();
        // 1. 采集轮：周期调度 → 组读 → 准入 → 事件日志（背压/拒绝可见）。
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
            eneros_runtime::println!("[svc-modbus-tcp] OBS-CHECKPOINT FAIL: {e:?}");
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
                    "[svc-modbus-tcp] ACQ point={} q=bad (quality={:#06x})",
                    ev.point_id,
                    ev.quality_bits
                );
            } else {
                any_good = true;
                eneros_runtime::println!(
                    "[svc-modbus-tcp] ACQ point={} value={} q=good seq={}",
                    ev.point_id,
                    eng,
                    ev.sequence
                );
            }
            frame_buf.extend_from_slice(wire);
        }
        if report.rejected > 0 || report.backpressured > 0 {
            eneros_runtime::println!(
                "[svc-modbus-tcp] ACQ-ROUND rejected={} backpressured={} (rate-limited={})",
                report.rejected,
                report.backpressured,
                report.rate_limited
            );
            // 拒绝原因逐条可观测（[07] §2.5：一切丢弃可观测）。
            for (point, reason) in self.module.take_reject_log() {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] OBS-REJECT point={} reason={:?}",
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
                "[svc-modbus-tcp] telemetry EP not granted —— 事件未上报（fail-closed）"
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
                        "[svc-modbus-tcp] OBS-ROUND rejected {:02x?}",
                        &resp[..resp.len().min(8)]
                    );
                    break;
                }
                Err(e) => {
                    all_ok = false;
                    eneros_runtime::println!("[svc-modbus-tcp] OBS-ROUND eipc={:?}", e);
                    break;
                }
            }
        }
        if all_ok {
            self.accepted_total = self
                .accepted_total
                .saturating_add(u64::from(accepted_total));
            eneros_runtime::println!(
                "[svc-modbus-tcp] OBS-ROUND sent={} accepted={}",
                total,
                accepted_total
            );
        }
    }

    /// 47（WP-47.1 协议出站写）：执行主站写 + 独立 readback，返回 2B
    /// `OutcomeFrame`。**本方法只执行**——授权已由 ControlDispatch 九步链
    /// 完成（不双头授权）。
    fn master_write_v2(
        &mut self,
        device: u32,
        point: u32,
        value: eneros_telemetry_model::command::CommandValue,
        command_digest: [u8; 32],
    ) -> [u8; 2] {
        use eneros_control_gateway::wire::{OutcomeCode, OutcomeFrame};
        use eneros_protocol_abstract::write::semantic_command_digest;
        let computed = semantic_command_digest(device, point, value);
        if computed != command_digest {
            eneros_runtime::println!(
                "[svc-modbus-tcp] MASTER-WRITE device={device} point={point} digest-mismatch —— Denied(2)"
            );
            return OutcomeFrame::new(OutcomeCode::Denied, 2).encode();
        }
        let Some(mapping) = self.registry.resolve(point) else {
            eneros_runtime::println!(
                "[svc-modbus-tcp] MASTER-WRITE point={point} 无点表映射 —— Denied(7)"
            );
            return OutcomeFrame::new(OutcomeCode::Denied, 7).encode();
        };
        if mapping.device_id != device as u16 {
            eneros_runtime::println!(
                "[svc-modbus-tcp] MASTER-WRITE device={device} != 点表 device={} —— Denied(7)",
                mapping.device_id
            );
            return OutcomeFrame::new(OutcomeCode::Denied, 7).encode();
        }
        let now = eneros_protocol_services::now_ms();
        let mut cd_hex = [0u8; 16];
        hex16_into(&command_digest, &mut cd_hex);
        let cd = core::str::from_utf8(&cd_hex).unwrap_or("?");
        let power = match value {
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(v) => v,
            _ => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] MASTER-WRITE point={point} 命令类别与模拟量寄存器不符 —— Denied(7)"
                );
                return OutcomeFrame::new(OutcomeCode::Denied, 7).encode();
            }
        };
        match self.plugin.borrow_mut().write_point(
            now,
            mapping,
            eneros_upa_model::PointValue::Float(power),
        ) {
            Ok(()) => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] MASTER-WRITE device={device} point={point} power={power} cd={cd} ACK"
                );
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] MASTER-WRITE device={device} point={point} 写失败 {e:?} —— Denied(8)"
                );
                return OutcomeFrame::new(OutcomeCode::Denied, 8).encode();
            }
        }
        // 独立 readback（重读该点；值比对——不得回显刚写载荷）。
        match self.plugin.borrow_mut().read_point(now, mapping) {
            Ok(read) if read.ok => {
                let eng = mapping.to_engineering(read.raw);
                let ok = (eng - power).abs() <= MASTER_READBACK_TOL_KW.max(f64::EPSILON);
                let td = eneros_protocol_abstract::write::semantic_command_digest(
                    device,
                    point,
                    eneros_telemetry_model::command::CommandValue::ActivePowerKw(eng),
                );
                let mut td_hex = [0u8; 16];
                hex16_into(&td, &mut td_hex);
                let tds = core::str::from_utf8(&td_hex).unwrap_or("?");
                eneros_runtime::println!(
                    "[svc-modbus-tcp] MASTER-READBACK point={point} eng={eng} target={power} {} td={tds}",
                    if ok { "VERIFIED" } else { "MISMATCH" }
                );
                if ok {
                    OutcomeFrame::new(OutcomeCode::Applied, 0).encode()
                } else {
                    OutcomeFrame::new(OutcomeCode::AppliedReadbackMismatch, 0).encode()
                }
            }
            _ => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] MASTER-READBACK point={point} 不可得 —— AppliedReadbackUnavailable"
                );
                OutcomeFrame::new(OutcomeCode::AppliedReadbackUnavailable, 0).encode()
            }
        }
    }
}

#[cfg(target_os = "none")]
impl eneros_drv_svc::runtime::DriverService for ModbusTcpIoService {
    fn capability(&self) -> Option<&'static str> {
        Some("net-modbus-tcp")
    }

    fn handle(
        &mut self,
        req: &eneros_drv_svc::runtime::DriverRequest,
        resp: &mut eneros_drv_svc::runtime::DriverResponse,
    ) {
        use eneros_drv_svc::runtime::*;
        match req.opcode {
            OP_EXECUTE => {
                self.transactions += 1;
                // 出站写：EXECUTE + 载荷子码 17B（codec 单源 =
                // protocol_abstract::write::decode_master_write_payload）。
                if let Some((device, point, value, digest)) =
                    eneros_protocol_abstract::write::decode_master_write_payload_v2(
                        &req.payload[..req.payload_len as usize],
                    )
                {
                    let frame = self.master_write_v2(device, point, value, digest);
                    resp.status = ST_OK;
                    resp.payload[..2].copy_from_slice(&frame);
                    resp.payload_len = 2;
                    return;
                }
                // 采集轮（module.tick 单点驱动）。
                let round_good = self.collect_and_report(eneros_protocol_services::now_ms());
                if round_good {
                    resp.status = ST_OK;
                } else {
                    // 本轮无任何成功读（如拔 peer）→ fail-closed 指示。
                    resp.status = ST_UNAVAILABLE;
                }
            }
            _ => {
                resp.status = ST_INVALID;
            }
        }
    }
}

/// 装配接入深模块（[07] §2.6.6：接 EIPC/capability/config 生成 source
/// descriptor 与点计划）。
///
/// 调度字段（period_ms/deadband）来自点表记录；身份（schema/unit/asset/
/// 世代）来自映射绑定——两路同源（同一次 bundle 解码）。
#[cfg(target_os = "none")]
fn build_module(
    sources: &[(u32, eneros_ingestion_core::SourceDescriptor)],
    own_source_id: u32,
    registry: &eneros_protocol_abstract::VecPointRegistry,
    records: &[eneros_config_bundle::points::PointRecord],
    plugin: eneros_protocol_services::modbus_ingest::SharedPlugin,
) -> Option<eneros_ingestion_core::IngestionModule> {
    use eneros_ingestion_core::{
        EventQos, InMemorySink, IngestionModule, MemEpochStore, OperationalProfile, PointPlan,
    };
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
        let (slave_addr, reg_addr, func_code) = match m.protocol_addr {
            eneros_protocol_abstract::ProtocolAddress::Modbus {
                slave_addr,
                reg_addr,
                func_code,
            } => (slave_addr, reg_addr, func_code),
            _ => continue,
        };
        // 调度字段来自点表记录（缺记录 fail-closed：无调度的点不进计划）。
        let Some(rec) = records.iter().find(|r| r.point_id == m.point_id) else {
            continue;
        };
        let period_ns = u64::from(rec.period_ms) * 1_000_000;
        plans.push(PointPlan {
            source_id: own_source_id,
            point_id: m.point_id,
            device_id: m.device_id,
            // 周期来自点表（period_ms；参考闭环 3s/5s）；事件驱动点（0）
            // 不轮询。
            period_ns,
            phase_offset_ns: 0,
            deadline_ns: period_ns.saturating_mul(2),
            // 参考闭环 QoS：Q1 + 不可抑制（控制资格点全量上报——与
            // Phase 0 基线一致；死区/QoS 分级在配置面扩展后生效）。
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
            func_code,
            reg_addr,
            slave_addr,
        });
    }
    module.install_points(1, plans).ok()?;
    let adapter = Box::new(
        eneros_protocol_services::modbus_ingest::ModbusIngestAdapter::new(own_source_id, plugin),
    );
    module.attach(adapter).ok()?;
    Some(module)
}

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
        service: "svc-modbus-tcp",
        config_client: "modbus-config",
        config_id: "protocol-modbus-tcp",
        capability: "net-modbus-tcp",
        // 45.4：点表注入——全量注册表 + 协议过滤（ModbusTcp|ModbusRtu）。
        points: protocol_ladder::PointsPolicy::Eager(|p| {
            matches!(
                p.protocol,
                PointProtocol::ModbusTcp | PointProtocol::ModbusRtu
            )
        }),
        config_dump: false,
        // 无自检窗口——启动期自检 = Ready 臂的 collect_and_report 一轮。
        self_check_rounds: 0,
        degraded: protocol_ladder::DegradedStyle::HandoverAlias,
    };
    let mut io = protocol_ladder::Sel4ServiceIo::new(
        parts.service,
        parts.config_client,
        mmio_vaddr,
        dma_vaddr,
        dma_phys,
    );
    // 插件在服务侧构造（descriptor 随 readiness 报告）；阶梯 start 后
    // 包成共享句柄供 adapter（组读）与本 bin（出站写）两用。
    let plugin_owned = eneros_modbus_tcp::plugin::TcpPlugin::new();
    protocol_ladder::print_descriptor(parts.service, &plugin_owned);
    let telemetry =
        eneros_agent_sdk::eipc_resolve("svc-modbus-tcp", "svc-telemetry").and_then(|(_, slot)| {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "modbus-telemetry",
                slot,
                eneros_drv_svc::now_ns,
            )
            .ok()
        });
    if telemetry.is_some() {
        eneros_runtime::println!("[svc-modbus-tcp] telemetry EIPC bound (svc-telemetry)");
    } else {
        eneros_runtime::println!("[svc-modbus-tcp] telemetry EIPC NOT granted");
    }
    // 43：设备移交（virtio-net 槽 8）→ 真实 I/O 服务；未移交 → 能力壳。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match protocol_ladder::run(wall_epoch, &parts, plugin_owned, &mut io) {
            protocol_ladder::Ladder::Ready {
                plugin,
                registry,
                sources,
                generation,
            } => {
                // 共享句柄（读=adapter 组读；写=出站写）。
                let plugin = alloc::rc::Rc::new(core::cell::RefCell::new(plugin));
                // 调度字段取数（period_ms/deadband；与阶梯同过滤——二次
                // fetch 先例 = All/Filtered bin 路径）。
                let fetched = eneros_protocol_services::service_cfg::fetch_points(
                    &mut protocol_ladder::ServiceIoAsSource(&mut io),
                    parts.config_client,
                    |p| {
                        matches!(
                            p.protocol,
                            PointProtocol::ModbusTcp | PointProtocol::ModbusRtu
                        )
                    },
                );
                let records = fetched.records;
                // 来源 descriptor（modbus-tcp 行 = 本适配器身份；[07] §2.4）。
                let mut descriptors: alloc::vec::Vec<(
                    u32,
                    eneros_ingestion_core::SourceDescriptor,
                )> = alloc::vec::Vec::new();
                let mut own_source_id: Option<u32> = None;
                for src in &sources {
                    if src.protocol == PointProtocol::ModbusTcp {
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
                                    let mut ids = alloc::vec::Vec::new();
                                    registry.collect_ids(&mut ids);
                                    let mut v = alloc::vec::Vec::new();
                                    for id in ids {
                                        if let Some(m) = registry.resolve(id) {
                                            if m.identity.schema_id != 0
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
                            "[svc-modbus-tcp] OBS-SOURCE disabled (no modbus-tcp [[source]] row)"
                        );
                        break 'build None;
                    };
                    let Some(mut module) =
                        build_module(&descriptors, own, &registry, &records, plugin.clone())
                    else {
                        eneros_runtime::println!("[svc-modbus-tcp] OBS-MODULE build FAIL");
                        break 'build None;
                    };
                    // 开源（floor = boot 单调时刻；持久 boot counter = ADR-0047）。
                    let epoch_floor = eneros_protocol_services::now_ms();
                    match module.open(own, epoch_floor) {
                        Ok(epoch) => {
                            eneros_runtime::println!(
                                "[svc-modbus-tcp] OBS-SOURCE id={} epoch={} (config-allocated)",
                                own,
                                epoch
                            );
                        }
                        Err(e) => {
                            eneros_runtime::println!(
                                "[svc-modbus-tcp] OBS-SOURCE open FAIL: {e:?}"
                            );
                            break 'build None;
                        }
                    }
                    let _ = generation; // generation 已注入 mapping.identity
                    let mut io = ModbusTcpIoService {
                        module,
                        source_id: own,
                        plugin,
                        registry,
                        telemetry,
                        tel_epoch: 0,
                        accepted_total: 0,
                        transactions: 0,
                    };
                    // 启动期自检：一轮真实采集（外部 peer 记录事务；串口
                    // 打印 = L2 互操作/45 采集证据行）。
                    io.collect_and_report(eneros_protocol_services::now_ms());
                    break 'build Some(Box::new(io));
                };
                built.unwrap_or_else(protocol_ladder::capability_shell)
            }
            protocol_ladder::Ladder::Degraded(_) => protocol_ladder::capability_shell(),
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "modbus-tcp",
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

/// 摘要前 8 字节 hex 写入 16B ASCII 缓冲（串口证据用；61 v0.314.25）。
#[cfg(target_os = "none")]
fn hex16_into(d: &[u8; 32], out: &mut [u8; 16]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, b) in d[..8].iter().enumerate() {
        out[i * 2] = HEX[(b >> 4) as usize];
        out[i * 2 + 1] = HEX[(b & 0xF) as usize];
    }
}
