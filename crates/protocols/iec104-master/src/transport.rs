//! IEC 104 主站传输层抽象与统计（D1）.
//!
//! `MasterTransport` trait 抽象 TCP 传输层访问，使主站独立于 smoltcp/eneros-net（D4）。
//! 类比 v0.48.0 `SlaveTransport`，但面向主站（connect/send/recv/close/now_ms）。
//!
//! D9：`SocketHandle` 抽象为 `ConnId = u32`。

use alloc::vec::Vec;

use crate::error::MasterError;

/// 连接标识类型别名（D9）
pub type ConnId = u32;

/// IEC 104 主站传输层抽象（D1）.
///
/// 由底层网络栈（如 smoltcp）实现，提供面向连接的建立/发送/接收/关闭与单调时钟。
/// 时间通过 `now_ms` 提供（D2：无 `MonotonicTime` 类型）。
pub trait MasterTransport {
    /// 建立到指定 IP:port 的连接，返回连接标识。
    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<ConnId, MasterError>;
    /// 向指定连接发送字节流。
    fn send(&mut self, conn: ConnId, data: &[u8]) -> Result<(), MasterError>;
    /// 从指定连接接收一帧数据；无数据时返回 `Ok(None)`。
    fn recv(&mut self, conn: ConnId) -> Result<Option<Vec<u8>>, MasterError>;
    /// 关闭指定连接。
    fn close(&mut self, conn: ConnId) -> Result<(), MasterError>;
    /// 返回当前单调时钟（毫秒）。
    fn now_ms(&self) -> u64;
}

/// IEC 104 主站统计信息
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MasterStats {
    /// 发送帧总数
    pub tx_count: u64,
    /// 接收帧总数
    pub rx_count: u64,
    /// 发送错误总数
    pub tx_error_count: u64,
    /// 接收错误总数
    pub rx_error_count: u64,
    /// 建立连接总数
    pub connect_count: u64,
    /// 断开连接总数
    pub disconnect_count: u64,
    /// 总召唤执行次数
    pub interrogation_count: u64,
    /// 遥控命令执行次数
    pub command_count: u64,
    /// 时钟同步执行次数
    pub clock_sync_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_default_all_zero() {
        let stats = MasterStats::default();
        assert_eq!(stats.tx_count, 0);
        assert_eq!(stats.rx_count, 0);
        assert_eq!(stats.tx_error_count, 0);
        assert_eq!(stats.rx_error_count, 0);
        assert_eq!(stats.connect_count, 0);
        assert_eq!(stats.disconnect_count, 0);
        assert_eq!(stats.interrogation_count, 0);
        assert_eq!(stats.command_count, 0);
        assert_eq!(stats.clock_sync_count, 0);
    }

    #[test]
    fn test_stats_clone_eq() {
        let stats = MasterStats {
            tx_count: 5,
            rx_count: 3,
            connect_count: 1,
            ..Default::default()
        };
        let cloned = stats.clone();
        assert_eq!(stats, cloned);
    }

    #[test]
    fn test_stats_mutation() {
        let mut stats = MasterStats::default();
        stats.tx_count += 1;
        stats.rx_count += 1;
        stats.connect_count += 1;
        stats.disconnect_count += 1;
        stats.interrogation_count += 1;
        stats.command_count += 1;
        stats.clock_sync_count += 1;
        assert_eq!(stats.tx_count, 1);
        assert_eq!(stats.rx_count, 1);
        assert_eq!(stats.connect_count, 1);
        assert_eq!(stats.disconnect_count, 1);
        assert_eq!(stats.interrogation_count, 1);
        assert_eq!(stats.command_count, 1);
        assert_eq!(stats.clock_sync_count, 1);
    }
}
