//! svc-ai-worker — 蓝图 49 WP-49.2：target AI worker（EL0，远程 dual-brain）。
//!
//! 决策供给（ADR-0045 方案 B）：System2 = 站端 LLM 服务（**不受信 peer**，
//! 经 MQTT QoS1 交互；应答经 intent-contract::peer 全量校验 + remote_brain
//! 状态机配对/超时/降级）；System1 = 静态启发式（SOC 死区，fail-operational
//! 兜底）。宣称纪律：「AI 辅助决策（需配套站端服务）」——非设备端本地推理。
//!
//! 线路：
//! - svc-energy-agent → 本服务 `[AI_PROPOSE=1][trusted wire]`（既有链）；
//! - svc-mqtt → 本服务 `[AI_PEER_MSG=0x10][应答 JSON]`（ChunkedClient 推送）；
//! - 本服务 → svc-mqtt `[EXECUTE][AI_REQ_BEGIN/CONT/END]`（请求 JSON 分片
//!   drv_call 发布）；
//! - 本服务 → svc-config `[CFG1 GET_ENTRY "ai-peer"]`（状态机参数，全必配）。
//!
//! 决策留痕（蓝图 WP-49.2 #5）：每次 AI 参与决策的 prompt 摘要/应答摘要/
//! 路径标记入 DecisionAudit 哈希链（截断 ≤48/≤64 字符，不落全文）+ 串口
//! REMOTE-BRAIN 证据行。
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::{string::String, vec::Vec};
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_control_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_drv_svc::allocator::HeapAllocator =
    eneros_drv_svc::allocator::HeapAllocator::new();

/// peer 应答推送子码（svc-mqtt → 本服务；agent-sdk::ai_relay 单一事实源）。
#[cfg(target_os = "none")]
const AI_PEER_MSG: u8 = eneros_agent_sdk::ai_relay::AI_PEER_MSG;

// ============================================================================
// PeerLink 实现：请求 JSON → svc-mqtt 分片 EXECUTE（drv wire 单帧 ≤64B）
// ============================================================================

