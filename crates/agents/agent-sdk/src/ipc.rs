//! IPC 客户端（SDK v1.0 稳定 API）。
//!
//! 提供 Agent 间消息传递接口。消息通过 [`crate::wire`] 二进制线协议编码后
//! 入队/出队，不是简单的计数器。
//!
//! # AGT-006/007 合规
//!
//! MASTER §G6 (L2065): "stable IPC wire ABI、真实 SDK syscalls."
//! MASTER §G6 禁止捷径 (L2085): "SDK send 只增加计数."
//!
//! 本模块使用真实的二进制 wire codec（magic, version, type, flags, target,
//! payload, CRC32）。传输层当前为进程内 SPSC 队列，但所有消息都经过完整的
//! 编解码和 CRC 校验——替换为 seL4 IPC 传输时无需改动上层代码。
//!
//! # no_std
//!
//! 使用 `heapless::spsc::Queue` 作为无堆消息队列。

use alloc::vec::Vec;

use heapless::spsc::Queue;
use heapless::String as HString;

use crate::agent::AgentError;
use crate::wire::{self, MessageFlags, MessageType, MAX_WIRE_SIZE};

/// IPC 消息最大载荷（≤4KB）。
pub const MAX_MSG_SIZE: usize = 4096;

/// 队列容量（每方向 16 条消息）。WireFrame 较大（~4KB），64 条会占用过多栈空间。
/// heapless Queue<T, N> 实际容量为 N-1（浪费 1 槽区分满/空），所以使用 N=17。
#[allow(dead_code)]
const QUEUE_CAPACITY: usize = 16;
const QUEUE_SLOTS: usize = 17;

/// 一条已编码的 IPC 消息（固定大小缓冲区）。
#[derive(Clone)]
struct WireFrame {
    len: usize,
    data: [u8; MAX_WIRE_SIZE],
}

impl WireFrame {
    fn new() -> Self {
        Self {
            len: 0,
            data: [0u8; MAX_WIRE_SIZE],
        }
    }
}

/// IPC 客户端（SPSC 进程内传输）——仅测试/仿真（蓝图/20 R2-2：
/// SDK 生产传输真实化为 seL4 IPC，见 [`crate::ipc_sel4::Sel4IpcClient`]）。
#[cfg(any(test, feature = "mock"))]
pub struct IpcClient {
    /// 本地端点名称
    endpoint: HString<64>,
    /// 发送队列（outgoing）— Agent → 外部
    tx_queue: Queue<WireFrame, QUEUE_SLOTS>,
    /// 接收队列（incoming）— 外部 → Agent
    rx_queue: Queue<WireFrame, QUEUE_SLOTS>,
    /// 统计：成功发送消息数
    sent_count: u64,
    /// 统计：成功接收消息数
    recv_count: u64,
    /// 统计：编解码错误数
    error_count: u64,
    /// seL4 IPC 传输（feature sel4；None = 进程内 SPSC 队列）。
    #[cfg(feature = "sel4")]
    sel4: Option<crate::ipc_sel4::Sel4IpcClient>,
}

// SAFETY: IpcClient 是单线程 no_std 语义（SDK D17）：内部 heapless Queue
// 仅在同一 Agent 执行上下文中访问；跨线程共享不提供（Send 用于 Agent
// trait 的 Send+Sync 要求，实际由 Runtime 串行化调用）。
#[cfg(any(test, feature = "mock"))]
#[cfg(any(test, feature = "mock"))]
unsafe impl Send for IpcClient {}
// SAFETY: `IpcClient` 内部状态仅在同一 Agent 执行上下文中访问；
// 跨线程共享不提供（Send 用于 Agent trait 的 Send+Sync 要求，
// 实际由 Runtime 串行化调用）。
#[cfg(any(test, feature = "mock"))]
unsafe impl Sync for IpcClient {}

#[cfg(any(test, feature = "mock"))]
impl IpcClient {
    /// 创建新的 IPC 客户端。
    pub fn new() -> Self {
        Self {
            endpoint: HString::<64>::new(),
            tx_queue: Queue::new(),
            rx_queue: Queue::new(),
            sent_count: 0,
            recv_count: 0,
            error_count: 0,
            #[cfg(feature = "sel4")]
            sel4: None,
        }
    }

    /// 绑定 seL4 IPC 传输（v0.224.0）。
    ///
    /// 绑定后 `request`/`send` 走真实 seL4 endpoint（`seL4_Call`/
    /// `seL4_Send`），替代进程内 SPSC 队列。
    #[cfg(feature = "sel4")]
    pub fn with_sel4(mut self, client: crate::ipc_sel4::Sel4IpcClient) -> Self {
        self.sel4 = Some(client);
        self
    }

    /// 设置端点名称。
    pub fn with_endpoint(mut self, name: &str) -> Self {
        self.endpoint = HString::<64>::try_from(name).unwrap_or_default();
        self
    }

