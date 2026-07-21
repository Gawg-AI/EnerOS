//! Modbus TCP 传输层抽象与统计.

use alloc::vec::Vec;

use crate::device::TcpDevice;
use crate::error::ModbusTcpError;

/// TCP 传输层抽象（D1）。
///
/// 由底层网络栈（如 smoltcp）实现，提供面向连接的字节流发送/接收与连接管理。
pub trait TcpTransport {
    /// 发送字节流。
    fn send(&mut self, data: &[u8]) -> Result<(), ModbusTcpError>;
    /// 接收字节流（阻塞直到收到数据或超时）。
    fn recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ModbusTcpError>;
    /// 建立到指定设备的 TCP 连接。
    fn connect(&mut self, device: &TcpDevice) -> Result<(), ModbusTcpError>;
}

/// Modbus TCP 主站统计信息
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TcpStats {
    /// 发送请求总数
    pub request_count: u32,
    /// 收到响应总数（含异常响应）
    pub response_count: u32,
    /// 错误总数（含超时/事务不匹配/解析错误等）
    pub error_count: u32,
    /// 超时次数
    pub timeout_count: u32,
    /// 重连次数（每次 connect 调用计数）
    pub reconnect_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_all_zero() {
        let stats = TcpStats::default();
        assert_eq!(stats.request_count, 0);
        assert_eq!(stats.response_count, 0);
        assert_eq!(stats.error_count, 0);
        assert_eq!(stats.timeout_count, 0);
        assert_eq!(stats.reconnect_count, 0);
    }

    #[test]
    fn test_clone_eq() {
        let stats = TcpStats {
            request_count: 5,
            response_count: 4,
            error_count: 1,
            ..Default::default()
        };
        let mut cloned = stats.clone();
        assert_eq!(stats, cloned);
        cloned.clone_from(&stats);
        assert_eq!(stats, cloned);
    }

    #[test]
    fn test_manual_mutation() {
        let mut stats = TcpStats::default();
        stats.request_count += 1;
        stats.reconnect_count += 1;
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.reconnect_count, 1);
        let other = TcpStats {
            request_count: 1,
            response_count: 0,
            error_count: 0,
            timeout_count: 0,
            reconnect_count: 1,
        };
        assert_eq!(stats, other);
    }
}
