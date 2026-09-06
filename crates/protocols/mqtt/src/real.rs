//! MQTT 真实传输层（v0.221.0，B2-04）。
//!
//! 此前仅 [`MockTransport`](crate::transport::MockTransport)（内存 mock）。
//! 本模块提供生产实现：
//!
//! 1. `NetTcpTransport`（feature `net`）— 基于 eneros-net 的
//!    `TcpSocket`/`NetworkInterface`（smoltcp，经 v0.196 真实 DMA 环）
//!    的 MQTT 字节流传输；
//! 2. [`SecureMqttTransport`] — 39 WP-39.9：`MqttTransport` +
//!    `SecureChannel` 组合；明文只能以 `InsecureStream`（security=Plain）
//!    存在，且必须经 [`eneros_protocol_abstract::ChannelPolicy`] 显式
//!    放行（release 默认拒绝）。旧 `TlsLayer`/`PlainTcp`（名字含 Tls
//!    但明文直通）已删除。
//!
//! # no_std 合规
//!
//! 本模块本体 `no_std` 兼容；TLS 提供方（rustls/商密）在蓝图 41
//! 安全服务注入（与 `eneros-tls` crate 一致）。

use alloc::boxed::Box;
#[cfg(feature = "net")]
use alloc::string::String;
use alloc::vec::Vec;

#[cfg(feature = "net")]
use eneros_net::tcpip::Ipv4Addr;
#[cfg(feature = "net")]
use eneros_net::tcpip::{NetworkInterface, SocketSet, TcpSocket};
#[cfg(feature = "net")]
use eneros_net::NetDevice;

use crate::error::MqttError;
use crate::transport::MqttTransport;

// ============================================================================
// TCP 传输（feature: net）
// ============================================================================

/// TCP 连接建立后的读缓冲大小。
#[cfg(feature = "net")]
const RX_BUF: usize = 4096;
/// TCP 发送缓冲大小。
#[cfg(feature = "net")]
const TX_BUF: usize = 4096;
/// 39 收尾复验（F4）：send 总时限（ms）——旧实现 `while sent < len`
/// 在 TX 满/对端不 ACK 时无限自旋；现有 deadline + 断连检测。
#[cfg(feature = "net")]
const SEND_DEADLINE_MS: u64 = 5000;
/// 39 收尾复验（F4）：连续零入队上限（双保险：时钟异常时也能终止）。
#[cfg(feature = "net")]
const SEND_MAX_ZERO_STREAK: u32 = 100_000;
/// 68.5（REG-MQTT-BROKER-HANG 修复）：握手等待窗（ms）——SYN-ACK 经
/// slirp 往返需真实时间；紧轮询微秒级跑完只会空转（68.3 bisect 铁证：
/// 有 broker 窗 CONNECT 字节从未到达 host——首试 SynSent 残留后
/// `connect()` 永久 Illegal 且 iface.poll 失去驱动）。
#[cfg(feature = "net")]
const HANDSHAKE_WINDOW_MS: u64 = 100;
/// 68.5：recv 有界等待窗（ms）——旧实现 100 次紧轮询（微秒级）在对端
/// RTT 面前恒空转（CONNACK/PUBACK 必超时）。
#[cfg(feature = "net")]
const RECV_WINDOW_MS: u64 = 100;

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
impl<D: NetDevice + 'static> NetTcpTransport<D> {
    /// 68.5：有界驱动协议栈直至握手完成（SYN-ACK 归来）。
    /// 窗口 = [`HANDSHAKE_WINDOW_MS`] 真实时间（紧轮询无时间预算时对端
    /// RTT 面前恒空转——68.3 bisect 形态）。Ok = Established。
    fn drive_handshake(&mut self) -> Result<(), MqttError> {
        let deadline = eneros_time::get_monotonic_ns() / 1_000_000 + HANDSHAKE_WINDOW_MS;
        loop {
            self.now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
            let _ = self.iface.poll(self.now_ms);
            if self.tcp.state(&self.iface).is_connected() {
                return Ok(());
            }
            if self.now_ms >= deadline {
                return Err(MqttError::PublishTimeout);
            }
        }
    }
}

