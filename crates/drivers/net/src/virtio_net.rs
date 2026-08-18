//! virtio-net 驱动（蓝图 39 WP-39.7）。
//!
//! QEMU virt 的 virtio-net 通过 virtio-mmio 暴露（device_id=1，legacy
//! transport——与 `eneros-storage` 的 virtio-blk 同一机制：QUEUE_PFN +
//! desc@+0/avail@+0x100/used@+0x1000 布局）。本驱动实现 [`NetDevice`]
//! trait：feature 协商（只开 MAC/STATUS）、RX 预置描述符（8 × 2048 B）、
//! TX 单缓冲提交/等待、MAC 与 link 状态读 config space（0x100 起）。
//!
//! # 不变量（fail-closed）
//!
//! - 未协商 `VIRTIO_NET_F_MAC` → 拒绝（不猜 MAC 地址）；
//! - 帧长 > [`TX_BUFFER_SIZE`] → `FrameTooLarge`（不截断发送）；
//! - used ring id 越界 → `RingCorrupt`（设备行为异常，显式错误）；
//! - link 状态来自设备 STATUS 寄存器（`link up ≠ IP ready`——上层判据）。

use core::sync::atomic::{compiler_fence, Ordering};

use eneros_virtio_core::{
    DmaPool, VirtioMmio, VirtqAvail, VirtqDesc, VirtqUsed, QUEUE_MASK, QUEUE_MAX,
    VIRTIO_MMIO_MAGIC, VIRTIO_MMIO_VERSION_1, VIRTIO_MMIO_VERSION_2, VIRTIO_STATUS_ACKNOWLEDGE,
    VIRTIO_STATUS_DRIVER, VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK, VIRTQ_DESC_F_WRITE,
    VMMIO_DEVICE_FEATURES, VMMIO_DEVICE_FEATURES_SEL, VMMIO_DEVICE_ID, VMMIO_DRIVER_FEATURES,
    VMMIO_DRIVER_FEATURES_SEL, VMMIO_GUEST_PAGE_SIZE, VMMIO_MAGIC_VALUE, VMMIO_QUEUE_ALIGN,
    VMMIO_QUEUE_DESC_HIGH, VMMIO_QUEUE_DESC_LOW, VMMIO_QUEUE_DEVICE_HIGH, VMMIO_QUEUE_DEVICE_LOW,
    VMMIO_QUEUE_DRIVER_HIGH, VMMIO_QUEUE_DRIVER_LOW, VMMIO_QUEUE_NOTIFY, VMMIO_QUEUE_NUM,
    VMMIO_QUEUE_NUM_MAX, VMMIO_QUEUE_PFN, VMMIO_QUEUE_READY, VMMIO_QUEUE_SEL, VMMIO_STATUS,
    VMMIO_VERSION,
};

use crate::dma_mem::ArmCache;
use crate::error::NetError;
use crate::error::NetStats;
use crate::mac::NetDevice;

/// virtio-net device id。
pub const VIRTIO_NET_DEVICE_ID: u32 = 1;
/// 设备 feature：MAC 地址可配置（bit 5）。
pub const VIRTIO_NET_F_MAC: u64 = 1 << 5;
/// 设备 feature：link status 可读（bit 16）。
pub const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
/// 设备 feature：设备给出 MTU（bit 3；39 收尾复验 F7——MTU 来自
/// capability 而非常量）。
pub const VIRTIO_NET_F_MTU: u64 = 1 << 3;

/// 无 F_MTU 能力时的以太网协议默认值（**非设备事实**——仅协议层
/// 兜底，文档语义；mtu() 返回值同时受 TX/RX 缓冲硬上限约束）。
pub const ETHERNET_DEFAULT_MTU: usize = 1500;

/// 设备状态寄存器：link up。
pub const VIRTIO_NET_S_LINK_UP: u16 = 1;

/// legacy virtio-net RX 头（virtio_net_hdr，10 字节）——legacy 设备
/// 每个 RX 缓冲以该头开头，驱动必须跳过（QEMU 实测帧 = 头 + 以太帧）。
pub const VIRTIO_NET_HDR_SIZE: usize = 10;

/// RX 缓冲大小（≥ virtio_net_hdr 10 + MTU 1514 + 裕量）。
pub const RX_BUFFER_SIZE: usize = 2048;
/// TX 缓冲大小（同 RX）。
pub const TX_BUFFER_SIZE: usize = 2048;
/// 预置 RX 描述符数（8 个在飞接收）。
pub const RX_SLOTS: usize = 8;

/// 池布局（legacy，9 页）：
/// - 0x0000: RX desc 表；0x0100: RX avail；0x1000: RX used；
/// - 0x2000: TX desc 表；0x2100: TX avail；0x3000: TX used；
/// - 0x4000: RX 缓冲（8 × 2048）；0x8000: TX 缓冲（2048）。
const RX_DESC_OFF: usize = 0x0000;
const RX_AVAIL_OFF: usize = 0x0100;
const RX_USED_OFF: usize = 0x1000;
const TX_DESC_OFF: usize = 0x2000;
const TX_AVAIL_OFF: usize = 0x2100;
const TX_USED_OFF: usize = 0x3000;
const RX_BUF_OFF: usize = 0x4000;
const TX_BUF_OFF: usize = 0x8000;
/// 池总大小（9 页 = 36 KiB）。
pub const VIRTIO_NET_POOL_SIZE: usize = 9 * 4096;

