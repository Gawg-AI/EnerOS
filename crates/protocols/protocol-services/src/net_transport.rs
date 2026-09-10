//! 协议服务网络传输（蓝图 43 L2 协议互操作面 + 46 WP-46.1 传输工厂补齐）。
//!
//! root 把 virtio-net 槽位（MMIO + 9 页连续 DMA 帧）移交给协议服务
//! （X3=MMIO vaddr, X4=DMA vaddr, X5=DMA phys，与 svc-storage 同机制）；
//! 服务侧在此构造：
//!
//! ```text
//! VirtioNetDriver（VolatileMmio + NetSvcDmaPool + NoopCache）
//!   → NetworkInterface（smoltcp，静态 IP 10.0.2.15/24）
//!   → NetStack（单一 NIC 上的统一栈）
//!   → NetTransportFactory（TransportFactory seam）：
//!       Tcp        → TcpStreamHandle（open 完成三次握手）
//!       Udp        → UdpStreamHandle（open bind 本地端口）
//!       RawEthernet→ RawEthernetHandle（device 直通帧收发，插件自滤 EtherType）
//!   Serial       → SerialTransportFactory（UartHw + RTU 成帧）
//! ```
//!
//! 46 WP-46.1 决策（2026-08-20 实测登记）：
//! - Tcp/Udp/RawEthernet 统一进本工厂（mqtt `NetTcpTransport` 与 104
//!   `RealMasterTransport` 的 smoltcp 机械留在各自 crate 由单元测试复用，
//!   服务侧统一走本 seam——死代码原地复用、不重写）。
//! - Serial 验证路径三选一实测结论 = **b**：`qemu-system-aarch64 -device
//!   help` 无 pl011 可加设备（virt 机内建单 UART 仅 -serial/chardev 映射；
//!   virtio-serial-port / pci-serial 非 PL011 且驱动不兼容）→ QEMU 无第二
//!   串口通道。Serial 工厂照常实现（真机/板卡 bring-up 时由 root 移交
//!   PL011 MMIO），验证路径取 host 级独立 peer（46.5 如实分级）。
//! - RawEthernet 句柄直通设备帧（virtio-net legacy：TX 前置 10B 头由驱动
//!   处理、RX 剥头后给净帧）；SV/GOOSE 的 EtherType/APPID 过滤在插件
//!   codec（sv_rx/goose_rx，46.1 同步补 802.1Q VLAN 偏移），工厂不猜。
//! - 资源占用：单一 NIC 由单一服务独占（46.2 spawn 装配校验强制）；同一
//!   工厂不混合 Tcp 轮询与 Raw 直读（帧由先到者消费）。

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::cell::RefCell;

// SPEC-R4 WP-R4.3：DMA 池页数单源 = handover-abi（历史本地定义 +
// 注释自称"单一事实源"已收编；root spawn 侧同为 re-export）。
pub use eneros_handover_abi::NET_DMA_PAGES;
use eneros_net::dma_mem::NoopCache;
use eneros_net::mac::NetDevice;
use eneros_net::tcpip::addr::SocketHandle;
use eneros_net::tcpip::{
    ip_endpoint, ipv4_addr, ipv4_cidr, InterfaceConfig, NetworkInterface, TcpSocket, UdpSocket,
};
use eneros_net::virtio_net::VirtioNetDriver;
use eneros_protocol_abstract::{
    parse_ip_port, TransportError, TransportFactory, TransportHandle, TransportKind, TransportSpec,
};
use eneros_storage::driver::virtio_blk::VolatileMmio;
use eneros_virtio_core::{DmaPool, VirtioMmio};

/// 服务侧 DMA 池：root 移交的连续物理帧（CPU 视图 = 服务 VSpace 映射
/// 虚拟地址，设备视图 = 物理基址）。与 drv-svc blk_backend 的
/// `SvcDmaPool` 同构，实现 `virtio-core::DmaPool`。
pub struct NetSvcDmaPool {
    /// CPU 视图基址（服务 VSpace 已映射虚拟地址）。
    cpu_ptr: *mut u8,
    /// 设备可见物理基址（Q_PFN/描述符 addr 写此值）。
    phys: u64,
    /// 池长度（字节）。
    len: usize,
}

// SAFETY: 池内存为 root 移交的静态帧映射，服务单线程独占（驱动内部
// RefCell 串行化）；指针不跨界读写。
unsafe impl Send for NetSvcDmaPool {}
unsafe impl Sync for NetSvcDmaPool {}

impl NetSvcDmaPool {
    /// 从 root 移交参数构造（cpu_vaddr / dma_phys / 页数）。
    ///
    /// # SAFETY
    /// `cpu_vaddr` 必须是 root 映射到服务 VSpace 的 DMA 帧虚拟地址，
    /// 且覆盖 `pages * 4096` 字节；`phys` 为同一批帧的物理基址。
    pub const unsafe fn new(cpu_vaddr: u64, phys: u64, pages: usize) -> Self {
        Self {
            cpu_ptr: cpu_vaddr as *mut u8,
            phys,
            len: pages * 4096,
        }
    }
}

impl DmaPool for NetSvcDmaPool {
    fn len(&self) -> usize {
        self.len
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn phys_base(&self) -> u64 {
        self.phys
    }
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: `cpu_ptr` 指向 root 移交的 'static 帧映射；`len` 为
        // 移交长度（<= 池容量）；驱动只在此范围内读写（virtio 描述符/
        // TX/RX 缓冲）。
        unsafe { core::slice::from_raw_parts_mut(self.cpu_ptr, self.len) }
    }
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: 同上（只读视图）。
        unsafe { core::slice::from_raw_parts(self.cpu_ptr, self.len) }
    }
}

/// 具体网络驱动（MMIO 后端 + 自持池 + 一致平台 NoopCache——QEMU virt
/// 声明 dma-coherent；非一致平台由 38 平台门禁拒绝，此处不伪造 cache）。
pub type NetDriver = VirtioNetDriver<VolatileMmio, NetSvcDmaPool, NoopCache>;