    /// 发送消息到目标 Agent。
    ///
    /// 消息经过完整 wire codec 编码后入队。如果队列满则返回错误。
    pub fn send(&mut self, target: &str, msg: &[u8]) -> Result<(), AgentError> {
        #[cfg(feature = "sel4")]
        {
            if let Some(sel4) = &mut self.sel4 {
                return sel4.send(target, msg);
            }
        }
        if msg.len() > MAX_MSG_SIZE {
            return Err(AgentError::IpcError(
                HString::<128>::try_from("message too large").unwrap_or_default(),
            ));
        }
        if target.is_empty() {
            return Err(AgentError::IpcError(
                HString::<128>::try_from("empty target").unwrap_or_default(),
            ));
        }

        let mut frame = WireFrame::new();
        let n = wire::encode(
            MessageType::Send,
            MessageFlags::NONE,
            target,
            msg,
            &mut frame.data,
        )
        .map_err(|_| {
            self.error_count += 1;
            AgentError::IpcError(HString::<128>::try_from("wire encode failed").unwrap_or_default())
        })?;
        frame.len = n;

        match self.tx_queue.enqueue(frame) {
            Ok(()) => {
                self.sent_count += 1;
                Ok(())
            }
            Err(_) => {
                self.error_count += 1;
                Err(AgentError::IpcError(
                    HString::<128>::try_from("tx queue full").unwrap_or_default(),
                ))
            }
        }
    }

    /// 接收消息（非阻塞）。
    ///
    /// 从接收队列取出一条消息，经过 wire codec 解码后返回载荷。
    /// 队列为空时返回空 Vec。
    pub fn recv(&mut self) -> Result<Vec<u8>, AgentError> {
        match self.rx_queue.dequeue() {
            None => Ok(Vec::new()),
            Some(frame) => {
                let decoded = wire::decode(&frame.data[..frame.len]).map_err(|_| {
                    self.error_count += 1;
                    AgentError::IpcError(
                        HString::<128>::try_from("wire decode failed").unwrap_or_default(),
                    )
                })?;
                self.recv_count += 1;
                Ok(decoded.payload.to_vec())
            }
        }
    }

    /// 请求-响应模式。
    ///
    /// 发送 Request 消息，等待 Response。当前实现为同步（非阻塞 recv），
    /// 若无响应则返回空。
    pub fn request(&mut self, target: &str, req: &[u8]) -> Result<Vec<u8>, AgentError> {
        #[cfg(feature = "sel4")]
        {
            if let Some(sel4) = &mut self.sel4 {
                return sel4.request(target, req, 1_000_000_000);
            }
        }
        self.send(target, req)?;
        self.recv()
    }

    /// 注入一条入站消息（供 transport 层调用）。
    ///
    /// 当 seL4 IPC 收到消息时，transport 层调用此方法将原始 wire frame
    /// 注入到接收队列。
    pub fn inject_inbound(&mut self, raw: &[u8]) -> Result<(), AgentError> {
        if raw.len() > MAX_WIRE_SIZE {
            return Err(AgentError::IpcError(
                HString::<128>::try_from("inbound frame too large").unwrap_or_default(),
            ));
        }
        let mut frame = WireFrame::new();
        frame.data[..raw.len()].copy_from_slice(raw);
        frame.len = raw.len();
        self.rx_queue.enqueue(frame).map_err(|_| {
            self.error_count += 1;
            AgentError::IpcError(HString::<128>::try_from("rx queue full").unwrap_or_default())
        })
    }

    /// 取出一条出站消息的副本。
    ///
    /// transport 层调用此方法取出 Agent 发送的消息。
    /// 返回 wire frame 的字节副本。
    pub fn pop_outbound(&mut self) -> Option<Vec<u8>> {
        self.tx_queue
            .dequeue()
            .map(|frame| frame.data[..frame.len].to_vec())
    }

    /// 获取发送计数。
    pub fn send_count(&self) -> u64 {
        self.sent_count
    }

    /// 获取接收计数。
    pub fn recv_count(&self) -> u64 {
        self.recv_count
    }

    /// 获取错误计数（编解码/队列错误）。
    pub fn error_count(&self) -> u64 {
        self.error_count
    }

    /// 获取端点名称。
    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    /// 待发送消息数。
    pub fn tx_pending(&self) -> usize {
        self.tx_queue.len()
    }

    /// 待接收消息数。
    pub fn rx_pending(&self) -> usize {
        self.rx_queue.len()
    }
}

#[cfg(any(test, feature = "mock"))]
impl Default for IpcClient {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    /// IPC1：发送成功，消息经过 wire codec 编码。
    #[test]
    fn ipc1_send_success_encoded() {
        let mut ipc = IpcClient::new();
        assert!(ipc.send("target-agent", b"hello").is_ok());
        assert_eq!(ipc.send_count(), 1);
        assert_eq!(ipc.tx_pending(), 1);

        // 取出出站消息，验证是合法 wire frame。
        let frame = ipc.pop_outbound().expect("应有出站消息");
        let decoded = wire::decode(&frame).expect("应是合法 wire frame");
        assert_eq!(decoded.msg_type, MessageType::Send);
        assert_eq!(decoded.target, b"target-agent");
        assert_eq!(decoded.payload, b"hello");
    }