#[cfg(target_os = "none")]
struct PeerLinkEipc {
    mqtt: eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    correlation: u32,
}
#[cfg(target_os = "none")]
impl PeerLink for PeerLinkEipc {
    fn send_request(&mut self, req_json: &str) -> Result<(), LinkError> {
        use eneros_agent_sdk::ai_relay::fragment_request;
        let (_total, frames) = fragment_request(req_json.as_bytes())
            .map_err(|e| LinkError(alloc::format!("{e:?}")))?;
        // 本地 IPC 正常亚毫秒；2s 上限防 mqtt 未就绪时长阻塞（心跳面安全边际）。
        let timeout_ns = 2_000_000_000u64;
        for frame in frames {
            self.correlation = self.correlation.wrapping_add(1);
            let mut req = eneros_drv_svc::runtime::DriverRequest::new(
                self.correlation,
                eneros_drv_svc::runtime::OP_EXECUTE,
            );
            req.payload[..frame.len()].copy_from_slice(&frame);
            req.payload_len = frame.len() as u16;
            let raw = eneros_drv_svc::runtime::wire::encode_request(&req);
            // drv-svc 服务端按 4B/word 回 MR（47 卷 padding 坑同款）——
            // 接受尾部补零后按精确长度解码。
            match self.mqtt.drv_call(&raw, timeout_ns) {
                Ok(resp) => {
                    let decode_len =
                        core::cmp::min(resp.len(), eneros_drv_svc::runtime::wire::RESPONSE_LEN);
                    let resp = eneros_drv_svc::runtime::wire::decode_response(&resp[..decode_len])
                        .ok_or_else(|| LinkError(String::from("bad-resp")))?;
                    if resp.status != eneros_drv_svc::runtime::ST_OK
                        || resp.payload.first() != Some(&0)
                    {
                        return Err(LinkError(alloc::format!(
                            "mqtt-reject status={}",
                            resp.status
                        )));
                    }
                }
                Err(e) => return Err(LinkError(alloc::format!("drv:{e:?}"))),
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "none")]
use eneros_control_services::ai_worker::AiWorkerService;
#[cfg(target_os = "none")]
use eneros_control_services::remote_brain::{
    LinkError, PeerLink, RemoteBrain, RemoteBrainConfig, SwitchEvent,
};

#[cfg(target_os = "none")]
type RemoteWorker = AiWorkerService<
    RemoteBrain<eneros_control_services::ai_worker::HeuristicBackend, PeerLinkEipc>,
>;
#[cfg(target_os = "none")]
type LocalWorker = AiWorkerService<eneros_control_services::ai_worker::HeuristicBackend>;

// ============================================================================
// ai-peer 配置拉取（svc-config CFG1；全必配 fail-closed）
// ============================================================================

#[cfg(target_os = "none")]
fn fetch_ai_peer_config(
    client: &mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
) -> Result<eneros_config_bundle::ai_peer::AiPeerConfig, &'static str> {
    let mut req: Vec<u8> = alloc::vec![b'C', b'F', b'G', b'1', 0x01];
    req.extend_from_slice(b"ai-peer");
    let mut transfer = eneros_agent_sdk::ChunkedClient::new(client, 0, 5_000);
    let payload = transfer
        .transfer("svc-config", &req)
        .map_err(|_| "config-eipc-transfer-failed")?;
    if payload.starts_with(b"ERR:") {
        // svc-config 结构化拒绝——透传原因串（诊断面）。
        let reason = core::str::from_utf8(&payload[..payload.len().min(40)]).unwrap_or("ERR:");
        eneros_runtime::println!("[svc-ai-worker] AI-PEER-CFG server {reason}");
        return Err("config-server-error");
    }
    if payload.len() < 4 {
        return Err("config-response-too-short");
    }
    let plen = u32::from_le_bytes(payload[..4].try_into().map_err(|_| "bad-prefix")?) as usize;
    if 4 + plen > payload.len() {
        return Err("config-length-malformed");
    }
    eneros_config_bundle::ai_peer::AiPeerConfig::decode(&payload[4..4 + plen])
        .map_err(|_| "config-decode-failed")
}

// ============================================================================
// 服务入口
// ============================================================================

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
    let base = eneros_agent_sdk::EIPC_EP_BASE_SLOT;
    // X3 表序（spawn.rs EIPC_GRANTS）：bit0=svc-config、bit1=svc-mqtt。
    // 任一未授予 → 对应能力 fail-closed（config 缺 → 启发式兜底；
    // mqtt 缺 → 无远程路径，纯启发式——诚实降级不伪造）。
    let config_client = if eipc_mask & 1 != 0 {
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
            "aiworker-config",
            base,
            eneros_drv_svc::now_ns,
        )
        .ok()
    } else {
        None
    };
    let cfg = config_client.and_then(|mut c| match fetch_ai_peer_config(&mut c) {
        Ok(c) => Some(c),
        Err(reason) => {
            eneros_runtime::println!("[svc-ai-worker] AI-PEER-CFG FAIL {reason}");
            None
        }
    });
    let mqtt_client = if eipc_mask & 2 != 0 {
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
            "aiworker-mqtt",
            base + 1,
            eneros_drv_svc::now_ns,
        )
        .ok()
    } else {
        None
    };

    match (cfg, mqtt_client) {
        (Some(c), Some(mqtt)) => {
            eneros_runtime::println!(
                "[svc-ai-worker] AI-PEER-CFG loaded timeout={} unhealthy={} probe={} max_kw={:.1} ttl={}",
                c.timeout_ms,
                c.unhealthy_after,
                c.probe_every,
                c.max_power_kw,
                c.ttl_secs
            );
            eneros_runtime::println!(
                "[svc-ai-worker] backend=remote-brain (System2=station peer via MQTT; System1=heuristic fallback; ADR-0045 plan B)"
            );
            let brain_cfg = RemoteBrainConfig {
                timeout_ms: c.timeout_ms,
                unhealthy_after: c.unhealthy_after,
                probe_every: c.probe_every,
                max_power_kw: c.max_power_kw,
                ttl_secs: c.ttl_secs,
            };
            run_remote(ep, ready_ntfn, heartbeat_slot, brain_cfg, mqtt)
        }
        (cfg, mqtt_client) => {
            eneros_runtime::println!(
                "[svc-ai-worker] backend=heuristic (cfg={} mqtt={} —— 远程路径 fail-closed)",
                cfg.is_some(),
                mqtt_client.is_some()
            );
            run_heuristic_only(ep, ready_ntfn, heartbeat_slot)
        }
    }
}