/// TCP/UDP socket 缓冲（与 mqtt/104 既有传输同量级）。
const SOCK_RX_BUF: usize = 4096;
const SOCK_TX_BUF: usize = 4096;
/// 连接/发送轮询上限（与 SmoltcpTcpTransport 同语义：2000 次协议栈
/// tick 内推进握手/发完，超限 fail-closed 不无限自旋）。
const POLL_ITERS: u32 = 2000;
/// raw Ethernet 最大帧（virtio-net TX 缓冲 2048 - 10B legacy 头，留余量）。
/// 46.5 修正：1500 → 1526——插件 `MAX_FRAME=1518`（标准以太网最大帧）本就
/// 超 1500，且 46.4 GOOSE VLAN 802.1Q TX 帧达 1522；旧值导致 sv/goose
/// open_raw 恒 ConnectFailed → plugin start ProtocolInit（QEMU 实弹登记）。
const MAX_RAW_FRAME: usize = 1526;

/// 统一网络栈（单一 NIC 上的 smoltcp 栈 + 有界 socket 集）。
pub struct NetStack<D: NetDevice> {
    iface: NetworkInterface<D>,
}

/// 生产构建（root 移交的 virtio-net 单一 NIC）。
impl NetStack<NetDriver> {
    /// 构建网络栈（root 移交参数；任一为 0 → Err → 服务保持无传输
    /// fail-closed，不伪造协议 I/O）。IP 配置为 QEMU slirp 网段默认；
    /// 生产网络配置由 46.3 config-bundle 注入（本 constructor 保持与
    /// 45 卷一致的既有默认）。
    pub fn new(mmio_vaddr: u64, dma_vaddr: u64, dma_phys: u64) -> Result<Self, &'static str> {
        if mmio_vaddr == 0 || dma_vaddr == 0 || dma_phys == 0 {
            return Err("device-handover-missing");
        }
        // SAFETY: root 已把 DMA 帧映射到 dma_vaddr（长度 = NET_DMA_PAGES×4K）。
        let pool = unsafe { NetSvcDmaPool::new(dma_vaddr, dma_phys, NET_DMA_PAGES) };
        let mmio = VolatileMmio::new(mmio_vaddr as usize);
        let driver = VirtioNetDriver::new(mmio, pool, NoopCache).map_err(|e| {
            // 46.5 诊断：virtio 发现失败检查点（QEMU 侧该槽设备的寄存器真值
            // dump，区分：设备缺失/版本/status 写不回流/FEATURES_OK 被拒）。
            let r = VolatileMmio::new(mmio_vaddr as usize);
            eneros_runtime::println!(
                "[net] virtio-net-init-failed mmio=0x{:x} err={:?} magic=0x{:08x} ver=0x{:08x} dev=0x{:08x} status=0x{:08x} feats0=0x{:08x}",
                mmio_vaddr,
                e,
                r.read_reg(0x000),
                r.read_reg(0x004),
                r.read_reg(0x008),
                r.read_reg(0x070),
                r.read_reg(0x010)
            );
            "virtio-net-init-failed"
        })?;
        eneros_runtime::println!("[net] netstack-built mmio=0x{:x}", mmio_vaddr);
        // mac 来自设备能力（未协商 MAC 时驱动拒绝构造——不猜）。
        let mac = driver.mac_address();
        let config = InterfaceConfig {
            mac_addr: mac,
            ipv4_addr: Some(ipv4_cidr(ipv4_addr(10, 0, 2, 15), 24)),
            gateway: Some(ipv4_addr(10, 0, 2, 2)),
            dhcp: false,
        };
        Ok(Self {
            iface: NetworkInterface::new(driver, config),
        })
    }
}

impl<D: NetDevice> NetStack<D> {
    fn poll(&mut self, now_ms: u64) {
        let _ = self.iface.poll(now_ms);
    }

    /// 打开 TCP socket 并完成连接（本地端口派生方式与 45 卷
    /// `SmoltcpTcpTransport` 一致——仅地址字节和；QEMU 证据端口不变）。
    fn tcp_connect(&mut self, ip: [u8; 4], port: u16) -> Result<SocketHandle, TransportError> {
        let handle = self.iface.sockets.add_tcp(SOCK_RX_BUF, SOCK_TX_BUF);
        let local_port = 40000u16
            .wrapping_add(ip.iter().fold(0u16, |acc, b| acc.wrapping_add(*b as u16)) % 10000);
        let remote = ip_endpoint(ipv4_addr(ip[0], ip[1], ip[2], ip[3]), port);
        let mut tcp = TcpSocket::new(handle);
        // 68.5 诊断（REG-MQTT-BROKER-HANG）：连接结局显式化（建立/拒绝/
        // 超时三态可观测；词形有界——每调用一行）。
        let peer = alloc::format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port);
        let r = tcp.connect(&mut self.iface, remote, local_port);
        if let Err(e) = r {
            eneros_runtime::println!("[net] tcp-connect FAIL {peer} err={e:?}");
            self.iface.sockets.remove(handle);
            return Err(TransportError::ConnectFailed);
        }
        for _ in 0..POLL_ITERS {
            let now = eneros_time::get_monotonic_ns() / 1_000_000;
            self.poll(now);
            if tcp.state(&self.iface).is_connected() {
                eneros_runtime::println!("[net] tcp-connect OK {peer}");
                return Ok(handle);
            }
        }
        let st = tcp.state(&self.iface);
        eneros_runtime::println!("[net] tcp-connect TIMEOUT {peer} state={st:?}");
        // 68.5（REG-MQTT-BROKER-HANG 根因修复）：裸 remove 不发任何段，
        // 对端（slirp）侧连接悬挂——同 4 元组的新 SYN 会被按重复丢弃。
        // abort 先行（RST 出环）再移除，对端同步清理。
        tcp.abort(&mut self.iface);
        let now = eneros_time::get_monotonic_ns() / 1_000_000;
        self.poll(now);
        self.iface.sockets.remove(handle);
        Err(TransportError::ConnectFailed)
    }

    /// 打开 UDP socket 并绑定本地端口。
    fn udp_open(&mut self, local_port: u16) -> Result<SocketHandle, TransportError> {
        let handle = self.iface.sockets.add_udp(SOCK_RX_BUF, SOCK_TX_BUF);
        let mut udp = UdpSocket::new(handle);
        if udp.bind(&mut self.iface, local_port).is_err() {
            self.iface.sockets.remove(handle);
            return Err(TransportError::Io);
        }
        Ok(handle)
    }
}

