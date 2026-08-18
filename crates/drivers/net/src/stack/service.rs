//! EL0 NetworkStackService（蓝图 39 WP-39.8）。
//!
//! 独立受限网络服务：持有 `NetworkInterface<D>`（smoltcp）+ **有界**
//! socket 集；versioned 服务句柄带 **connection epoch**——设备/接口重启
//! 递增 epoch，旧句柄显式失效（禁止旧 socket 空成功）。
//!
//! # 不变量
//!
//! - `max_sockets` 有界（默认 8）；超限 → `SocketLimit`（背压，不静默）；
//! - 句柄携带 (id, epoch)；epoch 不符 → `StaleEpoch`（重启/换设备后旧句柄
//!   显式失败，不伪装成功）；
//! - `device_changed()` 递增 epoch 并关闭全部 socket（上层收到连接 epoch
//!   通知后必须重建会话）；
//! - telemetry 分开 link/device/transport/application 状态（`service_stats`）。

use alloc::vec::Vec;

use crate::mac::NetDevice;
use crate::tcpip::addr::SocketHandle;
use crate::tcpip::addr::{ip_endpoint, ipv4_addr, ipv4_cidr};
use crate::tcpip::interface::InterfaceConfig;
use crate::tcpip::socket::TcpSocket;
use crate::tcpip::NetworkInterface;

/// 服务配置。
#[derive(Debug, Clone)]
pub struct NetStackConfig {
    /// 接口 MAC。
    pub mac: [u8; 6],
    /// 静态 IPv4（None = DHCP）。
    pub ipv4: Option<[u8; 4]>,
    /// 默认网关。
    pub gateway: Option<[u8; 4]>,
    /// 是否启用 DHCP。
    pub dhcp: bool,
    /// socket 上限（有界）。
    pub max_sockets: usize,
    /// TCP RX 缓冲。
    pub rx_buf: usize,
    /// TCP TX 缓冲。
    pub tx_buf: usize,
}

impl Default for NetStackConfig {
    fn default() -> Self {
        Self {
            mac: [0x02, 0, 0, 0, 0, 0x01],
            ipv4: None,
            gateway: None,
            dhcp: false,
            max_sockets: 8,
            rx_buf: 4096,
            tx_buf: 4096,
        }
    }
}

/// 服务级 socket 句柄（id + epoch）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceSocketHandle {
    id: u32,
    epoch: u64,
}

/// 服务错误（fail-closed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackError {
    /// socket 数已达上限（背压）。
    SocketLimit,
    /// 句柄无效/已关闭。
    InvalidHandle,
    /// 句柄 epoch 过期（设备/接口重启）。
    StaleEpoch,
    /// 底层 I/O 失败。
    Io,
    /// 超时。
    Timeout,
    /// 连接已关闭。
    Closed,
    /// 不支持的操作（UDP 上 listen 等）。
    Unsupported,
}

/// socket 种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StackSocketKind {
    Tcp,
    Udp,
}

/// 服务统计（分开 link/device/transport/application）。
#[derive(Debug, Clone, Copy, Default)]
pub struct StackStats {
    /// link up 状态（最近 poll）。
    pub link_up: bool,
    /// 累计 poll 次数。
    pub polls: u64,
    /// 当前打开的 socket 数。
    pub open_sockets: usize,
    /// 累计关闭的连接。
    pub closed_connections: u64,
    /// epoch 失效的旧句柄数。
    pub stale_handle_rejections: u64,
}

/// 网络栈服务（有界 socket + connection epoch）。
pub struct NetworkStackService<D: NetDevice> {
    iface: NetworkInterface<D>,
    config: NetStackConfig,
    epoch: u64,
    next_id: u32,
    /// (服务句柄, 种类, 底层 socket 句柄)。
    sockets: Vec<(ServiceSocketHandle, StackSocketKind, SocketHandle)>,
    stats: StackStats,
}

