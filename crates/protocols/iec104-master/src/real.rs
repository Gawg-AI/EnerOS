//! IEC 104 主站真实传输层（31 WP-31.1 / 吸收 WP-27.8）。
//!
//! 基于 eneros-net（smoltcp 封装）的 TCP 客户端传输，签名对照
//! `eneros-iec104-slave` 的 `SmoltcpSlaveTransport`（transport.rs 模式）：
//! connect/send/recv/close 内部驱动协议栈轮询；`now_ms` 来自单调时钟
//! （eneros-time）。connect 失败 fail-closed。

use alloc::vec::Vec;

use eneros_net::tcpip::{
    InterfaceConfig, NetworkInterface, SocketAddr, TcpSocket,
};
use eneros_net::NetDevice;

use crate::error::MasterError;
use crate::transport::{ConnId, MasterTransport};

/// TCP RX 缓冲（字节）。
const RX_BUF: usize = 4096;
/// TCP TX 缓冲（字节）。
const TX_BUF: usize = 4096;

/// 基于 eneros-net（smoltcp）的 IEC 104 主站传输。
///
/// 生产实现：`connect/send/recv/close/now_ms` 内部驱动协议栈轮询，
/// 面向主站（主动 connect）语义；与 `MockMasterTransport` 同 trait、
/// 不同类型（21-B B1：生产实现与 mock 并列，mock 仅测试）。
#[cfg(feature = "net")]
pub struct RealMasterTransport<D: NetDevice> {
    /// 网络接口（协议栈状态；内含 socket 集）。
    iface: NetworkInterface<D>,
    /// TCP socket 句柄。
    tcp: TcpSocket,
    /// 当前连接标识（0 = 未连接）。
    conn_id: ConnId,
    /// 对端地址。
    peer: SocketAddr,
}

#[cfg(feature = "net")]
impl<D: NetDevice> RealMasterTransport<D> {
    /// 创建主站传输（`device` = 网卡；`config` = 网络接口配置；
    /// `peer` = 对端地址——装配层注入，禁止生产路径字面量 IP）。
    pub fn new(device: D, config: InterfaceConfig, peer: SocketAddr) -> Result<Self, MasterError> {
        let mut iface = NetworkInterface::new(device, config);
        let handle = iface.sockets.add_tcp(RX_BUF, TX_BUF);
        let tcp = TcpSocket::new(handle);
        Ok(Self {
            iface,
            tcp,
            conn_id: 0,
            peer,
        })
    }

    /// 推进协议栈（单调时钟，毫秒）。
    fn poll(&mut self) {
        let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
        let _ = self.iface.poll(now_ms);
    }
}

#[cfg(feature = "net")]
impl<D: NetDevice> MasterTransport for RealMasterTransport<D> {
    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<ConnId, MasterError> {
        let remote = SocketAddr::new(ip.into(), port);
        // 本地端口：自动选择（从 20000 起扫描可用）。
        for local in 20000u16..20100 {
            self.poll();
            match self.tcp.connect(&mut self.iface, remote, local) {
                Ok(()) => {
                    self.conn_id = 1;
                    return Ok(1);
                }
                Err(_) => continue,
            }
        }
        Err(MasterError::ConnectFailed)
    }

    fn send(&mut self, conn: ConnId, data: &[u8]) -> Result<(), MasterError> {
        if conn != self.conn_id || self.conn_id == 0 {
            return Err(MasterError::NotConnected);
        }
        let mut sent = 0usize;
        // 阻塞语义：轮询推进直到全部字节写入 TX 缓冲。
        for _ in 0..2000 {
            self.poll();
            match self.tcp.send(&mut self.iface, &data[sent..]) {
                Ok(0) => continue,
                Ok(n) => sent += n,
                Err(_) => return Err(MasterError::SendFailed),
            }
            if sent >= data.len() {
                return Ok(());
            }
        }
        Err(MasterError::SendFailed)
    }

    fn recv(&mut self, conn: ConnId) -> Result<Option<Vec<u8>>, MasterError> {
        if conn != self.conn_id || self.conn_id == 0 {
            return Err(MasterError::NotConnected);
        }
        // 连接已关闭（对端 FIN/RST）→ 清连接（fail-closed）。
        if self.tcp.state(&self.iface).is_closed() {
            self.conn_id = 0;
            return Err(MasterError::RecvFailed);
        }
        self.poll();
        let mut buf = [0u8; 2048];
        match self.tcp.recv(&mut self.iface, &mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(_) => Err(MasterError::RecvFailed),
        }
    }

    fn close(&mut self, conn: ConnId) -> Result<(), MasterError> {
        if conn != self.conn_id || self.conn_id == 0 {
            return Err(MasterError::NotConnected);
        }
        self.tcp.close(&mut self.iface);
        for _ in 0..100 {
            self.poll();
        }
        self.conn_id = 0;
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        eneros_time::get_monotonic_ns() / 1_000_000
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

#[cfg(all(test, feature = "net"))]
mod tests {
    use super::*;

    /// 构造测试网络设备（mock 互联）。
    fn make_transport() -> RealMasterTransport<eneros_net::mock::MockNetDevice> {
        let dev = eneros_net::mock::MockNetDevice::new();
        let cfg = InterfaceConfig::new(
            [10, 0, 0, 2].into(),
            [10, 0, 0, 1].into(),
        );
        RealMasterTransport::new(dev, cfg, SocketAddr::new([10, 0, 0, 1].into(), 2404))
            .expect("transport")
    }

    /// connect 到不可达对端 → ConnectFailed（fail-closed）。
    #[test]
    fn connect_unreachable_fails_closed() {
        let mut t = make_transport();
        // 无对端监听 → connect 在扫描窗口内不建立。
        let r = t.connect([10, 0, 0, 9], 2404);
        assert!(r.is_err(), "不可达对端必须 ConnectFailed（fail-closed）");
    }
}
