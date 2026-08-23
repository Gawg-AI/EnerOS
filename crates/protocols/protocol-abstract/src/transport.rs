//! 协议传输接缝（蓝图 39 WP-39.10：TransportFactory/TransportHandle）。
//!
//! 统一 transport 获取面：协议插件启动时经 [`TransportFactory`] 请求
//! 所需 [`TransportSpec`]，获得有界 [`TransportHandle`]。具体传输
//! （RS485/UART、smoltcp TCP/UDP、raw Ethernet）由各协议 crate 实现
//! 并注入；本层不依赖任何网卡/串口实现（零硬件依赖）。

use alloc::boxed::Box;

/// 传输种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// 串口（RS485/UART）。
    Serial,
    /// TCP。
    Tcp,
    /// UDP。
    Udp,
    /// 原始以太网帧（含 VLAN）。
    RawEthernet,
}

impl TransportKind {
    pub const fn name(&self) -> &'static str {
        match self {
            TransportKind::Serial => "serial",
            TransportKind::Tcp => "tcp",
            TransportKind::Udp => "udp",
            TransportKind::RawEthernet => "raw-ethernet",
        }
    }
}

/// 传输规格（连接参数）。
#[derive(Debug, Clone)]
pub struct TransportSpec {
    /// 传输种类。
    pub kind: TransportKind,
    /// 对端描述（串口设备路径 / IP:port / 接口名——固定缓冲）。
    pub peer: [u8; 128],
    /// `peer` 有效字节数。
    pub peer_len: u8,
    /// 最大帧/报文长度（MTU）。
    pub mtu: usize,
    /// 连接超时（毫秒）。
    pub connect_timeout_ms: u64,
    /// 单次收发超时（毫秒）。
    pub recv_timeout_ms: u64,
}

impl TransportSpec {
    pub fn peer_str(&self) -> &str {
        core::str::from_utf8(&self.peer[..self.peer_len as usize]).unwrap_or("")
    }
}

/// 传输错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// 该种类/规格不受支持。
    Unsupported,
    /// 连接建立失败。
    ConnectFailed,
    /// I/O 失败。
    Io,
    /// 超时。
    Timeout,
    /// 已关闭/失效。
    Closed,
}

/// 已建立的传输句柄（有界收发）。
pub trait TransportHandle {
    /// 发送全部字节（内部可分段；失败不部分成功）。
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError>;
    /// 接收一帧（`buf` 不足截断并返回实际长度）。
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError>;
    /// 关闭。
    fn close(&mut self) -> Result<(), TransportError>;
    /// 是否已连接。
    fn is_connected(&self) -> bool;
    /// 50（WP-50.1）：断线重连到原 peer（broker 重启 seam）。默认 =
    /// 不支持（诚实：未接入重连的传输明确 Unsupported）。
    fn reconnect(&mut self, _peer: &str) -> Result<(), TransportError> {
        Err(TransportError::Unsupported)
    }
    /// 向下转型（具体工厂恢复具体类型——mqtt `as_any_mut` 先例）。
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any;
}

/// 传输工厂（协议插件获取 transport 的唯一入口）。
pub trait TransportFactory {
    /// 打开一个传输。不支持的种类 → `Unsupported`（fail-closed，不猜）。
    fn open(&mut self, spec: &TransportSpec) -> Result<Box<dyn TransportHandle>, TransportError>;
    /// 该工厂是否支持某传输种类。
    fn supports(&self, kind: TransportKind) -> bool;
}

/// 显示辅助（工厂名称用）。
pub trait TransportFactoryExt {
    fn name(&self) -> &str;
}

/// 解析 "a.b.c.d:port"（IP 点分 + 端口；失败 None——不猜）。
pub fn parse_ip_port(text: &str) -> Option<([u8; 4], u16)> {
    let (ip_part, port_part) = text.rsplit_once(':')?;
    let port = port_part.parse::<u16>().ok()?;
    let mut ip = [0u8; 4];
    let mut idx = 0usize;
    for octet in ip_part.split('.') {
        if idx >= 4 {
            return None;
        }
        ip[idx] = octet.parse::<u8>().ok()?;
        idx += 1;
    }
    if idx != 4 {
        return None;
    }
    Some((ip, port))
}

/// 构造 Tcp TransportSpec（peer = "ip:port"）。
pub fn tcp_spec(
    peer: &str,
    mtu: usize,
    connect_timeout_ms: u64,
    recv_timeout_ms: u64,
) -> TransportSpec {
    let mut spec = TransportSpec {
        kind: TransportKind::Tcp,
        peer: [0; 128],
        peer_len: 0,
        mtu,
        connect_timeout_ms,
        recv_timeout_ms,
    };
    let bytes = peer.as_bytes();
    let len = bytes.len().min(spec.peer.len());
    spec.peer[..len].copy_from_slice(&bytes[..len]);
    spec.peer_len = len as u8;
    spec
}

/// 把错误转为字符串（诊断/审计）。
impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TransportError::Unsupported => write!(f, "unsupported transport"),
            TransportError::ConnectFailed => write!(f, "connect failed"),
            TransportError::Io => write!(f, "transport io error"),
            TransportError::Timeout => write!(f, "transport timeout"),
            TransportError::Closed => write!(f, "transport closed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EchoFactory, EchoTransport};

    #[test]
    fn echo_handle_roundtrip() {
        let mut t = EchoTransport::new();
        t.send(b"ping").unwrap();
        let mut buf = [0u8; 16];
        let n = t.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ping");
        assert!(t.is_connected());
        t.close().unwrap();
        assert!(!t.is_connected());
    }

    #[test]
    fn echo_factory_rejects_unsupported_kinds() {
        let mut f = EchoFactory;
        assert!(f.supports(TransportKind::Serial));
        assert!(!f.supports(TransportKind::Tcp));
        let spec = TransportSpec {
            kind: TransportKind::Tcp,
            peer: [0; 128],
            peer_len: 0,
            mtu: 1500,
            connect_timeout_ms: 1000,
            recv_timeout_ms: 1000,
        };
        assert!(matches!(f.open(&spec), Err(TransportError::Unsupported)));
    }
}
