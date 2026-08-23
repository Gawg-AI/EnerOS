//! svc-observability — target 可观测队列 EL0（蓝图 41 WP-41.12）。
//! EIPC 协议：magic "OBS1" ‖ cmd ‖ payload。
//! - 0x01 ENQUEUE：class(1)‖service(16)‖schema(8)‖created(8)‖payload(≤512) → ok(1)
//! - 0x02 FLUSH：batch(4) → exported(8)
//! - 0x03 STATS：→ enqueued(8)‖dropped(8)‖redacted(8)‖exported(8)‖failed(8)
//!
//! exporter 未接通（外部 AgentBus peer 缺）→ flush 显式失败不丢数据。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate alloc;
#[cfg(target_os = "none")]
extern crate eneros_business_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_business_services::HeapAllocator =
    eneros_business_services::HeapAllocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 50（WP-50.1）：真 MQTT 导出器——svc-mqtt 端点在授予槽 base+1
    // （槽 0 = svc-storage，bit 1 置位 = svc-mqtt 已授予）。未授予 → 导出
    // 显式失败（fail-closed，durable outbox 兜底）。
    let mqtt = if eipc_mask & 2 != 0 {
        match eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
            "obs-mqtt",
            eneros_agent_sdk::EIPC_EP_BASE_SLOT + 1,
            eneros_business_services::now_ms,
        ) {
            Ok(c) => {
                eneros_runtime::println!("[svc-observability] MQTT-EXPORTER EIPC bound (svc-mqtt)");
                Some(c)
            }
            Err(_) => {
                eneros_runtime::println!(
                    "[svc-observability] MQTT-EXPORTER EIPC bind FAILED —— 导出 Unavailable（fail-closed）"
                );
                None
            }
        }
    } else {
        eneros_runtime::println!(
            "[svc-observability] svc-mqtt NOT granted -> MQTT exporter Unavailable"
        );
        None
    };
    let mut svc = eneros_business_services::observability::ObservabilityService::new(
        MqttTelemetryExporter::new(mqtt),
        eneros_business_services::observability::OverflowPolicy::DropLowPriority,
        64,
    );
    // 存储授予时挂接真 durable outbox（AppendOutbox over BlockIo）。
    if eipc_mask & 1 != 0 {
        if let Some(io) = eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0,
            5_000,
        ) {
            let mut ob = eneros_durable_state::AppendOutbox::new(
                io,
                OBS_OUTBOX_OFFSET,
                OBS_OUTBOX_SLOT_LEN,
                OBS_OUTBOX_MAX_SLOTS,
            );
            match ob.open() {
                Ok(()) => {
                    eneros_runtime::println!(
                        "[svc-observability] durable telemetry outbox attached (svc-storage)"
                    );
                    svc.durable = Some(alloc::boxed::Box::new(
                        eneros_business_services::target_durable::MaybeOutbox::Real(ob),
                    ));
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-observability] durable outbox open FAILED {:?} —— 显式 Unavailable",
                        e
                    );
                }
            }
        } else {
            eneros_runtime::println!(
                "[svc-observability] storage NOT granted -> durable outbox Unavailable"
            );
        }
    } else {
        eneros_runtime::println!(
            "[svc-observability] storage NOT granted -> durable outbox Unavailable"
        );
    }
    eneros_runtime::println!("[svc-observability] target telemetry queue up");
    let mut handler = ObsHandler { svc };
    eneros_control_services::control_service_main(
        ep,
        "observability",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
const OBS_OUTBOX_OFFSET: u64 = 57344;
#[cfg(target_os = "none")]
const OBS_OUTBOX_SLOT_LEN: usize = 1024;
// 50（WP-50.1 实弹修复）：4 → 16——flush batch=8 单批即超 4 槽
// （OutOfSpace → durable-enqueue-failed → 队列永不排空 → queue-full）。
// 16 槽 = 2 批余量（断网期累积）；区域 57344+64+16KiB ≈ 73.8KB，
// 共享存储区最高注册偏移之上（无冲突）。
#[cfg(target_os = "none")]
const OBS_OUTBOX_MAX_SLOTS: usize = 16;

#[cfg(target_os = "none")]
struct UnavailableExporter;

#[cfg(target_os = "none")]
impl eneros_business_services::observability::TelemetryExporter for UnavailableExporter {
    fn export_batch(
        &mut self,
        _items: &[eneros_business_services::observability::TelemetryItem],
    ) -> Result<eneros_durable_state::PeerReceipt, &'static str> {
        Err("agentbus-exporter-unavailable")
    }
}