/// virtio-net 驱动（实现 [`NetDevice`]）。
pub struct VirtioNetDriver<R: VirtioMmio, P: DmaPool, C: ArmCache> {
    mmio: core::cell::RefCell<R>,
    pool: core::cell::RefCell<P>,
    cache: C,
    /// 协商后的 MAC（config space；F_MAC 未协商 → 构造失败）。
    mac: core::cell::Cell<[u8; 6]>,
    /// MTU（F_MTU 协商成功 → 设备 config 真值；否则协议默认）。
    mtu: core::cell::Cell<usize>,
    /// 设备是否初始化完成（DRIVER_OK 已写）。
    initialized: bool,
    /// legacy transport（QEMU virtio-mmio = legacy，与 virtio-blk 一致）。
    legacy: core::cell::Cell<bool>,
    /// RX 描述符分配游标。
    rx_desc_head: core::cell::Cell<u16>,
    /// RX used ring 消费游标。
    rx_used_next: core::cell::Cell<u16>,
    /// TX used ring 消费游标。
    tx_used_next: core::cell::Cell<u16>,
    /// 统计。
    stats: core::cell::RefCell<NetStats>,
}

impl<R: VirtioMmio, P: DmaPool, C: ArmCache> VirtioNetDriver<R, P, C> {
    /// 创建设备：初始化 + feature 协商 + 双队列 setup + 预置 RX。
    ///
    /// `pool` 必须 ≥ [`VIRTIO_NET_POOL_SIZE`]。
    pub fn new(mmio: R, pool: P, cache: C) -> Result<Self, NetError> {
        if pool.len() < VIRTIO_NET_POOL_SIZE {
            return Err(NetError::NoBuffer);
        }
        let mut drv = Self {
            mmio: core::cell::RefCell::new(mmio),
            pool: core::cell::RefCell::new(pool),
            cache,
            mac: core::cell::Cell::new([0; 6]),
            mtu: core::cell::Cell::new(ETHERNET_DEFAULT_MTU),
            initialized: false,
            legacy: core::cell::Cell::new(true),
            rx_desc_head: core::cell::Cell::new(0),
            rx_used_next: core::cell::Cell::new(0),
            tx_used_next: core::cell::Cell::new(0),
            stats: core::cell::RefCell::new(NetStats::new()),
        };
        drv.device_init()?;
        drv.queue_setup(RX_DESC_OFF / 0x1000, 0)?;
        drv.queue_setup(TX_DESC_OFF / 0x1000, 1)?;
        drv.post_rx_buffers()?;
        // Wire the device model (RAM simulation) after queue setup.
        let pool_ptr = drv.pool_ptr();
        drv.mmio.borrow().post_init(pool_ptr);
        drv.initialized = true;
        Ok(drv)
    }

