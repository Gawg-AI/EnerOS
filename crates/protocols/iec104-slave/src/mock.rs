//! 测试用 Mock IEC 104 从站传输层.
//!
//! - `accept()` 从 `accepted_conns` 队列弹出一个预置连接
//! - `send()` 记录到 `tx_frames`
//! - `recv()` 从 `rx_data` 队列弹出预置数据；无数据返回 `Ok(0)`
//! - `now_ms()` 返回可配置的虚拟时钟

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::error::Iec104Error;
use crate::transport::{ConnId, SlaveTransport};

/// Mock IEC 104 从站传输层
pub struct MockSlaveTransport {
    accepted_conns: VecDeque<ConnId>,
    rx_data: BTreeMap<ConnId, VecDeque<Vec<u8>>>,
    tx_frames: Vec<(ConnId, Vec<u8>)>,
    now_ms_value: u64,
    closed_conns: Vec<ConnId>,
    next_conn_id: ConnId,
}

impl MockSlaveTransport {
    /// 创建空 mock。
    pub fn new() -> Self {
        Self {
            accepted_conns: VecDeque::new(),
            rx_data: BTreeMap::new(),
            tx_frames: Vec::new(),
            now_ms_value: 0,
            closed_conns: Vec::new(),
            next_conn_id: 1,
        }
    }

    /// 预置一个待接受的新连接，返回连接 ID。
    pub fn accept_conn(&mut self) -> ConnId {
        let id = self.next_conn_id;
        self.next_conn_id += 1;
        self.accepted_conns.push_back(id);
        self.rx_data.entry(id).or_default();
        id
    }

    /// 向指定连接的接收队列推入数据（模拟主站发送的帧）。
    pub fn push_rx_data(&mut self, conn: ConnId, data: Vec<u8>) {
        self.rx_data.entry(conn).or_default().push_back(data);
    }

    /// 返回所有已发送帧 `(ConnId, Vec<u8>)`。
    pub fn tx_frames(&self) -> &[(ConnId, Vec<u8>)] {
        &self.tx_frames
    }

    /// 返回指定连接的已发送帧引用。
    pub fn tx_frames_for(&self, conn: ConnId) -> Vec<&[u8]> {
        self.tx_frames
            .iter()
            .filter(|(c, _)| *c == conn)
            .map(|(_, d)| d.as_slice())
            .collect()
    }

    /// 推进虚拟时钟。
    pub fn advance_now_ms(&mut self, delta: u64) {
        self.now_ms_value += delta;
    }

    /// 设置虚拟时钟。
    pub fn set_now_ms(&mut self, ms: u64) {
        self.now_ms_value = ms;
    }

    /// 返回已关闭的连接列表。
    pub fn closed_conns(&self) -> &[ConnId] {
        &self.closed_conns
    }

    /// 清空所有状态。
    pub fn clear(&mut self) {
        self.accepted_conns.clear();
        self.rx_data.clear();
        self.tx_frames.clear();
        self.closed_conns.clear();
        self.now_ms_value = 0;
        self.next_conn_id = 1;
    }
}

impl Default for MockSlaveTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl SlaveTransport for MockSlaveTransport {
    fn accept(&mut self) -> Option<ConnId> {
        self.accepted_conns.pop_front()
    }

    fn send(&mut self, conn: ConnId, data: &[u8]) -> Result<(), Iec104Error> {
        self.tx_frames.push((conn, Vec::from(data)));
        Ok(())
    }

    fn recv(&mut self, conn: ConnId, buf: &mut [u8]) -> Result<usize, Iec104Error> {
        if let Some(queue) = self.rx_data.get_mut(&conn) {
            if let Some(data) = queue.pop_front() {
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                return Ok(n);
            }
        }
        Ok(0)
    }

    fn close(&mut self, conn: ConnId) {
        self.closed_conns.push(conn);
    }

    fn now_ms(&self) -> u64 {
        self.now_ms_value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_empty() {
        let mock = MockSlaveTransport::new();
        assert_eq!(mock.tx_frames().len(), 0);
        assert_eq!(mock.closed_conns().len(), 0);
    }

    #[test]
    fn test_accept_conn_and_accept() {
        let mut mock = MockSlaveTransport::new();
        let id = mock.accept_conn();
        assert_eq!(id, 1);
        assert_eq!(mock.accept(), Some(1));
        assert_eq!(mock.accept(), None);
    }

    #[test]
    fn test_push_rx_and_recv() {
        let mut mock = MockSlaveTransport::new();
        let id = mock.accept_conn();
        mock.push_rx_data(id, vec![0x68, 0x04, 0x07, 0x00, 0x00, 0x00]);
        let mut buf = [0u8; 256];
        let n = mock.recv(id, &mut buf).expect("recv ok");
        assert_eq!(n, 6);
        assert_eq!(buf[0], 0x68);
    }

    #[test]
    fn test_recv_empty_returns_zero() {
        let mut mock = MockSlaveTransport::new();
        let id = mock.accept_conn();
        let mut buf = [0u8; 256];
        let n = mock.recv(id, &mut buf).expect("recv ok");
        assert_eq!(n, 0);
    }

    #[test]
    fn test_send_records_tx_frames() {
        let mut mock = MockSlaveTransport::new();
        mock.send(1, &[0x68, 0x04]).expect("send ok");
        mock.send(2, &[0x68, 0x06]).expect("send ok");
        assert_eq!(mock.tx_frames().len(), 2);
        assert_eq!(mock.tx_frames()[0].0, 1);
        assert_eq!(mock.tx_frames()[1].0, 2);
    }

    #[test]
    fn test_tx_frames_for() {
        let mut mock = MockSlaveTransport::new();
        mock.send(1, &[0x01]).expect("send ok");
        mock.send(2, &[0x02]).expect("send ok");
        mock.send(1, &[0x03]).expect("send ok");
        let frames = mock.tx_frames_for(1);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], &[0x01]);
        assert_eq!(frames[1], &[0x03]);
    }

    #[test]
    fn test_now_ms() {
        let mut mock = MockSlaveTransport::new();
        assert_eq!(mock.now_ms(), 0);
        mock.set_now_ms(1000);
        assert_eq!(mock.now_ms(), 1000);
        mock.advance_now_ms(500);
        assert_eq!(mock.now_ms(), 1500);
    }

    #[test]
    fn test_close() {
        let mut mock = MockSlaveTransport::new();
        mock.close(1);
        mock.close(2);
        assert_eq!(mock.closed_conns().len(), 2);
    }

    #[test]
    fn test_clear() {
        let mut mock = MockSlaveTransport::new();
        mock.accept_conn();
        mock.send(1, &[0x01]).expect("send ok");
        mock.close(1);
        mock.set_now_ms(100);
        mock.clear();
        assert_eq!(mock.tx_frames().len(), 0);
        assert_eq!(mock.closed_conns().len(), 0);
        assert_eq!(mock.now_ms(), 0);
        assert_eq!(mock.accept(), None);
    }

    #[test]
    fn test_default() {
        let mock = MockSlaveTransport::default();
        assert_eq!(mock.tx_frames().len(), 0);
    }

    #[test]
    fn test_multiple_conns_distinct_ids() {
        let mut mock = MockSlaveTransport::new();
        let id1 = mock.accept_conn();
        let id2 = mock.accept_conn();
        assert_ne!(id1, id2);
    }
}