/// 50（WP-50.1 exporter 真通道）：svc-mqtt EIPC 真导出器（v1.0 单选 MQTT）。
///
/// flush 出口：遥测/指标/日志批量帧（固定二进制 OBSF + schema 版本）经
/// svc-mqtt 分片 EXECUTE 送达 broker topic（46 卷链 + 47 卷 TLS profile）。
/// svc-mqtt 侧 broker 断连/未配置主题/坏帧 → 回非 0 状态 → 本导出器返回
/// Err（durable outbox 兜底补传，不丢数据）。成功 → 全帧 ST_OK+status 0
/// → PeerReceipt。
#[cfg(target_os = "none")]
struct MqttTelemetryExporter {
    /// svc-mqtt EIPC 客户端（授予槽 `EIPC_EP_BASE_SLOT + 1`；槽 0 = svc-storage）。
    mqtt: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 帧序号（补传窗口连续序号证据；单调递增）。
    frame_seq: u64,
    /// drv 请求 correlation（单调）。
    correlation: u32,
    /// 已成功导出帧数（观测）。
    pub exported_frames: u64,
}

#[cfg(target_os = "none")]
impl MqttTelemetryExporter {
    fn new(mqtt: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>) -> Self {
        Self {
            mqtt,
            frame_seq: 0,
            correlation: 0,
            exported_frames: 0,
        }
    }

    /// 发送一帧：encode → 分片（BEGIN/CONT/END）→ drv_call 逐片 → 全片
    /// `ST_OK` 且 END 状态字节 0 才算成功（否则 Err——durable 兜底）。
    fn send_frame(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        use eneros_agent_sdk::telemetry_relay::fragment_telemetry_frame;
        let Some(client) = &mut self.mqtt else {
            eneros_runtime::println!(
                "[svc-observability] EXPORT-DROP svc-mqtt EP NOT granted（fail-closed → 补传）"
            );
            return Err("mqtt-ep-not-granted");
        };
        let (_total, frames) =
            fragment_telemetry_frame(frame).map_err(|_| "frame-fragment-failed")?;
        let timeout_ns = 2_000_000_000u64;
        for (i, f) in frames.iter().enumerate() {
            self.correlation = self.correlation.wrapping_add(1);
            let mut req = eneros_drv_svc::runtime::DriverRequest::new(
                self.correlation,
                eneros_drv_svc::runtime::OP_EXECUTE,
            );
            req.payload[..f.len()].copy_from_slice(f);
            req.payload_len = f.len() as u16;
            let raw = eneros_drv_svc::runtime::wire::encode_request(&req);
            match client.drv_call(&raw, timeout_ns) {
                Ok(resp) => {
                    let decode_len =
                        core::cmp::min(resp.len(), eneros_drv_svc::runtime::wire::RESPONSE_LEN);
                    let resp = eneros_drv_svc::runtime::wire::decode_response(&resp[..decode_len])
                        .ok_or("bad-resp")?;
                    let is_end = i + 1 == frames.len();
                    // END 帧的状态字节 = svc-mqtt 边界裁决（0 接受 / 1 拒绝）。
                    let status_byte = resp.payload.first().copied().unwrap_or(1);
                    if resp.status != eneros_drv_svc::runtime::ST_OK || (is_end && status_byte != 0)
                    {
                        eneros_runtime::println!(
                            "[svc-observability] EXPORT-REJECT frame_seq={} status={} byte={}（补传）",
                            self.frame_seq,
                            resp.status,
                            status_byte
                        );
                        return Err("mqtt-reject");
                    }
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-observability] EXPORT-REJECT drv={:?} frame_seq={}（补传）",
                        e,
                        self.frame_seq
                    );
                    return Err("mqtt-eipc-down");
                }
            }
        }
        self.frame_seq = self.frame_seq.saturating_add(1);
        self.exported_frames = self.exported_frames.saturating_add(1);
        Ok(())
    }
}

