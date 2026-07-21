//! IEC 104 主站连接管理.
//!
//! 每个远端设备对应一个 [`MasterConnection`]，维护发送/接收序列号（15 位回绕）、
//! 各类时间戳（总召唤/时钟同步/活动）、连接状态与待确认计数。

use crate::device::{ConnState, RemoteDevice};
use crate::transport::ConnId;

/// 主站连接状态
///
/// 封装单个远端设备的连接信息与协议状态。
#[derive(Debug, Clone)]
pub struct MasterConnection {
    /// 远端设备描述
    pub remote: RemoteDevice,
    /// 传输层连接标识
    pub conn_id: ConnId,
    /// 发送序列号（15 位，0~32767 回绕）
    pub send_seq: u16,
    /// 接收序列号（15 位，0~32767 回绕）
    pub recv_seq: u16,
    /// 上次总召唤时间戳（毫秒）
    pub last_interrogation_ms: u64,
    /// 上次时钟同步时间戳（毫秒）
    pub last_clock_sync_ms: u64,
    /// 上次活动时间戳（毫秒，收发均更新）
    pub last_activity_ms: u64,
    /// 连接状态
    pub state: ConnState,
    /// 待确认 I 帧数
    pub pending_acks: u16,
}

impl MasterConnection {
    /// 创建连接，初始序列号为 0，状态为 `StartDtPending`。
    pub fn new(remote: RemoteDevice, conn_id: ConnId, now_ms: u64) -> Self {
        Self {
            remote,
            conn_id,
            send_seq: 0,
            recv_seq: 0,
            last_interrogation_ms: 0,
            last_clock_sync_ms: 0,
            last_activity_ms: now_ms,
            state: ConnState::StartDtPending,
            pending_acks: 0,
        }
    }

    /// 取出当前发送序列号并递增（15 位回绕）。
    pub fn next_send_seq(&mut self) -> u16 {
        let s = self.send_seq;
        self.send_seq = (self.send_seq + 1) & 0x7FFF;
        s
    }

    /// 取出当前接收序列号并递增（15 位回绕）。
    pub fn next_recv_seq(&mut self) -> u16 {
        let r = self.recv_seq;
        self.recv_seq = (self.recv_seq + 1) & 0x7FFF;
        r
    }

    /// 更新活动时间戳。
    pub fn touch(&mut self, now_ms: u64) {
        self.last_activity_ms = now_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_conn() -> MasterConnection {
        MasterConnection::new(
            RemoteDevice::new([192, 168, 1, 1], 2404, 1, 30_000),
            1,
            1000,
        )
    }

    #[test]
    fn test_new_initial_state() {
        let conn = make_conn();
        assert_eq!(conn.send_seq, 0);
        assert_eq!(conn.recv_seq, 0);
        assert_eq!(conn.state, ConnState::StartDtPending);
        assert_eq!(conn.last_activity_ms, 1000);
        assert_eq!(conn.pending_acks, 0);
    }

    #[test]
    fn test_next_send_seq_increment() {
        let mut conn = make_conn();
        assert_eq!(conn.next_send_seq(), 0);
        assert_eq!(conn.next_send_seq(), 1);
        assert_eq!(conn.next_send_seq(), 2);
        assert_eq!(conn.send_seq, 3);
    }

    #[test]
    fn test_next_recv_seq_increment() {
        let mut conn = make_conn();
        assert_eq!(conn.next_recv_seq(), 0);
        assert_eq!(conn.next_recv_seq(), 1);
        assert_eq!(conn.recv_seq, 2);
    }

    #[test]
    fn test_send_seq_wraparound() {
        let mut conn = make_conn();
        conn.send_seq = 0x7FFF;
        assert_eq!(conn.next_send_seq(), 0x7FFF);
        // 回绕到 0
        assert_eq!(conn.next_send_seq(), 0);
        assert_eq!(conn.send_seq, 1);
    }

    #[test]
    fn test_recv_seq_wraparound() {
        let mut conn = make_conn();
        conn.recv_seq = 0x7FFF;
        assert_eq!(conn.next_recv_seq(), 0x7FFF);
        assert_eq!(conn.next_recv_seq(), 0);
        assert_eq!(conn.recv_seq, 1);
    }

    #[test]
    fn test_touch() {
        let mut conn = make_conn();
        assert_eq!(conn.last_activity_ms, 1000);
        conn.touch(5000);
        assert_eq!(conn.last_activity_ms, 5000);
    }
}