/// 由 peer 派生的确定性本地端口（TCP 同方案；测试/诊断可预测）。
fn derived_local_port(ip: [u8; 4]) -> u16 {
    40000u16.wrapping_add(ip.iter().fold(0u16, |acc, b| acc.wrapping_add(*b as u16)) % 10000)
}

/// 传输工厂统一实现（Tcp/Udp/RawEthernet；Serial 走 [`SerialTransportFactory`]）。
pub struct NetTransportFactory<D: NetDevice> {
    stack: Rc<RefCell<NetStack<D>>>,
}

impl<D: NetDevice> NetTransportFactory<D> {
    pub fn new(stack: NetStack<D>) -> Self {
        Self {
            stack: Rc::new(RefCell::new(stack)),
        }
    }
}

impl<D: NetDevice + 'static> TransportFactory for NetTransportFactory<D> {
    fn supports(&self, kind: TransportKind) -> bool {
        matches!(
            kind,
            TransportKind::Tcp | TransportKind::Udp | TransportKind::RawEthernet
        )
    }

    fn open(&mut self, spec: &TransportSpec) -> Result<Box<dyn TransportHandle>, TransportError> {
        match spec.kind {
            TransportKind::Tcp => self.open_tcp(spec),
            TransportKind::Udp => self.open_udp(spec),
            TransportKind::RawEthernet => self.open_raw(spec),
            TransportKind::Serial => Err(TransportError::Unsupported),
        }
    }
}

impl<D: NetDevice + 'static> NetTransportFactory<D> {
    fn open_tcp(
        &mut self,
        spec: &TransportSpec,
    ) -> Result<Box<dyn TransportHandle>, TransportError> {
        let peer = spec.peer_str();
        let (ip, port) = parse_ip_port(peer).ok_or(TransportError::ConnectFailed)?;
        let mut stack = self.stack.borrow_mut();
        let id = stack.tcp_connect(ip, port)?;
        Ok(Box::new(TcpStreamHandle {
            stack: self.stack.clone(),
            id,
            recv_timeout_ms: spec.recv_timeout_ms,
            connected: true,
        }))
    }

    fn open_udp(
        &mut self,
        spec: &TransportSpec,
    ) -> Result<Box<dyn TransportHandle>, TransportError> {
        let peer = spec.peer_str();
        let (ip, port) = parse_ip_port(peer).ok_or(TransportError::ConnectFailed)?;
        let mut stack = self.stack.borrow_mut();
        let id = stack.udp_open(derived_local_port(ip))?;
        Ok(Box::new(UdpStreamHandle {
            stack: self.stack.clone(),
            id,
            peer: (ip, port),
            local_port: derived_local_port(ip),
            recv_timeout_ms: spec.recv_timeout_ms,
        }))
    }

    fn open_raw(
        &mut self,
        spec: &TransportSpec,
    ) -> Result<Box<dyn TransportHandle>, TransportError> {
        let peer = spec.peer_str();
        // 解析成功即校验格式（fail-closed 于坏格式/坏 MAC）；句柄透传帧，
        // 不持有对端描述。
        parse_mac(peer).ok_or(TransportError::ConnectFailed)?;
        if spec.mtu == 0 || spec.mtu > MAX_RAW_FRAME {
            return Err(TransportError::ConnectFailed);
        }
        Ok(Box::new(RawEthernetHandle {
            stack: self.stack.clone(),
            mtu: spec.mtu,
            recv_timeout_ms: spec.recv_timeout_ms.max(1),
        }))
    }
}

/// `TransportHandle` 实现：TCP 流（连接在 `open` 完成；收发语义与 45 卷
/// `HandleAdapter`/`SmoltcpTcpTransport` 一致——轮询推进协议栈）。
///
/// 蓝图 70.4（WP-70.4.1 / N8·N9 修复）：socket 所有权显式状态机——
/// `connected = true` 当且仅当 `id` 指向集合内有效 socket。close 恒幂等
/// （二次 close/close 后观察不再触碰已移除槽位）；reconnect 失败后句柄
/// 停留 Disconnected（可再次 reconnect/send 得到显式错误，不再以陈旧 id
/// 触碰后来复用的新连接）。`id` 仅在 `connected == true` 时被解引用。
pub struct TcpStreamHandle<D: NetDevice> {
    stack: Rc<RefCell<NetStack<D>>>,
    id: SocketHandle,
    recv_timeout_ms: u64,
    /// 显式所有权状态（70.4 WP-70.4.1 单一事实源）：false = 无 socket
    /// （close/建联失败/重连失败后）；此时 `id` 是陈旧值，任何方法不得
    /// 用其构造 `TcpSocket`。
    connected: bool,
}

