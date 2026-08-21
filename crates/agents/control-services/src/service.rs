//! 蓝图 40：EL0 控制服务通用 IPC 主循环（WP-40.9 服务循环模式）。
//!
//! 复用 agent-sdk 分块传输协议（`ChunkedReceiver`）接收大载荷
//! （ControlEnvelope 270B 超 seL4 MR 120B 预算）：header(request)→ack、
//! data(send)×n、final(request)→处理回执。任何协议违例 → 明确错误响应，
//! 服务端半组装缓冲丢弃（fail-closed）。
//!
//! host 构建：`main` no-op（与 svc-drv-* 同模式——target-only 代码由
//! 协议层 host 测试覆盖，见 agent-sdk `chunked` 与各服务逻辑模块测试）。

#[cfg(target_os = "none")]
use alloc::format;

/// 服务处理器（收到完整载荷后调用；返回响应字节）。
pub trait ControlServiceHandler {
    /// 处理完整请求载荷并写入响应。
    ///
    /// `now_ms` = 单调毫秒；返回 false = 处理失败（fail-closed：响应
    /// 为结构化错误）。
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool;
}

/// 目标侧服务主循环（分块 EIPC）。
///
/// `ep` = 服务 endpoint cap；`ready_ntfn` = readiness 通知 cap；
/// `now_ms`/`epoch` = 时钟与服务实例 epoch；`heartbeat_slot` = 心跳
/// notification 的服务 CSpace 槽（0 = 未授予，不心跳——spawn 期由 root
/// mint，supervisor 心跳超时监控用，蓝图 40 收尾 WP-40.4 隔离面）。
///
/// 响应方向分页（ENVP/ENVN）：handler 产出响应缓存后回分页头；客户端
/// 逐页拉取（每页 ≤104B）——单帧响应受 MR 112B 预算约束（SNAPSHOT
/// 145B 响应在无分页时会被静默截断，QEMU 实弹暴露）。
#[cfg(target_os = "none")]
pub fn control_service_main(
    ep: u64,
    name: &'static str,
    ready_ntfn: u64,
    now_ms: fn() -> u64,
    epoch: u64,
    heartbeat_slot: u64,
    handler: &mut dyn ControlServiceHandler,
) -> ! {
    use eneros_sel4_sys::safe::{
        get_mr, ipc_recv, ipc_reply_recv, ipc_signal, message_info_new, set_mr,
    };
    use eneros_sel4_sys::types::message_info_length;
    let _ = epoch;
    // readiness 信号（root 侧收集 PRODUCT-READY）。
    let _ = ipc_signal(ready_ntfn);
    // 46.5（QEMU 实弹暴露的并发缺陷修复）：分块状态机**按发送方 badge**
    // 独立维护——单接收器在多客户端交错时（modbus INGEST × energy-agent
    // SNAPSHOT 同窗口）Header 互相 reset → ERR → ServerRejected（45 卷
    // 靠启动时序侥幸规避）。badge → 槽线性探测。
    //
    // 46.5 二修：槽数 4 → 24——svc-config 启动期客户端 >4（6 协议服务各自
    // ConfigGet + 点表 + 业务服务）实测仍有共享槽交错 → ERR（serverrejected/
    // protocol config unavailable，QEMU 实弹登记）。每槽仅持分块重组缓冲
    //（少量 KB），24 槽对 EL0 栈/堆预算可忽略；服务运行期客户数 ≤24 硬上限，
    // 超出退化为共享槽 0（背压面）。
    const MAX_CLIENTS: usize = 24;
    struct ClientState {
        badge: u64,
        rx: eneros_agent_sdk::ChunkedReceiver,
        paged_resp: alloc::vec::Vec<u8>,
        pending_full: Option<alloc::vec::Vec<u8>>,
    }
    let mut clients: [Option<ClientState>; MAX_CLIENTS] = core::array::from_fn(|_| None);
    fn slot_of(clients: &[Option<ClientState>; 24], badge: u64) -> usize {
        for (i, c) in clients.iter().enumerate() {
            if c.as_ref().map(|c| c.badge) == Some(badge) {
                return i;
            }
        }
        for (i, c) in clients.iter().enumerate() {
            if c.is_none() {
                return i;
            }
        }
        0 // 满 24 槽：退化为共享槽 0（背压面——运行期客户端数超预算）。
    }
    let mut sender = 0u64;
    let mut reply_info = 0u64;
    loop {
        // 接收一条消息（首条 Call；后续 reply+recv——Send 消息的 reply
        // 由 seL4 自动丢弃）。
        let mi = {
            let r = if reply_info == 0 {
                ipc_recv(ep, Some(&mut sender))
            } else {
                ipc_reply_recv(ep, reply_info, Some(&mut sender))
            };
            match r {
                Ok(m) => m,
                Err(_) => {
                    // fail-closed：IPC 错误挂起（root/watchdog 复位）。
                    loop {
                        core::hint::spin_loop();
                    }
                }
            }
        };
        // 读取 MR 载荷（QEMU 实弹语义：数据区从 MR[0] 开始——seL4 非
        // MCS IPC 拷贝发送方 buffer 前 length words，从 offset 0 起；
        // 40 原始设计的 MR[1..] 在实弹下拷不到，响应端残留垃圾）。
        // **必须按接收 MessageInfo 的 length（words）截断**——不截断会
        // 读满缓冲（含残留字节），协议解码 BadLength（QEMU 实弹暴露）。
        let msg_words = message_info_length(mi) as usize;
        let mut payload = [0u8; eneros_agent_sdk::CHUNK_FRAME_LEN];
        let mut len = 0usize;
        for i in 0..core::cmp::min(msg_words, 15) {
            match get_mr(i as u64) {
                Ok(word) => {
                    let bytes = word.to_le_bytes();
                    let n = core::cmp::min(8, payload.len().saturating_sub(len));
                    payload[len..len + n].copy_from_slice(&bytes[..n]);
                    len += n;
                    if len >= payload.len() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = name;
        // 心跳：处理任何一条消息前先 signal（消息驱动的进展证明——
        // supervisor 以"有请求在途却无心跳"判定卡死）。
        if heartbeat_slot != 0 {
            let _ = ipc_signal(heartbeat_slot);
        }
        // 分页拉取请求（分块状态机前拦截——ENVN 不是 chunked magic）。
        let si = slot_of(&clients, sender);
        if clients[si].is_none() {
            clients[si] = Some(ClientState {
                badge: sender,
                rx: eneros_agent_sdk::ChunkedReceiver::new(),
                paged_resp: alloc::vec::Vec::new(),
                pending_full: None,
            });
        }
        let cs = clients[si].as_mut().expect("slot");
        let response: alloc::vec::Vec<u8> = if eneros_agent_sdk::is_pull_request(&payload[..len]) {
            eneros_agent_sdk::page_slice(&cs.paged_resp, payload[4]).to_vec()
        } else if len >= 4 && &payload[..4] == eneros_agent_sdk::CHUNK_MAGIC_FINAL {
            // Final（request）：触发完整载荷处理 + 分页头回执。
            match cs.pending_full.take() {
                Some(full) => {
                    let mut resp = alloc::vec::Vec::new();
                    let ok = handler.handle(&full, &mut resp, now_ms());
                    if !ok && resp.is_empty() {
                        resp.extend_from_slice(b"ERR");
                    }
                    // 缓存完整响应 → Final 回执 = 分页头（客户端逐页拉取）。
                    cs.paged_resp = resp;
                    eneros_agent_sdk::encode_paged_header(cs.paged_resp.len()).to_vec()
                }
                None => {
                    let resp = b"ERR:NoTransfer".to_vec();
                    cs.paged_resp = resp.clone();
                    resp
                }
            }
        } else {
            match cs.rx.on_message(&payload[..len], true) {
                eneros_agent_sdk::ChunkedEvent::Header { .. } => {
                    eneros_agent_sdk::ack_bytes().to_vec()
                }
                eneros_agent_sdk::ChunkedEvent::Data { .. } => alloc::vec::Vec::new(),
                eneros_agent_sdk::ChunkedEvent::Complete(full) => {
                    // 缓存完整载荷，等待 Final 触发处理（QEMU 实弹修正）。
                    cs.pending_full = Some(full);
                    alloc::vec::Vec::new()
                }
                eneros_agent_sdk::ChunkedEvent::Malformed(e) => {
                    let mut resp = alloc::vec::Vec::new();
                    resp.extend_from_slice(format!("ERR:{e:?}").as_bytes());
                    cs.paged_resp = resp.clone();
                    resp
                }
            }
        };
        // 回执（≤112B：MR[0..]；Send 消息 reply 自动丢弃）。数据区从
        // MR[0] 开始（QEMU 实弹语义）；length = **words 数**（与
        // hello/svc_entry 全仓一致；内核拷 length words）。客户端按
        // words 组装并按协议头（分页）截断。
        for (i, chunk) in response.chunks(8).enumerate() {
            if i >= 14 {
                break;
            }
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            let _ = set_mr(i as u64, u64::from_le_bytes(word));
        }
        match message_info_new(0, 0, 0, response.len().div_ceil(8) as u64) {
            Ok(mi) => reply_info = mi,
            Err(_) => loop {
                core::hint::spin_loop();
            },
        }
    }
}

/// host 构建 no-op（与 svc-drv-* 同模式——使 host 编译/测试可达）。
#[cfg(not(target_os = "none"))]
pub fn control_service_main(
    _ep: u64,
    _name: &'static str,
    _ready_ntfn: u64,
    _now_ms: fn() -> u64,
    _epoch: u64,
    _heartbeat_slot: u64,
    _handler: &mut dyn ControlServiceHandler,
) -> ! {
    // host 构建不运行 target 服务循环；发散桩（禁 core::panic 宏族，clippy.toml）。
    loop {
        core::hint::spin_loop()
    }
}