impl<D: NetDevice> NetworkStackService<D> {
    /// 创建服务（`config.max_sockets` 有界；mac 必须单播）。
    pub fn new(device: D, config: NetStackConfig) -> Result<Self, StackError> {
        if config.mac[0] & 1 == 1 {
            return Err(StackError::Unsupported); // 组播 MAC 拒绝
        }
        let iface_config = InterfaceConfig {
            mac_addr: config.mac,
            ipv4_addr: config
                .ipv4
                .map(|ip| ipv4_cidr(ipv4_addr(ip[0], ip[1], ip[2], ip[3]), 24)),
            gateway: config.gateway.map(|g| ipv4_addr(g[0], g[1], g[2], g[3])),
            dhcp: config.dhcp,
        };
        let iface = NetworkInterface::new(device, iface_config);
        Ok(Self {
            iface,
            config,
            epoch: 1,
            next_id: 1,
            sockets: Vec::new(),
            stats: StackStats::default(),
        })
    }

    /// 当前连接 epoch（设备/接口重启后递增）。
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// 推进协议栈（调用方周期驱动；时间毫秒）。
    pub fn poll(&mut self, now_ms: u64) -> Result<(), StackError> {
        self.iface.poll(now_ms).map_err(|_| StackError::Io)?;
        self.stats.polls += 1;
        // link 状态来自底层设备（NetDevice::link_up——真实能力，不构造）。
        self.stats.link_up = self.iface.device.device.link_up();
        self.stats.open_sockets = self.sockets.len();
        Ok(())
    }

    /// 打开 TCP socket（有界）。
    pub fn open_tcp(&mut self) -> Result<ServiceSocketHandle, StackError> {
        self.open(StackSocketKind::Tcp)
    }

    /// 打开 UDP socket（有界）。
    pub fn open_udp(&mut self) -> Result<ServiceSocketHandle, StackError> {
        self.open(StackSocketKind::Udp)
    }

    fn open(&mut self, kind: StackSocketKind) -> Result<ServiceSocketHandle, StackError> {
        if self.sockets.len() >= self.config.max_sockets {
            return Err(StackError::SocketLimit);
        }
        let raw = match kind {
            StackSocketKind::Tcp => self
                .iface
                .sockets
                .add_tcp(self.config.rx_buf, self.config.tx_buf),
            StackSocketKind::Udp => self
                .iface
                .sockets
                .add_udp(self.config.rx_buf, self.config.tx_buf),
        };
        let handle = ServiceSocketHandle {
            id: self.next_id,
            epoch: self.epoch,
        };
        self.next_id = self.next_id.wrapping_add(1);
        self.sockets.push((handle, kind, raw));
        self.stats.open_sockets = self.sockets.len();
        Ok(handle)
    }

    /// 关闭 socket（epoch 校验；stale 句柄显式失败）。
    pub fn close(&mut self, handle: ServiceSocketHandle) -> Result<(), StackError> {
        // 39 收尾复验（F2）：epoch 不符 → StaleEpoch（即使条目已随
        // device_changed 回收——"曾经有效、现已过期"与"从未存在"
        // 仍可区分）。
        if handle.epoch != self.epoch {
            self.stats.stale_handle_rejections += 1;
            return Err(StackError::StaleEpoch);
        }
        let pos = self
            .sockets
            .iter()
            .position(|(h, _, _)| h.id == handle.id)
            .ok_or(StackError::InvalidHandle)?;
        let (_, kind, raw) = self.sockets.remove(pos);
        match kind {
            StackSocketKind::Tcp => TcpSocket::new(raw).close(&mut self.iface),
            StackSocketKind::Udp => {
                self.iface.sockets.inner.remove(raw);
            }
        }
        self.stats.closed_connections += 1;
        self.stats.open_sockets = self.sockets.len();
        Ok(())
    }