impl<D: NetDevice + 'static> TransportHandle for TcpStreamHandle<D> {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        if !self.connected {
            return Err(TransportError::Closed);
        }
        let mut stack = self.stack.borrow_mut();
        let mut sent = 0usize;
        let mut tcp = TcpSocket::new(self.id);
        for _ in 0..POLL_ITERS {
            let now = eneros_time::get_monotonic_ns() / 1_000_000;
            stack.poll(now);
            match tcp.send(&mut stack.iface, &data[sent..]) {
                Ok(0) => continue,
                Ok(n) => sent += n,
                Err(_) => return Err(TransportError::Io),
            }
            if sent >= data.len() {
                return Ok(());
            }
        }
        Err(TransportError::Io)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        if !self.connected {
            return Err(TransportError::Closed);
        }
        let mut stack = self.stack.borrow_mut();
        let mut tcp = TcpSocket::new(self.id);
        for _ in 0..self.recv_timeout_ms.max(1) {
            let now = eneros_time::get_monotonic_ns() / 1_000_000;
            stack.poll(now);
            if tcp.state(&stack.iface).is_closed() {
                return Err(TransportError::Closed);
            }
            if let Ok(n) = tcp.recv(&mut stack.iface, buf) {
                if n > 0 {
                    return Ok(n);
                }
            }
        }
        Err(TransportError::Timeout)
    }

    fn close(&mut self) -> Result<(), TransportError> {
        // 70.4（WP-70.4.1 / N9 修复）：close 恒幂等——无 socket 时直接
        // 成功返回，不得以陈旧 id 触碰已移除槽位（可能已复用为他人连接）。
        if !self.connected {
            return Ok(());
        }
        let mut stack = self.stack.borrow_mut();
        let mut tcp = TcpSocket::new(self.id);
        // 68.5 复查补口（REG-MQTT-BROKER-HANG 同族）：裸 remove 不发任何
        // 段——FIN/RST 永不出环，对端（slirp）侧连接悬挂占住同 4 元组，
        // 后续新建连接的 SYN 被按重复丢弃（与 reconnect/tcp_connect 超时
        // 路径 68.5 根因修复同机制）。abort（RST 语义足够）+ poll 出环
        // 后再移除。
        tcp.abort(&mut stack.iface);
        let now = eneros_time::get_monotonic_ns() / 1_000_000;
        stack.poll(now);
        stack.iface.sockets.remove(self.id);
        self.connected = false;
        Ok(())
    }

    fn set_recv_timeout_ms(&mut self, ms: u64) {
        self.recv_timeout_ms = ms;
    }

    fn is_connected(&self) -> bool {
        // 70.4：显式状态单源——无 socket 即 false（N9：close 后旧实现
        // 仍以陈旧 id 读栈返回真值）。
        if !self.connected {
            return false;
        }
        let stack = self.stack.borrow();
        TcpSocket::new(self.id).state(&stack.iface).is_connected()
    }

    /// 50（WP-50.1）：断线重连到原 peer（broker 重启 seam）——释放旧
    /// socket（若持有）并新建连接（tcp_connect 内 poll 到 connected）。
    ///
    /// 70.4（WP-70.4.1 / N8 修复）：建联失败后句柄停留 Disconnected
    /// （`connected = false`），不把 `self.id` 留在"已移除槽位"上——旧
    /// 实现 `tcp_connect()?` 提前返回后，后续 send/recv/is_connected 以
    /// 陈旧 id 触碰可能已复用的新连接。重试前不重复移除（`connected`
    /// 已 false 时直接建联）。
    fn reconnect(&mut self, peer: &str) -> Result<(), TransportError> {
        let (ip, port) = parse_ip_port(peer).ok_or(TransportError::ConnectFailed)?;
        if self.connected {
            let mut stack = self.stack.borrow_mut();
            // 68.5（REG-MQTT-BROKER-HANG 根因修复）：旧 socket 先 abort
            // （RST 出环让 slirp 侧连接同步清理）再移除——裸 remove 留下
            // 悬挂连接占用同 4 元组，重建的新 SYN 永被丢弃。
            {
                let mut old = TcpSocket::new(self.id);
                old.abort(&mut stack.iface);
                let now = eneros_time::get_monotonic_ns() / 1_000_000;
                stack.poll(now);
            }
            stack.iface.sockets.remove(self.id);
            self.connected = false;
        }
        // 建联（无 socket 状态下；失败 = 显式 Err，所有权保持 Disconnected）。
        let mut stack = self.stack.borrow_mut();
        let id = stack.tcp_connect(ip, port)?;
        self.id = id;
        self.connected = true;
        Ok(())
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

/// `TransportHandle` 实现：UDP 数据报（open 已 bind；send_to/recv_from）。
pub struct UdpStreamHandle<D: NetDevice> {
    stack: Rc<RefCell<NetStack<D>>>,
    id: SocketHandle,
    peer: ([u8; 4], u16),
    /// 本地（已绑定）端口——诊断/测试（downcast 用）。
    pub local_port: u16,
    recv_timeout_ms: u64,
}

impl<D: NetDevice + 'static> TransportHandle for UdpStreamHandle<D> {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        let mut stack = self.stack.borrow_mut();
        let mut udp = UdpSocket::new(self.id);
        let remote = ip_endpoint(
            ipv4_addr(
                self.peer.0[0],
                self.peer.0[1],
                self.peer.0[2],
                self.peer.0[3],
            ),
            self.peer.1,
        );
        udp.send_to(&mut stack.iface, data, remote)
            .map_err(|_| TransportError::Io)?;
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let mut stack = self.stack.borrow_mut();
        let mut udp = UdpSocket::new(self.id);
        for _ in 0..self.recv_timeout_ms.max(1) {
            let now = eneros_time::get_monotonic_ns() / 1_000_000;
            stack.poll(now);
            if let Ok((n, _)) = udp.recv_from(&mut stack.iface, buf) {
                if n > 0 {
                    return Ok(n);
                }
            }
        }
        Err(TransportError::Timeout)
    }

    fn close(&mut self) -> Result<(), TransportError> {
        let mut stack = self.stack.borrow_mut();
        stack.iface.sockets.remove(self.id);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

/// `TransportHandle` 实现：raw Ethernet 帧收发（设备直通；virtio 头已由
/// 驱动处理）。`send` 透传完整帧（含 EtherType/可选 VLAN 标签——codec
/// 构建）；`recv` 返回净帧（长度不足 / 超时 fail-closed；EtherType 过滤
/// 属 codec 职责）。
pub struct RawEthernetHandle<D: NetDevice> {
    stack: Rc<RefCell<NetStack<D>>>,
    mtu: usize,
    recv_timeout_ms: u64,
}

impl<D: NetDevice + 'static> TransportHandle for RawEthernetHandle<D> {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        if data.is_empty() || data.len() > self.mtu {
            return Err(TransportError::Io);
        }
        let mut stack = self.stack.borrow_mut();
        stack
            .iface
            .device
            .device
            .send(data)
            .map_err(|_| TransportError::Io)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let mut stack = self.stack.borrow_mut();
        for _ in 0..self.recv_timeout_ms {
            match stack.iface.device.device.recv(buf) {
                Ok(0) => continue,
                Ok(n) => return Ok(n),
                Err(_) => continue, // 无帧（NoBuffer）→ 继续轮询至超时。
            }
        }
        Err(TransportError::Timeout)
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

/// 解析 MAC：12 位十六进制（"010ccd040001"）或冒号分隔
/// （"01:0c:cd:04:00:01"）；其余格式 → None（不猜）。
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    if text.len() == 12 && !text.contains(':') {
        let mut out = [0u8; 6];
        for i in 0..6 {
            out[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
        }
        return Some(out);
    }
    if text.len() == 17 {
        let mut out = [0u8; 6];
        let mut idx = 0usize;
        for part in text.split(':') {
            if idx >= 6 || part.len() != 2 {
                return None;
            }
            out[idx] = u8::from_str_radix(part, 16).ok()?;
            idx += 1;
        }
        if idx == 6 {
            return Some(out);
        }
    }
    None
}

