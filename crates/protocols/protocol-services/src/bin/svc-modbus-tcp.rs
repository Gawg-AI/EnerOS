//! svc-modbus-tcp — Modbus TCP 协议 EL0 服务（蓝图 39 WP-39.12；43 L2 协议
//! 互操作接线；45.5 遥测通路先行者）。

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
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 45.5：点表驱动 + 遥测上报的 modbus-tcp 真实 I/O 服务。
///
/// `_start` 收到 root 移交的 virtio-net 设备（X3=MMIO vaddr, X4=DMA vaddr,
/// X5=DMA phys）→ 经 [`net_transport`] 构造 smoltcp TCP 传输并连接外部
/// Modbus-TCP 从站 peer；点表经 EIPC 从 svc-config `ConfigGet("points")`
/// 解码注入（**不自造地址**）；`EXECUTE` 触发真实读保持寄存器事务 →
/// `ProtocolPointMapping::to_engineering` 工程量变换 → 58B 样本
/// （`agent_sdk::sample_wire`）INGEST 到 svc-telemetry（EIPC 授予先行者）。
///
/// 点表来源与遥测端点均经 EIPC 授予（`EIPC_GRANTS`：槽 3=svc-telemetry、
/// 槽 4=svc-config）——46 卷句柄移交同批，modbus-tcp 先行者先拿到。
/// 47（WP-47.1）：读回容差（kW；与 device-agent ReadbackPolicy::tolerance
/// 同量级——ACK 成功但物理状态未达目标 → Mismatch）。
pub const MASTER_READBACK_TOL_KW: f64 = 0.1;

#[cfg(target_os = "none")]
struct ModbusTcpIoService {
    /// 45.4/45.5：点表驱动真读插件（地址来自注入点表）。
    plugin: eneros_modbus_tcp::plugin::TcpPlugin,
    /// 注入点表（modbus-tcp 点；自 svc-config 解码）。
    registry: eneros_protocol_abstract::VecPointRegistry,
    /// svc-telemetry EIPC 客户端（授予槽 `EIPC_EP_BASE_SLOT`）。
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
    /// 构造：经 svc-config 取点表 + 建 net transport + 启动插件（点表驱动）。
    fn try_new(
        mmio_vaddr: u64,
        dma_vaddr: u64,
        dma_phys: u64,
        telemetry: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
        mut registry: eneros_protocol_abstract::VecPointRegistry,
    ) -> Result<Self, &'static str> {
        // 43 既有 seam：NetTransportFactory 把 smoltcp 传输经 HandleAdapter
        // 暴露为 `TransportHandle`（共享 Rc<RefCell> 实例；open 完成三次握手）。
        let transport = eneros_protocol_services::net_transport::build_net_transport(
            mmio_vaddr, dma_vaddr, dma_phys,
        )?;
        let mut factory =
            eneros_protocol_services::net_transport::NetTransportFactory::new(transport);
        // 46.3：peer/unit_id 来源 = config-bundle（protocol-modbus-tcp.toml），
        // 消灭源码 peer 常量；配置不可得 → 装配失败 → 能力壳（fail-closed）。
        let Some(cfg) = eneros_protocol_services::service_cfg::fetch_protocol_config(
            "modbus-config",
            eneros_agent_sdk::EIPC_EP_BASE_SLOT + 1,
            "protocol-modbus-tcp",
        ) else {
            eneros_runtime::println!(
                "[svc-modbus-tcp] protocol config unavailable —— capability 壳（EXECUTE ST_UNAVAILABLE）"
            );
            return Err("protocol-config-missing");
        };
        let mut plugin = eneros_modbus_tcp::plugin::TcpPlugin::new();
        // 插件 start 消费点表（45.4 路由注入；borrow 于本调用内结束）。
        plugin
            .start(&cfg, &mut factory, &mut registry)
            .map_err(|_| "plugin-start-failed")?;
        Ok(Self {
            plugin,
            registry,
            telemetry,
            tel_epoch: 0,
            // 评估器 last_seq 默认 0：首轮序列从 1 起（root 合成写法同源）。
            seq: 1,
            accepted_total: 0,
            transactions: 0,
        })
    }

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
    fn master_write(&mut self, device: u32, point: u32, power: f64) -> [u8; 2] {
        use eneros_control_gateway::wire::{OutcomeCode, OutcomeFrame};
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
        match self
            .plugin
            .write_point(now, mapping, eneros_upa_model::PointValue::Float(power))
        {
            Ok(()) => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] MASTER-WRITE device={device} point={point} power={power} ACK"
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
                eneros_runtime::println!(
                    "[svc-modbus-tcp] MASTER-READBACK point={point} eng={eng} target={power} {}",
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
                // 45.5：点表驱动真读 + 遥测上报（真实采集替换合成源）。
                let round_good = self.collect_and_report(eneros_protocol_services::now_ms());
                if round_good {
                    resp.status = ST_OK;
                } else {
                    // 本轮无任何成功读（如拔 peer）→ fail-closed 指示。
                    resp.status = ST_UNAVAILABLE;
                }
            }
            eneros_protocol_abstract::write::OP_MASTER_WRITE => {
                // 47（WP-47.1 协议出站写）：16B 载荷 {device u32, point u32,
                // power f64}。长度不符 → ST_INVALID（坏信封，不执行）。
                if req.payload_len != 16 {
                    resp.status = ST_INVALID;
                    return;
                }
                let device = u32::from_le_bytes(req.payload[0..4].try_into().unwrap_or([0; 4]));
                let point = u32::from_le_bytes(req.payload[4..8].try_into().unwrap_or([0; 4]));
                let power = f64::from_le_bytes(req.payload[8..16].try_into().unwrap_or([0.0; 8]));
                let frame = self.master_write(device, point, power);
                resp.status = ST_OK;
                resp.payload[..2].copy_from_slice(&frame);
                resp.payload_len = 2;
            }
            _ => {
                resp.status = ST_INVALID;
            }
        }
    }
}

