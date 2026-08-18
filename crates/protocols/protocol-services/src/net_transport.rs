//! 协议服务网络传输（蓝图 43 L2 协议互操作面）。
//!
//! root 把 virtio-net 槽位（MMIO + 9 页连续 DMA 帧）移交给协议服务
//! （X3=MMIO vaddr, X4=DMA vaddr, X5=DMA phys，与 svc-storage 同机制）；
//! 服务侧在此构造：
//!
//! ```text
//! VirtioNetDriver（VolatileMmio + NetSvcDmaPool + NoopCache）
//!   → NetworkInterface（smoltcp，静态 IP 10.0.2.15/24）
//!   → SmoltcpTcpTransport（modbus-tcp 生产 TCP 传输）
//!   → TransportFactory/TransportHandle（protocol-abstract 唯一 seam）
//! ```
//!
//! QEMU user-mode（slirp）网络事实：guest 10.0.2.0/24，网关 10.0.2.2 =
//! host loopback 别名——L2 互操作测试 peer（10.0.2.2:5502）。生产 peer
//! 应来自 config-bundle（41/43 后续接线，未接通前以能力壳 fail-closed）。

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::cell::RefCell;

use eneros_modbus_tcp::device::TcpDevice;
use eneros_modbus_tcp::transport::{SmoltcpTcpTransport, TcpTransport};
use eneros_net::dma_mem::NoopCache;
use eneros_net::mac::NetDevice;
use eneros_net::tcpip::{ipv4_addr, ipv4_cidr, InterfaceConfig};
use eneros_net::virtio_net::VirtioNetDriver;
use eneros_protocol_abstract::{
    parse_ip_port, TransportError, TransportFactory, TransportHandle, TransportKind, TransportSpec,
};
use eneros_storage::driver::virtio_blk::VolatileMmio;
use eneros_virtio_core::DmaPool;

/// DMA 池页数（virtio-net 队列下限 = VIRTIO_NET_POOL_SIZE/4K；与 root
/// spawn.rs `NET_DMA_PAGES` 一致——单一事实源）。
pub const NET_DMA_PAGES: usize = 9;

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
/// smoltcp TCP 传输（modbus-tcp 生产实现）。
pub type NetTransport = SmoltcpTcpTransport<NetDriver>;

/// 构建网络栈（root 移交参数；任一为 0 → Err → 服务保持无传输
/// fail-closed，不伪造协议 I/O）。
pub fn build_net_transport(
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
) -> Result<NetTransport, &'static str> {
    if mmio_vaddr == 0 || dma_vaddr == 0 || dma_phys == 0 {
        return Err("device-handover-missing");
    }
    // SAFETY: root 已把 DMA 帧映射到 dma_vaddr（长度 = NET_DMA_PAGES×4K）。
    let pool = unsafe { NetSvcDmaPool::new(dma_vaddr, dma_phys, NET_DMA_PAGES) };
    let mmio = VolatileMmio::new(mmio_vaddr as usize);
    let driver =
        VirtioNetDriver::new(mmio, pool, NoopCache).map_err(|_| "virtio-net-init-failed")?;
    // mac 来自设备能力（未协商 MAC 时驱动拒绝构造——不猜）。
    let mac = driver.mac_address();
    let config = InterfaceConfig {
        mac_addr: mac,
        ipv4_addr: Some(ipv4_cidr(ipv4_addr(10, 0, 2, 15), 24)),
        gateway: Some(ipv4_addr(10, 0, 2, 2)),
        dhcp: false,
    };
    Ok(SmoltcpTcpTransport::new(driver, config))
}

/// 传输工厂：把 smoltcp TCP 传输暴露为 `TransportFactory`（protocol-
/// abstract 唯一 seam）。连接在 `open` 完成（三次握手轮询），后续
/// `TransportHandle` 与工厂共享同一传输实例。
pub struct NetTransportFactory {
    transport: Rc<RefCell<NetTransport>>,
}

impl NetTransportFactory {
    pub fn new(transport: NetTransport) -> Self {
        Self {
            transport: Rc::new(RefCell::new(transport)),
        }
    }
}

impl TransportFactory for NetTransportFactory {
    fn supports(&self, kind: TransportKind) -> bool {
        kind == TransportKind::Tcp
    }

    fn open(&mut self, spec: &TransportSpec) -> Result<Box<dyn TransportHandle>, TransportError> {
        if spec.kind != TransportKind::Tcp {
            return Err(TransportError::Unsupported);
        }
        let peer = core::str::from_utf8(&spec.peer[..spec.peer_len as usize])
            .map_err(|_| TransportError::ConnectFailed)?;
        let (ip, port) = parse_ip_port(peer).ok_or(TransportError::ConnectFailed)?;
        let device = TcpDevice {
            ip,
            port,
            unit_id: 0,
            timeout_ms: spec.recv_timeout_ms as u32,
        };
        {
            let mut t = self.transport.borrow_mut();
            t.connect(&device)
                .map_err(|_| TransportError::ConnectFailed)?;
        }
        Ok(Box::new(HandleAdapter {
            transport: self.transport.clone(),
            device,
        }))
    }
}

/// `TransportHandle` 实现（经共享传输收发；连接与握手在 `open` 完成）。
struct HandleAdapter {
    transport: Rc<RefCell<NetTransport>>,
    device: TcpDevice,
}

impl TransportHandle for HandleAdapter {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        let mut t = self.transport.borrow_mut();
        t.send(data).map_err(|_| TransportError::Io)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let mut t = self.transport.borrow_mut();
        let data = t
            .recv(self.device.timeout_ms)
            .map_err(|_| TransportError::Timeout)?;
        let n = core::cmp::min(data.len(), buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }

    fn close(&mut self) -> Result<(), TransportError> {
        // smoltcp 连接复用于主站轮询（open 即建连）；关闭为 no-op——
        // 换设备由上层重新 open（PTC-002 语义）。
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.transport.borrow().is_connected(&self.device)
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}
