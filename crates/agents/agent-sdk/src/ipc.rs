//! IPC 客户端（SDK v1.0 稳定 API）。
//!
//! 提供 Agent 间消息传递接口。SDK 层提供接口定义与 Mock 实现，
//! 真实 IPC 由 Agent Runtime 注入。

use alloc::vec::Vec;

use heapless::String as HString;

use crate::agent::AgentError;

/// IPC 消息（≤4KB 载荷）。
pub const MAX_MSG_SIZE: usize = 4096;

/// IPC 客户端。
///
/// Agent 通过此结构与系统中其他 Agent 通信。
pub struct IpcClient {
    /// 本地端点名称
    endpoint: HString<64>,
    /// 发送计数（统计用）
    send_count: u64,
    /// 接收计数（统计用）
    recv_count: u64,
}

impl IpcClient {
    /// 创建新的 IPC 客户端。
    pub fn new() -> Self {
        Self {
            endpoint: HString::<64>::new(),
            send_count: 0,
            recv_count: 0,
        }
    }

    /// 设置端点名称。
    pub fn with_endpoint(mut self, name: &str) -> Self {
        self.endpoint = HString::<64>::try_from(name).unwrap_or_default();
        self
    }

    /// 发送消息到目标 Agent。
    ///
    /// # 参数
    /// - `target`：目标 Agent 名称
    /// - `msg`：消息载荷
    pub fn send(&mut self, target: &str, msg: &[u8]) -> Result<(), AgentError> {
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
        // SDK 层仅做参数校验，真实发送由 Runtime 注入
        self.send_count += 1;
        Ok(())
    }

    /// 接收消息（非阻塞）。
    ///
    /// 无消息时返回空 Vec。
    pub fn recv(&mut self) -> Result<Vec<u8>, AgentError> {
        self.recv_count += 1;
        // SDK 层 Mock：返回空消息
        Ok(Vec::new())
    }

    /// 请求-响应模式。
    ///
    /// 发送请求并等待响应。
    pub fn request(&mut self, target: &str, req: &[u8]) -> Result<Vec<u8>, AgentError> {
        self.send(target, req)?;
        // SDK 层 Mock：返回空响应
        self.recv()
    }

    /// 获取发送计数。
    pub fn send_count(&self) -> u64 {
        self.send_count
    }

    /// 获取接收计数。
    pub fn recv_count(&self) -> u64 {
        self.recv_count
    }

    /// 获取端点名称。
    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }
}

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
    use super::*;

    /// IPC1：发送成功。
    #[test]
    fn ipc1_send_success() {
        let mut ipc = IpcClient::new();
        assert!(ipc.send("target-agent", b"hello").is_ok());
        assert_eq!(ipc.send_count(), 1);
    }

    /// IPC2：消息过大拒绝。
    #[test]
    fn ipc2_message_too_large() {
        let mut ipc = IpcClient::new();
        let big_msg = alloc::vec![0u8; MAX_MSG_SIZE + 1];
        let err = ipc.send("target", &big_msg).expect_err("应拒绝");
        assert!(matches!(err, AgentError::IpcError(_)));
    }

    /// IPC3：空目标拒绝。
    #[test]
    fn ipc3_empty_target_rejected() {
        let mut ipc = IpcClient::new();
        let err = ipc.send("", b"hello").expect_err("应拒绝");
        assert!(matches!(err, AgentError::IpcError(_)));
    }

    /// IPC4：接收消息。
    #[test]
    fn ipc4_recv() {
        let mut ipc = IpcClient::new();
        let msg = ipc.recv().expect("应成功");
        assert!(msg.is_empty()); // Mock 返回空
        assert_eq!(ipc.recv_count(), 1);
    }

    /// IPC5：请求-响应。
    #[test]
    fn ipc5_request_response() {
        let mut ipc = IpcClient::new();
        let resp = ipc.request("target", b"query").expect("应成功");
        assert!(resp.is_empty()); // Mock 返回空
        assert_eq!(ipc.send_count(), 1);
        assert_eq!(ipc.recv_count(), 1);
    }

    /// IPC6：端点名称。
    #[test]
    fn ipc6_endpoint() {
        let ipc = IpcClient::new().with_endpoint("my-agent");
        assert_eq!(ipc.endpoint(), "my-agent");
    }
}
