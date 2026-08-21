//! svc-control-dispatch — 蓝图 40 WP-40.6：唯一 ControlDispatch 服务（EL0）。
//!
//! 装配（40 收尾）：产品链（policy/lease/safety 同源配置）+ EIPC 投递
//! 通道（root spawn 授予 svc-device-agent 端点 → X3 bit0 → 槽 3）。
//! 未授予（X3 bit0=0）→ UnboundChannel 显式失败（fail-closed）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::string::String;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use eneros_audit::{AuditLoggerImpl, MemStore};
#[cfg(target_os = "none")]
use eneros_control_dispatch::{DispatchConfig, FeedbackPointMapping, InMemoryIntentStore};

#[cfg(target_os = "none")]
extern crate eneros_control_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_drv_svc::allocator::HeapAllocator =
    eneros_drv_svc::allocator::HeapAllocator::new();

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
    let mut handler = DispatchHandler::new(eipc_mask);
    eneros_control_services::control_service_main(
        ep,
        "control-dispatch",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

/// 47（WP-47.1 速率可调）：取 control-write 配置（svc-config ConfigGet）。
#[cfg(target_os = "none")]
fn fetch_control_write(
    config: &mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
) -> Option<eneros_config_bundle::control_write::ControlWriteConfig> {
    let mut req: alloc::vec::Vec<u8> = alloc::vec![b'C', b'F', b'G', b'1', 0x01];
    req.extend_from_slice(b"control-write");
    let mut transfer = eneros_agent_sdk::ChunkedClient::new(config, 0, 5_000);
    let payload = transfer.transfer("svc-config", &req).ok()?;
    if payload.len() < 4 {
        return None;
    }
    let plen = u32::from_le_bytes(payload[..4].try_into().unwrap_or([0; 4])) as usize;
    if 4 + plen > payload.len() {
        return None;
    }
    eneros_config_bundle::control_write::ControlWriteConfig::decode(&payload[4..4 + plen]).ok()
}

/// 47（WP-47.4）：意图持久存储装配——svc-storage EIPC（X3 bit4）→
/// `DurableIntentStore`（open 失败 → InMemory 兜底 + 登记）；未授予 →
/// InMemory 兜底（fail-closed 登记）。
#[cfg(target_os = "none")]
fn build_intent_store(
    eipc_mask: u64,
) -> alloc::boxed::Box<dyn eneros_control_dispatch::IntentStore> {
    use eneros_agent_sdk::block_io::eipc_block_io;
    use eneros_control_dispatch::durable_intent::DurableIntentStore;
    if eipc_mask & 16 != 0 {
        if let Some(io) = eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT + 4,
            0, // 首个响应吸收 svc-storage 实例 epoch（业务服务先例）
            5_000,
        ) {
            // 区域：偏移 0，槽 512B × 256（意图日志区；svc-storage 独立
            // 区域，host QEMU 盘 16MB 起）。
            let mut store = DurableIntentStore::new(io, 0, 512, 256);
            match store.open() {
                Ok(()) => {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] DURABLE-INTENT-STORE open OK（AppendOutbox over svc-storage）"
                    );
                    return alloc::boxed::Box::new(store);
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] durable intent store open FAILED {e:?} —— InMemory 兜底（重启对账不可用）"
                    );
                }
            }
        } else {
            eneros_runtime::println!(
                "[svc-control-dispatch] storage EIPC bind FAILED —— InMemory 兜底（重启对账不可用）"
            );
        }
    } else {
        eneros_runtime::println!(
            "[svc-control-dispatch] storage EP NOT granted（X3 bit4=0）—— InMemory 兜底（重启对账不可用）"
        );
    }
    alloc::boxed::Box::new(InMemoryIntentStore::new(1024))
}

#[cfg(target_os = "none")]
struct DispatchHandler {
    engine: eneros_control_dispatch::DispatchEngine,
    /// 47（WP-47.1 协议入站写）：svc-telemetry 快照客户端（授予槽
    /// `EIPC_EP_BASE_SLOT + 2`）——意图授权前取受信任遥测（新鲜度/安全
    /// 目录由 12 态链裁决）；未授予 → 意图路径 fail-closed。
    telemetry: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 47（WP-47.1）：意图线码请求处理计数（观测）。
    pub intent_requests: u64,
}

