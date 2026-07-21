//! 测试用 Mock IEC 104 主站传输层.
//!
//! - `connect()` 返回递增的 `ConnId`（从 1 开始）
//! - `send()` 记录到 `tx_frames`
//! - `recv()` 从 `rx_data` 队列弹出一帧；无数据返回 `Ok(None)`
//! - `now_ms()` 返回可配置的虚拟时钟

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::error::MasterError;
use crate::transport::{ConnId, MasterTransport};

/// Mock IEC 104 主站传输层
pub struct MockMasterTransport {
    next_conn_id: ConnId,
    connections: BTreeMap<ConnId, ([u8; 4], u16)>,
    rx_data: BTreeMap<ConnId, VecDeque<Vec<u8>>>,
    tx_frames: Vec<(ConnId, Vec<u8>)>,
    current_time_ms: u64,
}

impl MockMasterTransport {
    /// 创建空 mock。
    pub fn new() -> Self {
        Self {
            next_conn_id: 1,
            connections: BTreeMap::new(),
            rx_data: BTreeMap::new(),
            tx_frames: Vec::new(),
            current_time_ms: 0,
        }
    }

    /// 向指定连接的接收队列预置数据（模拟从站发送的帧）。
    pub fn push_rx(&mut self, conn: ConnId, data: Vec<u8>) {
        self.rx_data.entry(conn).or_default().push_back(data);
    }

    /// 返回所有已发送帧 `(ConnId, Vec<u8>)`。
    pub fn tx_frames(&self) -> &[(ConnId, Vec<u8>)] {
        &self.tx_frames
    }

    /// 推进虚拟时钟。
    pub fn advance_time(&mut self, ms: u64) {
        self.current_time_ms += ms;
    }

    /// 设置虚拟时钟。
    pub fn set_time(&mut self, ms: u64) {
        self.current_time_ms = ms;
    }

    /// 返回当前虚拟时钟值。
    pub fn current_time(&self) -> u64 {
        self.current_time_ms
    }

    /// 返回活跃连接数。
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// 返回下一个将被 `connect()` 分配的 ConnId。
    pub fn next_conn_id(&self) -> ConnId {
        self.next_conn_id
    }
}

impl Default for MockMasterTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MasterTransport for MockMasterTransport {
    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<ConnId, MasterError> {
        let id = self.next_conn_id;
        self.next_conn_id += 1;
        self.connections.insert(id, (ip, port));
        self.rx_data.entry(id).or_default();
        Ok(id)
    }

    fn send(&mut self, conn: ConnId, data: &[u8]) -> Result<(), MasterError> {
        self.tx_frames.push((conn, Vec::from(data)));
        Ok(())
    }

    fn recv(&mut self, conn: ConnId) -> Result<Option<Vec<u8>>, MasterError> {
        if let Some(queue) = self.rx_data.get_mut(&conn) {
            if let Some(data) = queue.pop_front() {
                return Ok(Some(data));
            }
        }
        Ok(None)
    }

    fn close(&mut self, conn: ConnId) -> Result<(), MasterError> {
        self.connections.remove(&conn);
        self.rx_data.remove(&conn);
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        self.current_time_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_empty() {
        let mock = MockMasterTransport::new();
        assert_eq!(mock.tx_frames().len(), 0);
        assert_eq!(mock.connection_count(), 0);
        assert_eq!(mock.current_time(), 0);
        assert_eq!(mock.next_conn_id(), 1);
    }

    #[test]
    fn test_connect_returns_incrementing_ids() {
        let mut mock = MockMasterTransport::new();
        let id1 = mock.connect([192, 168, 1, 1], 2404).expect("connect 1");
        let id2 = mock.connect([192, 168, 1, 2], 2404).expect("connect 2");
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(mock.connection_count(), 2);
    }

    #[test]
    fn test_send_records_tx_frames() {
        let mut mock = MockMasterTransport::new();
        mock.send(1, &[0x68, 0x04]).expect("send 1");
        mock.send(2, &[0x68, 0x06]).expect("send 2");
        assert_eq!(mock.tx_frames().len(), 2);
        assert_eq!(mock.tx_frames()[0].0, 1);
        assert_eq!(mock.tx_frames()[1].0, 2);
    }

    #[test]
    fn test_push_rx_and_recv() {
        let mut mock = MockMasterTransport::new();
        let id = mock.connect([10, 0, 0, 1], 2404).expect("connect");
        mock.push_rx(id, vec![0x68, 0x04, 0x07, 0x00, 0x00, 0x00]);
        let data = mock.recv(id).expect("recv ok");
        assert!(data.is_some());
        assert_eq!(data.unwrap()[0], 0x68);
    }

    #[test]
    fn test_recv_empty_returns_none() {
        let mut mock = MockMasterTransport::new();
        let id = mock.connect([10, 0, 0, 1], 2404).expect("connect");
        let data = mock.recv(id).expect("recv ok");
        assert!(data.is_none());
    }

    #[test]
    fn test_recv_multiple_frames() {
        let mut mock = MockMasterTransport::new();
        let id = mock.connect([10, 0, 0, 1], 2404).expect("connect");
        mock.push_rx(id, vec![0x01]);
        mock.push_rx(id, vec![0x02]);
        mock.push_rx(id, vec![0x03]);
        assert_eq!(mock.recv(id).unwrap().unwrap(), vec![0x01]);
        assert_eq!(mock.recv(id).unwrap().unwrap(), vec![0x02]);
        assert_eq!(mock.recv(id).unwrap().unwrap(), vec![0x03]);
        assert!(mock.recv(id).unwrap().is_none());
    }

    #[test]
    fn test_close_removes_connection() {
        let mut mock = MockMasterTransport::new();
        let id = mock.connect([10, 0, 0, 1], 2404).expect("connect");
        assert_eq!(mock.connection_count(), 1);
        mock.close(id).expect("close ok");
        assert_eq!(mock.connection_count(), 0);
    }

    #[test]
    fn test_advance_time() {
        let mut mock = MockMasterTransport::new();
        assert_eq!(mock.now_ms(), 0);
        mock.advance_time(1000);
        assert_eq!(mock.now_ms(), 1000);
        mock.advance_time(500);
        assert_eq!(mock.now_ms(), 1500);
    }

    #[test]
    fn test_set_time() {
        let mut mock = MockMasterTransport::new();
        mock.set_time(5000);
        assert_eq!(mock.now_ms(), 5000);
    }

    #[test]
    fn test_default() {
        let mock = MockMasterTransport::default();
        assert_eq!(mock.tx_frames().len(), 0);
    }
}