    /// IPC2：消息过大拒绝。
    #[test]
    fn ipc2_message_too_large() {
        let mut ipc = IpcClient::new();
        let big_msg = alloc::vec![0u8; MAX_MSG_SIZE + 1];
        let err = ipc.send("target", &big_msg).expect_err("应拒绝");
        assert!(matches!(err, AgentError::IpcError(_)));
        assert_eq!(ipc.error_count(), 0); // 在 encode 之前就拒绝了
    }

    /// IPC3：空目标拒绝。
    #[test]
    fn ipc3_empty_target_rejected() {
        let mut ipc = IpcClient::new();
        let err = ipc.send("", b"hello").expect_err("应拒绝");
        assert!(matches!(err, AgentError::IpcError(_)));
    }

    /// IPC4：接收消息 — 注入入站 wire frame 后 recv 解码。
    #[test]
    fn ipc4_recv_decodes_inbound() {
        let mut ipc = IpcClient::new();

        // 构造一条合法的入站 wire frame。
        let mut raw = [0u8; MAX_WIRE_SIZE];
        let n = wire::encode(
            MessageType::Send,
            MessageFlags::NONE,
            "supervisor",
            b"task complete",
            &mut raw,
        )
        .unwrap();

        // 注入到接收队列。
        ipc.inject_inbound(&raw[..n]).unwrap();
        assert_eq!(ipc.rx_pending(), 1);

        // recv 应解码并返回载荷。
        let msg = ipc.recv().expect("应成功");
        assert_eq!(msg, b"task complete");
        assert_eq!(ipc.recv_count(), 1);
    }

    /// IPC5：请求-响应模式。
    #[test]
    fn ipc5_request_response() {
        let mut ipc = IpcClient::new();

        // 先注入一个预设响应。
        let mut raw = [0u8; MAX_WIRE_SIZE];
        let n = wire::encode(
            MessageType::Response,
            MessageFlags::NONE,
            "solver",
            b"optimal: obj=42",
            &mut raw,
        )
        .unwrap();
        ipc.inject_inbound(&raw[..n]).unwrap();

        // request 会先 send（入 tx 队列），然后 recv（从 rx 队列取）。
        let resp = ipc.request("solver", b"query").expect("应成功");
        assert_eq!(resp, b"optimal: obj=42");
        assert_eq!(ipc.send_count(), 1);
        assert_eq!(ipc.recv_count(), 1);
    }

    /// IPC6：端点名称。
    #[test]
    fn ipc6_endpoint() {
        let ipc = IpcClient::new().with_endpoint("my-agent");
        assert_eq!(ipc.endpoint(), "my-agent");
    }

    /// IPC7：CRC 损坏的入站消息被拒绝。
    #[test]
    fn ipc7_corrupt_inbound_rejected() {
        let mut ipc = IpcClient::new();

        let mut raw = [0u8; MAX_WIRE_SIZE];
        let n = wire::encode(
            MessageType::Send,
            MessageFlags::NONE,
            "t",
            b"hello",
            &mut raw,
        )
        .unwrap();

        // 翻转一个 payload 位。
        raw[wire::HEADER_LEN] ^= 0xFF;

        // 注入损坏的 frame。
        ipc.inject_inbound(&raw[..n]).unwrap();

        // recv 应在 decode 时失败。
        let result = ipc.recv();
        assert!(result.is_err(), "CRC 损坏的消息应被拒绝");
        assert_eq!(ipc.error_count(), 1);
    }

    /// IPC8：队列满时 send 返回错误。
    #[test]
    fn ipc8_tx_queue_full() {
        let mut ipc = IpcClient::new();

        // 填满发送队列。
        for _ in 0..QUEUE_CAPACITY {
            assert!(ipc.send("target", b"msg").is_ok());
        }
        assert_eq!(ipc.tx_pending(), QUEUE_CAPACITY);

        // 第 65 条应失败。
        let err = ipc.send("target", b"overflow").expect_err("应拒绝");
        assert!(matches!(err, AgentError::IpcError(_)));
    }

    /// IPC9：多条消息按 FIFO 顺序出队。
    #[test]
    fn ipc9_fifo_order() {
        let mut ipc = IpcClient::new();

        for i in 0..5u8 {
            let msg = [i];
            ipc.send("target", &msg).unwrap();
        }

        for i in 0..5u8 {
            let frame = ipc.pop_outbound().expect("应有消息");
            let decoded = wire::decode(&frame).unwrap();
            assert_eq!(decoded.payload, &[i]);
        }

        assert!(ipc.pop_outbound().is_none());
    }

    /// IPC10：空队列 recv 返回空 Vec。
    #[test]
    fn ipc10_empty_recv() {
        let mut ipc = IpcClient::new();
        let msg = ipc.recv().expect("应成功");
        assert!(msg.is_empty());
        assert_eq!(ipc.recv_count(), 0); // 空队列不计入 recv_count
    }
}