/// 47（WP-47.1 协议出站通道）：`DeliveryChannel` impl——授权后的
/// `ControlEnvelope` 经 drv-svc 信封（`OP_MASTER_WRITE`）投给
/// svc-modbus-tcp 执行主站写 + 独立 readback，返回 2B `OutcomeFrame`
/// 字节（dispatch 引擎按既有回执语义分类）。
///
/// **只执行**：九步链授权在 dispatch（不双头授权）；本通道不解码/不
/// 重授权信封内命令。
#[cfg(target_os = "none")]
struct EipcMasterChannel {
    client: eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    correlation: u32,
}

#[cfg(target_os = "none")]
impl EipcMasterChannel {
    fn new(endpoint: &str, dest_cap: u64) -> Result<Self, eneros_agent_sdk::AgentError> {
        Ok(Self {
            client: eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                endpoint,
                dest_cap,
                eneros_drv_svc::now_ns,
            )?,
            correlation: 0,
        })
    }
}

#[cfg(target_os = "none")]
impl eneros_control_dispatch::DeliveryChannel for EipcMasterChannel {
    fn deliver(
        &mut self,
        _target: &str,
        payload: &[u8],
        timeout_ms: u64,
    ) -> Result<alloc::vec::Vec<u8>, eneros_control_dispatch::ChannelError> {
        use eneros_control_dispatch::ChannelError;
        // 解码 ControlEnvelope（270B）→ 信封 + 功率域。
        let ce = eneros_control_gateway::wire::ControlEnvelope::decode(payload)
            .map_err(|e| ChannelError::Protocol(format!("env:{e:?}")))?;
        if !ce.verify_payload_digest() {
            return Err(ChannelError::Protocol(String::from("payload-digest")));
        }
        let env = eneros_control_gateway::envelope::CommandEnvelope::decode(&ce.envelope)
            .map_err(|e| ChannelError::Protocol(format!("inner:{e:?}")))?;
        // 装配 drv-svc 写请求：**OP_EXECUTE + 载荷子码**（drv-svc runtime
        // 只把 OP_EXECUTE 路由到服务 handle；自定义 opcode 一律 ST_INVALID）。
        // payload = [sub=OP_MASTER_WRITE][device u32][point u32][power f64] = 17B。
        use eneros_drv_svc::runtime::{wire, DriverRequest, OP_EXECUTE};
        use eneros_protocol_abstract::write::OP_MASTER_WRITE;
        self.correlation = self.correlation.wrapping_add(1);
        let mut req = DriverRequest::new(self.correlation, OP_EXECUTE);
        req.payload[0] = OP_MASTER_WRITE;
        req.payload[1..5].copy_from_slice(&env.device_id.to_le_bytes());
        req.payload[5..9].copy_from_slice(&env.point_id.to_le_bytes());
        req.payload[9..17].copy_from_slice(&ce.power_kw.to_le_bytes());
        req.payload_len = 17;
        let raw = wire::encode_request(&req);
        let timeout_ns = if timeout_ms == 0 {
            5_000_000_000u64
        } else {
            timeout_ms.saturating_mul(1_000_000)
        };
        let resp_bytes = self
            .client
            .drv_call(&raw, timeout_ns)
            .map_err(|e| match e {
                eneros_agent_sdk::AgentError::Timeout => ChannelError::Timeout,
                _ => ChannelError::Protocol(format!("drv:{e:?}")),
            })?;
        // drv-svc 服务端按 4B/word 回 MR：RESPONSE_LEN=79 会以 20 words
        //（80B）返回；decode_response 要求精确 79B，直接解码会误判
        // bad-envelope（block_io.rs 同款 QEMU 实测坑）——接受尾部 padding。
        let decode_len = core::cmp::min(
            resp_bytes.len(),
            eneros_drv_svc::runtime::wire::RESPONSE_LEN,
        );
        let resp =
            wire::decode_response(&resp_bytes[..decode_len]).ok_or(ChannelError::BadResponse)?;
        if resp.status != eneros_drv_svc::runtime::ST_OK {
            return Err(ChannelError::Rejected);
        }
        if resp.payload_len != 2 {
            return Err(ChannelError::BadResponse);
        }
        Ok(resp.payload[..2].to_vec())
    }
}

