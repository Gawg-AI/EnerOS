//! 测试用 Mock TCP 传输层.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::device::TcpDevice;
use crate::error::ModbusTcpError;
use crate::transport::TcpTransport;

/// Mock TCP 传输层
///
/// - `connect()` 将设备追加到 `connect_calls` 并返回 Ok
/// - `send()` 将发送的帧追加到 `sent_frames`
/// - `recv()` 从 `rx_queue` 弹出一帧；若 `recv_timeout=true` 或队列空则返回 `Timeout`
pub struct MockTcpTransport {
    rx_queue: VecDeque<Vec<u8>>,
    sent_frames: Vec<Vec<u8>>,
    connect_calls: Vec<TcpDevice>,
    recv_timeout: bool,
    /// 39 §6.3 负例：send 失败注入（backpressure 测试——窗口/链路边界）。
    send_fail: Option<ModbusTcpError>,
    /// PTC-002：当前连接的设备（连接复用判断——同设备复用）.
    connected_device: Option<TcpDevice>,
    /// 调试：recv 历史（每次弹出的帧）.
    pub recv_log: Vec<Vec<u8>>,
}

impl MockTcpTransport {
    /// 创建空 mock
    pub fn new() -> Self {
        Self {
            rx_queue: VecDeque::new(),
            sent_frames: Vec::new(),
            connect_calls: Vec::new(),
            recv_timeout: false,
            send_fail: None,
            connected_device: None,
            recv_log: Vec::new(),
        }
    }

    /// 预置一帧响应数据
    pub fn push_response(&mut self, resp: Vec<u8>) {
        self.rx_queue.push_back(resp);
    }

    /// 返回已发送帧的引用
    pub fn sent_frames(&self) -> &[Vec<u8>] {
        &self.sent_frames
    }

    /// 返回已建立连接的设备调用记录
    pub fn connect_calls(&self) -> &[TcpDevice] {
        &self.connect_calls
    }

    /// 设置 `recv()` 是否总是返回超时
    pub fn set_recv_timeout(&mut self, timeout: bool) {
        self.recv_timeout = timeout;
    }

    /// 39 §6.3 负例：设置 `send()` 失败注入的错误（None = 恢复正常）。
    pub fn set_send_fail(&mut self, err: Option<ModbusTcpError>) {
        self.send_fail = err;
    }

    /// 清空所有状态
    pub fn clear(&mut self) {
        self.rx_queue.clear();
        self.sent_frames.clear();
        self.connect_calls.clear();
        self.recv_timeout = false;
        self.send_fail = None;
    }
}

impl Default for MockTcpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpTransport for MockTcpTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), ModbusTcpError> {
        if let Some(e) = &self.send_fail {
            return Err(e.clone());
        }
        self.sent_frames.push(data.to_vec());
        Ok(())
    }

    fn recv(&mut self, _timeout_ms: u32) -> Result<Vec<u8>, ModbusTcpError> {
        if self.recv_timeout {
            return Err(ModbusTcpError::Timeout);
        }
        match self.rx_queue.pop_front() {
            Some(f) => {
                self.recv_log.push(f.clone());
                Ok(f)
            }
            None => Err(ModbusTcpError::Timeout),
        }
    }

    fn connect(&mut self, device: &TcpDevice) -> Result<(), ModbusTcpError> {
        self.connect_calls.push(*device);
        self.connected_device = Some(*device);
        Ok(())
    }

    fn is_connected(&self, device: &TcpDevice) -> bool {
        self.connected_device == Some(*device)
    }
}