    /// TCP 连接（`remote` = ip:port；本地端口自动选择）。
    pub fn tcp_connect(
        &mut self,
        handle: ServiceSocketHandle,
        remote: ([u8; 4], u16),
    ) -> Result<(), StackError> {
        let (_, kind, raw) = self.lookup(handle)?;
        if kind != StackSocketKind::Tcp {
            return Err(StackError::Unsupported);
        }
        let mut tcp = TcpSocket::new(raw);
        let peer = ip_endpoint(
            ipv4_addr(remote.0[0], remote.0[1], remote.0[2], remote.0[3]),
            remote.1,
        );
        tcp.connect(&mut self.iface, peer, 0)
            .map_err(|_| StackError::Io)
    }

    /// 发送（TCP 连接上）。
    ///
    /// 39 收尾复验（F1）：返回**实际入队字节数**——TX 缓冲满时
    /// 部分写不再被 `.map(|_| ())` 吞掉（旧实现背压变静默丢数据）；
    /// `n < data.len()` 即背压信号，调用方 poll 后重试剩余部分。
    /// 零入队（缓冲全满）→ `Timeout`（不谎报成功）。
    pub fn send(&mut self, handle: ServiceSocketHandle, data: &[u8]) -> Result<usize, StackError> {
        let (_, kind, raw) = self.lookup(handle)?;
        if kind != StackSocketKind::Tcp {
            return Err(StackError::Unsupported);
        }
        let mut tcp = TcpSocket::new(raw);
        let n = tcp
            .send(&mut self.iface, data)
            .map_err(|_| StackError::Io)?;
        if n == 0 && !data.is_empty() {
            return Err(StackError::Timeout); // 背压：零入队不得谎报成功
        }
        Ok(n)
    }

    /// 接收（TCP 连接上；无数据 → `Timeout`，调用方按 poll 语义处理）。
    pub fn recv(
        &mut self,
        handle: ServiceSocketHandle,
        buf: &mut [u8],
    ) -> Result<usize, StackError> {
        let (_, kind, raw) = self.lookup(handle)?;
        if kind != StackSocketKind::Tcp {
            return Err(StackError::Unsupported);
        }
        let mut tcp = TcpSocket::new(raw);
        tcp.recv(&mut self.iface, buf)
            .map_err(|_| StackError::Timeout)
    }

    /// 设备/接口重启通知：递增 epoch——旧句柄全部失效（StaleEpoch）。
    ///
    /// 39 收尾复验（F2）：**回收旧资源**——底层 socket 立即关闭并从
    /// iface 移除，条目清空（旧实现只置 epoch=0 不移除 → 重启一次
    /// 即耗尽 socket 配额，服务永久瘫痪）。上层收到 epoch 变化后
    /// 重建会话；重建不受旧条目阻塞。
    pub fn device_changed(&mut self) -> u64 {
        self.epoch = self.epoch.wrapping_add(1);
        for (_, kind, raw) in self.sockets.drain(..) {
            match kind {
                StackSocketKind::Tcp => TcpSocket::new(raw).close(&mut self.iface),
                StackSocketKind::Udp => {
                    self.iface.sockets.inner.remove(raw);
                }
            }
        }
        self.stats.open_sockets = 0;
        self.epoch
    }

    /// 服务统计。
    pub fn stats(&self) -> StackStats {
        self.stats
    }

