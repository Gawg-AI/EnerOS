//! 测试用 Mock RTU 传输层.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use eneros_driver_framework::DriverError;

use crate::master::RtuTransport;

/// Mock RTU 传输层
///
/// - `send()` 将发送的帧存入 `sent_frames`
/// - `recv()` 从 `rx_queue` 弹出一帧；若 `recv_timeout=true` 或队列空则返回 `Timeout`
pub struct MockRtuTransport {
    rx_queue: VecDeque<Vec<u8>>,
    sent_frames: Vec<Vec<u8>>,
    recv_timeout: bool,
}

impl MockRtuTransport {
    /// 创建空 mock
    pub fn new() -> Self {
        Self {
            rx_queue: VecDeque::new(),
            sent_frames: Vec::new(),
            recv_timeout: false,
        }
    }

    /// 预置一帧响应数据
    pub fn push_response(&mut self, frame: Vec<u8>) {
        self.rx_queue.push_back(frame);
    }

    /// 返回已发送帧的引用
    pub fn sent_frames(&self) -> &[Vec<u8>] {
        &self.sent_frames
    }

    /// 设置 `recv()` 是否总是返回超时
    pub fn set_recv_timeout(&mut self, timeout: bool) {
        self.recv_timeout = timeout;
    }

    /// 清空所有状态
    pub fn clear(&mut self) {
        self.rx_queue.clear();
        self.sent_frames.clear();
        self.recv_timeout = false;
    }
}

impl Default for MockRtuTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl RtuTransport for MockRtuTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), DriverError> {
        self.sent_frames.push(data.to_vec());
        Ok(())
    }

    fn recv(&mut self, _timeout_ms: u32) -> Result<Vec<u8>, DriverError> {
        if self.recv_timeout {
            return Err(DriverError::Timeout);
        }
        self.rx_queue.pop_front().ok_or(DriverError::Timeout)
    }
}