#[cfg(target_os = "none")]
impl eneros_business_services::observability::TelemetryExporter for MqttTelemetryExporter {
    fn export_batch(
        &mut self,
        items: &[eneros_business_services::observability::TelemetryItem],
    ) -> Result<eneros_durable_state::PeerReceipt, &'static str> {
        let frame =
            eneros_business_services::observability::encode_batch_frame(items, self.frame_seq)?;
        self.send_frame(&frame)?;
        eneros_runtime::println!(
            "[svc-observability] EXPORT-FRAME frame_seq={} items={} bytes={} total_frames={}",
            self.frame_seq - 1,
            items.len(),
            frame.len(),
            self.exported_frames
        );
        Ok(eneros_durable_state::PeerReceipt {
            peer: "mqtt-exporter".into(),
            receipt_id: self.frame_seq,
            at_ms: eneros_business_services::now_ms(),
        })
    }
}

#[cfg(target_os = "none")]
struct ObsHandler {
    svc: eneros_business_services::observability::ObservabilityService<MqttTelemetryExporter>,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for ObsHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        use eneros_business_services::observability::*;
        const MAGIC: &[u8; 4] = b"OBS1";
        if req.len() < 5 || &req[..4] != MAGIC {
            resp.extend_from_slice(b"ERR:proto");
            return false;
        }
        match req[4] {
            0x01 => {
                if req.len() < 5 + 34 {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let p = &req[5..];
                let class = match p[0] {
                    0 => TelemetryClass::SecurityAudit,
                    1 => TelemetryClass::Control,
                    2 => TelemetryClass::Alarm,
                    3 => TelemetryClass::Telemetry,
                    4 => TelemetryClass::Log,
                    5 => TelemetryClass::Debug,
                    _ => {
                        resp.extend_from_slice(b"ERR:class");
                        return false;
                    }
                };
                let mut service = [0u8; 16];
                service.copy_from_slice(&p[1..17]);
                let service = alloc::string::String::from_utf8_lossy(&service).into_owned();
                let mut schema = [0u8; 8];
                schema.copy_from_slice(&p[17..25]);
                let schema = alloc::string::String::from_utf8_lossy(&schema).into_owned();
                let created = u64::from_le_bytes(p[25..33].try_into().unwrap());
                let plen = p[33] as usize;
                if 5 + 34 + plen > req.len() {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let item = TelemetryItem {
                    class,
                    service,
                    schema,
                    payload: req[5 + 34..5 + 34 + plen].to_vec(),
                    created_ms: created,
                    boot_id: 1,
                    config_generation: 0,
                };
                match self.svc.enqueue(item) {
                    Ok(()) => {
                        resp.push(1);
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:");
                        resp.extend_from_slice(e.as_bytes());
                        false
                    }
                }
            }
            0x02 => {
                if req.len() != 9 {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                let batch = u32::from_le_bytes(req[5..9].try_into().unwrap()) as usize;
                match self.svc.flush_batch(batch) {
                    Ok(n) => {
                        resp.extend_from_slice(&(n as u64).to_le_bytes());
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:");
                        resp.extend_from_slice(e.as_bytes());
                        false
                    }
                }
            }
            0x03 => {
                let s = &self.svc.stats;
                resp.extend_from_slice(&s.enqueued.to_le_bytes());
                resp.extend_from_slice(&s.dropped.values().sum::<u64>().to_le_bytes());
                resp.extend_from_slice(&s.redacted.to_le_bytes());
                resp.extend_from_slice(&s.exported.to_le_bytes());
                resp.extend_from_slice(&s.export_failed.to_le_bytes());
                let _ = now_ms;
                true
            }
            _ => {
                resp.extend_from_slice(b"ERR:cmd");
                false
            }
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
