//! IEC 104 从站传输层抽象与统计（D1）.
//!
//! `SlaveTransport` trait 抽象 TCP 传输层访问，使从站独立于 smoltcp/eneros-net（D8）。
//! 类比 v0.46.0 Modbus TCP 的 `TcpTransport`，但面向从站（accept/send/recv/close/now_ms）。

use alloc::vec::Vec;

#[cfg(feature = "net")]
use eneros_net::tcpip::{InterfaceConfig, NetworkInterface, TcpSocket};
#[cfg(feature = "net")]
use eneros_net::NetDevice;

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
    /// 返回已发送给 `conn` 的帧切片引用（诊断/测试用）。默认实现返回空。
    fn tx_frames_for(&self, _conn: ConnId) -> Vec<&[u8]> {
        Vec::new()
    }
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

// ============================================================================
// 21-B B1：真实 TCP 传输（feature = "net"）
// ============================================================================

/// RX 缓冲大小（smoltcp 接收环形缓冲）.
#[cfg(feature = "net")]
const RX_BUF: usize = 4096;
/// TX 缓冲大小（smoltcp 发送环形缓冲）.
#[cfg(feature = "net")]
const TX_BUF: usize = 4096;
/// 监听 socket 状态：单连接从站模型（slave.rs 每次仅处理一个连接，
/// 连接关闭后才重新监听）。
#[cfg(feature = "net")]
enum ListenState {
    /// 正在监听（可接受新连接）。
    Listening,
    /// 已接受连接（conn_id = 1）。
    Established,
}

/// 基于 eneros-net（smoltcp）的 IEC 104 从站传输。
///
/// 生产实现：`accept/send/recv/close` 内部驱动协议栈轮询，面向从站
/// （listen + accept）语义，与 `MockSlaveTransport` 同 trait、不同类型
/// （21-B B1 强约束：生产实现与 mock 并列，mock 仅 `#[cfg(test)]`）。
#[cfg(feature = "net")]
pub struct SmoltcpSlaveTransport<D: NetDevice> {
    /// 网络接口（协议栈状态；内含 socket 集）。
    iface: NetworkInterface<D>,
    /// TCP socket 句柄（固定 handle：监听 ↔ 已接受连接复用）。
    tcp: TcpSocket,
    /// 监听端口.
    port: u16,
    /// 连接状态机.
    state: ListenState,
    /// 从站统计（接收/发送/连接计数）.
    stats: SlaveStats,
}

#[cfg(feature = "net")]
impl<D: NetDevice> SmoltcpSlaveTransport<D> {
    /// 创建从站传输并开始监听 `port`（默认 2404）。
    ///
    /// `device` 为网卡设备（真实 DMA 环或测试互联设备）；`config` 为
    /// 网络接口配置（静态 IP/MAC）。
    pub fn new(device: D, config: InterfaceConfig, port: u16) -> Result<Self, Iec104Error> {
        let mut iface = NetworkInterface::new(device, config);
        let handle = iface.sockets.add_tcp(RX_BUF, TX_BUF);
        let mut tcp = TcpSocket::new(handle);
        tcp.listen(&mut iface, port)
            .map_err(|_| Iec104Error::Transport)?;
        Ok(Self {
            iface,
            tcp,
            port,
            state: ListenState::Listening,
            stats: SlaveStats::default(),
        })
    }

    /// 推进协议栈（单调时钟，毫秒）。
    fn poll(&mut self) {
        let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
        let _ = self.iface.poll(now_ms);
    }
}

#[cfg(feature = "net")]
impl<D: NetDevice> SlaveTransport for SmoltcpSlaveTransport<D> {
    fn accept(&mut self) -> Option<ConnId> {
        self.poll();
        match self.state {
            ListenState::Listening => {
                // smoltcp 0.13：listening socket 收到 SYN 后自身进入
                // established——accept 即状态转移（单连接模型）。
                if self.tcp.state(&self.iface).is_connected() {
                    self.state = ListenState::Established;
                    self.stats.connections_accepted += 1;
                    Some(1)
                } else {
                    None
                }
            }
            ListenState::Established => None,
        }
    }

    fn send(&mut self, conn: ConnId, data: &[u8]) -> Result<(), Iec104Error> {
        if conn != 1 || !matches!(self.state, ListenState::Established) {
            return Err(Iec104Error::Transport);
        }
        let mut sent = 0usize;
        // 阻塞语义：轮询推进直到全部字节写入 TX 缓冲（smoltcp 的
        // send_slice 只入缓冲，实际发送在后续 poll 中完成）。
        for _ in 0..2000 {
            self.poll();
            match self.tcp.send(&mut self.iface, &data[sent..]) {
                Ok(0) => continue,
                Ok(n) => sent += n,
                Err(_) => {
                    self.stats.tx_error_count += 1;
                    return Err(Iec104Error::Transport);
                }
            }
            if sent >= data.len() {
                self.stats.tx_count += 1;
                return Ok(());
            }
        }
        self.stats.tx_error_count += 1;
        Err(Iec104Error::Transport)
    }

    fn recv(&mut self, conn: ConnId, buf: &mut [u8]) -> Result<usize, Iec104Error> {
        if conn != 1 || !matches!(self.state, ListenState::Established) {
            return Err(Iec104Error::Transport);
        }
        // 连接已关闭（对端 FIN/RST）→ Err：slave.rs 收到 Err 会清连接
        // 并重新监听（语义对齐）。
        if self.tcp.state(&self.iface).is_closed() {
            return Err(Iec104Error::Transport);
        }
        self.poll();
        match self.tcp.recv(&mut self.iface, buf) {
            Ok(0) => Ok(0),
            Ok(n) => {
                self.stats.rx_count += 1;
                Ok(n)
            }
            Err(_) => Err(Iec104Error::Transport),
        }
    }

    fn close(&mut self, conn: ConnId) {
        if conn != 1 {
            return;
        }
        self.tcp.close(&mut self.iface);
        // 推进 FIN 发送 + 回到监听态（重新 listen 同一端口）。
        for _ in 0..100 {
            self.poll();
        }
        if self.tcp.listen(&mut self.iface, self.port).is_ok() {
            self.state = ListenState::Listening;
            self.stats.connections_closed += 1;
        }
    }

    fn now_ms(&self) -> u64 {
        eneros_time::get_monotonic_ns() / 1_000_000
    }

    fn tx_frames_for(&self, _conn: ConnId) -> Vec<&[u8]> {
        Vec::new()
    }
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
