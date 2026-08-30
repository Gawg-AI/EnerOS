//! svc-modbus-tcp — Modbus TCP 协议 EL0 服务（蓝图 39 WP-39.12；43 L2 协议
//! 互操作接线；45.5 遥测通路先行者；S1：装配阶梯单源 `protocol_ladder`，
//! `ModbusTcpIoService` 数据面留 bin）。

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

/// 45.5：点表驱动 + 遥测上报的 modbus-tcp 真实 I/O 服务。
///
/// `_start` 收到 root 移交的 virtio-net 设备（X3=MMIO vaddr, X4=DMA vaddr,
/// X5=DMA phys）→ 经阶梯（`protocol_ladder::run`，S1）构造 smoltcp TCP 传输
/// 并连接外部 Modbus-TCP 从站 peer；点表经 EIPC 从 svc-config
/// `ConfigGet("points")` 解码注入（**不自造地址**）；`EXECUTE` 触发真实读
/// 保持寄存器事务 → `ProtocolPointMapping::to_engineering` 工程量变换 →
/// 58B 样本（`agent_sdk::sample_wire`）INGEST 到 svc-telemetry（EIPC 授予
/// 先行者）。
///
/// 点表来源与遥测端点均经 EIPC 授予（`EIPC_GRANTS`：svc-modbus-tcp 表序
/// telemetry=槽 3、config=槽 4）——46 卷句柄移交同批，modbus-tcp 先行者先
/// 拿到。47（WP-47.1）：读回容差（kW；与 device-agent
/// ReadbackPolicy::tolerance 同量级——ACK 成功但物理状态未达目标 →
/// Mismatch）。复查 FIX：值单源 = control-gateway
/// `PCS_SETPOINT_READBACK_TOL_KW`（与签发侧 ControlContract.tolerance_abs
/// 同一事实；本 bin 历史手抄常量退役）。
pub use eneros_control_gateway::PCS_SETPOINT_READBACK_TOL_KW as MASTER_READBACK_TOL_KW;

#[cfg(target_os = "none")]
struct ModbusTcpIoService {
    /// 45.4/45.5：点表驱动真读插件（地址来自注入点表）。
    plugin: eneros_modbus_tcp::plugin::TcpPlugin,
    /// 注入点表（modbus-tcp 点；自 svc-config 解码）。
    registry: eneros_protocol_abstract::VecPointRegistry,
    /// svc-telemetry EIPC 客户端（授予槽按名解析——S1 起不手数槽位）。
    telemetry: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 遥测分块 epoch（单调）。
    tel_epoch: u64,
    /// 采样序号（单调；重放判定）。
    seq: u64,
    /// INGEST 累计接受计数（观测——真实采集链）。
    pub accepted_total: u64,
    transactions: u32,
}