/// 本次移交的传输（兼容型别名：svc-modbus-tcp 的
/// `build_net_transport(...) -> NetTransport` 仍成立）。
/// 生产用统一传输（root 移交的 virtio-net 单一 NIC）。
pub type NetTransport = NetStack<NetDriver>;

/// 构建统一传输栈（root 移交参数；任一为 0 → Err → 服务保持无传输
/// fail-closed，不伪造协议 I/O）。
pub fn build_net_transport(
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
) -> Result<NetTransport, &'static str> {
    NetStack::new(mmio_vaddr, dma_vaddr, dma_phys)
}

// ============================================================================
// Serial 工厂（46 WP-46.1：PL011 UartHw 包装为 TransportFactory）
// ============================================================================

use eneros_driver_framework::DriverError;
use eneros_modbus_rtu::transport::Rs485RtuTransport;
use eneros_rs485::config::{Parity, StopBits};
use eneros_rs485::uart_hw::UartHw;

/// Serial 传输工厂：把 UART 硬件（PL011 MMIO 或 host 仿真）暴露为
/// `TransportFactory`（kind=Serial → `Rs485RtuTransport` 成帧句柄）。
///
/// 单次移交语义：`open` 把工厂持有的 UART 移交给句柄（`Option::take`——
/// 一个串口链恰被一个传输独占，46 资源占用表强制；二次 open → fail-closed）。
/// `H: UartHw` 自带 `Send + Sync` 超级 trait（rs485 设计约束）。
///
/// 真机路径：root 移交 PL011 MMIO（QEMU 无第二串口——46 决策 b）后由
/// svc-* 以硬件实例构造；host 验证用 `MockUartHw`（46.5）。
pub struct SerialTransportFactory<H: UartHw + 'static> {
    uart: Option<H>,
    baud: u32,
    /// SPEC-D WP-D.2：校验位（配置键 `parity`；默认 None = 8N1）。数据位
    /// 8 / 停止位 1 维持 B1 强约束固定（RTU 标准帧形）。
    parity: Parity,
}

impl<H: UartHw + 'static> SerialTransportFactory<H> {
    /// `baud` 为链路波特率（RTU 帧间隔 3.5 字符时间依赖它）。
    pub fn new(uart: H, baud: u32) -> Self {
        Self {
            uart: Some(uart),
            baud,
            parity: Parity::None,
        }
    }

    /// SPEC-D WP-D.2：链式注入校验位（svc bin 按配置键 `parity` 解析；
    /// 非法值在解析层 fail-closed，不落此处）。
    pub fn with_parity(mut self, parity: Parity) -> Self {
        self.parity = parity;
        self
    }
}

impl<H: UartHw + 'static> TransportFactory for SerialTransportFactory<H> {
    fn supports(&self, kind: TransportKind) -> bool {
        kind == TransportKind::Serial
    }

    fn open(&mut self, spec: &TransportSpec) -> Result<Box<dyn TransportHandle>, TransportError> {
        if spec.kind != TransportKind::Serial {
            return Err(TransportError::Unsupported);
        }
        // B1 强约束：生产实现不隐藏硬件配置——open 即配置（SPEC-D 起校验位
        // 可配置，默认仍 8N1）+ DE/RE 归收。
        let mut uart = self.uart.take().ok_or(TransportError::Io)?;
        uart.configure(self.baud, 8, StopBits::One, self.parity)
            .map_err(|_| TransportError::Io)?;
        uart.configure_de_re(None).map_err(|_| TransportError::Io)?;
        uart.set_de_re(false).map_err(|_| TransportError::Io)?;
        // 成帧传输（3.5 字符间隔成帧——真实 RTU 帧边界）。
        let rtu = Rs485RtuTransport::new(uart, self.baud);
        Ok(Box::new(SerialHandle {
            rtu,
            recv_timeout_ms: spec.recv_timeout_ms,
        }))
    }
}

/// `TransportHandle` 实现：RS485/RTU 字节流（`send` 发整帧字节；
/// `recv` 超时驱动 + 3.5 字符间隔成帧返回完整帧）。
pub struct SerialHandle<H: UartHw + 'static> {
    rtu: Rs485RtuTransport<H>,
    recv_timeout_ms: u64,
}

