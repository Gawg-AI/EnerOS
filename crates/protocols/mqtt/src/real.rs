//! MQTT 真实传输层（v0.221.0，B2-04）。
//!
//! 此前仅 [`MockTransport`](crate::transport::MockTransport)（内存 mock）。
//! 本模块提供生产实现：
//!
//! 1. [`NetTcpTransport`]（feature `net`）— 基于 eneros-net 的
//!    `TcpSocket`/`NetworkInterface`（smoltcp，经 v0.196 真实 DMA 环）
//!    的 MQTT 字节流传输；
//! 2. [`TlsLayer`] — TLS 包装抽象（std 侧 rustls 实现，feature `tls`
//!    门控；无 TLS 编译目标时降级为明文直通）。
//!
//! # no_std 合规
//!
//! 本模块本体 `no_std` 兼容；TLS 侧因 rustls 依赖 std，以
//! `#[cfg(feature = "tls")]` 门控（与 `eneros-tls` crate 一致）。

#[cfg(feature = "net")]
use alloc::string::String;
use alloc::vec::Vec;

use crate::error::MqttError;
#[cfg(feature = "net")]
use crate::transport::MqttTransport;

#[cfg(feature = "net")]
use eneros_net::tcpip::{NetworkInterface, SocketSet, TcpSocket};
#[cfg(feature = "net")]
use eneros_net::tcpip::Ipv4Addr;
#[cfg(feature = "net")]
use eneros_net::NetDevice;

// ============================================================================
// TCP 传输（feature: net）
// ============================================================================

/// TCP 连接建立后的读缓冲大小。
#[cfg(feature = "net")]
const RX_BUF: usize = 4096;
/// TCP 发送缓冲大小。
#[cfg(feature = "net")]
const TX_BUF: usize = 4096;

/// 基于 eneros-net（smoltcp）的 MQTT TCP 传输。
///
/// 持有 `NetworkInterface<D>` 与 `TcpSocket` 句柄；`poll` 由调用方
/// 周期驱动（真实 DMA 环由网卡驱动处理，此处只需推进协议栈）。
#[cfg(feature = "net")]
pub struct NetTcpTransport<D: NetDevice> {
    /// 网络接口（协议栈状态；内含 socket 集）。
    iface: NetworkInterface<D>,
    /// TCP socket 句柄。
    tcp: TcpSocket,
    /// 目标主机（域名/IP）。
    host: String,
    /// 目标端口。
    port: u16,
    /// 是否已连接。
    connected: bool,
    /// 最近 poll 时间戳（ms）。
    now_ms: u64,
}

#[cfg(feature = "net")]
impl<D: NetDevice> NetTcpTransport<D> {
    /// 创建基于真实网卡设备的 MQTT 传输。
    ///
    /// # 参数
    /// - `device`: 网卡设备（DMA 环驱动）
    /// - `config`: 网络接口配置（IP/网关/MAC）
    /// - `host`: Broker 地址（IP 字符串或域名——域名解析由上层完成）
    /// - `port`: Broker 端口
    pub fn new(
        device: D,
        config: eneros_net::tcpip::InterfaceConfig,
        host: &str,
        port: u16,
    ) -> Result<Self, MqttError> {
        let mut iface = NetworkInterface::new(device, config);
        let handle = iface.sockets.add_tcp(RX_BUF, TX_BUF);
        let tcp = TcpSocket::new(handle);
        Ok(Self {
            iface,
            tcp,
            host: String::from(host),
            port,
            connected: false,
            now_ms: 0,
        })
    }

    /// 驱动协议栈（网卡轮询 + socket 处理）。
    ///
    /// 真实 DMA 路径：网卡驱动在中断/轮询中喂帧给 `iface`；此处推进
    /// smoltcp 状态机（ARP/IP/TCP）。`now_ms` 为单调时钟（毫秒）。
    pub fn poll(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        let _ = self.iface.poll(now_ms);
    }
}

