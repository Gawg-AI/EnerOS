//! 蓝图 40：EL0 控制服务通用 IPC 主循环（WP-40.9 服务循环模式）。
//!
//! 复用 agent-sdk 分块传输协议（`ChunkedReceiver`）接收大载荷
//! （ControlEnvelope 270B 超 seL4 MR 120B 预算）：header(request)→ack、
//! data(send)×n、final(request)→处理回执。任何协议违例 → 明确错误响应，
//! 服务端半组装缓冲丢弃（fail-closed）。
//!
//! host 构建：`main` no-op（与 svc-drv-* 同模式——target-only 代码由
//! 协议层 host 测试覆盖，见 agent-sdk `chunked`、本 crate `service_slots`
//! 与各服务逻辑模块测试）。

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

    use crate::service_slots::SlotTable;
    let _ = epoch;
    // readiness 信号（root 侧收集 PRODUCT-READY）。
    let _ = ipc_signal(ready_ntfn);
    // 46.5 起：分块状态机按发送方 badge 独立维护（多客户端交错）。68.1
    // WP-68.1.2：槽位表提取为 [`crate::service_slots`] 单源——槽满拒绝
    //（CHUNKED-SLOT-FULL）/ 同 badge 双会话拒绝（CHUNKED-DUP-SESSION）/
    // 分页覆盖可观测（CHUNKED-PAGE-EVICTED）/ 慢调用观测（CHUNKED-SLOW），
    // 词形行经串口落日志（每词形每分钟至多 1 行）。
    let mut slots = SlotTable::new();
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
        // 心跳：处理任何一条消息前先 signal（消息驱动的进展证明——
        // supervisor 以"有请求在途却无心跳"判定卡死）。
        if heartbeat_slot != 0 {
            let _ = ipc_signal(heartbeat_slot);
        }
        // 分发（槽位表全权：分块状态机 / Final 处理 / 分页拉取；词形行
        // 落串口）。
        let mut nowfn = now_ms;
        let out = slots.step(sender, &payload[..len], handler, &mut nowfn);
        for line in &out.lines {
            eneros_runtime::println!("[{name}] {line}");
        }
        let response = out.response;
        // 回执（≤112B：MR[0..]；Send 消息 reply 自动丢弃）。数据区从
        // MR[0] 开始（QEMU 实弹语义）；length = **words 数**（与
        // hello/svc_entry 全仓一致；内核拷 length words）。客户端按
        // words 组装并按协议头（分页）截断。
        // 69.1（I3 / REG-CHUNKED-MR-WORDS-CAP）：words 上限单源
        // MR_REPLY_MAX_WORDS（原 14 魔数两端手抄）；单帧回执 ≤ 预算由
        // agent-sdk 静态锁 + service_slots step() 出口兜底保证。
        for (i, chunk) in response.chunks(8).enumerate() {
            if i >= eneros_agent_sdk::MR_REPLY_MAX_WORDS {
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
