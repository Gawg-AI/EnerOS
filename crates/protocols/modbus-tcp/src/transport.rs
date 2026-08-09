//! Modbus TCP 传输层抽象与统计.

use alloc::vec::Vec;

#[cfg(feature = "net")]
use eneros_net::tcpip::{ip_endpoint, InterfaceConfig, Ipv4Addr, NetworkInterface, TcpSocket};
#[cfg(feature = "net")]
use eneros_net::NetDevice;

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
    /// 当前是否已建立到 `device` 的连接（PTC-002 连接复用判断——
    /// 同设备复用，换设备视为未连接）。
    fn is_connected(&self, device: &TcpDevice) -> bool;
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

// ============================================================================
// 21-B B1：真实 TCP 传输（feature = "net"）
// ============================================================================

/// RX 缓冲大小（smoltcp 接收环形缓冲）.
#[cfg(feature = "net")]
const RX_BUF: usize = 4096;
/// TX 缓冲大小（smoltcp 发送环形缓冲）.
#[cfg(feature = "net")]
const TX_BUF: usize = 4096;

/// 基于 eneros-net（smoltcp）的 Modbus TCP 主站传输。
///
/// 生产实现：`connect/send/recv` 内部驱动协议栈轮询（`iface.poll`），
/// 提供 `TcpTransport` trait 要求的阻塞语义（超时由 `device.timeout_ms`
/// 与 `recv(timeout_ms)` 控制）。网卡设备 `D: NetDevice` 由调用方注入
/// （真实 DMA 环驱动或测试用互联设备）。
///
/// 与 `MockTcpTransport` 同 trait、不同类型（21-B B1 强约束：禁止 trait
/// 上 `unimplemented!()`；生产实现与 mock 并列，mock 仅 `#[cfg(test)]`）。
#[cfg(feature = "net")]
pub struct SmoltcpTcpTransport<D: NetDevice> {
    /// 网络接口（协议栈状态；内含 socket 集）。
    iface: NetworkInterface<D>,
    /// TCP socket 句柄。
    tcp: TcpSocket,
    /// 当前已连接的设备（PTC-002：同设备复用，换设备视为未连接）。
    current: Option<TcpDevice>,
    /// 传输层统计（超时/重连/错误计数）。
    stats: TcpStats,
}

#[cfg(feature = "net")]
impl<D: NetDevice> SmoltcpTcpTransport<D> {
    /// 创建主站传输。
    ///
    /// `device` 为网卡设备（真实 DMA 环或测试互联设备）；`config` 为
    /// 网络接口配置（静态 IP/MAC；DHCP 由调用方按需开启）。
    pub fn new(device: D, config: InterfaceConfig) -> Self {
        let mut iface = NetworkInterface::new(device, config);
        let handle = iface.sockets.add_tcp(RX_BUF, TX_BUF);
        Self {
            iface,
            tcp: TcpSocket::new(handle),
            current: None,
            stats: TcpStats::default(),
        }
    }

    /// 推进协议栈（单调时钟，毫秒）。
    fn poll(&mut self) {
        let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
        let _ = self.iface.poll(now_ms);
    }

    /// 基于设备 IP 派生的本地端口（smoltcp 0.13 要求非零本地端口）。
    fn local_port(device: &TcpDevice) -> u16 {
        40000u16.wrapping_add(
            device
                .ip
                .iter()
                .fold(0u16, |acc, b| acc.wrapping_add(*b as u16))
                % 10000,
        )
    }
}

#[cfg(feature = "net")]
impl<D: NetDevice> TcpTransport for SmoltcpTcpTransport<D> {
    fn connect(&mut self, device: &TcpDevice) -> Result<(), ModbusTcpError> {
        // PTC-002：同设备已连接 → 复用（不重连）。
        if self.is_connected(device) {
            return Ok(());
        }
        // 换设备：先关闭旧连接。
        if self.current.is_some() {
            self.tcp.close(&mut self.iface);
            self.current = None;
        }
        let ip = Ipv4Addr::from(device.ip);
        let remote = ip_endpoint(ip, device.port);
        let local_port = Self::local_port(device);
        self.tcp
            .connect(&mut self.iface, remote, local_port)
            .map_err(|_| ModbusTcpError::ConnectionFailed)?;
        self.stats.reconnect_count += 1;
        // 有限轮询推进 TCP 握手（三次握手在本地互联下即时完成）。
        // 2000 次：对端（从站/服务器）poll 循环与主站异步，握手需跨越
        // 多次调度切换（21-B B1 loopback 实测：200 次在竞态下不足）。
        for _ in 0..2000 {
            self.poll();
            if self.tcp.state(&self.iface).is_connected() {
                self.current = Some(*device);
                return Ok(());
            }
        }
        self.current = None;
        Err(ModbusTcpError::ConnectionFailed)
    }

    fn send(&mut self, data: &[u8]) -> Result<(), ModbusTcpError> {
        if self.current.is_none() {
            return Err(ModbusTcpError::NotConnected);
        }
        let mut sent = 0usize;
        // 阻塞语义：轮询推进直到全部字节写入 TX 缓冲。
        for _ in 0..2000 {
            self.poll();
            match self.tcp.send(&mut self.iface, &data[sent..]) {
                Ok(0) => continue,
                Ok(n) => sent += n,
                Err(_) => {
                    self.stats.error_count += 1;
                    return Err(ModbusTcpError::Closed);
                }
            }
            if sent >= data.len() {
                return Ok(());
            }
        }
        self.stats.error_count += 1;
        Err(ModbusTcpError::Closed)
    }

    fn recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ModbusTcpError> {
        if self.current.is_none() {
            return Err(ModbusTcpError::NotConnected);
        }
        let mut buf = [0u8; RX_BUF];
        // 阻塞语义：轮询直到收到数据或超时（超时 = timeout_ms 次轮询，
        // 每次轮询约推进一个协议栈 tick；真实时钟超时由上层重试控制）。
        for _ in 0..timeout_ms.max(1) {
            self.poll();
            match self.tcp.recv(&mut self.iface, &mut buf) {
                Ok(0) => continue,
                Ok(n) => return Ok(buf[..n].to_vec()),
                Err(_) => {
                    self.stats.error_count += 1;
                    return Err(ModbusTcpError::Closed);
                }
            }
        }
        self.stats.timeout_count += 1;
        Err(ModbusTcpError::Timeout)
    }

    fn is_connected(&self, device: &TcpDevice) -> bool {
        self.current.as_ref() == Some(device) && self.tcp.state(&self.iface).is_connected()
    }
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