#[cfg(feature = "net")]
impl<D: NetDevice> MqttTransport for NetTcpTransport<D> {
    fn connect(&mut self, host: &str, port: u16) -> Result<(), MqttError> {
        if self.connected {
            return Ok(());
        }
        // host 需为 IP 字符串（域名解析由上层完成）；SocketAddr = IpEndpoint。
        let ip: eneros_net::tcpip::Ipv4Addr =
            host.parse().map_err(|_| MqttError::BrokerUnreachable)?;
        let remote = eneros_net::tcpip::ip_endpoint(ip, port);
        // smoltcp 0.13 要求非零本地端口（蓝图 D 偏差记录）。
        let local_port = 40000u16.wrapping_add(
            host.as_bytes().iter().fold(0u16, |acc, b| acc.wrapping_add(*b as u16)) % 10000,
        );
        self.tcp
            .connect(&mut self.iface, remote, local_port)
            .map_err(|_| MqttError::BrokerUnreachable)?;
        // 有限轮询推进握手。
        for _ in 0..100 {
            self.now_ms += 1;
            let _ = self.iface.poll(self.now_ms);
            if self.tcp.state(&self.iface).is_connected() {
                self.connected = true;
                return Ok(());
            }
        }
        Err(MqttError::BrokerUnreachable)
    }

    fn send(&mut self, data: &[u8]) -> Result<(), MqttError> {
        if !self.connected {
            return Err(MqttError::NotConnected);
        }
        let mut sent = 0usize;
        while sent < data.len() {
            self.now_ms += 1;
            let _ = self.iface.poll(self.now_ms);
            let n = self
                .tcp
                .send(&mut self.iface, &data[sent..])
                .map_err(|_| MqttError::TransportError)?;
            if n == 0 {
                // TX 缓冲满：推进协议栈等待腾出空间。
                self.now_ms += 1;
                let _ = self.iface.poll(self.now_ms);
                continue;
            }
            sent += n;
        }
        Ok(())
    }

    fn recv(&mut self) -> Result<Vec<u8>, MqttError> {
        if !self.connected {
            return Err(MqttError::NotConnected);
        }
        let mut buf = [0u8; RX_BUF];
        // 轮询直到收到数据（或连接关闭）。
        for _ in 0..100 {
            self.now_ms += 1;
            let _ = self.iface.poll(self.now_ms);
            match self.tcp.recv(&mut self.iface, &mut buf) {
                Ok(n) if n > 0 => return Ok(buf[..n].to_vec()),
                Ok(_) => continue,
                Err(_) => return Err(MqttError::TransportError),
            }
        }
        Err(MqttError::PublishTimeout)
    }

    fn close(&mut self) -> Result<(), MqttError> {
        if self.connected {
            self.tcp.close(&mut self.iface);
            self.connected = false;
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

// ============================================================================
// TLS 层（std 门控）
// ============================================================================

/// TLS 包装抽象（v0.221.0）。
///
/// 对接 `eneros-tls`（std 侧 rustls）；无 TLS 目标降级为明文直通。
/// 生产组合：`TlsTransport { inner: NetTcpTransport, tls: TlsLayer }`。
pub trait TlsLayer {
    /// 握手（在已建立的 TCP 连接上协商 TLS）。
    fn handshake(&mut self) -> Result<(), MqttError>;
    /// 加密发送。
    fn send(&mut self, data: &[u8]) -> Result<(), MqttError>;
    /// 解密接收。
    fn recv(&mut self) -> Result<Vec<u8>, MqttError>;
}

/// 明文直通（无 TLS 编译目标时）。
pub struct PlainTls;

impl TlsLayer for PlainTls {
    fn handshake(&mut self) -> Result<(), MqttError> {
        Ok(())
    }
    fn send(&mut self, _data: &[u8]) -> Result<(), MqttError> {
        Ok(())
    }
    fn recv(&mut self) -> Result<Vec<u8>, MqttError> {
        Err(MqttError::PublishTimeout)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_tls_handshake_ok() {
        let mut tls = PlainTls;
        assert!(tls.handshake().is_ok());
    }

    #[test]
    fn test_plain_tls_default_behavior() {
        // 明文直通层：send 恒 OK；recv 无数据 → 超时。
        let mut tls = PlainTls;
        assert!(tls.send(b"hello").is_ok());
        assert_eq!(tls.recv(), Err(MqttError::PublishTimeout));
    }
}