#[cfg(target_os = "none")]
impl ModbusTcpIoService {
    /// 一轮点表驱动真读 + 遥测 INGEST（45.5 真实采集链；ACQ 证据行）。
    ///
    /// INGEST 协议语义 = 新采集周期全量替换（TEL_INGEST 每请求先 clear 再入样）
    /// ——**同一轮的全部点合成单个批请求上报**（逐点单发会导致每请求清空、
    /// 只剩末点）。cycle 序号整批共享（对齐 root 合成写法的评估器语义，
    /// 每点按 cycle 单调，跨周期不重放）。
    fn collect_and_report(&mut self, now_ms: u64) -> bool {
        let mut any_good = false;
        let mut ids: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
        self.registry.collect_ids(&mut ids);
        let mut batch: alloc::vec::Vec<eneros_safety_validator::TelemetrySample> =
            alloc::vec::Vec::new();
        for id in &ids {
            let Some(mapping) = self.registry.resolve(*id).cloned() else {
                continue;
            };
            // 47（WP-47.2）：点表新增 Control 点（写目标）——控制/设点/
            // 计数点不进入遥测采集（遥测 = analog/digital）。
            match mapping.data_type {
                eneros_upa_model::PointType::Analog | eneros_upa_model::PointType::Digital => {}
                _ => continue,
            }
            match self.plugin.read_point(now_ms, &mapping) {
                Ok(read) if read.ok => {
                    let eng = mapping.to_engineering(read.raw);
                    any_good = true;
                    eneros_runtime::println!(
                        "[svc-modbus-tcp] ACQ point={} value={} q=good seq={}",
                        mapping.point_id,
                        eng,
                        self.seq
                    );
                    // 58B 样本（source=1 = svc-telemetry allowed_sources；L2
                    // 测试链受信完整性=Verified，HMAC 认证链为 47 卷范围）。
                    batch.push(eneros_safety_validator::TelemetrySample {
                        point: mapping.point_id,
                        value: eng,
                        source: 1,
                        source_epoch: 0,
                        measured_at_ms: now_ms.saturating_sub(50),
                        received_at_ms: now_ms,
                        quality: eneros_safety_validator::SampleQuality::Good,
                        sequence: self.seq,
                        integrity: eneros_safety_validator::IntegrityStatus::Verified,
                        mapping_version: 1,
                    });
                }
                Ok(_) => {
                    eneros_runtime::println!(
                        "[svc-modbus-tcp] ACQ point={} q=bad (read ok=false)",
                        mapping.point_id
                    );
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-modbus-tcp] ACQ point={} q=bad {:?}",
                        mapping.point_id,
                        e
                    );
                }
            }
        }
        if batch.is_empty() {
            return false;
        }
        // 整轮单次 INGEST（全量替换语义）。
        let mut req: alloc::vec::Vec<u8> = alloc::vec![1u8]; // TEL_INGEST
        for sample in &batch {
            req.extend_from_slice(&eneros_agent_sdk::sample_wire::encode_sample(sample));
        }
        self.seq = self.seq.saturating_add(1);
        if let Some(client) = &mut self.telemetry {
            let mut transfer = eneros_agent_sdk::ChunkedClient::new(client, self.tel_epoch, 5_000);
            self.tel_epoch = self.tel_epoch.saturating_add(1);
            match transfer.transfer("svc-telemetry", &req) {
                Ok(resp) if resp.len() >= 2 && resp[0] == 0 => {
                    self.accepted_total = self.accepted_total.saturating_add(resp[1] as u64);
                    eneros_runtime::println!(
                        "[svc-modbus-tcp] INGEST round={} samples={} accepted={} total={}",
                        batch.len(),
                        batch.len(),
                        resp[1],
                        self.accepted_total
                    );
                }
                Ok(resp) => {
                    eneros_runtime::println!(
                        "[svc-modbus-tcp] INGEST rejected {:02x?}",
                        &resp[..resp.len().min(8)]
                    );
                }
                Err(e) => {
                    eneros_runtime::println!("[svc-modbus-tcp] INGEST eipc={:?}", e);
                }
            }
        } else {
            eneros_runtime::println!(
                "[svc-modbus-tcp] telemetry EP not granted —— 样本未上报（fail-closed）"
            );
        }
        any_good
    }
    /// 47（WP-47.1 协议出站写）：执行主站写 + 独立 readback，返回 2B
    /// `OutcomeFrame`（与 dispatch/device-agent 同源回执语义）。
    ///
    /// **本方法只执行**——授权已由 ControlDispatch 九步链完成（不双头授权）。
    /// 点表解析失败 → `Denied(class=7 Mismatch)`；协议写失败 → `Denied(8
    /// Protocol)`；写后重读不可得 → `AppliedReadbackUnavailable`；重读值
    /// 与目标不符（超过容差）→ `AppliedReadbackMismatch`；相符 →
    /// `Applied`。
    fn master_write_v2(
        &mut self,
        device: u32,
        point: u32,
        value: eneros_telemetry_model::command::CommandValue,
        command_digest: [u8; 32],
    ) -> [u8; 2] {
        use eneros_control_gateway::wire::{OutcomeCode, OutcomeFrame};
        use eneros_protocol_abstract::write::semantic_command_digest;
        // 61 v0.314.25 §3.5.6：writer 校验语义摘要——重算与收到的 digest
        // 比对（授权 50 写 60 必然可检出），绑定 device/point/value。
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
        match self
            .plugin
            .write_point(now, mapping, eneros_upa_model::PointValue::Float(power))
        {
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
        match self.plugin.read_point(now, mapping) {
            Ok(read) if read.ok => {
                let eng = mapping.to_engineering(read.raw);
                let ok = (eng - power).abs() <= MASTER_READBACK_TOL_KW.max(f64::EPSILON);
                // 61 v0.314.25：transport digest 绑定（协议帧语义级摘要——
                // 独立回读值 + 目标值 + 点；映射世代入证据归 26 配置化）。
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
                // 47（WP-47.1 出站写）：EXECUTE + 载荷子码 17B（drv-svc
                // runtime 只把 OP_EXECUTE 路由到 handle）。17B codec 单源 =
                // protocol_abstract::write::decode_master_write_payload
                // （SPEC-B WP-B.2 收编，历史手抄解码面删除）。
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
                // 45.5：点表驱动真读 + 遥测上报（真实采集替换合成源）。
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

/// 45.5 + S1：EIPC 授予（先行者）——遥测端点按名解析（svc-modbus-tcp
/// 表序首边 = svc-telemetry）；点表经阶梯 Eager 策略在传输前注入。
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
        // 43：READY 行 capability=net-modbus-tcp（驱动服务 capability 同串）。
        capability: "net-modbus-tcp",
        // 45.4：点表注入——全量注册表 + 协议过滤（ModbusTcp|ModbusRtu）；
        // Eager = 现行串口行序（点表行先于传输阶段，R2 保序）。
        points: protocol_ladder::PointsPolicy::Eager(|p| {
            matches!(
                p.protocol,
                PointProtocol::ModbusTcp | PointProtocol::ModbusRtu
            )
        }),
        config_dump: false,
        // 无自检窗口——启动期自检 = Ready 臂的 collect_and_report 一轮真读。
        self_check_rounds: 0,
        // try_new→_start 两段打印的降级词形（handover missing 别名，M-6）。
        degraded: protocol_ladder::DegradedStyle::HandoverAlias,
    };
    let mut io = protocol_ladder::Sel4ServiceIo::new(
        parts.service,
        parts.config_client,
        mmio_vaddr,
        dma_vaddr,
        dma_phys,
    );
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_modbus_tcp::plugin::TcpPlugin::new();
    protocol_ladder::print_descriptor(parts.service, &plugin);
    // 45.5：svc-telemetry EIPC 客户端（按名解析；未授予 → 样本不上报）。
    let telemetry =
        eneros_agent_sdk::eipc_resolve("svc-modbus-tcp", "svc-telemetry").and_then(|(_, slot)| {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "modbus-telemetry",
                slot,
                eneros_drv_svc::now_ns,
            )
            .ok()
        });
    // S1 evidence M-1：telemetry 绑定观测行前移至阶梯调用前（原在点表行后、
    // 传输阶段前——判定行/计数集词形与相对序不受影响）。
    if telemetry.is_some() {
        eneros_runtime::println!("[svc-modbus-tcp] telemetry EIPC bound (svc-telemetry)");
    } else {
        eneros_runtime::println!("[svc-modbus-tcp] telemetry EIPC NOT granted");
    }
    // 43：设备移交（virtio-net 槽 8）→ 真实 I/O 服务；未移交 → 能力壳。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match protocol_ladder::run(wall_epoch, &parts, plugin, &mut io) {
            protocol_ladder::Ladder::Ready { plugin, registry } => {
                let mut io = ModbusTcpIoService {
                    plugin,
                    registry,
                    telemetry,
                    tel_epoch: 0,
                    // 评估器 last_seq 默认 0：首轮序列从 1 起（root 合成写法同源）。
                    seq: 1,
                    accepted_total: 0,
                    transactions: 0,
                };
                // 启动期自检：点表驱动一轮真实读（外部 peer 记录事务；串口
                // 打印 = L2 互操作/45 采集证据行）。
                io.collect_and_report(eneros_protocol_services::now_ms());
                Box::new(io)
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
