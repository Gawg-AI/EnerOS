//! virtio-net 驱动（DRV-008）：RX/TX/MTU/link/reset。
//!
//! 双队列（0=RX，1=TX）。RX 预投递缓冲（12B virtio-net header + MTU
//! 帧）；TX 提交 header+帧。link status 与 MTU 来自配置区（STATUS/
//! MTU 特性）。

use core::ptr;

use eneros_libeneros::dma_alloc;

use crate::mmio::barrier;
use crate::virtio_mmio::{
    probe, VirtioMmio, VirtQueue, DEVICE_ID_NET,
};

/// virtio-net 特性位。
pub const VIRTIO_NET_F_CSUM: u32 = 1 << 0;
pub const VIRTIO_NET_F_GUEST_CSUM: u32 = 1 << 1;
pub const VIRTIO_NET_F_MTU: u32 = 1 << 3;
pub const VIRTIO_NET_F_MAC: u32 = 1 << 5;
pub const VIRTIO_NET_F_STATUS: u32 = 1 << 16;
pub const VIRTIO_NET_F_MRG_RXBUF: u32 = 1 << 15;

/// 配置区偏移。
pub const CONFIG_MAC: usize = 0x00;
pub const CONFIG_STATUS: usize = 0x06;
pub const CONFIG_MAX_VIRTQ_PAIRS: usize = 0x08;
pub const CONFIG_MTU: usize = 0x0A;

/// link 状态位（config.status）。
pub const VIRTIO_NET_S_LINK_UP: u16 = 1;

/// virtio-net header（12B，无 MRG_RXBUF）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
    pub num_buffers: u16,
}

/// 默认 MTU（无 MTU 特性时）。
pub const DEFAULT_MTU: usize = 1500;
/// RX 缓冲大小（header + MTU + 余量）。
pub const RX_BUF_SIZE: usize = 2048;

/// virtio-net 设备。
pub struct VirtioNet {
    pub dev: VirtioMmio,
    pub rxq: VirtQueue,
    pub txq: VirtQueue,
    pub mac: [u8; 6],
    pub mtu: usize,
    pub link_up: bool,
    /// RX 缓冲（预投递）。
    pub rx_va: usize,
    pub rx_phys: u64,
    /// TX 缓冲。
    pub tx_va: usize,
    pub tx_phys: u64,
}

pub fn init(base_phys: usize) -> Result<VirtioNet, &'static str> {
    let mut dev = probe(base_phys)?;
    if dev.device_id != DEVICE_ID_NET {
        return Err("not a net device");
    }
    let features = dev.device_features();
    let accepted = features
        & (VIRTIO_NET_F_MTU | VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS);
    dev.set_driver_features(accepted)?;
    let has_mtu = features & VIRTIO_NET_F_MTU != 0;
    let has_status = features & VIRTIO_NET_F_STATUS != 0;
    let rxq = VirtQueue::setup(&mut dev, 0, 64)?;
    let txq = VirtQueue::setup(&mut dev, 1, 64)?;
    dev.driver_ok()?;
    let mut mac = [0u8; 6];
    // SAFETY：配置区映射读（6 字节）。
    unsafe {
        ptr::copy_nonoverlapping(
            (dev.regs_va + crate::virtio_mmio::REG_CONFIG + CONFIG_MAC) as *const u8,
            mac.as_mut_ptr(),
            6,
        );
    }
    let mtu = if has_mtu {
        dev.config_read16(CONFIG_MTU) as usize
    } else {
        DEFAULT_MTU
    };
    let status = if has_status {
        dev.config_read16(CONFIG_STATUS)
    } else {
        VIRTIO_NET_S_LINK_UP
    };
    let (status_alloc, rx_va, rx_phys) = dma_alloc(RX_BUF_SIZE as u64);
    if status_alloc != 0 {
        return Err("dma_alloc rx failed");
    }
    let (status_alloc, tx_va, tx_phys) = dma_alloc(RX_BUF_SIZE as u64);
    if status_alloc != 0 {
        return Err("dma_alloc tx failed");
    }
    let rx_va = rx_va as usize;
    let tx_va = tx_va as usize;
    let mut net = VirtioNet {
        dev,
        rxq,
        txq,
        mac,
        mtu,
        link_up: status & VIRTIO_NET_S_LINK_UP != 0,
        rx_va,
        rx_phys,
        tx_va,
        tx_phys,
    };
    // 预投递 RX 缓冲。
    net.refill_rx()?;
    Ok(net)
}

impl VirtioNet {
    /// 预投递一个 RX 缓冲。
    pub fn refill_rx(&mut self) -> Result<(), &'static str> {
        let chain = [(self.rx_va, RX_BUF_SIZE as u32, true)];
        let head = self.rxq.alloc_chain(&chain)?;
        self.rxq.submit(head);
        barrier();
        self.dev.notify(0);
        Ok(())
    }

    /// 轮询 RX：收到帧返回长度（>0），否则 0。
    pub fn poll_rx(&mut self) -> usize {
        if let Some((_id, len)) = self.rxq.pop_used() {
            let frame_len = len.saturating_sub(core::mem::size_of::<NetHdr>() as u32);
            // 重新投递。
            let _ = self.refill_rx();
            frame_len as usize
        } else {
            0
        }
    }

    /// TX：发送帧（data[..len]）。返回 0=OK。
    pub fn transmit(&mut self, data: &[u8]) -> Result<(), ()> {
        if data.len() + core::mem::size_of::<NetHdr>() > RX_BUF_SIZE {
            return Err(());
        }
        // 写 header（无 offload）。
        // SAFETY：TX 缓冲已映射。
        unsafe {
            let header = self.tx_va as *mut NetHdr;
            ptr::write_volatile(
                header,
                NetHdr {
                    flags: 0,
                    gso_type: 0,
                    hdr_len: 0,
                    gso_size: 0,
                    csum_start: 0,
                    csum_offset: 0,
                    num_buffers: 1,
                },
            );
            ptr::copy_nonoverlapping(
                data.as_ptr(),
                (self.tx_va + core::mem::size_of::<NetHdr>()) as *mut u8,
                data.len(),
            );
        }
        let len = data.len() as u32 + core::mem::size_of::<NetHdr>() as u32;
        let chain = [(self.tx_va, len, false)];
        let head = self.txq.alloc_chain(&chain).map_err(|_| ())?;
        self.txq.submit(head);
        barrier();
        self.dev.notify(1);
        // 等待 TX 完成。
        let mut spins = 0u32;
        loop {
            if self.txq.pop_used().is_some() {
                break;
            }
            spins += 1;
            if spins > 2_000_000 {
                return Err(());
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    /// 读 link 状态（配置区）。
    pub fn read_link(&self) -> bool {
        let status = self.dev.config_read16(CONFIG_STATUS);
        status & VIRTIO_NET_S_LINK_UP != 0
    }

    /// 设备复位（DRV-008 reset）。
    pub fn reset_device(&mut self) -> Result<(), &'static str> {
        self.dev.reset();
        let features = self.dev.device_features();
        let accepted = features
            & (VIRTIO_NET_F_MTU | VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS);
        self.dev.set_driver_features(accepted)?;
        self.rxq = VirtQueue::setup(&mut self.dev, 0, 64)?;
        self.txq = VirtQueue::setup(&mut self.dev, 1, 64)?;
        self.dev.driver_ok()?;
        self.refill_rx()
    }
}