#[cfg(feature = "net")]
impl<D: NetDevice + 'static> MqttTransport for NetTcpTransport<D> {
    // 31-review：as_any_mut→&mut dyn Any 需 Self:'static（PR-15 引入该方法时漏约束致 net feature 编译失败）
    fn connect(&mut self, host: &str, port: u16) -> Result<(), MqttError> {
        use eneros_net::tcpip::TcpState;
        if self.connected {
            return Ok(());
        }
        // host 需为 IP 字符串（域名解析由上层完成）；SocketAddr = IpEndpoint。
        let ip: eneros_net::tcpip::Ipv4Addr =
            host.parse().map_err(|_| MqttError::BrokerUnreachable)?;
        let remote = eneros_net::tcpip::ip_endpoint(ip, port);
        // smoltcp 0.13 要求非零本地端口（蓝图 D 偏差记录）。
        let local_port = 40000u16.wrapping_add(
            host.as_bytes()
                .iter()
                .fold(0u16, |acc, b| acc.wrapping_add(*b as u16))
                % 10000,
        );
        // 68.5（REG-MQTT-BROKER-HANG 根因修复）：socket 状态机复位纪律。
        // 旧缺陷：首试紧轮询（微秒级）等不到 SYN-ACK → Err 后 socket 残留
        // SynSent；插件下轮重试 `tcp.connect()` 对非 Closed socket 永久
        // Illegal，且 Err 早退使 iface.poll 彻底失去驱动——握手永不完成
        // （有 broker 窗冻结形态；无 broker 时 slirp 快速 RST 复位 socket
        // 反而每次全新 SYN，窗口全绿——双态之谜的解释）。
        match self.tcp.state(&self.iface) {
            TcpState::Established => {
                self.connected = true;
                return Ok(());
            }
            TcpState::SynSent | TcpState::SynReceived => {
                // 残留握手态：有界驱动（SYN 重传随 iface 时间推进）；
                // 仍未完成 → abort 复位（RST/Closed），落回全新 connect。
                let _ = self.drive_handshake();
                if self.tcp.state(&self.iface) != TcpState::Established {
                    self.tcp.abort(&mut self.iface);
                    self.now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
                    let _ = self.iface.poll(self.now_ms);
                    if self.tcp.state(&self.iface) != TcpState::Closed {
                        return Err(MqttError::BrokerUnreachable);
                    }
                } else {
                    self.connected = true;
                    return Ok(());
                }
            }
            _ => {}
        }
        self.tcp
            .connect(&mut self.iface, remote, local_port)
            .map_err(|_| MqttError::BrokerUnreachable)?;
        if self.drive_handshake().is_ok() {
            self.connected = true;
            return Ok(());
        }
        Err(MqttError::BrokerUnreachable)
    }

    fn send(&mut self, data: &[u8]) -> Result<(), MqttError> {
        if !self.connected {
            return Err(MqttError::NotConnected);
        }
        // 39 收尾复验（F4）：有界重试——deadline（单调时钟）+ 连接
        // 存活检测 + 零入队连续上限；旧实现对端不 ACK/链路已死而
        // `connected` 仍 true 时永久自旋。
        let deadline = self
            .now_ms
            .saturating_add(SEND_DEADLINE_MS)
            .max(eneros_time::get_monotonic_ns() / 1_000_000 + SEND_DEADLINE_MS);
        let mut sent = 0usize;
        let mut zero_streak = 0u32;
        while sent < data.len() {
            self.now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
            if self.now_ms >= deadline {
                return Err(MqttError::PublishTimeout);
            }
            let _ = self.iface.poll(self.now_ms);
            // 链路/连接已死 → 不再重试（fail-closed，不伪装发送成功）。
            if !self.tcp.state(&self.iface).is_connected() {
                self.connected = false;
                return Err(MqttError::NotConnected);
            }
            let n = self
                .tcp
                .send(&mut self.iface, &data[sent..])
                .map_err(|_| MqttError::TransportError)?;
            if n == 0 {
                // TX 缓冲满：推进协议栈等待腾出空间（连续零入队有界）。
                zero_streak += 1;
                if zero_streak > SEND_MAX_ZERO_STREAK {
                    return Err(MqttError::PublishTimeout);
                }
                continue;
            }
            zero_streak = 0;
            sent += n;
        }
        Ok(())
    }

    fn recv(&mut self) -> Result<Vec<u8>, MqttError> {
        if !self.connected {
            return Err(MqttError::NotConnected);
        }
        let mut buf = [0u8; RX_BUF];
        // 68.5：真实时间预算窗（旧 100 次紧轮询 = 微秒级，对端 RTT 面前
        // 恒空转——CONNACK/PUBACK 等首包必超时的同族根因）。
        let deadline = eneros_time::get_monotonic_ns() / 1_000_000 + RECV_WINDOW_MS;
        loop {
            self.now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
            let _ = self.iface.poll(self.now_ms);
            match self.tcp.recv(&mut self.iface, &mut buf) {
                Ok(n) if n > 0 => return Ok(buf[..n].to_vec()),
                Ok(_) => {}
                Err(_) => return Err(MqttError::TransportError),
            }
            if self.now_ms >= deadline {
                // 链路/连接已死 → 不再等（fail-closed，不伪装）。
                if !self.tcp.state(&self.iface).is_connected() {
                    self.connected = false;
                    return Err(MqttError::NotConnected);
                }
                return Err(MqttError::PublishTimeout);
            }
        }
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

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

// ============================================================================
// 安全通道（39 WP-39.9：TlsLayer/PlainTcp 已删除——明文不冒充 secure）
// ============================================================================

/// 安全 MQTT 传输组合：`MqttTransport`（明文字节流）+ `SecureChannel`
/// （加密/身份）。`negotiated_security() == Plain` 的通道（
/// [`InsecureStream`]）只有显式策略放行才能使用（release 默认拒绝）。
///
/// 生产组合：`SecureMqttTransport { inner: NetTcpTransport, channel:
/// SecureChannel }`——TLS 提供方（rustls/商密）在蓝图 41 安全服务注入；
/// 本层保证"明文实现不叫 secure 名"。
pub struct SecureMqttTransport {
    inner: Box<dyn MqttTransport>,
    channel: Box<dyn eneros_protocol_abstract::SecureChannel>,
}

impl SecureMqttTransport {
    /// 组合（39 收尾复验 F5：**策略在执行点强制**——不再依赖
    /// "调用方自觉"或尚不存在的工厂）：`policy.allows(通道安全等级)`
    /// 为 false 时直接 `PolicyDenied`（release 默认策略下明文通道
    /// 无法进入）。
    pub fn new(
        inner: Box<dyn MqttTransport>,
        channel: Box<dyn eneros_protocol_abstract::SecureChannel>,
        policy: &eneros_protocol_abstract::ChannelPolicy,
    ) -> Result<Self, MqttError> {
        if !policy.allows(channel.negotiated_security()) {
            return Err(MqttError::PolicyDenied);
        }
        Ok(Self { inner, channel })
    }

    /// 实际安全等级。
    pub fn security(&self) -> eneros_protocol_abstract::ChannelSecurity {
        self.channel.negotiated_security()
    }
}

impl MqttTransport for SecureMqttTransport {
    fn connect(&mut self, host: &str, port: u16) -> Result<(), MqttError> {
        self.inner.connect(host, port)
    }
    fn send(&mut self, data: &[u8]) -> Result<(), MqttError> {
        self.channel
            .send(data)
            .map_err(|_| MqttError::TransportError)
    }
    fn recv(&mut self) -> Result<Vec<u8>, MqttError> {
        // 有界接收：一次读满缓冲；协议剩余长度由 MqttClient 处理。
        let mut buf = [0u8; 4096];
        let n = self
            .channel
            .recv(&mut buf)
            .map_err(|_| MqttError::NotConnected)?;
        Ok(buf[..n].to_vec())
    }
    fn close(&mut self) -> Result<(), MqttError> {
        let _ = self.channel.close();
        self.inner.close()
    }
    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use eneros_protocol_abstract::{
        ChannelPolicy, ChannelSecurity, InsecureStream, TransportError, TransportHandle,
    };

    use super::*;

    /// 测试用空实现传输句柄（InsecureStream 包装用）。
    struct NoopHandle;

    impl TransportHandle for NoopHandle {
        fn send(&mut self, _data: &[u8]) -> Result<(), TransportError> {
            Ok(())
        }
        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, TransportError> {
            Err(TransportError::Closed)
        }
        fn close(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    fn plain_channel() -> Box<dyn eneros_protocol_abstract::SecureChannel> {
        Box::new(InsecureStream::new(Box::new(NoopHandle)))
    }

    /// 39 收尾复验（F5 mutation）：明文通道 + release 默认策略 →
    /// PolicyDenied（蓝图 §8："PlainTcp 实现 secure → Gate 必须转红"
    /// 的编码断言；旧实现策略零执行点）。
    #[test]
    fn plain_channel_rejected_under_release_policy() {
        let inner = Box::new(crate::transport::MockTransport::new());
        let policy = ChannelPolicy::default_release();
        let result = SecureMqttTransport::new(inner, plain_channel(), &policy);
        assert_eq!(result.err(), Some(MqttError::PolicyDenied));
    }

    /// 显式放行明文（dev/lab 策略）→ 组合成功且 security 如实报 Plain。
    #[test]
    fn plain_channel_allowed_only_with_explicit_policy() {
        let inner = Box::new(crate::transport::MockTransport::new());
        let policy = ChannelPolicy::default_dev();
        let t = SecureMqttTransport::new(inner, plain_channel(), &policy).expect("dev policy");
        assert_eq!(t.security(), ChannelSecurity::Plain);
    }
}