/// 经 svc-config EIPC 取点表 canonical payload → decode_points → 注册表。
#[cfg(target_os = "none")]
fn fetch_points_registry() -> eneros_protocol_abstract::VecPointRegistry {
    // 45.5：svc-config 授予槽 = EIPC_EP_BASE_SLOT+1（表序第 2 个 grant）。
    let Ok(mut config) = eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
        "modbus-config",
        eneros_agent_sdk::EIPC_EP_BASE_SLOT + 1,
        eneros_drv_svc::now_ns,
    ) else {
        eneros_runtime::println!("[svc-modbus-tcp] config EIPC NOT granted -> points unavailable");
        return eneros_protocol_abstract::VecPointRegistry::new(alloc::vec::Vec::new());
    };
    let mut req: alloc::vec::Vec<u8> = alloc::vec![b'C', b'F', b'G', b'1', 0x01];
    req.extend_from_slice(b"points");
    let mut transfer = eneros_agent_sdk::ChunkedClient::new(&mut config, 0, 5_000);
    let payload = match transfer.transfer("svc-config", &req) {
        Ok(p) => p,
        Err(e) => {
            eneros_runtime::println!("[svc-modbus-tcp] config ConfigGet(points) EIPC fail: {e:?}");
            return eneros_protocol_abstract::VecPointRegistry::new(alloc::vec::Vec::new());
        }
    };
    // svc-config GET_ENTRY 响应 = 4B LE 长度 + canonical payload（与
    // svc-mgmt 先例一致）；剥前缀后再解码（长度$不对即拒绝）。
    let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
    let body = if payload.len() >= 4 {
        let plen = u32::from_le_bytes(payload[..4].try_into().unwrap_or([0; 4])) as usize;
        if 4 + plen <= payload.len() {
            &payload[4..4 + plen]
        } else {
            eneros_runtime::println!("[svc-modbus-tcp] config length prefix malformed");
            return eneros_protocol_abstract::VecPointRegistry::new(alloc::vec::Vec::new());
        }
    } else {
        eneros_runtime::println!("[svc-modbus-tcp] config response too short");
        return eneros_protocol_abstract::VecPointRegistry::new(alloc::vec::Vec::new());
    };
    match eneros_config_bundle::points::decode_points(body, &cfg_id) {
        Ok(table) => {
            // 服务层按协议过滤注入（45.4）：只保留 modbus-tcp 点（其余协议
            // 点的地址不属于本插件，注入只会产生 AddrTypeMismatch 噪音）。
            let mappings: alloc::vec::Vec<_> = table
                .points
                .iter()
                .filter(|p| {
                    p.protocol == eneros_config_bundle::points::PointProtocol::ModbusTcp
                        || p.protocol == eneros_config_bundle::points::PointProtocol::ModbusRtu
                })
                .filter_map(|p| p.to_mapping())
                .collect();
            eneros_runtime::println!(
                "[svc-modbus-tcp] POINTS-LOADED {} modbus points (registry {})",
                mappings.len(),
                table.len()
            );
            eneros_protocol_abstract::VecPointRegistry::new(mappings)
        }
        Err(e) => {
            eneros_runtime::println!("[svc-modbus-tcp] points decode FAIL: {e:?}");
            eneros_protocol_abstract::VecPointRegistry::new(alloc::vec::Vec::new())
        }
    }
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
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // smoltcp 传输用 eneros_time 单调时钟推进；EL0 服务进程内初始化
    // （arm64 通用定时器，EL0 可读；rtc_base 0 = 无板载 RTC 事实）。
    eneros_time::time_init(eneros_hal::arm64::timer::clock(), 0);
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_modbus_tcp::plugin::TcpPlugin::new();
    eneros_runtime::println!(
        "[protocol-service] modbus-tcp plugin={} role={:?} interop={:?}",
        plugin.descriptor().id,
        plugin.descriptor().role,
        plugin.descriptor().interop_level
    );
    // 45.5：EIPC 授予（先行者）——槽 EIPC_EP_BASE_SLOT=svc-telemetry、
    // +1=svc-config；点表经 svc-config 获取。
    let telemetry = eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
        "modbus-telemetry",
        eneros_agent_sdk::EIPC_EP_BASE_SLOT,
        eneros_drv_svc::now_ns,
    )
    .ok();
    let registry = fetch_points_registry();
    if telemetry.is_some() {
        eneros_runtime::println!("[svc-modbus-tcp] telemetry EIPC bound (svc-telemetry)");
    } else {
        eneros_runtime::println!("[svc-modbus-tcp] telemetry EIPC NOT granted");
    }
    let now_ms = eneros_protocol_services::now_ms();
    // 43：设备移交（virtio-net 槽 8）→ 真实 I/O 服务；未移交 → 能力壳。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match ModbusTcpIoService::try_new(mmio_vaddr, dma_vaddr, dma_phys, telemetry, registry) {
            Ok(mut io) => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] NET-TRANSPORT READY capability=net-modbus-tcp"
                );
                // 启动期自检：点表驱动一轮真实读（外部 peer 记录事务；串口
                // 打印 = L2 互操作/45 采集证据行）。
                io.collect_and_report(now_ms);
                Box::new(io)
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] handover missing ({:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                    e
                );
                Box::new(eneros_protocol_services::service::UnassignedProtocolService)
            }
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