#[cfg(target_os = "none")]
fn run_remote(
    ep: u64,
    ready_ntfn: u64,
    heartbeat_slot: u64,
    cfg: RemoteBrainConfig,
    mqtt: eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
) -> ! {
    // 决策审计（ADR-0042 边界：dual-brain/audit 为决策语义门面；genesis 与
    // dispatch 装配常量同源 [0x47;32]）。
    let mut audit = eneros_dual_brain::audit::DecisionAudit::new([0x47; 32]);
    let link = PeerLinkEipc {
        mqtt,
        correlation: 0,
    };
    let brain = RemoteBrain::new(
        eneros_control_services::ai_worker::HeuristicBackend::new(),
        link,
        cfg,
    );
    let mut worker = AiWorkerService::new(brain, [0x41; 16]);
    let mut handler = RemoteAiHandler {
        worker: &mut worker,
        audit: &mut audit,
    };
    eneros_control_services::control_service_main(
        ep,
        "ai-worker",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
fn run_heuristic_only(ep: u64, ready_ntfn: u64, heartbeat_slot: u64) -> ! {
    let mut audit = eneros_dual_brain::audit::DecisionAudit::new([0x47; 32]);
    let mut worker = AiWorkerService::new(
        eneros_control_services::ai_worker::HeuristicBackend::new(),
        [0x41; 16],
    );
    let mut handler = HeuristicOnlyHandler {
        worker: &mut worker,
        audit: &mut audit,
    };
    eneros_control_services::control_service_main(
        ep,
        "ai-worker",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

// ============================================================================
// 审计留痕
// ============================================================================

/// 审计入链 + 串口证据行（决策留痕；截断已在 remote_brain 摘要侧完成）。
#[cfg(target_os = "none")]
fn log_decision(
    audit: &mut eneros_dual_brain::audit::DecisionAudit,
    intent: &str,
    lp: &str,
    now_ms: u64,
) {
    use eneros_dual_brain::audit::{DecisionRecord, ExecOutcome, SafetyOutcome};
    let _ = audit.log_decision(&DecisionRecord {
        actor: "ai-worker",
        target: "pcs",
        intent,
        lp_solution: lp,
        safety: SafetyOutcome::Passed,
        exec: ExecOutcome::Proposed,
        deviation: 0.0,
        now_ms,
    });
    eneros_runtime::println!(
        "[svc-ai-worker] AUDIT-AI appended={} dropped={}",
        audit.chain_state().entry_count,
        audit.dropped_count()
    );
}

// ============================================================================
// 远程 dual-brain Handler（主路径）
// ============================================================================

#[cfg(target_os = "none")]
struct RemoteAiHandler<'a> {
    worker: &'a mut RemoteWorker,
    audit: &'a mut eneros_dual_brain::audit::DecisionAudit,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for RemoteAiHandler<'_> {
    fn handle(&mut self, req: &[u8], resp: &mut Vec<u8>, now_ms: u64) -> bool {
        use eneros_control_services::ai_worker::AI_PROPOSE;
        use eneros_control_services::remote_brain::PeerMsgVerdict;
        match req.first() {
            Some(&AI_PROPOSE) => {
                *resp = self.worker.propose(req, now_ms);
                if resp.first() == Some(&eneros_control_services::ai_worker::AI_OK) {
                    self.log_proposal(now_ms);
                }
                true
            }
            Some(&AI_PEER_MSG) => {
                // peer 应答推送（不受信 JSON）→ 全量校验 + 配对。
                let detail;
                let brain = self.worker.backend_mut();
                let verdict = match core::str::from_utf8(&req[1..]) {
                    Ok(j) => {
                        let v = brain.on_peer_message(j, now_ms);
                        detail = brain.last_reject_detail.clone().unwrap_or_default();
                        v
                    }
                    Err(e) => {
                        brain.bad_responses_total = brain.bad_responses_total.saturating_add(1);
                        detail = alloc::format!("utf8:{e}");
                        PeerMsgVerdict::Bad
                    }
                };
                let brain = self.worker.backend_mut();
                eneros_runtime::println!(
                    "[svc-ai-worker] PEER-MSG verdict={verdict:?} bad={} late={} healthy={} detail={}",
                    brain.bad_responses_total,
                    brain.late_total,
                    brain.is_healthy(),
                    detail
                );
                *resp = alloc::vec![AI_PEER_MSG, 0];
                true
            }
            _ => {
                *resp = alloc::vec![0xFF];
                true
            }
        }
    }
}

#[cfg(target_os = "none")]
impl RemoteAiHandler<'_> {
    /// 成功提案的决策留痕：切换事件 + 路径标记摘要入审计链与串口。
    fn log_proposal(&mut self, now_ms: u64) {
        let events: Vec<SwitchEvent> = self.worker.backend_mut().drain_events();
        for ev in &events {
            match ev {
                SwitchEvent::PeerUnhealthy {
                    consecutive_timeouts,
                } => {
                    eneros_runtime::println!(
                        "[svc-ai-worker] REMOTE-BRAIN event=peer-unhealthy consecutive={consecutive_timeouts}"
                    );
                    log_decision(self.audit, "event:peer-unhealthy", "", now_ms);
                }
                SwitchEvent::PeerRecovered => {
                    eneros_runtime::println!("[svc-ai-worker] REMOTE-BRAIN event=peer-recovered");
                    log_decision(self.audit, "event:peer-recovered", "", now_ms);
                }
            }
        }
        let brain = self.worker.backend_mut();
        let healthy = brain.is_healthy();
        let timeouts = brain.timeouts_total;
        let bad = brain.bad_responses_total;
        let late = brain.late_total;
        let link_err = brain.link_errors;
        if let Some(meta) = brain.last_meta() {
            eneros_runtime::println!(
                "[svc-ai-worker] REMOTE-BRAIN used={} path={:?} req={:?} prompt=\"{}\" resp=\"{}\" healthy={} timeouts={} bad={} late={} link_err={}",
                meta.used_remote,
                meta.path,
                meta.req_id,
                meta.prompt_summary,
                meta.resp_summary,
                healthy,
                timeouts,
                bad,
                late,
                link_err,
            );
            let intent = if meta.used_remote {
                alloc::format!("sys2-remote {}", meta.prompt_summary)
            } else {
                String::from("sys1-heuristic")
            };
            let sol = if meta.used_remote {
                meta.resp_summary.clone()
            } else {
                String::from("deadband ±5kW")
            };
            log_decision(self.audit, &intent, &sol, now_ms);
        }
    }
}

// ============================================================================
// 纯启发式兜底 Handler（config/mqtt 未授予）
// ============================================================================

#[cfg(target_os = "none")]
struct HeuristicOnlyHandler<'a> {
    worker: &'a mut LocalWorker,
    audit: &'a mut eneros_dual_brain::audit::DecisionAudit,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for HeuristicOnlyHandler<'_> {
    fn handle(&mut self, req: &[u8], resp: &mut Vec<u8>, now_ms: u64) -> bool {
        use eneros_control_services::ai_worker::AI_PROPOSE;
        match req.first() {
            Some(&AI_PROPOSE) => {
                *resp = self.worker.propose(req, now_ms);
                if resp.first() == Some(&eneros_control_services::ai_worker::AI_OK) {
                    log_decision(self.audit, "sys1-heuristic", "deadband ±5kW", now_ms);
                }
                true
            }
            _ => {
                *resp = alloc::vec![0xFF];
                true
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

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
