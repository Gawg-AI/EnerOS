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
}

impl MockTcpTransport {
    /// 创建空 mock
    pub fn new() -> Self {
        Self {
            rx_queue: VecDeque::new(),
            sent_frames: Vec::new(),
            connect_calls: Vec::new(),
            recv_timeout: false,
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

    /// 清空所有状态
    pub fn clear(&mut self) {
        self.rx_queue.clear();
        self.sent_frames.clear();
        self.connect_calls.clear();
        self.recv_timeout = false;
    }
}

impl Default for MockTcpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpTransport for MockTcpTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), ModbusTcpError> {
        self.sent_frames.push(data.to_vec());
        Ok(())
    }

    fn recv(&mut self, _timeout_ms: u32) -> Result<Vec<u8>, ModbusTcpError> {
        if self.recv_timeout {
            return Err(ModbusTcpError::Timeout);
        }
        self.rx_queue.pop_front().ok_or(ModbusTcpError::Timeout)
    }

    fn connect(&mut self, device: &TcpDevice) -> Result<(), ModbusTcpError> {
        self.connect_calls.push(*device);
        Ok(())
    }
}
