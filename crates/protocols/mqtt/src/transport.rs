//! MQTT 传输层抽象（D12：trait + 内存 mock，解耦 smoltcp TCP）.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::error::MqttError;

/// MQTT 传输层 trait（D12：解耦 smoltcp TCP，与 v0.46.0/v0.49.0 模式一致）.
///
/// 不要求 `Send + Sync`（D17：no_std 单线程）。
/// 生产环境注入 smoltcp TCP 实现，测试环境使用 [`MockTransport`]。
pub trait MqttTransport {
    /// 连接到远端（host:port）.
    fn connect(&mut self, host: &str, port: u16) -> Result<(), MqttError>;
    /// 发送数据.
    fn send(&mut self, data: &[u8]) -> Result<(), MqttError>;
    /// 接收一个完整报文（以 MQTT 剩余长度为准）.
    ///
    /// 无数据时返回 `Err(NotConnected)` 或空 Vec，调用方需自行处理。
    fn recv(&mut self) -> Result<Vec<u8>, MqttError>;
    /// 关闭连接.
    fn close(&mut self) -> Result<(), MqttError>;
    /// 是否已连接.
    fn is_connected(&self) -> bool;
}

/// 内存 mock 传输（用于测试）.
///
/// - `sent_packets`：记录所有已发送报文（调用方可读取校验）
/// - `recv_queue`：预置的入站报文队列（`recv()` 弹出一个）
/// - `connected`：连接状态（可通过 `set_connected()` 切换）
#[derive(Debug, Default)]
pub struct MockTransport {
    /// 已发送报文记录.
    sent_packets: Vec<Vec<u8>>,
    /// 预置入站报文队列（FIFO）.
    recv_queue: VecDeque<Vec<u8>>,
    /// 连接状态.
    connected: bool,
}

impl MockTransport {
    /// 构造空 mock（默认未连接）.
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置连接状态.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    /// 预置一个入站报文（追加到 recv 队列尾部）.
    pub fn enqueue_recv(&mut self, data: Vec<u8>) {
        self.recv_queue.push_back(data);
    }

    /// 返回已发送报文切片（只读）.
    pub fn sent_packets(&self) -> &[Vec<u8>] {
        &self.sent_packets
    }

    /// 返回已发送报文数.
    pub fn sent_count(&self) -> usize {
        self.sent_packets.len()
    }

    /// 清空已发送记录.
    pub fn clear_sent(&mut self) {
        self.sent_packets.clear();
    }
}

impl MqttTransport for MockTransport {
    fn connect(&mut self, _host: &str, _port: u16) -> Result<(), MqttError> {
        self.connected = true;
        Ok(())
    }

    fn send(&mut self, data: &[u8]) -> Result<(), MqttError> {
        if !self.connected {
            return Err(MqttError::TransportError);
        }
        self.sent_packets.push(Vec::from(data));
        Ok(())
    }

    fn recv(&mut self) -> Result<Vec<u8>, MqttError> {
        if !self.connected {
            return Err(MqttError::NotConnected);
        }
        match self.recv_queue.pop_front() {
            Some(data) => Ok(data),
            None => Err(MqttError::NotConnected),
        }
    }

    fn close(&mut self) -> Result<(), MqttError> {
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_transport_send_recv() {
        let mut t = MockTransport::new();
        // 未连接时 send 失败
        assert!(matches!(t.send(&[1, 2, 3]), Err(MqttError::TransportError)));
        // 连接
        assert!(t.connect("localhost", 1883).is_ok());
        assert!(t.is_connected());
        // 发送
        assert!(t.send(&[1, 2, 3]).is_ok());
        assert_eq!(t.sent_count(), 1);
        assert_eq!(t.sent_packets()[0], vec![1, 2, 3]);
        // 预置入站报文
        t.enqueue_recv(vec![0x20, 0x02, 0x00, 0x00]);
        let r = t.recv().unwrap();
        assert_eq!(r, vec![0x20, 0x02, 0x00, 0x00]);
        // 队列空时 recv 返回 NotConnected
        assert!(matches!(t.recv(), Err(MqttError::NotConnected)));
    }

    #[test]
    fn test_mock_transport_close() {
        let mut t = MockTransport::new();
        t.set_connected(true);
        assert!(t.is_connected());
        assert!(t.close().is_ok());
        assert!(!t.is_connected());
    }
}