    /// 设备发现 + feature 协商 + DRIVER_OK（与 virtio-blk 同构）。
    fn device_init(&self) -> Result<(), NetError> {
        let mut mmio = self.mmio.borrow_mut();
        if mmio.read_reg(VMMIO_MAGIC_VALUE) != VIRTIO_MMIO_MAGIC {
            return Err(NetError::NotInitialized);
        }
        let version = mmio.read_reg(VMMIO_VERSION);
        if version != VIRTIO_MMIO_VERSION_1 && version != VIRTIO_MMIO_VERSION_2 {
            return Err(NetError::NotInitialized);
        }
        self.legacy.set(version == VIRTIO_MMIO_VERSION_1);
        // 39 收尾复验（F9）：非 legacy（modern）transport 的队列布局
        // 与本驱动的 legacy 池布局不符——显式拒绝（fail-closed），
        // 不留误导性死分支。
        if !self.legacy.get() {
            return Err(NetError::NotInitialized);
        }
        if mmio.read_reg(VMMIO_DEVICE_ID) != VIRTIO_NET_DEVICE_ID {
            return Err(NetError::NotInitialized);
        }
        mmio.write_reg(VMMIO_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
        mmio.write_reg(
            VMMIO_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER,
        );
        // 读设备 features（word 0/1）。
        mmio.write_reg(VMMIO_DEVICE_FEATURES_SEL, 0);
        let f0 = mmio.read_reg(VMMIO_DEVICE_FEATURES);
        mmio.write_reg(VMMIO_DEVICE_FEATURES_SEL, 1);
        let f1 = mmio.read_reg(VMMIO_DEVICE_FEATURES);
        let dev_features: u64 = (f0 as u64) | ((f1 as u64) << 32);
        // 只协商已实现功能：MAC + STATUS（+ MTU 若设备提供）；
        // 其余（CSUM/mergeable 等）不启用。
        let implemented = VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS | VIRTIO_NET_F_MTU;
        let driver_features = dev_features & implemented;
        if driver_features & VIRTIO_NET_F_MAC == 0 {
            // 没有 MAC 能力 → 拒绝（不猜地址）。
            return Err(NetError::NotInitialized);
        }
        mmio.write_reg(VMMIO_DRIVER_FEATURES_SEL, 0);
        mmio.write_reg(VMMIO_DRIVER_FEATURES, driver_features as u32);
        mmio.write_reg(VMMIO_DRIVER_FEATURES_SEL, 1);
        mmio.write_reg(VMMIO_DRIVER_FEATURES, (driver_features >> 32) as u32);
        mmio.write_reg(
            VMMIO_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
        );
        let st = mmio.read_reg(VMMIO_STATUS);
        if st & VIRTIO_STATUS_FEATURES_OK == 0 {
            return Err(NetError::NotInitialized);
        }
        // 读 MAC（config space offset 0，6 字节 LE）。
        let lo = mmio.read_reg(0x100) as u64;
        let hi = mmio.read_reg(0x104) as u64;
        let mac_bytes = lo | (hi << 32);
        let mac = [
            (mac_bytes & 0xFF) as u8,
            ((mac_bytes >> 8) & 0xFF) as u8,
            ((mac_bytes >> 16) & 0xFF) as u8,
            ((mac_bytes >> 24) & 0xFF) as u8,
            ((mac_bytes >> 32) & 0xFF) as u8,
            ((mac_bytes >> 40) & 0xFF) as u8,
        ];
        if mac == [0; 6] {
            return Err(NetError::NotInitialized); // 全零 MAC = 设备未配置
        }
        self.mac.set(mac);
        // 39 收尾复验（F7）：MTU 协商成功 → 读设备 config 真值
        //（u16 位于 config 偏移 4，即 0x104 字低 16 位——禁止非对齐
        // 读，与 config_status 同一寄存器字）；未协商 → 协议默认。
        // 0 = 设备未填 → 保持默认（fail-safe，不用 0 当 MTU）。
        if driver_features & VIRTIO_NET_F_MTU != 0 {
            let dev_mtu = (mmio.read_reg(0x104) & 0xFFFF) as usize;
            if dev_mtu > 0 {
                self.mtu
                    .set(dev_mtu.min(TX_BUFFER_SIZE - VIRTIO_NET_HDR_SIZE));
            }
        }
        mmio.write_reg(
            VMMIO_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE
                | VIRTIO_STATUS_DRIVER
                | VIRTIO_STATUS_FEATURES_OK
                | VIRTIO_STATUS_DRIVER_OK,
        );
        Ok(())
    }

    /// 队列 setup（legacy：GUEST_PAGE_SIZE/QUEUE_ALIGN/QUEUE_NUM/QUEUE_PFN）。
    fn queue_setup(&self, page: usize, queue: u32) -> Result<(), NetError> {
        let base_pa = self.pool.borrow().phys_base();
        let mut mmio = self.mmio.borrow_mut();
        mmio.write_reg(VMMIO_QUEUE_SEL, queue);
        let num_max = mmio.read_reg(VMMIO_QUEUE_NUM_MAX);
        if (num_max as usize) < QUEUE_MAX {
            return Err(NetError::NoBuffer);
        }
        if self.legacy.get() {
            mmio.write_reg(VMMIO_GUEST_PAGE_SIZE, 4096);
            mmio.write_reg(VMMIO_QUEUE_ALIGN, 4096);
            mmio.write_reg(VMMIO_QUEUE_NUM, QUEUE_MAX as u32);
            mmio.write_reg(VMMIO_QUEUE_PFN, ((base_pa >> 12) + page as u64) as u32);
        } else {
            let base = base_pa + (page * 0x1000) as u64;
            mmio.write_reg(VMMIO_QUEUE_NUM, QUEUE_MAX as u32);
            mmio.write_reg(VMMIO_QUEUE_DESC_LOW, base as u32);
            mmio.write_reg(VMMIO_QUEUE_DESC_HIGH, (base >> 32) as u32);
            mmio.write_reg(VMMIO_QUEUE_DRIVER_LOW, (base + 0x1000) as u32);
            mmio.write_reg(VMMIO_QUEUE_DRIVER_HIGH, ((base + 0x1000) >> 32) as u32);
            mmio.write_reg(VMMIO_QUEUE_DEVICE_LOW, (base + 0x2000) as u32);
            mmio.write_reg(VMMIO_QUEUE_DEVICE_HIGH, ((base + 0x2000) >> 32) as u32);
            mmio.write_reg(VMMIO_QUEUE_READY, 1);
        }
        Ok(())
    }

    /// pool 内存指针（post_init 给 RAM 模拟设备）。
    fn pool_ptr(&self) -> *mut u8 {
        self.pool.borrow_mut().as_bytes_mut().as_mut_ptr()
    }

    /// 预置 RX 描述符（RX_SLOTS 个 2048B 缓冲，WRITE 标志）+ 一次通知。
    fn post_rx_buffers(&self) -> Result<(), NetError> {
        let base_pa = self.pool.borrow().phys_base();
        {
            let mut pool = self.pool.borrow_mut();
            let bytes = pool.as_bytes_mut();
            // SAFETY: 区间 [0x0000, 0x1000)（desc）与 [0x0100, 0x1100)
            // （avail）互不重叠且布局匹配（virtio-blk 同款先例）。
            let descs = unsafe {
                core::slice::from_raw_parts_mut(
                    bytes[RX_DESC_OFF..RX_DESC_OFF + 0x1000].as_mut_ptr() as *mut VirtqDesc,
                    QUEUE_MAX,
                )
            };
            let avail = unsafe {
                &mut *(bytes[RX_AVAIL_OFF..RX_AVAIL_OFF + 0x1000].as_mut_ptr() as *mut VirtqAvail)
            };
            for slot in 0..RX_SLOTS {
                let head = self.rx_desc_head.get();
                descs[head as usize] = VirtqDesc {
                    addr: base_pa + (RX_BUF_OFF + slot * RX_BUFFER_SIZE) as u64,
                    len: RX_BUFFER_SIZE as u32,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                };
                avail.ring[avail.idx as usize & QUEUE_MASK] = head;
                compiler_fence(Ordering::SeqCst);
                avail.idx = avail.idx.wrapping_add(1);
                self.rx_desc_head.set((head + 1) & QUEUE_MASK as u16);
            }
        }
        self.cache.clean(self.va_of(RX_DESC_OFF), RX_SLOTS * 16);
        // 39 收尾复验（F10）：avail 环（flags/idx/ring 更新）必须一并
        // clean——否则非一致 cache 下设备看不到新投递（与 mac.rs
        // init_dma 同契约）。
        self.cache
            .clean(self.va_of(RX_AVAIL_OFF), core::mem::size_of::<VirtqAvail>());
        self.notify(0)
    }

    /// 通知设备（queue 号）。
    fn notify(&self, queue: u32) -> Result<(), NetError> {
        let mut mmio = self.mmio.borrow_mut();
        mmio.write_reg(VMMIO_QUEUE_SEL, queue);
        compiler_fence(Ordering::SeqCst);
        mmio.write_reg(VMMIO_QUEUE_NOTIFY, queue);
        Ok(())
    }

    /// pool 内偏移的 CPU 虚拟地址（cache 维护用）。
    fn va_of(&self, offset: usize) -> usize {
        self.pool.borrow().as_bytes().as_ptr() as usize + offset
    }

    /// 读 config space 的 link status（STATUS u16 位于 config 偏移 6——
    /// 即 0x104 字的高 16 位；**禁止非对齐读**：设备内存不支持未对齐
    /// 访问，QEMU 实测 0x106 处 u32 读触发 vm fault）。
    fn config_status(&self) -> u16 {
        let mmio = self.mmio.borrow();
        (mmio.read_reg(0x104) >> 16) as u16
    }

    /// 非阻塞检查 RX used ring 是否已有新元素。
    fn rx_pending(&self) -> bool {
        compiler_fence(Ordering::SeqCst);
        self.cache
            .invalidate(self.va_of(RX_USED_OFF), core::mem::size_of::<VirtqUsed>());
        let pool = self.pool.borrow();
        let bytes = pool.as_bytes();
        // SAFETY: used 区间只读视图。
        let used =
            unsafe { &*(bytes[RX_USED_OFF..RX_USED_OFF + 0x1000].as_ptr() as *const VirtqUsed) };
        used.idx != self.rx_used_next.get()
    }

    /// 等待 RX used ring 出现新元素（有界轮询）。
    fn wait_rx_used(&self) -> Result<u32, NetError> {
        let used_va = self.va_of(RX_USED_OFF);
        for _ in 0..20_000_000u32 {
            compiler_fence(Ordering::SeqCst);
            self.cache
                .invalidate(used_va, core::mem::size_of::<VirtqUsed>());
            let pool = self.pool.borrow();
            let bytes = pool.as_bytes();
            // SAFETY: used 区间只读视图。
            let used = unsafe {
                &*(bytes[RX_USED_OFF..RX_USED_OFF + 0x1000].as_ptr() as *const VirtqUsed)
            };
            if used.idx != self.rx_used_next.get() {
                let idx = self.rx_used_next.get() as usize & QUEUE_MASK;
                let elem = used.ring[idx];
                self.rx_used_next
                    .set(self.rx_used_next.get().wrapping_add(1));
                // 39 收尾复验（F8）：RX 只发放过 desc id 0..RX_SLOTS；
                // 越界 id = used ring 损坏（旧阈值 QUEUE_MAX 会让
                // id % RX_SLOTS 别名到在飞缓冲——双投同一缓冲）。
                if (elem.id as usize) >= RX_SLOTS {
                    return Err(NetError::DmaError(0x8000_0001));
                }
                return Ok(elem.len);
            }
        }
        Err(NetError::Timeout)
    }

    /// 等待 TX used ring 出现新元素。
    fn wait_tx_used(&self) -> Result<(), NetError> {
        let used_va = self.va_of(TX_USED_OFF);
        for _ in 0..20_000_000u32 {
            compiler_fence(Ordering::SeqCst);
            self.cache
                .invalidate(used_va, core::mem::size_of::<VirtqUsed>());
            let pool = self.pool.borrow();
            let bytes = pool.as_bytes();
            // SAFETY: used 区间只读视图。
            let used = unsafe {
                &*(bytes[TX_USED_OFF..TX_USED_OFF + 0x1000].as_ptr() as *const VirtqUsed)
            };
            if used.idx != self.tx_used_next.get() {
                let idx = self.tx_used_next.get() as usize & QUEUE_MASK;
                let elem = used.ring[idx];
                self.tx_used_next
                    .set(self.tx_used_next.get().wrapping_add(1));
                if (elem.id as usize) >= QUEUE_MAX {
                    return Err(NetError::DmaError(0x8000_0002));
                }
                return Ok(());
            }
        }
        Err(NetError::Timeout)
    }

    /// 归还一个 RX 缓冲给设备（消费后重投）。
    fn repost_rx_buffer(&self, desc_id: u16) -> Result<(), NetError> {
        let base_pa = self.pool.borrow().phys_base();
        {
            let mut pool = self.pool.borrow_mut();
            let bytes = pool.as_bytes_mut();
            // SAFETY: 同 post_rx_buffers 的区间不变量。
            let descs = unsafe {
                core::slice::from_raw_parts_mut(
                    bytes[RX_DESC_OFF..RX_DESC_OFF + 0x1000].as_mut_ptr() as *mut VirtqDesc,
                    QUEUE_MAX,
                )
            };
            let avail = unsafe {
                &mut *(bytes[RX_AVAIL_OFF..RX_AVAIL_OFF + 0x1000].as_mut_ptr() as *mut VirtqAvail)
            };
            let slot = (desc_id as usize) % RX_SLOTS;
            descs[desc_id as usize] = VirtqDesc {
                addr: base_pa + (RX_BUF_OFF + slot * RX_BUFFER_SIZE) as u64,
                len: RX_BUFFER_SIZE as u32,
                flags: VIRTQ_DESC_F_WRITE,
                next: 0,
            };
            avail.ring[avail.idx as usize & QUEUE_MASK] = desc_id;
            compiler_fence(Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
        }
        self.cache
            .clean(self.va_of(RX_DESC_OFF + desc_id as usize * 16), 16);
        // F10：avail idx 更新同样 clean。
        self.cache
            .clean(self.va_of(RX_AVAIL_OFF), core::mem::size_of::<VirtqAvail>());
        self.notify(0)
    }

    /// 非阻塞接收：有已投递帧 → `Ok(Some(n))`；无 → `Ok(None)`（立即
    /// 返回，不轮询——事件循环轮询用；缓冲始终预置，设备随时可投递）。
    pub fn try_recv(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetError> {
        if !self.initialized {
            return Err(NetError::NotInitialized);
        }
        if !self.rx_pending() {
            return Ok(None);
        }
        self.recv(buf).map(Some)
    }

    /// 测试钩子：通知 RX 队列（让模拟设备把入站帧写入 used ring，
    /// 但驱动不消费）——用于在 recv 前篡改 used ring 验证损坏防御。
    #[cfg(test)]
    pub fn test_notify_rx(&self) -> Result<(), NetError> {
        self.notify(0)
    }
}

impl<R: VirtioMmio, P: DmaPool, C: ArmCache> NetDevice for VirtioNetDriver<R, P, C> {
    fn send(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if !self.initialized {
            return Err(NetError::NotInitialized);
        }
        if frame.is_empty() || frame.len() + VIRTIO_NET_HDR_SIZE > TX_BUFFER_SIZE {
            return Err(NetError::FrameTooLarge {
                size: frame.len(),
                max: TX_BUFFER_SIZE - VIRTIO_NET_HDR_SIZE,
            });
        }
        if !self.link_up() {
            return Err(NetError::LinkDown);
        }
        let base_pa = self.pool.borrow().phys_base();
        {
            let mut pool = self.pool.borrow_mut();
            let bytes = pool.as_bytes_mut();
            // legacy virtio-net：TX 缓冲以 10 字节 virtio_net_hdr 开头
            // （零头 = 无校验和/无分段；QEMU 实测缺头时把帧前 10 字节
            // 当头部吃掉，对端收到截断帧）。
            bytes[TX_BUF_OFF..TX_BUF_OFF + VIRTIO_NET_HDR_SIZE].fill(0);
            bytes[TX_BUF_OFF + VIRTIO_NET_HDR_SIZE..TX_BUF_OFF + VIRTIO_NET_HDR_SIZE + frame.len()]
                .copy_from_slice(frame);
            // SAFETY: TX desc 区间 [0x2000, 0x3000)。单 TX 缓冲 → 恒用 desc 0。
            let descs = unsafe {
                core::slice::from_raw_parts_mut(
                    bytes[TX_DESC_OFF..TX_DESC_OFF + 0x1000].as_mut_ptr() as *mut VirtqDesc,
                    QUEUE_MAX,
                )
            };
            descs[0] = VirtqDesc {
                addr: base_pa + TX_BUF_OFF as u64,
                len: (VIRTIO_NET_HDR_SIZE + frame.len()) as u32,
                flags: 0,
                next: 0,
            };
            // SAFETY: TX avail ring is within the pre-validated MMIO pool; single TX queue and index masked.
            let avail = unsafe {
                &mut *(bytes[TX_AVAIL_OFF..TX_AVAIL_OFF + 0x1000].as_mut_ptr() as *mut VirtqAvail)
            };
            avail.ring[avail.idx as usize & QUEUE_MASK] = 0;
            compiler_fence(Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
        }
        self.cache
            .clean(self.va_of(TX_BUF_OFF), VIRTIO_NET_HDR_SIZE + frame.len());
        // F10：TX desc + avail 环一并 clean（非一致 cache 下设备可见）。
        self.cache.clean(self.va_of(TX_DESC_OFF), 16);
        self.cache
            .clean(self.va_of(TX_AVAIL_OFF), core::mem::size_of::<VirtqAvail>());
        self.notify(1)?;
        self.wait_tx_used()?;
        let mut stats = self.stats.borrow_mut();
        stats.tx_packets += 1;
        stats.tx_bytes += frame.len() as u64;
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        if !self.initialized {
            return Err(NetError::NotInitialized);
        }
        // 39 收尾复验（F3）：NetDevice::recv 契约统一为**非阻塞**——
        // 无待取帧立即 `Err(NoBuffer)`（与 MacController 同语义），
        // 协议栈 drain_rx 依赖该契约终止；阻塞等待语义由调用方
        // 轮询 try_recv/本方法实现，不在驱动内 20M 次忙等。
        if !self.rx_pending() {
            // 通知设备（归还路径/新到帧投递窗口）后仍无帧 → NoBuffer。
            self.notify(0)?;
            if !self.rx_pending() {
                return Err(NetError::NoBuffer);
            }
        }
        let len = self.wait_rx_used()?;
        if len as usize > RX_BUFFER_SIZE {
            return Err(NetError::DmaError(0x8000_0003));
        }
        // 从 used ring 回看 desc id（rx_used_next 已推进到下一位置）。
        let desc_id = {
            let pool = self.pool.borrow();
            let bytes = pool.as_bytes();
            // SAFETY: 只读视图。
            let used = unsafe {
                &*(bytes[RX_USED_OFF..RX_USED_OFF + 0x1000].as_ptr() as *const VirtqUsed)
            };
            let idx = (self.rx_used_next.get().wrapping_sub(1)) as usize & QUEUE_MASK;
            used.ring[idx].id as u16
        };
        let slot = (desc_id as usize) % RX_SLOTS;
        let src = RX_BUF_OFF + slot * RX_BUFFER_SIZE;
        // legacy virtio-net：RX 缓冲以 10 字节 virtio_net_hdr 开头——
        // 帧数据从 hdr 之后开始；头不足/帧空 → 丢弃（fail-closed）。
        let payload = (len as usize).saturating_sub(VIRTIO_NET_HDR_SIZE);
        if payload == 0 {
            self.repost_rx_buffer(desc_id)?;
            return Ok(0);
        }
        let copy_len = payload.min(buf.len());
        self.cache
            .invalidate(self.va_of(src + VIRTIO_NET_HDR_SIZE), copy_len);
        {
            let pool = self.pool.borrow();
            let bytes = pool.as_bytes();
            buf[..copy_len].copy_from_slice(
                &bytes[src + VIRTIO_NET_HDR_SIZE..src + VIRTIO_NET_HDR_SIZE + copy_len],
            );
        }
        // 归还缓冲给设备。
        self.repost_rx_buffer(desc_id)?;
        let mut stats = self.stats.borrow_mut();
        stats.rx_packets += 1;
        stats.rx_bytes += copy_len as u64;
        Ok(copy_len)
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac.get()
    }

    fn mtu(&self) -> usize {
        self.mtu.get()
    }

    fn link_up(&self) -> bool {
        self.config_status() & VIRTIO_NET_S_LINK_UP != 0
    }

    fn set_promiscuous(&mut self, _on: bool) {
        // virtio-net 对端（QEMU socket 后端）按已实现语义投递；v1 驱动
        // 不区分混杂模式（不伪造能力）。
    }

    fn stats(&self) -> NetStats {
        *self.stats.borrow()
    }
}

// ============================================================================
// Tests（RAM 模拟设备：regs + 帧注入/捕获；经 Rc 共享给测试与驱动）
// ============================================================================

#[cfg(test)]
mod tests {
    use alloc::collections::VecDeque;
    use alloc::rc::Rc;
    use alloc::vec;

    use eneros_virtio_core::VirtqUsedElem;

    use super::*;
    use crate::dma_mem::NoopCache;

    const POOL_PHYS: u64 = 0x2000_0000;

    struct RamPool {
        bytes: Vec<u8>,
    }

    impl RamPool {
        fn new() -> Self {
            Self {
                bytes: vec![0u8; VIRTIO_NET_POOL_SIZE],
            }
        }
    }

    impl DmaPool for RamPool {
        fn len(&self) -> usize {
            self.bytes.len()
        }
        fn phys_base(&self) -> u64 {
            POOL_PHYS
        }
        fn as_bytes(&self) -> &[u8] {
            &self.bytes
        }
        fn as_bytes_mut(&mut self) -> &mut [u8] {
            &mut self.bytes
        }
    }

    /// RAM 模拟 virtio-net 设备（QEMU 设备模型角色）。
    struct MockNetMmio {
        regs: [u32; 64],
        /// config space（字节寻址，0x100 起）：MAC(6) + STATUS(2)。
        config: [u8; 16],
        /// 测试注入的入站帧（peer → 设备）。
        inbound: core::cell::RefCell<VecDeque<Vec<u8>>>,
        /// 设备捕获的出站帧（设备 → peer）。
        outbound: core::cell::RefCell<Vec<Vec<u8>>>,
        pool_ptr: core::cell::Cell<*mut u8>,
    }

    impl MockNetMmio {
        fn new() -> Self {
            let mut regs = [0u32; 64];
            regs[VMMIO_MAGIC_VALUE / 4] = VIRTIO_MMIO_MAGIC;
            regs[VMMIO_VERSION / 4] = VIRTIO_MMIO_VERSION_1;
            regs[VMMIO_DEVICE_ID / 4] = VIRTIO_NET_DEVICE_ID;
            regs[VMMIO_QUEUE_NUM_MAX / 4] = 32;
            // features: MAC | STATUS
            regs[VMMIO_DEVICE_FEATURES / 4] = (VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS) as u32;
            let mut config = [0u8; 16];
            config[0..6].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
            config[6..8].copy_from_slice(&VIRTIO_NET_S_LINK_UP.to_le_bytes());
            MockNetMmio {
                regs,
                config,
                inbound: core::cell::RefCell::new(VecDeque::new()),
                outbound: core::cell::RefCell::new(Vec::new()),
                pool_ptr: core::cell::Cell::new(core::ptr::null_mut()),
            }
        }

        fn inject_frame(&self, frame: Vec<u8>) {
            self.inbound.borrow_mut().push_back(frame);
        }

        fn outbound(&self) -> Vec<Vec<u8>> {
            self.outbound.borrow().clone()
        }

        /// 处理一次队列通知（每次至多投递/捕获 1 个条目——驱动会再通知）。
        fn process_queue(&self, queue: u32, pool: *mut u8) {
            if pool.is_null() {
                return;
            }
            // SAFETY: pool 为驱动 DMA 池（合法且 36 KiB）；本函数只在
            // 模拟设备上下文中调用，与驱动借用互斥（单线程）。
            unsafe {
                let bytes = core::slice::from_raw_parts_mut(pool, VIRTIO_NET_POOL_SIZE);
                let (desc_off, avail_off, used_off, is_rx) = match queue {
                    0 => (RX_DESC_OFF, RX_AVAIL_OFF, RX_USED_OFF, true),
                    1 => (TX_DESC_OFF, TX_AVAIL_OFF, TX_USED_OFF, false),
                    _ => return,
                };
                let descs = core::slice::from_raw_parts_mut(
                    bytes[desc_off..desc_off + 0x1000].as_mut_ptr() as *mut VirtqDesc,
                    QUEUE_MAX,
                );
                let avail =
                    &mut *(bytes[avail_off..avail_off + 0x1000].as_mut_ptr() as *mut VirtqAvail);
                let used =
                    &mut *(bytes[used_off..used_off + 0x1000].as_mut_ptr() as *mut VirtqUsed);
                // 有未消费 avail 条目才处理。
                if (avail.idx as usize & QUEUE_MASK) != (used.idx as usize & QUEUE_MASK) {
                    let head = avail.ring[used.idx as usize & QUEUE_MASK];
                    let desc = descs[head as usize];
                    let buf_off = (desc.addr - POOL_PHYS) as usize;
                    if is_rx {
                        if let Some(frame) = self.inbound.borrow_mut().pop_front() {
                            let dst = core::slice::from_raw_parts_mut(
                                (pool as usize + buf_off) as *mut u8,
                                RX_BUFFER_SIZE,
                            );
                            // legacy 设备：RX 缓冲以 10 字节 virtio_net_hdr 开头。
                            let n = frame.len().min(RX_BUFFER_SIZE - VIRTIO_NET_HDR_SIZE);
                            dst[VIRTIO_NET_HDR_SIZE..VIRTIO_NET_HDR_SIZE + n]
                                .copy_from_slice(&frame[..n]);
                            used.ring[used.idx as usize & QUEUE_MASK] = VirtqUsedElem {
                                id: head as u32,
                                len: (VIRTIO_NET_HDR_SIZE + n) as u32,
                            };
                            used.idx = used.idx.wrapping_add(1);
                        }
                    } else {
                        // legacy：TX 缓冲含 10 字节头——捕获时剥离。
                        let src = core::slice::from_raw_parts(
                            (pool as usize + buf_off + VIRTIO_NET_HDR_SIZE) as *const u8,
                            (desc.len as usize).saturating_sub(VIRTIO_NET_HDR_SIZE),
                        );
                        self.outbound.borrow_mut().push(Vec::from(src));
                        used.ring[used.idx as usize & QUEUE_MASK] = VirtqUsedElem {
                            id: head as u32,
                            len: desc.len,
                        };
                        used.idx = used.idx.wrapping_add(1);
                    }
                }
            }
        }
    }

    /// 本地 newtype：Rc 共享的模拟设备实现 [`VirtioMmio`]（孤儿规则——
    /// 外来 trait 不能直接为 `Rc<RefCell<..>>` 实现）。
    #[derive(Clone)]
    struct SharedMmio(Rc<core::cell::RefCell<MockNetMmio>>);

    impl SharedMmio {
        fn new() -> Self {
            Self(Rc::new(core::cell::RefCell::new(MockNetMmio::new())))
        }

        fn borrow(&self) -> core::cell::Ref<'_, MockNetMmio> {
            self.0.borrow()
        }

        fn borrow_mut(&self) -> core::cell::RefMut<'_, MockNetMmio> {
            self.0.borrow_mut()
        }
    }

    impl VirtioMmio for SharedMmio {
        fn read_reg(&self, offset: usize) -> u32 {
            let m = self.borrow();
            if offset >= 0x100 && offset + 4 <= 0x100 + m.config.len() {
                let off = offset - 0x100;
                return u32::from_le_bytes(m.config[off..off + 4].try_into().unwrap());
            }
            m.regs.get(offset / 4).copied().unwrap_or(0)
        }
        fn write_reg(&mut self, offset: usize, value: u32) {
            if offset == VMMIO_QUEUE_NOTIFY {
                let queue = value;
                let pool = self.borrow().pool_ptr.get();
                self.borrow().process_queue(queue, pool);
            }
            if offset >= 0x100 && offset + 4 <= 0x100 + self.borrow().config.len() {
                let off = offset - 0x100;
                self.borrow_mut().config[off..off + 4].copy_from_slice(&value.to_le_bytes());
                return;
            }
            if offset / 4 < self.borrow().regs.len() {
                self.borrow_mut().regs[offset / 4] = value;
            }
        }
        fn post_init(&self, pool: *mut u8) {
            self.borrow().pool_ptr.set(pool);
        }
    }
    type TestDriver = VirtioNetDriver<SharedMmio, RamPool, NoopCache>;

    fn make_driver() -> (SharedMmio, TestDriver) {
        let mmio = SharedMmio::new();
        let pool = RamPool::new();
        let drv = VirtioNetDriver::new(mmio.clone(), pool, NoopCache).expect("init");
        (mmio, drv)
    }

    #[test]
    fn init_negotiates_features_and_reads_mac() {
        let (_mmio, drv) = make_driver();
        assert_eq!(drv.mac_address(), [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        assert_eq!(drv.mtu(), 1500);
        assert!(drv.link_up());
        assert!(drv.stats().rx_packets == 0);
    }

    #[test]
    fn init_rejects_missing_mac_feature() {
        let mmio = SharedMmio::new();
        mmio.borrow_mut().regs[VMMIO_DEVICE_FEATURES / 4] = VIRTIO_NET_F_STATUS as u32; // 无 MAC
        let pool = RamPool::new();
        assert!(VirtioNetDriver::new(mmio, pool, NoopCache).is_err());
    }

    #[test]
    fn rx_delivers_injected_frame() {
        let (mmio, mut drv) = make_driver();
        // 构造后注入（Rc 共享）——recv 的 (b) 路径通知设备投递。
        mmio.borrow().inject_frame(b"hello-peer".to_vec());
        let mut buf = [0u8; 2048];
        let n = drv.recv(&mut buf).expect("recv");
        assert_eq!(&buf[..n], b"hello-peer");
        assert_eq!(drv.stats().rx_packets, 1);
        assert_eq!(drv.stats().rx_bytes, 10);
    }

    #[test]
    fn rx_consumes_and_reposts_multiple_frames() {
        let (mmio, mut drv) = make_driver();
        mmio.borrow().inject_frame(b"frame-a".to_vec());
        mmio.borrow().inject_frame(b"frame-b".to_vec());
        let mut buf = [0u8; 2048];
        let n = drv.recv(&mut buf).expect("recv-a");
        assert_eq!(&buf[..n], b"frame-a");
        let n = drv.recv(&mut buf).expect("recv-b");
        assert_eq!(&buf[..n], b"frame-b");
        assert_eq!(drv.stats().rx_packets, 2);
    }

    #[test]
    fn tx_captures_sent_frame_and_returns() {
        let (mmio, mut drv) = make_driver();
        drv.send(b"frame-out").expect("send");
        let out = mmio.borrow().outbound();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], b"frame-out");
        assert_eq!(drv.stats().tx_packets, 1);
        assert_eq!(drv.stats().tx_bytes, 9);
    }

    #[test]
    fn send_rejects_oversized_frame() {
        let (_mmio, mut drv) = make_driver();
        // 帧长 + 10B 头 > TX_BUFFER_SIZE → 拒绝。
        let big = vec![0u8; TX_BUFFER_SIZE - VIRTIO_NET_HDR_SIZE + 1];
        assert_eq!(
            drv.send(&big),
            Err(NetError::FrameTooLarge {
                size: TX_BUFFER_SIZE - VIRTIO_NET_HDR_SIZE + 1,
                max: TX_BUFFER_SIZE - VIRTIO_NET_HDR_SIZE
            })
        );
    }

    #[test]
    fn send_rejects_empty_frame() {
        let (_mmio, mut drv) = make_driver();
        assert!(drv.send(b"").is_err());
    }

    #[test]
    fn link_down_rejects_send() {
        let (mmio, mut drv) = make_driver();
        mmio.borrow_mut().config[6..8].copy_from_slice(&0u16.to_le_bytes()); // link down
        assert_eq!(drv.send(b"x"), Err(NetError::LinkDown));
        assert!(!drv.link_up());
    }

    #[test]
    fn rx_no_frame_is_nonblocking_nobuffer() {
        let (_mmio, mut drv) = make_driver();
        let mut buf = [0u8; 64];
        // 39 收尾复验（F3）：无注入帧 → 立即 NoBuffer（非阻塞，
        // 与 MacController 同契约；不返回垃圾、不 20M 忙等）。
        assert_eq!(drv.recv(&mut buf), Err(NetError::NoBuffer));
        // try_recv 同语义：Ok(None)。
        assert_eq!(drv.try_recv(&mut buf), Ok(None));
    }

    #[test]
    fn mtu_from_device_capability() {
        // 39 收尾复验（F7）：F_MTU 协商 → mtu() 读设备真值。
        let mmio = SharedMmio::new();
        mmio.borrow_mut().regs[VMMIO_DEVICE_FEATURES / 4] =
            (VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS | VIRTIO_NET_F_MTU) as u32;
        // config 偏移 4（0x104 字低 16 位）= MTU 1400；偏移 6 = link up。
        mmio.borrow_mut().config[4..6].copy_from_slice(&1400u16.to_le_bytes());
        let pool = RamPool::new();
        let drv = VirtioNetDriver::new(mmio, pool, NoopCache).expect("init");
        assert_eq!(drv.mtu(), 1400);
        // 无 F_MTU → 协议默认（非设备事实，文档语义）。
        let (_mmio2, drv2) = make_driver();
        assert_eq!(drv2.mtu(), ETHERNET_DEFAULT_MTU);
    }

    #[test]
    fn rx_used_ring_corrupt_id_rejected() {
        // 39 收尾复验（F8）：设备回超出已发放范围的 desc id（只发放
        // 过 0..8）→ 拒绝（旧阈值 QUEUE_MAX=16 会 id%RX_SLOTS 别名
        // 到在飞缓冲——双投）。
        let (mmio, mut drv) = make_driver();
        mmio.borrow().inject_frame(b"corrupt".to_vec());
        // 通知设备投递（used ring 写入 elem），但驱动不消费。
        drv.test_notify_rx().expect("notify");
        // 篡改 used ring 首 elem 的 id 为 9（> RX_SLOTS，< QUEUE_MAX）。
        {
            let pool_ptr = mmio.borrow().pool_ptr.get();
            assert!(!pool_ptr.is_null());
            // SAFETY: test-only mutation of the used ring within the pre-allocated MMIO pool; fixed RX_USED_OFF window and RX_SLOTS bounds.
            unsafe {
                let bytes = core::slice::from_raw_parts_mut(pool_ptr, VIRTIO_NET_POOL_SIZE);
                let used =
                    &mut *(bytes[RX_USED_OFF..RX_USED_OFF + 0x1000].as_mut_ptr() as *mut VirtqUsed);
                used.ring[0].id = 9;
            }
        }
        let mut buf = [0u8; 64];
        assert_eq!(drv.recv(&mut buf), Err(NetError::DmaError(0x8000_0001)));
    }

    #[test]
    fn modern_transport_rejected() {
        // 39 收尾复验（F9）：非 legacy transport 显式拒绝。
        let mmio = SharedMmio::new();
        mmio.borrow_mut().regs[VMMIO_VERSION / 4] = VIRTIO_MMIO_VERSION_2;
        let pool = RamPool::new();
        assert!(VirtioNetDriver::new(mmio, pool, NoopCache).is_err());
    }
}
