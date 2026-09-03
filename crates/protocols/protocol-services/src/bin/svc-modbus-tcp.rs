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
    /// 遥测上行端（交付半环单源——分批/chunk/解析/epoch 递增内化；
    /// WP-62.2.3）。
    uplink: eneros_protocol_services::ingestion_stage::TelemetryUplink<
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    >,
    transactions: u32,
}

#[cfg(target_os = "none")]
impl ModbusTcpIoService {
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
                // 采集轮（交付半环单源——WP-62.2.3 IngestionStage）。
                let round_good = eneros_protocol_services::ingestion_stage::run_acq_round(
                    &mut self.module,
                    &mut self.uplink,
                    "svc-modbus-tcp",
                    eneros_drv_svc::now_ns(),
                );
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
/// 蓝图 64 卷 64.2（WP-64.2.2）：64 行点计划孪生收编 lib
/// `plans_from_records`（15 字段默认值单源）；本 bin 只保留协议差异点
/// （Modbus 地址反构造闭包 + 事件驱动 period 0 保持不轮询）。
#[cfg(target_os = "none")]
fn build_module(
    sources: &[(u32, eneros_ingestion_core::SourceDescriptor)],
    own_source_id: u32,
    registry: &eneros_protocol_abstract::VecPointRegistry,
    records: &[eneros_config_bundle::points::PointRecord],
    plugin: eneros_protocol_services::modbus_ingest::SharedPlugin,
) -> Option<eneros_ingestion_core::IngestionModule> {
    use eneros_protocol_services::ingestion_stage::{new_reference_module, plans_from_records};
    let mut module = new_reference_module(sources, own_source_id)?;
    // 点计划装配单源（WP-64.2.2）：addr_of 反构造 Modbus 地址三字段；
    // period 缺省 0 = 事件驱动点（period_ms=0）不轮询——原 bin 语义。
    let plans = plans_from_records(
        records,
        own_source_id,
        registry,
        |m| match m.protocol_addr {
            eneros_protocol_abstract::ProtocolAddress::Modbus {
                slave_addr,
                reg_addr,
                func_code,
            } => Some(eneros_protocol_services::ingestion_stage::PlanAddr {
                slave_addr,
                reg_addr,
                func_code,
            }),
            _ => None,
        },
        0,
        |_m, _addr| {},
    );
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
    // 遥测上行端装配单源（WP-64.2.3：bound/NOT-granted 词形归模块；
    // 绑定时机在阶梯 run 前 = 串口行序不变）。
    let telemetry = eneros_protocol_services::ingestion_stage::bind_telemetry("svc-modbus-tcp");
    // 43：设备移交（virtio-net 槽 8）→ 真实 I/O 服务；未移交 → 能力壳。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match protocol_ladder::run(wall_epoch, &parts, plugin_owned, &mut io) {
            protocol_ladder::Ladder::Ready {
                plugin,
                registry,
                sources,
                generation,
                records,
            } => {
                // 共享句柄（读=adapter 组读；写=出站写）。
                let plugin = alloc::rc::Rc::new(core::cell::RefCell::new(plugin));
                let _ = generation; // generation 已注入 mapping.identity
                                    // 装配序列单源（蓝图 63 卷 63.2 WP-63.2.4）：descriptor
                                    // 循环→build→open→uplink→自检→壳兜底进 lib；本 bin 只保留
                                    // 协议差异点。schema 谓词 = 范围口径恒真（本适配器过滤
                                    // 口径 = 本来源映射点全集——协议路由已在 fetch 阶段过滤，
                                    // 不与 iec104 静默统一）。records 由 Ready 携带（WP-64.2.3：
                                    // 二次 fetch 删除——POINTS-LOADED 2→1 已登记）。
                eneros_protocol_services::ingestion_stage::build_ingested_service(
                    "svc-modbus-tcp",
                    "no modbus-tcp [[source]] row",
                    &sources,
                    PointProtocol::ModbusTcp,
                    registry,
                    |reg, id| reg.resolve(id).map(|m| m.identity.schema_id),
                    |reg, descriptors, own| {
                        build_module(descriptors, own, reg, &records, plugin.clone())
                    },
                    telemetry,
                    |module, own, uplink, registry| {
                        Box::new(ModbusTcpIoService {
                            module,
                            source_id: own,
                            plugin: plugin.clone(),
                            registry,
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