#[cfg(target_os = "none")]
impl DispatchHandler {
    fn new(eipc_mask: u64) -> Self {
        // 装配（40 收尾 + 47 WP-47.1）：产品链 + 投递通道（协议出站优先）。
        use eneros_control_services::dispatch_service::{
            prod_chain, ASSEMBLY_TEST_KEY, PROD_EPOCH_SEED, PROD_LEASE_ID, PROD_POLICY_VERSION,
        };
        let now = eneros_drv_svc::now_ms();
        // 47（WP-47.2）：写目标（设备/点）由 45 卷点表 Control 点绑定
        // （svc-config config "points"）；不可得 → 0/0 fail-closed（全部
        // 提交/意图 NotAuthorized 拒绝，串口登记）。
        let mut config_client = if eipc_mask & 8 != 0 {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "control-dispatch",
                eneros_agent_sdk::EIPC_EP_BASE_SLOT + 3,
                eneros_drv_svc::now_ns,
            )
            .ok()
        } else {
            None
        };
        let binding = match &mut config_client {
            Some(c) => eneros_control_services::cfg_bindings::fetch_control_binding(c),
            None => {
                eneros_runtime::println!(
                    "[svc-control-dispatch] config EP NOT granted —— 控制绑定缺失（fail-closed）"
                );
                None
            }
        };
        let (device_id, point_id) = binding.unwrap_or((0, 0));
        let chain = prod_chain(ASSEMBLY_TEST_KEY, device_id, point_id, now);
        // 47（WP-47.1 速率可调）：rate gate 每小时配额 = control-write
        // 配置注入（bundle；缺省 = 产品默认 60/h——qemu-test 演示放宽值
        // 经 configs/control-write.toml 编译进 bundle）。配置不可得 →
        // 60/h 产品默认（fail-closed 面：配额不可放宽）。
        let mut rate_per_hour: u32 = 60;
        if let Some(c) = &mut config_client {
            if let Some(cfg) = fetch_control_write(c) {
                rate_per_hour = cfg.rate_per_hour.unwrap_or(60);
            }
        }
        eneros_runtime::println!(
            "[svc-control-dispatch] RATE-GATE quota={rate_per_hour}/h（产品默认 60/h）"
        );
        // 47（WP-47.1）：投递通道选择——X3 bit1（svc-modbus-tcp 主站）→
        // 协议出站通道（release 设备写走协议）；bit0（svc-device-agent）→
        // 虚拟设备通道（qemu-test 保留）；均未授予 → Unbound（fail-closed）。
        let delivery: alloc::boxed::Box<dyn eneros_control_dispatch::DeliveryChannel> = if eipc_mask
            & 2
            != 0
        {
            match EipcMasterChannel::new(
                "control-dispatch",
                eneros_agent_sdk::EIPC_EP_BASE_SLOT + 1,
            ) {
                Ok(ch) => {
                    eneros_runtime::println!(
                            "[svc-control-dispatch] protocol-outbound channel bound: slot={} target=svc-modbus-tcp (OP_MASTER_WRITE)",
                            eneros_agent_sdk::EIPC_EP_BASE_SLOT + 1
                        );
                    alloc::boxed::Box::new(ch)
                }
                Err(e) => {
                    eneros_runtime::println!(
                            "[svc-control-dispatch] master channel bind FAILED: {e:?} — Unbound (fail-closed)"
                        );
                    alloc::boxed::Box::new(UnboundChannel)
                }
            }
        } else if eipc_mask & 1 != 0 {
            match eneros_control_dispatch::Sel4EipcChannel::new(
                "control-dispatch",
                eneros_agent_sdk::EIPC_EP_BASE_SLOT,
                eneros_drv_svc::instance_epoch(),
                5_000,
                eneros_drv_svc::now_ns,
            ) {
                Ok(ch) => {
                    eneros_runtime::println!(
                            "[svc-control-dispatch] EIPC delivery bound: slot={} target=svc-device-agent (qemu-test 虚拟设备)",
                            eneros_agent_sdk::EIPC_EP_BASE_SLOT
                        );
                    alloc::boxed::Box::new(ch)
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] EIPC bind FAILED: {e:?} — Unbound (fail-closed)"
                    );
                    alloc::boxed::Box::new(UnboundChannel)
                }
            }
        } else {
            eneros_runtime::println!(
                    "[svc-control-dispatch] EIPC cap not granted (X3=0x{eipc_mask:x}) — Unbound (fail-closed)"
                );
            alloc::boxed::Box::new(UnboundChannel)
        };
        // 47（WP-47.1）：svc-telemetry 快照客户端（X3 bit2）。
        let telemetry = if eipc_mask & 4 != 0 {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "control-dispatch",
                eneros_agent_sdk::EIPC_EP_BASE_SLOT + 2,
                eneros_drv_svc::now_ns,
            )
            .ok()
        } else {
            None
        };
        if telemetry.is_none() {
            eneros_runtime::println!(
                "[svc-control-dispatch] telemetry EP NOT granted —— 意图路径 fail-closed"
            );
        }
        let config = DispatchConfig {
            chain,
            key: ASSEMBLY_TEST_KEY.to_vec(),
            lease_id: PROD_LEASE_ID,
            fencing_token: PROD_EPOCH_SEED + 1,
            issuer_service: [0x41; 16],
            issuer_epoch: 0,
            policy_version: PROD_POLICY_VERSION,
            device_id,
            point_id,
            target_endpoint: alloc::string::String::from("svc-device-agent"),
            max_power_kw: 500.0,
            // 47（WP-47.1）：rate gate 默认产品值 60/h；bundle control-write
            // 可调（qemu-test 注入放宽值；源码不写测试值）。
            gate: eneros_dual_brain::gate::DecisionGate::new(rate_per_hour, 1000.0, 3_600_000),
            // 47（WP-47.4 意图持久化）：DurableIntentStore（AppendOutbox
            // over BlockIo → svc-storage，槽 base+4）；未授予/打开失败 →
            // InMemory 兜底 + 串口登记（重启对账不可用面）。
            intent_log: build_intent_store(eipc_mask),
            // 47（WP-47.4 审计持久化）：SHA-256 控制事件链（D4 线）。
            control_audit: Some(alloc::boxed::Box::new(eneros_audit::AuditLoggerImpl::new(
                eneros_audit::MemStore::new(),
                [0x47; 32],
            ))),
            audit: Some(eneros_dual_brain::audit::DecisionAudit::new([0x47; 32])),
            delivery,
            feedback_mapping: FeedbackPointMapping {
                grid_up: 5,
                soc: 4,
                dc_bus_v: 1,
                overcurrent: 6,
                overtemp: 6,
            },
            now_fn: eneros_drv_svc::now_ms,
        };
        Self {
            engine: eneros_control_dispatch::DispatchEngine::new(config),
            telemetry,
            intent_requests: 0,
        }
    }

    /// 47（WP-47.1）：取受信任遥测快照（svc-telemetry TEL_SNAPSHOT →
    /// 144B wire）。不可得 → None（调用方 fail-closed 拒绝）。
    fn fetch_trusted_telemetry(
        telemetry: &mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    ) -> Option<eneros_safety_validator::TrustedTelemetry> {
        let mut transfer = eneros_agent_sdk::ChunkedClient::new(telemetry, 0, 5_000);
        let resp = transfer.transfer("svc-telemetry", &[2u8]).ok()?; // TEL_SNAPSHOT
        if resp.first() != Some(&0) || resp.len() != 1 + 144 {
            return None;
        }
        eneros_control_services::telemetry::decode_trusted(&resp[1..])
    }
}

