//! IEC 104 从站传输层抽象与统计（D1）.
//!
//! `SlaveTransport` trait 抽象 TCP 传输层访问，使从站独立于 smoltcp/eneros-net（D8）。
//! 类比 v0.46.0 Modbus TCP 的 `TcpTransport`，但面向从站（accept/send/recv/close/now_ms）。

use crate::error::Iec104Error;

/// 连接标识类型别名
pub type ConnId = u32;

/// IEC 104 从站传输层抽象（D1）.
///
/// 由底层网络栈（如 smoltcp）实现，提供面向连接的接收/发送/关闭与单调时钟。
/// 时间通过 `now_ms` 提供（D3：无 `MonotonicTime` 类型）。
pub trait SlaveTransport {
    /// 接受一个新连接，返回连接标识；无连接时返回 `None`。
    fn accept(&mut self) -> Option<ConnId>;
    /// 向指定连接发送字节流。
    fn send(&mut self, conn: ConnId, data: &[u8]) -> Result<(), Iec104Error>;
    /// 从指定连接接收字节流到 `buf`，返回读取字节数；无数据时返回 `Ok(0)`。
    fn recv(&mut self, conn: ConnId, buf: &mut [u8]) -> Result<usize, Iec104Error>;
    /// 关闭指定连接。
    fn close(&mut self, conn: ConnId);
    /// 返回当前单调时钟（毫秒）。
    fn now_ms(&self) -> u64;
}

/// IEC 104 从站统计信息
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlaveStats {
    /// 发送帧总数
    pub tx_count: u32,
    /// 接收帧总数
    pub rx_count: u32,
    /// 发送错误总数
    pub tx_error_count: u32,
    /// 接收错误总数
    pub rx_error_count: u32,
    /// 接受连接总数
    pub connections_accepted: u32,
    /// 关闭连接总数
    pub connections_closed: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_default_all_zero() {
        let stats = SlaveStats::default();
        assert_eq!(stats.tx_count, 0);
        assert_eq!(stats.rx_count, 0);
        assert_eq!(stats.tx_error_count, 0);
        assert_eq!(stats.rx_error_count, 0);
        assert_eq!(stats.connections_accepted, 0);
        assert_eq!(stats.connections_closed, 0);
    }

    #[test]
    fn test_stats_clone_eq() {
        let stats = SlaveStats {
            tx_count: 5,
            rx_count: 3,
            connections_accepted: 1,
            ..Default::default()
        };
        let cloned = stats.clone();
        assert_eq!(stats, cloned);
    }

    #[test]
    fn test_stats_mutation() {
        let mut stats = SlaveStats::default();
        stats.tx_count += 1;
        stats.rx_count += 1;
        stats.connections_accepted += 1;
        stats.connections_closed += 1;
        assert_eq!(stats.tx_count, 1);
        assert_eq!(stats.rx_count, 1);
        assert_eq!(stats.connections_accepted, 1);
        assert_eq!(stats.connections_closed, 1);
    }
}