    fn lookup(
        &mut self,
        handle: ServiceSocketHandle,
    ) -> Result<(ServiceSocketHandle, StackSocketKind, SocketHandle), StackError> {
        // 39 收尾复验（F2）：epoch 不符 → StaleEpoch（先于条目查找，
        // 回收后的旧句柄仍报"过期"而非"从未存在"）。
        if handle.epoch != self.epoch {
            self.stats.stale_handle_rejections += 1;
            return Err(StackError::StaleEpoch);
        }
        let pos = self
            .sockets
            .iter()
            .position(|(h, _, _)| h.id == handle.id)
            .ok_or(StackError::InvalidHandle)?;
        Ok(self.sockets[pos])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{NetError, NetStats};

    /// 测试用网卡（与 tcpip 各测试模块同构）。
    struct MockNetDevice {
        link_up: bool,
    }

    impl MockNetDevice {
        fn new() -> Self {
            Self { link_up: true }
        }
    }

    impl NetDevice for MockNetDevice {
        fn send(&mut self, _frame: &[u8]) -> Result<(), NetError> {
            if !self.link_up {
                return Err(NetError::LinkDown);
            }
            Ok(())
        }
        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, NetError> {
            Err(NetError::NoBuffer)
        }
        fn mac_address(&self) -> [u8; 6] {
            [0x02, 0, 0, 0, 0, 0x01]
        }
        fn mtu(&self) -> usize {
            1500
        }
        fn link_up(&self) -> bool {
            self.link_up
        }
        fn set_promiscuous(&mut self, _on: bool) {}
        fn stats(&self) -> NetStats {
            NetStats::new()
        }
    }

    fn make_service() -> NetworkStackService<MockNetDevice> {
        let device = MockNetDevice::new();
        NetworkStackService::new(device, NetStackConfig::default()).expect("new")
    }

    #[test]
    fn socket_limit_is_bounded() {
        let mut svc = make_service();
        for _ in 0..8 {
            svc.open_tcp().expect("open");
        }
        assert_eq!(svc.open_tcp(), Err(StackError::SocketLimit));
        assert_eq!(svc.open_udp(), Err(StackError::SocketLimit));
    }

    #[test]
    fn stale_epoch_rejects_old_handles() {
        let mut svc = make_service();
        let h1 = svc.open_tcp().expect("open");
        // 设备重启 → epoch 递增，旧句柄失效。
        svc.device_changed();
        assert_eq!(svc.close(h1), Err(StackError::StaleEpoch));
        // 新句柄正常。
        let h2 = svc.open_tcp().expect("open2");
        svc.close(h2).expect("close2");
    }

    #[test]
    fn close_invalid_handle_rejected() {
        let mut svc = make_service();
        let h = ServiceSocketHandle { id: 999, epoch: 1 };
        assert_eq!(svc.close(h), Err(StackError::InvalidHandle));
        assert_eq!(
            svc.tcp_connect(h, ([10, 0, 0, 1], 502)),
            Err(StackError::InvalidHandle)
        );
    }

    #[test]
    fn device_changed_invalidates_all_sockets_and_recovers() {
        let mut svc = make_service();
        let h = svc.open_tcp().expect("open");
        svc.poll(1).unwrap();
        let epoch = svc.device_changed();
        assert!(epoch > 1);
        // 旧句柄显式失败（不空成功）。
        assert_eq!(svc.close(h), Err(StackError::StaleEpoch));
        assert_eq!(svc.send(h, b"x"), Err(StackError::StaleEpoch));
        assert!(svc.stats().stale_handle_rejections >= 1);
        // 39 收尾复验（F2）：旧资源已回收——重建不受配额阻塞
        //（旧实现重启一次即永久 SocketLimit）。
        assert_eq!(svc.stats().open_sockets, 0);
        for _ in 0..8 {
            svc.open_tcp().expect("reopen after device_changed");
        }
    }

    #[test]
    fn poll_updates_link_and_counts() {
        let mut svc = make_service();
        svc.poll(100).unwrap();
        assert_eq!(svc.stats().polls, 1);
        assert!(svc.stats().open_sockets == 0);
    }

    #[test]
    fn multicast_mac_rejected() {
        let mut config = NetStackConfig::default();
        config.mac[0] = 0x01;
        let device = MockNetDevice::new();
        assert!(matches!(
            NetworkStackService::new(device, config),
            Err(StackError::Unsupported)
        ));
    }
}