/// 未绑定端点（无 EIPC cap 授予）——deliver 明确失败。
#[cfg(target_os = "none")]
struct UnboundChannel;

#[cfg(target_os = "none")]
impl eneros_control_dispatch::DeliveryChannel for UnboundChannel {
    fn deliver(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<alloc::vec::Vec<u8>, eneros_control_dispatch::ChannelError> {
        Err(eneros_control_dispatch::ChannelError::Unreachable)
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for DispatchHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        use eneros_control_services::dispatch_service::{DISP_SUBMIT, DISP_SUBMIT_INTENT};
        match req.first() {
            Some(&DISP_SUBMIT) => {
                *resp =
                    eneros_control_services::dispatch_service::handle_submit(&mut self.engine, req);
            }
            Some(&DISP_SUBMIT_INTENT) => {
                // 47（WP-47.1 协议入站写）：意图授权前取受信任遥测快照；
                // 不可得 → 服务侧 fail-closed 拒绝（不猜成功）。
                self.intent_requests += 1;
                let tel = match &mut self.telemetry {
                    Some(t) => Self::fetch_trusted_telemetry(t),
                    None => None,
                };
                if tel.is_none() {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] INTENT request: telemetry snapshot unavailable —— fail-closed"
                    );
                }
                *resp = eneros_control_services::dispatch_service::handle_submit_intent(
                    &mut self.engine,
                    req,
                    tel.as_ref(),
                );
            }
            _ => {
                *resp =
                    eneros_control_services::dispatch_service::handle_submit(&mut self.engine, req);
            }
        }
        true
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