impl<H: UartHw + 'static> TransportHandle for SerialHandle<H> {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        use eneros_modbus_rtu::master::RtuTransport;
        self.rtu.send(data).map_err(|_| TransportError::Io)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        use eneros_modbus_rtu::master::RtuTransport;
        let data = self
            .rtu
            .recv(self.recv_timeout_ms.max(1) as u32)
            .map_err(|e| {
                if e == DriverError::Timeout {
                    TransportError::Timeout
                } else {
                    TransportError::Io
                }
            })?;
        let n = core::cmp::min(data.len(), buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
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

#[cfg(test)]
mod tests {
    use alloc::collections::VecDeque;
    use alloc::rc::Rc;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    use eneros_net::error::{NetError, NetStats};
    use eneros_net::mac::NetDevice;

    use super::*;

    /// 脚本化网卡：RX 队列经 Rc 共享（测试注入帧）+ TX 记录。
    /// RX 队列共享句柄（测试注入帧）。
    type RxQueue = Rc<RefCell<VecDeque<Vec<u8>>>>;
    /// TX 记录共享句柄。
    type TxQueue = Rc<RefCell<Vec<Vec<u8>>>>;

    struct ScriptedNet {
        rx: RxQueue,
        tx: TxQueue,
        link_up: bool,
    }

    impl ScriptedNet {
        fn new() -> (Self, RxQueue, TxQueue) {
            let rx = Rc::new(RefCell::new(VecDeque::new()));
            let tx = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    rx: Rc::clone(&rx),
                    tx: Rc::clone(&tx),
                    link_up: true,
                },
                rx,
                tx,
            )
        }
    }

    impl NetDevice for ScriptedNet {
        fn send(&mut self, frame: &[u8]) -> Result<(), NetError> {
            if !self.link_up {
                return Err(NetError::LinkDown);
            }
            if frame.len() > 1514 {
                return Err(NetError::FrameTooLarge {
                    size: frame.len(),
                    max: 1514,
                });
            }
            self.tx.borrow_mut().push(frame.to_vec());
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
            let Some(frame) = self.rx.borrow_mut().pop_front() else {
                return Err(NetError::NoBuffer);
            };
            let n = frame.len().min(buf.len());
            buf[..n].copy_from_slice(&frame[..n]);
            Ok(n)
        }
        fn mac_address(&self) -> [u8; 6] {
            [0x02, 0, 0, 0, 0, 0x02]
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

    /// 直接构造 NetStack（测试不走 virtio MMIO 移交路径）。
    fn stack_with(device: ScriptedNet) -> NetStack<ScriptedNet> {
        let mac = device.mac_address();
        NetStack {
            iface: NetworkInterface::new(
                device,
                InterfaceConfig {
                    mac_addr: mac,
                    ipv4_addr: Some(ipv4_cidr(ipv4_addr(10, 0, 2, 15), 24)),
                    gateway: Some(ipv4_addr(10, 0, 2, 2)),
                    dhcp: false,
                },
            ),
        }
    }

    fn net_spec(kind: TransportKind, peer: &str, mtu: usize, recv_timeout: u64) -> TransportSpec {
        let mut spec = TransportSpec {
            kind,
            peer: [0; 128],
            peer_len: 0,
            mtu,
            connect_timeout_ms: 1000,
            recv_timeout_ms: recv_timeout,
        };
        let b = peer.as_bytes();
        let len = b.len().min(spec.peer.len());
        spec.peer[..len].copy_from_slice(&b[..len]);
        spec.peer_len = len as u8;
        spec
    }

    // ========================================================================
    // Tcp 工厂
    // ========================================================================

    #[test]
    fn tcp_factory_supports_kinds() {
        let (dev, _rx, _tx) = ScriptedNet::new();
        let f = NetTransportFactory::new(stack_with(dev));
        assert!(f.supports(TransportKind::Tcp));
        assert!(f.supports(TransportKind::Udp));
        assert!(f.supports(TransportKind::RawEthernet));
        assert!(!f.supports(TransportKind::Serial));
    }

    #[test]
    fn tcp_open_rejects_bad_peer() {
        let (dev, _rx, _tx) = ScriptedNet::new();
        let mut f = NetTransportFactory::new(stack_with(dev));
        // 空 peer / 无法解析的 peer → ConnectFailed（fail-closed，不猜）。
        for bad in ["", "not-an-ip!", "1.2.3.4"] {
            let spec = net_spec(TransportKind::Tcp, bad, 260, 1000);
            assert!(
                matches!(f.open(&spec), Err(TransportError::ConnectFailed)),
                "peer={bad:?}"
            );
        }
    }

    #[test]
    fn tcp_open_unreachable_peer_fails_closed() {
        // 脚本网卡从不回包 → 握手无法完成 → 有界轮询后 ConnectFailed
        //（不无限自旋；负例=不可达 fail-closed，绝非挂死）。
        let (dev, _rx, _tx) = ScriptedNet::new();
        let mut f = NetTransportFactory::new(stack_with(dev));
        let spec = net_spec(TransportKind::Tcp, "10.0.2.2:5502", 260, 1000);
        assert!(matches!(f.open(&spec), Err(TransportError::ConnectFailed)));
    }

    // ========================================================================
    // Udp 工厂
    // ========================================================================

    /// 构造 IPv4+UDP 数据报帧（校验和按 RFC 768/791 计算——smoltcp 校验）。
    fn udp_frame(
        src_ip: [u8; 4],
        src_port: u16,
        dst_ip: [u8; 4],
        dst_port: u16,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // dst
        frame.extend_from_slice(&[0x02, 0, 0, 0, 0, 0x02]); // src
        frame.extend_from_slice(&0x0800u16.to_be_bytes()); // IPv4
                                                           // ---- IPv4 header（20B）----
        let total_len = 20 + 8 + payload.len();
        let mut ip = Vec::new();
        ip.push(0x45); // version + IHL
        ip.push(0); // DSCP/ECN
        ip.extend_from_slice(&(total_len as u16).to_be_bytes());
        ip.extend_from_slice(&0x0000u16.to_be_bytes()); // id
        ip.extend_from_slice(&0x0000u16.to_be_bytes()); // flags/frag
        ip.push(64); // TTL
        ip.push(17); // UDP
        ip.extend_from_slice(&0x0000u16.to_be_bytes()); // checksum（下面算）
        ip.extend_from_slice(&src_ip);
        ip.extend_from_slice(&dst_ip);
        let ip_sum = checksum(&ip);
        ip[10] = (ip_sum >> 8) as u8;
        ip[11] = (ip_sum & 0xff) as u8;
        frame.extend_from_slice(&ip);
        // ---- UDP header（8B）----
        let mut udp = Vec::new();
        udp.extend_from_slice(&src_port.to_be_bytes());
        udp.extend_from_slice(&dst_port.to_be_bytes());
        udp.extend_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
        udp.extend_from_slice(&0x0000u16.to_be_bytes()); // checksum（下面算）
                                                         // UDP 校验和 = 伪头 + UDP 头 + 载荷。
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(&src_ip);
        pseudo.extend_from_slice(&dst_ip);
        pseudo.push(0);
        pseudo.push(17);
        pseudo.extend_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
        let mut udp_full = udp.clone();
        udp_full.extend_from_slice(payload);
        let mut check_input = pseudo;
        check_input.extend_from_slice(&udp_full);
        let udp_sum = checksum(&check_input);
        udp[6] = (udp_sum >> 8) as u8;
        udp[7] = (udp_sum & 0xff) as u8;
        frame.extend_from_slice(&udp);
        frame.extend_from_slice(payload);
        frame
    }

    /// 16 位二进制反码和（RFC 1071）。
    fn checksum(data: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        let mut i = 0;
        while i + 1 < data.len() {
            sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            i += 2;
        }
        if i < data.len() {
            sum += (data[i] as u32) << 8;
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    #[test]
    fn udp_open_send_recv_roundtrip() {
        let (dev, rx, _tx) = ScriptedNet::new();
        let mut f = NetTransportFactory::new(stack_with(dev));
        let mut handle = f
            .open(&net_spec(TransportKind::Udp, "10.0.2.2:9999", 1500, 500))
            .expect("udp open");
        // open 已绑定确定性本地端口（peer 派生）。
        let local_port = handle
            .as_any_mut()
            .downcast_mut::<UdpStreamHandle<ScriptedNet>>()
            .expect("downcast udp")
            .local_port;
        assert_eq!(local_port, 40000 + 14); // sum(10,0,2,2)=14
                                            // send：enqueue 成功（对端可达性由 ARP/路由仲裁，不在此断言）。
        assert!(handle.send(b"ping").is_ok());
        // 注入对端回包（IPv4+UDP 帧，合法校验和）→ poll 后 recv。
        rx.borrow_mut().push_back(udp_frame(
            [10, 0, 2, 2],
            9999,
            [10, 0, 2, 15],
            local_port,
            b"pong",
        ));
        let mut buf = [0u8; 64];
        let n = handle.recv(&mut buf).expect("udp recv");
        assert_eq!(&buf[..n], b"pong");
    }

    #[test]
    fn udp_recv_timeout_when_silent() {
        let (dev, _rx, _tx) = ScriptedNet::new();
        let mut f = NetTransportFactory::new(stack_with(dev));
        let mut handle = f
            .open(&net_spec(TransportKind::Udp, "10.0.2.2:9999", 1500, 3))
            .expect("udp open");
        let mut buf = [0u8; 64];
        assert!(matches!(
            handle.recv(&mut buf),
            Err(TransportError::Timeout)
        ));
    }

    // ========================================================================
    // RawEthernet 工厂
    // ========================================================================

    #[test]
    fn raw_open_rejects_bad_mac_and_mtu() {
        let (dev, _rx, _tx) = ScriptedNet::new();
        let mut f = NetTransportFactory::new(stack_with(dev));
        let spec = net_spec(TransportKind::RawEthernet, "zz", 1500, 1000);
        assert!(matches!(f.open(&spec), Err(TransportError::ConnectFailed)));
        let spec = net_spec(TransportKind::RawEthernet, "010ccd040001", 2000, 1000);
        assert!(matches!(f.open(&spec), Err(TransportError::ConnectFailed)));
    }

    #[test]
    fn raw_roundtrip_vlan_and_plain_frames() {
        use eneros_net::eth_frame::{encode_vlan, header_info, net_ether_type};
        let (dev, rx, tx) = ScriptedNet::new();
        let mut f = NetTransportFactory::new(stack_with(dev));
        let mut handle = f
            .open(&net_spec(
                TransportKind::RawEthernet,
                "010ccd040001",
                1500,
                500,
            ))
            .expect("raw open");
        // 正例：VLAN 标记 SV 帧（0x88BA，VID=0x2a）→ 收发一致（工厂直通）。
        let mut sv = Vec::new();
        sv.extend_from_slice(&[0x01, 0x0c, 0xcd, 0x04, 0x00, 0x01]); // dst
        sv.extend_from_slice(&[0x02, 0, 0, 0, 0, 0x02]); // src
        sv.extend_from_slice(&0x88BAu16.to_be_bytes());
        sv.extend_from_slice(&0x4001u16.to_be_bytes()); // APPID
        let tagged = encode_vlan(&sv, 0x2a).expect("vlan");
        assert_eq!(header_info(&tagged).unwrap().0, 16);
        assert_eq!(net_ether_type(&tagged), Some(0x88BA));
        handle.send(&tagged).expect("raw send");
        assert_eq!(tx.borrow().len(), 1, "发送帧必须原样入设备（含 VLAN）");
        assert_eq!(tx.borrow()[0], tagged);
        // 回环：注入→recv。
        rx.borrow_mut().push_back(tagged.clone());
        let mut buf = [0u8; 128];
        let n = handle.recv(&mut buf).expect("raw recv");
        assert_eq!(&buf[..n], &tagged[..]);
    }

    #[test]
    fn raw_send_rejects_oversized_and_empty() {
        let (dev, _rx, _tx) = ScriptedNet::new();
        let mut f = NetTransportFactory::new(stack_with(dev));
        let mut handle = f
            .open(&net_spec(
                TransportKind::RawEthernet,
                "010ccd040001",
                100,
                500,
            ))
            .expect("raw open");
        // 超 MTU / 空帧 → Io（fail-closed，不把坏帧交给设备）。
        assert!(matches!(handle.send(&[0u8; 101]), Err(TransportError::Io)));
        assert!(matches!(handle.send(&[]), Err(TransportError::Io)));
        // 边界：恰 MTU → 通过。
        assert!(handle.send(&[0u8; 100]).is_ok());
    }

    #[test]
    fn raw_recv_timeout_when_silent() {
        let (dev, _rx, _tx) = ScriptedNet::new();
        let mut f = NetTransportFactory::new(stack_with(dev));
        let mut handle = f
            .open(&net_spec(
                TransportKind::RawEthernet,
                "010ccd040001",
                1500,
                3,
            ))
            .expect("raw open");
        let mut buf = [0u8; 64];
        assert!(matches!(
            handle.recv(&mut buf),
            Err(TransportError::Timeout)
        ));
    }

    // ========================================================================
    // Serial 工厂
    // ========================================================================

    use eneros_rs485::mock::MockUartHw;

    #[test]
    fn serial_factory_supports_serial_only() {
        let f = SerialTransportFactory::new(MockUartHw::new(), 9600);
        assert!(f.supports(TransportKind::Serial));
        assert!(!f.supports(TransportKind::Tcp));
        assert!(!f.supports(TransportKind::Udp));
        assert!(!f.supports(TransportKind::RawEthernet));
    }

    #[test]
    fn serial_open_rejects_other_kinds() {
        let mut f = SerialTransportFactory::new(MockUartHw::new(), 9600);
        let spec = net_spec(TransportKind::Tcp, "10.0.2.2:5502", 260, 1000);
        assert!(matches!(f.open(&spec), Err(TransportError::Unsupported)));
    }

    #[test]
    fn serial_send_recv_roundtrip() {
        // MockUartHw 环回：预填一帧（RTU 成帧在 transport 内完成——
        // 字节间距 < 3.5 字符间隔时同帧；收尾 flush 以 3.5 间隔结帧）。
        let mut uart = MockUartHw::new();
        uart.set_time_step_ns(1_000_000); // 每 now_ns 推进 1ms（默认）
        let frame: [u8; 9] = [0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A, 0x00];
        uart.push_rx_slice(&frame);
        let mut f = SerialTransportFactory::new(uart, 9600);
        let mut handle = f.open(&serial_spec(1000)).expect("serial open");
        // 发送：字节入驱动 written 记录（0x03 读保持寄存器请求）。
        assert!(handle.send(b"\x01\x03\x00\x00\x00\x01\x84\x0a").is_ok());
        // 接收：mock 预填帧（流式 flush）→ 成帧返回。
        let mut buf = [0u8; 32];
        let n = handle.recv(&mut buf).expect("serial recv");
        assert_eq!(&buf[..n], &frame[..]);
    }

    #[test]
    fn serial_recv_timeout_when_silent() {
        let mut f = SerialTransportFactory::new(MockUartHw::new(), 9600);
        let mut handle = f.open(&serial_spec(10)).expect("serial open");
        let mut buf = [0u8; 32];
        assert!(matches!(
            handle.recv(&mut buf),
            Err(TransportError::Timeout)
        ));
    }

    #[test]
    fn serial_recv_drops_oversized_prefix_and_returns_tail() {
        // 300 字节超长帧（> RX_MAX 256）：成帧层在 256 处溢出丢弃并重启
        // 缓冲——recv 只返回重启后的尾部（44 字节），绝不把整套超长帧
        // 当有效载荷（fail-closed 负例；溢出帧被丢弃而非透传）。
        let mut uart = MockUartHw::new();
        let junk: Vec<u8> = vec![0xAA; 300];
        uart.push_rx_slice(&junk);
        let mut f = SerialTransportFactory::new(uart, 9600);
        let mut handle = f.open(&serial_spec(1000)).expect("serial open");
        let mut buf = [0u8; 512];
        let n = handle.recv(&mut buf).expect("serial recv");
        assert_eq!(
            n, 44,
            "300 字节超长帧：前缀 256 已丢弃，只剩重启尾部 44 字节"
        );
        assert!(buf[..n].iter().all(|b| *b == 0xAA));
    }

    fn serial_spec(recv_timeout_ms: u64) -> TransportSpec {
        TransportSpec {
            kind: TransportKind::Serial,
            peer: [0; 128],
            peer_len: 0,
            mtu: 256,
            connect_timeout_ms: 1000,
            recv_timeout_ms,
        }
    }

    // ========================================================================
    // 蓝图 70.4 WP-70.4.1：socket 所有权显式状态机（N8/N9 修复）
    // ========================================================================

    use core::cell::RefCell as TcRefCell;

    /// 直构造持有合法 socket 的 TCP 句柄（不经握手——所有权语义与连接
    /// 成败正交；真实握手恢复由 vol4 gate 目标窗承载）。
    fn handle_with_live_socket(
        stack: &Rc<TcRefCell<NetStack<ScriptedNet>>>,
    ) -> TcpStreamHandle<ScriptedNet> {
        let id = stack
            .borrow_mut()
            .iface
            .sockets
            .add_tcp(SOCK_RX_BUF, SOCK_TX_BUF);
        TcpStreamHandle {
            stack: Rc::clone(stack),
            id,
            recv_timeout_ms: 1,
            connected: true,
        }
    }

    fn socket_count(stack: &Rc<TcRefCell<NetStack<ScriptedNet>>>) -> usize {
        stack.borrow().iface.sockets.inner.iter().count()
    }

    /// N9 修复：close 恒幂等；close 后观察/收发走显式 Disconnected 事实
    /// 源（红 = 旧实现以陈旧 id 读栈——is_connected 可能仍真、send 触碰
    /// 已移除槽位）。
    #[test]
    fn close_is_idempotent_and_observers_report_disconnected() {
        let (dev, _rx, _tx) = ScriptedNet::new();
        let stack = Rc::new(TcRefCell::new(stack_with(dev)));
        let mut h = handle_with_live_socket(&stack);
        assert_eq!(socket_count(&stack), 1);
        assert!(h.close().is_ok());
        assert_eq!(socket_count(&stack), 0, "close 必须释放 socket");
        // 二次 close：幂等成功、零 PANIC、socket 计数不变。
        assert!(h.close().is_ok());
        assert_eq!(socket_count(&stack), 0);
        // close 后观察/收发 = Disconnected。
        assert!(!h.is_connected(), "close 后 is_connected 必须为 false");
        assert!(matches!(h.send(b"x"), Err(TransportError::Closed)));
        let mut buf = [0u8; 4];
        assert!(matches!(h.recv(&mut buf), Err(TransportError::Closed)));
    }

    /// N8 修复：重连失败后句柄停留 Disconnected（可安全观察与重试），
    /// 不再以陈旧 id 触碰后来复用的槽位；连续失败重连零 socket 泄漏、
    /// 零 PANIC（红 = 旧实现失败路径后 send 直接命中已移除 id）。
    #[test]
    fn reconnect_failure_leaves_disconnected_and_no_socket_leak() {
        let (dev, _rx, _tx) = ScriptedNet::new();
        let stack = Rc::new(TcRefCell::new(stack_with(dev)));
        let mut h = handle_with_live_socket(&stack);
        // 首次重连（无监听者）：旧 socket 释放 → tcp_connect 有界轮询后
        // ConnectFailed（其本次 socket 亦释放）。
        assert!(matches!(
            h.reconnect("10.0.2.2:5502"),
            Err(TransportError::ConnectFailed)
        ));
        assert!(!h.is_connected(), "失败后必须 Disconnected（N8 事实源）");
        assert!(matches!(h.send(b"x"), Err(TransportError::Closed)));
        let mut buf = [0u8; 4];
        assert!(matches!(h.recv(&mut buf), Err(TransportError::Closed)));
        let n0 = socket_count(&stack);
        // 连续多次失败重连：状态保持、socket 数量稳定（零增长）。
        for _ in 0..3 {
            assert!(matches!(
                h.reconnect("10.0.2.2:5502"),
                Err(TransportError::ConnectFailed)
            ));
            assert!(!h.is_connected());
        }
        assert_eq!(socket_count(&stack), n0, "失败重连不得泄漏 socket");
        // 失败重连后的 close：幂等成功（不得再移除/触碰他人槽位）。
        assert!(h.close().is_ok());
        assert_eq!(socket_count(&stack), n0);
        assert!(!h.is_connected());
    }
}
