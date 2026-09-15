//! virtio-blk driver (virtio-mmio transport) — real virtqueue + DMA.
//!
//! Implements the VirtIO 1.0 virtqueue protocol over the virtio-mmio
//! transport as used by QEMU `-device virtio-blk-device` (legacy
//! force-legacy layout on QEMU 9.2, version=1). The reference implementation
//! is `third_party/sel4-root/root_task.c` (G3 DRV-007) — this crate mirrors
//! its register offsets, queue layout, and request descriptor chain, but as a
//! host-testable Rust `BlockDevice`.
//!
//! # Architecture
//!
//! - [`VirtioMmio`] abstracts the virtio-mmio register file. On real hardware
//!   it is backed by volatile mapped memory; on the host the test backend
//!   simulates QEMU's register semantics over a `Vec<u8>`.
//! - [`DmaPool`] supplies contiguous DMA pages (descriptor ring + request
//!   buffers). On seL4/real hardware it is backed by untyped retype frames;
//!   on the host it is a `Vec<u8>` arena.
//! - [`ArmCache`] performs dcache clean/invalidate around descriptor and data
//!   buffers. On aarch64 real hardware it issues `dc cvac`/`dc ivac`; on the
//!   host it is a no-op (cache coherent simulation).
//!
//! # Virtqueue layout (legacy, 16 descriptors)
//!
//! ```text
//! base + 0x0000  desc  (16 × 16 B = 256 B)
//! base + 0x0100  avail (2 + 2 + 16×2 + 2 = 36 B, 1 page)
//! base + 0x1000  used  (2 + 2 + 16×8 + 2, 1 page)
//! base + 0x2000  req header page
//! base + 0x3000.. data/status slot buffers
//! ```
//!
//! A block request uses a 3-descriptor chain: `req` (read), `data`
//! (read/write), `status` (write), exactly as the C reference.

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{compiler_fence, Ordering};

// ============================================================================
// virtio-mmio 共享原语（39 WP-39.5：抽取至 eneros-virtio-core，此处 re-export
// 保持既有消费方 API 不变——禁止另造同义 ring/常量类型）
// ============================================================================
pub use eneros_virtio_core::{
    DmaPool, VirtioMmio, VirtqAvail, VirtqDesc, VirtqUsed, VirtqUsedElem, QUEUE_MASK, QUEUE_MAX,
    VIRTIO_MMIO_MAGIC, VIRTIO_MMIO_VERSION_1, VIRTIO_MMIO_VERSION_2, VIRTIO_STATUS_ACKNOWLEDGE,
    VIRTIO_STATUS_DRIVER, VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK, VIRTQ_DESC_F_NEXT,
    VIRTQ_DESC_F_WRITE, VMMIO_DEVICE_FEATURES, VMMIO_DEVICE_FEATURES_SEL, VMMIO_DEVICE_ID,
    VMMIO_DRIVER_FEATURES, VMMIO_DRIVER_FEATURES_SEL, VMMIO_GUEST_PAGE_SIZE, VMMIO_INTERRUPT_ACK,
    VMMIO_INTERRUPT_STATUS, VMMIO_MAGIC_VALUE, VMMIO_QUEUE_ALIGN, VMMIO_QUEUE_DESC_HIGH,
    VMMIO_QUEUE_DESC_LOW, VMMIO_QUEUE_DEVICE_HIGH, VMMIO_QUEUE_DEVICE_LOW, VMMIO_QUEUE_DRIVER_HIGH,
    VMMIO_QUEUE_DRIVER_LOW, VMMIO_QUEUE_NOTIFY, VMMIO_QUEUE_NUM, VMMIO_QUEUE_NUM_MAX,
    VMMIO_QUEUE_PFN, VMMIO_QUEUE_READY, VMMIO_QUEUE_SEL, VMMIO_STATUS, VMMIO_VENDOR_ID,
    VMMIO_VERSION,
};

use crate::driver::types::{DeviceHealth, StorageConfig};
use crate::driver::BlockDevice;
use crate::error::StorageError;

/// Block device ID (virtio-blk).
pub const VIRTIO_BLK_DEVICE_ID: u32 = 2;

/// Block request types.
pub const VIRTIO_BLK_T_IN: u32 = 0;
pub const VIRTIO_BLK_T_OUT: u32 = 1;
pub const VIRTIO_BLK_T_FLUSH: u32 = 4;

/// Block request status codes.
pub const VIRTIO_BLK_S_OK: u8 = 0;
pub const VIRTIO_BLK_S_IOERR: u8 = 1;

/// Sector size for virtio-blk (always 512 bytes).
const SECTOR_SIZE: usize = 512;

/// Number of request slots for data/status rotation (C uses 3 data + 4 status).
const REQ_SLOTS: usize = 3;

/// Block request header (16 bytes, virtio-blk request).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioBlkReq {
    pub kind: u32,
    pub reserved: u32,
    pub sector: u64,
}

// ============================================================================
// Cache abstractions（virtio-mmio 寄存器/ring/DMA 池在 eneros-virtio-core）
// ============================================================================

/// ARM64 dcache maintenance around DMA buffers.
///
/// Real hardware: `dc cvac`/`dc ivac` + `dsb`. Host simulation is
/// cache-coherent, so these are no-ops (still called to exercise the path).
pub trait ArmCache {
    /// Clean (write back) the given virtual range before device read.
    fn clean(&self, va: usize, len: usize);
    /// Invalidate the given virtual range after device write.
    fn invalidate(&self, va: usize, len: usize);
}

/// No-op cache maintenance for host simulation / cache-coherent targets.
pub struct NoopCache;

impl ArmCache for NoopCache {
    fn clean(&self, _va: usize, _len: usize) {}
    fn invalidate(&self, _va: usize, _len: usize) {}
}

/// ARM64 dcache maintenance for DMA buffers (real hardware path).
///
/// Issues `dc cvac` (clean to point of coherency) before a device read and
/// `dc ivac` (invalidate) after a device write, bracketed by `dsb ish`.
/// This is the standard DMA cache-coherency protocol for non-coherent
/// virtio-mmio on aarch64. On non-aarch64 targets it degrades to a no-op
/// (host simulation is cache-coherent; x86 is coherent in practice).
#[cfg(target_arch = "aarch64")]
pub struct Aarch64Cache;

/// 读取 CTR_EL0 的 DminLine 字段推导 dcache line 大小（字节）。
///
/// CVR-38-01：ARMv8 允许实现定义的 cache line（64B 只是常见值），
/// 固定 64B 会在 128B line 的盒子上做不全的 clean/invalidate。DminLine
/// 为 4 位字段：`(DminLine + 1) * 4` 字节；值为 0 表示实现不支持
/// 读/写分配（此时维护无法进行，调用方按失败处理由调用者保证不会到达）。
#[cfg(target_arch = "aarch64")]
fn cache_line_bytes() -> usize {
    let ctr_el0: u64;
    // SAFETY: `mrs ctr_el0` 是 EL0 可读的系统寄存器（读缓存类型寄存器），
    // 无副作用；在 aarch64 目标上始终合法。
    unsafe {
        core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr_el0, options(nostack));
    }
    ((ctr_el0 & 0xF) as usize + 1) * 4
}

#[cfg(target_arch = "aarch64")]
impl ArmCache for Aarch64Cache {
    fn clean(&self, va: usize, len: usize) {
        // SAFETY: inline asm `dc cvac` operates on a virtual address that the
        // caller guarantees is mapped and valid for `len` bytes.
        unsafe {
            let line = cache_line_bytes();
            let mut addr = va;
            let end = va.wrapping_add(len);
            while addr < end {
                core::arch::asm!("dc cvac, {addr}", addr = in(reg) addr, options(nostack));
                addr = addr.wrapping_add(line);
            }
            core::arch::asm!("dsb ish", options(nostack));
        }
    }

    fn invalidate(&self, va: usize, len: usize) {
        // SAFETY: `dc ivac` invalidates the cache line for the given VA; the
        // caller guarantees the VA is mapped and valid for `len` bytes.
        unsafe {
            let line = cache_line_bytes();
            let mut addr = va;
            let end = va.wrapping_add(len);
            while addr < end {
                core::arch::asm!("dc ivac, {addr}", addr = in(reg) addr, options(nostack));
                addr = addr.wrapping_add(line);
            }
            core::arch::asm!("dsb ish", options(nostack));
        }
    }
}

/// Non-aarch64 fallback: cache-coherent host simulation.
#[cfg(not(target_arch = "aarch64"))]
pub type Aarch64Cache = NoopCache;

// ============================================================================
// virtio-blk driver
// ============================================================================

/// virtio-blk block device over virtio-mmio.
///
/// Wraps a [`VirtioMmio`] register file, a [`DmaPool`] holding the queue
/// memory, and an [`ArmCache`] for DMA coherence. `new` performs device
/// discovery + feature negotiation + queue setup; [`BlockDevice`] methods
/// then issue single-block read/write/flush requests through the real
/// virtqueue.
pub struct VirtioBlkDriver<M: VirtioMmio, P: DmaPool, C: ArmCache> {
    /// Registers (MMIO). Mutated during requests (notify); `RefCell` allows
    /// `read_block(&self)` per the `BlockDevice` trait contract.
    mmio: core::cell::RefCell<M>,
    /// DMA pool (queue memory). Mutated during requests.
    pool: core::cell::RefCell<P>,
    cache: C,
    config: StorageConfig,
    block_count: core::cell::Cell<u64>,
    initialized: bool,
    /// Legacy (version 1) transport: uses QueuePFN + guest page size.
    legacy: core::cell::Cell<bool>,
    /// Free descriptor head index (simple FIFO allocator over the ring).
    desc_head: core::cell::Cell<usize>,
    /// Number of descriptors currently allocated.
    desc_used: core::cell::Cell<usize>,
    /// Request slot rotation counter.
    req_slot: core::cell::Cell<usize>,
    /// Index of the next used element to consume.
    last_used: core::cell::Cell<u16>,
}

impl<M: VirtioMmio, P: DmaPool, C: ArmCache> VirtioBlkDriver<M, P, C> {
    /// Creates the driver, performs device init + queue setup.
    ///
    /// `pool` must be at least 9 DMA pages (36 KiB): 2 pages for the queue
    /// (desc+avail in page 0, used in page 1), 1 page for the request header,
    /// 3 pages for data slots, 3 pages for status slots. The physical base of
    /// `pool` is used to program the queue address.
    pub fn new(mmio: M, pool: P, cache: C, config: StorageConfig) -> Result<Self, StorageError> {
        // 31 DR-09：block_size 范围校验——>4096 内部 4KiB 栈缓冲越界；
        // <SECTOR_SIZE(512) 致 capacity/(block_size/512) 除零 panic（31-review 补下界）。
        if config.block_size > 4096 || config.block_size < SECTOR_SIZE {
            return Err(StorageError::UnsupportedBlockSize);
        }
        // 31 DR-09：capacity 读 config space offset 0（legacy virtio-blk
        // capacity 字段，8 字节 LE）——旧实现 max_transfer_blocks*16 占位。
        let capacity = {
            let lo = mmio.read_reg(0x100) as u64; // config offset 0
            let hi = mmio.read_reg(0x100 + 4) as u64;
            (hi << 32) | lo
        };
        // 39 WP-39.5（P1-14）：capacity=0 **不得虚构**（旧实现回退
        // `max_transfer_blocks*16` 占位——容量是设备事实，猜容量即
        // 允许越界写）。设备未报告容量 → fail-closed。
        if capacity == 0 {
            return Err(StorageError::Unsupported {
                detail: "virtio-blk capacity=0 (device did not report capacity)",
            });
        }
        let block_count = capacity / (config.block_size as u64 / SECTOR_SIZE as u64);
        // 下界保护：至少 1 块（容量 < 块大小时按 1 块处理仍可读头块诊断；
        // 正常介质不会出现，防御除零后的空盘面）。
        let block_count = block_count.max(1);
        let mut drv = Self {
            mmio: core::cell::RefCell::new(mmio),
            pool: core::cell::RefCell::new(pool),
            cache,
            config,
            block_count: core::cell::Cell::new(block_count),
            initialized: false,
            legacy: core::cell::Cell::new(true),
            desc_head: core::cell::Cell::new(0),
            desc_used: core::cell::Cell::new(0),
            req_slot: core::cell::Cell::new(0),
            last_used: core::cell::Cell::new(0),
        };
        drv.device_init()?;
        drv.queue_setup()?;
        // 70.7（REG-70.7-BOOT-DEV-IO-HANG 终局修复）：设备 reset 后其
        // last_avail/used 计数器 = 0——池内环计数器（avail.idx@0x102、
        // used.idx@0x1002）必须同步归零。supervisor 重启后池页**不清零**
        //（映射保留、前实例状态残留）：不归零则新实例 last_used=0 对
        // 池内 used.idx=15 → 首次 wait_used 误判完成 → I/O 全死
        //（run36-40 重启后 I/O 实弹）。
        {
            let mut pool = drv.pool.borrow_mut();
            let bytes = pool.as_bytes_mut();
            bytes[0x0102..0x0104].copy_from_slice(&0u16.to_le_bytes());
            bytes[0x1002..0x1004].copy_from_slice(&0u16.to_le_bytes());
        }
        // Wire the device model (RAM simulation) to the pool after queue setup.
        let pool_ptr = drv.pool_ptr();
        drv.mmio.borrow().post_init(pool_ptr);
        drv.initialized = true;
        Ok(drv)
    }

    /// Performs virtio-mmio device discovery, feature negotiation, and
    /// DRIVER_OK handshake (mirrors `vmmio_device_init`).
    ///
    /// REG-70.7-BOOT-DEV-IO-HANG（终局修复）：标识寄存器读完后**显式
    /// 写 STATUS=0 复位设备**，再走 ACK/DRIVER…。virtio 规范 2.4.2：
    /// 写 0 到 Status 复位设备——复位清空设备侧队列内部态
    ///（`last_avail_idx`/vring 地址/broken 标志）。服务重启（supervisor
    /// 重启 svc-storage）后 guest 侧环归零，而设备侧 last_avail_idx
    /// 停留在旧值：新请求使 `avail.idx - last_avail_idx` 下溢 → QEMU
    /// `virtio_error("Virtqueue size exceeded")` → 设备置 broken，此后
    /// **所有**请求静默丢弃（实弹：run51 时间线 `BOOT-MMIO-DUMP` 后
    /// 8 次 TIMEOUT last_used=0 + 双副本读全灭）。
    ///
    /// 顺序约束（WP-27.3 D4 遗留观察）：DEVICE_ID 在复位**之后**读会
    /// 返回 0（QEMU 9.2.4 legacy 实测），故复位点放在 magic → version →
    /// device_id 三读**之后**、ACK 之前——首启（本就处于复位态）重复
    /// 复位为幂等空操作。
    fn device_init(&self) -> Result<(), StorageError> {
        let mut mmio = self.mmio.borrow_mut();
        if mmio.read_reg(VMMIO_MAGIC_VALUE) != VIRTIO_MMIO_MAGIC {
            return Err(StorageError::HardwareFault {
                detail: "virtio-mmio magic mismatch",
            });
        }
        let version = mmio.read_reg(VMMIO_VERSION);
        if version != VIRTIO_MMIO_VERSION_1 && version != VIRTIO_MMIO_VERSION_2 {
            return Err(StorageError::HardwareFault {
                detail: "unsupported virtio-mmio version",
            });
        }
        self.legacy.set(version == VIRTIO_MMIO_VERSION_1);
        if mmio.read_reg(VMMIO_DEVICE_ID) != VIRTIO_BLK_DEVICE_ID {
            return Err(StorageError::HardwareFault {
                detail: "not a virtio-blk device",
            });
        }
        mmio.write_reg(VMMIO_STATUS, 0);
        mmio.write_reg(VMMIO_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
        mmio.write_reg(
            VMMIO_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER,
        );
        // Read device features (word 0 and 1).
        mmio.write_reg(VMMIO_DEVICE_FEATURES_SEL, 0);
        let f0 = mmio.read_reg(VMMIO_DEVICE_FEATURES);
        mmio.write_reg(VMMIO_DEVICE_FEATURES_SEL, 1);
        let f1 = mmio.read_reg(VMMIO_DEVICE_FEATURES);
        let dev_features: u64 = (f0 as u64) | ((f1 as u64) << 32);
        // Negotiate: modern requires VERSION_1 feature; legacy accepts all
        // (QEMU 9.2 force-legacy has no VIRTIO_F_VERSION_1 semantics).
        let driver_features = if self.legacy.get() {
            dev_features
        } else {
            dev_features & (1u64 << 32)
        };
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
            return Err(StorageError::HardwareFault {
                detail: "virtio feature negotiation rejected",
            });
        }
        Ok(())
    }

    /// Sets up queue 0 with the legacy (or modern) ring layout.
    fn queue_setup(&self) -> Result<(), StorageError> {
        let base_pa = self.pool.borrow().phys_base();
        if self.pool.borrow().len() < 9 * 4096 {
            return Err(StorageError::HardwareFault {
                detail: "DMA pool too small for virtio-blk queue",
            });
        }
        let mut mmio = self.mmio.borrow_mut();
        mmio.write_reg(VMMIO_QUEUE_SEL, 0);
        let num_max = mmio.read_reg(VMMIO_QUEUE_NUM_MAX);
        if (num_max as usize) < QUEUE_MAX {
            return Err(StorageError::HardwareFault {
                detail: "virtio queue max size too small",
            });
        }
        if self.legacy.get() {
            // Order matters: QueueNum first, then QueuePFN (PFN triggers
            // update_rings which computes the legacy layout).
            mmio.write_reg(VMMIO_GUEST_PAGE_SIZE, 4096);
            mmio.write_reg(VMMIO_QUEUE_ALIGN, 4096);
            mmio.write_reg(VMMIO_QUEUE_NUM, QUEUE_MAX as u32);
            mmio.write_reg(VMMIO_QUEUE_PFN, (base_pa >> 12) as u32);
            // Legacy layout: desc@+0, avail@+256, used@+4096.
        } else {
            mmio.write_reg(VMMIO_QUEUE_NUM, QUEUE_MAX as u32);
            mmio.write_reg(VMMIO_QUEUE_DESC_LOW, base_pa as u32);
            mmio.write_reg(VMMIO_QUEUE_DESC_HIGH, (base_pa >> 32) as u32);
            mmio.write_reg(VMMIO_QUEUE_DRIVER_LOW, (base_pa + 0x1000) as u32);
            mmio.write_reg(VMMIO_QUEUE_DRIVER_HIGH, ((base_pa + 0x1000) >> 32) as u32);
            mmio.write_reg(VMMIO_QUEUE_DEVICE_LOW, (base_pa + 0x2000) as u32);
            mmio.write_reg(VMMIO_QUEUE_DEVICE_HIGH, ((base_pa + 0x2000) >> 32) as u32);
            mmio.write_reg(VMMIO_QUEUE_READY, 1);
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

    /// Virtual address of a pool offset (for cache ops).
    fn va_of(&self, offset: usize) -> usize {
        self.pool.borrow().as_bytes().as_ptr() as usize + offset
    }

    /// Issues one block request through the virtqueue.
    ///
    /// `data` is the data buffer (input for OUT, output for IN); `datalen`
    /// must be a multiple of the sector size. Returns the used-ring `len` of
    /// the data descriptor (for IN requests).
    fn issue(
        &self,
        kind: u32,
        sector: u64,
        data: &mut [u8],
        datalen: usize,
    ) -> Result<u32, StorageError> {
        match self.issue_once(kind, sector, data, datalen) {
            Ok(v) => Ok(v),
            Err(StorageError::Timeout { .. }) => {
                // 2026-09-14（`REG-70.15-BLK-DEVICE-IOERR-SUSTAINED`）：设备在
                // `wait_used` 窗内没有完成请求 ⇒ 设备侧 `last_avail_idx` 与
                // guest 侧 `avail.idx` 失步，此后**所有**请求静默丢弃（实弹：
                // `[blk-wait] TIMEOUT device never completed (20M spins)
                // last_used=38028` 冻结不变 → 级联到 `SNAP-SKIP store-fail`
                // → 控制链"活着但不再控制"）。按 REG-70.7 既有口径**复位重建
                // 队列**（写 STATUS=0 清设备侧内部态 + 重跑 device_init/
                // queue_setup + 环计数器与描述符游标归零），然后**重试一次**：
                // 可恢复的瞬时停顿不得升级成永久停摆。
                if self.recover_queue() {
                    self.issue_once(kind, sector, data, datalen)
                } else {
                    Err(StorageError::Timeout {
                        operation: "virtio-blk request",
                        ms: 0,
                    })
                }
            }
            Err(e) => Err(e),
        }
    }

    /// 队列自恢复：复位设备 + 重建队列 + 归零 guest 侧环计数器/描述符游标。
    /// 返回 true = 已重建（调用方可重试）。
    ///
    /// 与 `VirtioBlkDriver::new` 的初始化序列同源（device_init → queue_setup →
    /// 环计数器归零 → post_init）；差别只在**不重建驱动实例**（MMIO 映射与
    /// DMA 池不变），并把 `desc_head`/`desc_used`/`last_used` 一并归零。
    fn recover_queue(&self) -> bool {
        if self.device_init().is_err() || self.queue_setup().is_err() {
            #[cfg(target_os = "none")]
            eneros_runtime::println!("[blk] QUEUE-RECOVER failed (device_init/queue_setup)");
            return false;
        }
        {
            let mut pool = self.pool.borrow_mut();
            let bytes = pool.as_bytes_mut();
            // avail.idx @0x0102 / used.idx @0x1002（与 new 的归零点同址同宽）。
            bytes[0x0102..0x0104].copy_from_slice(&0u16.to_le_bytes());
            bytes[0x1002..0x1004].copy_from_slice(&0u16.to_le_bytes());
        }
        self.desc_head.set(0);
        self.desc_used.set(0);
        self.last_used.set(0);
        let pool_ptr = self.pool_ptr();
        self.mmio.borrow().post_init(pool_ptr);
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[blk] QUEUE-RECOVERED —— 队列已复位重建，重试本次请求");
        true
    }

    fn issue_once(
        &self,
        kind: u32,
        sector: u64,
        data: &mut [u8],
        datalen: usize,
    ) -> Result<u32, StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        // FLUSH 不需要数据描述符；QEMU legacy 拒绝 zero-sized buffer，
        // 因此 flush 使用 2 描述符链（req→status），读写仍为 3 描述符。
        let ndesc = if kind == VIRTIO_BLK_T_FLUSH { 2 } else { 3 };
        if self.desc_used.get() + ndesc > QUEUE_MAX {
            return Err(StorageError::HardwareFault {
                detail: "virtqueue full",
            });
        }
        #[cfg(target_os = "none")]
        if self.desc_used.get() + ndesc > QUEUE_MAX {
            eneros_runtime::println!(
                "[blk] issue VIRTQUEUE FULL desc_used={} ndesc={ndesc}",
                self.desc_used.get()
            );
        }
        let slot = self.req_slot.get() % REQ_SLOTS;
        self.req_slot.set(self.req_slot.get() + 1);
        // Layout: +0x2000 req header, +0x3000 data slots, +0x4000 status slots.
        let req_off = 0x2000;
        let data_off = 0x3000 + slot * 0x1000;
        let status_off = 0x4000 + slot * 0x1000;

        let head = self.desc_head.get();
        let base_pa = self.pool.borrow().phys_base();
        {
            // Phase 1: fill request header, OUT data, status init, descriptors,
            // avail ring — all within one scoped mutable borrow of the pool.
            let mut pool = self.pool.borrow_mut();
            let bytes = pool.as_bytes_mut();
            // SAFETY: `req_off` 对齐且 `req_off + 16 <= 0x2000`（请求头固定
            // 在 pool 内），`VirtioBlkReq` 为 16 字节 POD，布局匹配。
            let req =
                unsafe { &mut *(bytes[req_off..req_off + 16].as_mut_ptr() as *mut VirtioBlkReq) };
            req.kind = kind;
            req.reserved = 0;
            req.sector = sector;

            if kind == VIRTIO_BLK_T_OUT && datalen > 0 {
                bytes[data_off..data_off + datalen].copy_from_slice(&data[..datalen]);
            }
            // Initialize status to IOERR so we can detect a missing device write.
            bytes[status_off] = VIRTIO_BLK_S_IOERR;

            // SAFETY: desc 区间 [0x0000, 0x1000) 与 avail/used/数据区间互不
            // 重叠，`VirtqDesc` 布局匹配，长度 QUEUE_MAX 不越界。
            let descs = unsafe {
                core::slice::from_raw_parts_mut(
                    bytes[0x0000..0x1000].as_mut_ptr() as *mut VirtqDesc,
                    QUEUE_MAX,
                )
            };
            if kind == VIRTIO_BLK_T_FLUSH {
                // FLUSH：仅请求头 + 状态字节，避免 zero-sized data 描述符。
                descs[head] = VirtqDesc {
                    addr: base_pa + req_off as u64,
                    len: 16,
                    flags: VIRTQ_DESC_F_NEXT,
                    next: (head + 1) as u16 & QUEUE_MASK as u16,
                };
                descs[(head + 1) & QUEUE_MASK] = VirtqDesc {
                    addr: base_pa + status_off as u64,
                    len: 1,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                };
            } else {
                descs[head] = VirtqDesc {
                    addr: base_pa + req_off as u64,
                    len: 16,
                    flags: VIRTQ_DESC_F_NEXT,
                    next: (head + 1) as u16 & QUEUE_MASK as u16,
                };
                descs[(head + 1) & QUEUE_MASK] = VirtqDesc {
                    addr: base_pa + data_off as u64,
                    len: datalen as u32,
                    flags: VIRTQ_DESC_F_NEXT
                        | if kind == VIRTIO_BLK_T_IN {
                            VIRTQ_DESC_F_WRITE
                        } else {
                            0
                        },
                    next: (head + 2) as u16 & QUEUE_MASK as u16,
                };
                descs[(head + 2) & QUEUE_MASK] = VirtqDesc {
                    addr: base_pa + status_off as u64,
                    len: 1,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                };
            }

            // SAFETY: avail 视图区间 [0x0100, 0x1100) 与 desc/used 区间不重叠，
            // 布局与 VirtioAvail 匹配；pool 借用独占。
            let avail = unsafe { &mut *(bytes[0x0100..0x1100].as_mut_ptr() as *mut VirtqAvail) };
            avail.ring[avail.idx as usize & QUEUE_MASK] = head as u16;
            compiler_fence(Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
        }
        self.desc_head
            .set((self.desc_head.get() + ndesc) & QUEUE_MASK);
        self.desc_used.set(self.desc_used.get() + ndesc);

        // Clean descriptors + req + OUT data before notify.
        self.cache.clean(self.va_of(0x0000), 256);
        self.cache.clean(self.va_of(req_off), 16);
        if kind == VIRTIO_BLK_T_OUT {
            self.cache.clean(self.va_of(data_off), datalen);
        }

        // Notify the device.
        {
            let mut mmio = self.mmio.borrow_mut();
            mmio.write_reg(VMMIO_QUEUE_SEL, 0);
            compiler_fence(Ordering::SeqCst);
            mmio.write_reg(VMMIO_QUEUE_NOTIFY, 0);
        }

        // Wait for the used element.
        //
        // ⚠ 2026-09-14（70.15 长窗实弹，`REG-70.15-VIRTQ-FULL-UNDER-LOAD`）：
        // **描述符回收必须在每一个出口完成**。旧实现在此用 `?` 提前返回、且在
        // `status != OK` 分支 `return` 之前也没有回收 ⇒ 每一次失败请求都永久
        // 漏掉 `ndesc` 个描述符。实测级联：数次瞬时失败（TCG 下 `wait_used`
        // 超时 / 设备回 IOERR）后 `desc_used` 触顶 ⇒ 之后**每一次** `issue`
        // 都返回 `HardwareFault{"virtqueue full"}` ⇒ 块层整条死掉 ⇒ 快照
        // blob 落盘 `SNAP-SKIP store-fail` 连败 ⇒ 冻结输入停止发布 ⇒ 控制链
        // "进程活着但不再控制"（`Committed` 停住而 `round=` 仍涨）。
        //
        // 设备已经消费了这批描述符（无论结果如何），故回收对**已完成**的请求
        // 是严格正确的；对`wait_used` 超时（设备疑似失联）也一并回收——代价是
        // 理论上的"迟到完成"可能踩到被复用的描述符，收益是**不再把块层永久
        // 钉死**（本驱动单请求串行，超时后再不复用才是灾难）。宁可一次瞬时
        // 失败，不要整条链停摆。
        let used_len = match self.wait_used() {
            Ok(v) => v,
            Err(e) => {
                self.desc_used.set(self.desc_used.get() - ndesc);
                return Err(e);
            }
        };

        // Check status byte and copy IN data back.
        self.cache.invalidate(self.va_of(status_off), 1);
        {
            let pool = self.pool.borrow();
            let bytes = pool.as_bytes();
            let status = bytes[status_off];
            if status != VIRTIO_BLK_S_OK {
                #[cfg(target_os = "none")]
                eneros_runtime::println!("[blk-wait] device IOERR status={status}");
                drop(pool);
                self.desc_used.set(self.desc_used.get() - ndesc);
                return Err(StorageError::HardwareFault {
                    detail: "virtio-blk device reported I/O error",
                });
            }
            if kind == VIRTIO_BLK_T_IN && datalen > 0 {
                self.cache.invalidate(self.va_of(data_off), datalen);
                data[..datalen].copy_from_slice(&bytes[data_off..data_off + datalen]);
            }
        }
        self.desc_used.set(self.desc_used.get() - ndesc);
        Ok(used_len)
    }

    /// Polls the used ring until a new element appears (bounded spins).
    ///
    /// v0.214.0（B1-02）：轮询前 invalidate used ring 区域——设备经 DMA
    /// 写 used ring，CPU 侧非一致缓存下必须使该区域缓存失效才能看到新
    /// 元素（此前缺失，真机 cache 一致性缺口）。
    fn wait_used(&self) -> Result<u32, StorageError> {
        let used_va = self.va_of(0x1000);
        // 2026-09-14（70.15 长窗实弹，`REG-70.15-BLK-DEVICE-IOERR-SUSTAINED`
        // 首发形态）：预算 20M（≈2-20s）被**合法的长操作**打穿——同窗实测
        // `TSDB-CHECKPOINT-DUR ms=14847`（≈14.8s ≈ 20M spins），其后紧跟
        // `TIMEOUT device never completed`。TCG 下设备完成随宿主负载波动，
        // 预算必须覆盖**最坏合法操作**（checkpoint/组写/擦除），而非静默
        // 负载均值。20M → 200M（≈20-200s，有界 ×10）；真挂死由
        // `recover_queue()`（复位重建）兜底，不再"超时即砖化"。
        //
        // 70.7 保留口径：挂死请求仍要干净失败，避免单次读垄断 vCPU 数百
        // 秒拖垮整窗（run44-49 实弹）——只是上限必须 ≥ 最坏合法操作时长。
        for _spin in 0..200_000_000u32 {
            // 70.7（兜底，非真因）：周期性 **MMIO 读**（STATUS，只读无副作用）
            // ——单线程 TCG 下纯 RAM 自旋不产生退出、主循环（AIO 完成处理）
            // 可能被饿死的防御性踢。**本卷终局实证：本案"永不完成"的真因
            // 是设备被 QEMU 置 broken**（svc-storage 栈溢出 → 重启 → 设备侧
            // last_avail_idx 残留 → `virtio_error`；见 `device_init` 文档），
            // 那种状态下任何踢都无效——真修复 = 栈帧收窄 + STATUS=0 复位。
            // 保留踢为对真机无害的防御（普通 MMIO 读）。
            if _spin % 1024 == 0 {
                let _ = self.mmio.borrow().read_reg(VMMIO_STATUS);
            }
            // 周期性补打门铃（notify 丢失竞态兜底；只写 NOTIFY 0x50）。
            if _spin != 0 && _spin % 1_000_000 == 0 {
                let mut mmio = self.mmio.borrow_mut();
                mmio.write_reg(VMMIO_QUEUE_NOTIFY, 0);
            }
            // 70.7（REG-70.7 run48 实弹）：**禁止 yield**——seL4_Yield 在
            // 「无可运行线程 + timer 未 ACK」时进入 idle，且 root 阻塞于
            // chunked Call 无法 ACK timer → 永久 idle（比忙等更糟）。纯
            // 自旋保证本读超时后系统必然恢复（run32/36/37 实证）。
            // 设备写 used ring（DMA）：轮询前使缓存失效。
            self.cache
                .invalidate(used_va, core::mem::size_of::<VirtqUsed>());
            let pool = self.pool.borrow();
            let bytes = pool.as_bytes();
            // SAFETY: used 视图区间 [0x1000, 0x2000) 与 desc/avail 区间不重叠，
            // 布局与 VirtqUsed 匹配；仅读取（cache invalidate 已刷新）。
            let used = unsafe { &*(bytes[0x1000..0x2000].as_ptr() as *const VirtqUsed) };
            let used_idx = used.idx;
            if used_idx != self.last_used.get() {
                let idx = self.last_used.get() as usize & QUEUE_MASK;
                let elem = used.ring[idx];
                self.last_used.set(self.last_used.get().wrapping_add(1));
                // 31 DR-09（注释诚实化）：旧注释声称校验 used_elem.id 与
                // 下发描述符一致但从未实现。精确一致性校验依赖 avail ring
                // 快照与多缓冲在飞场景的槽位同步（真机语义复杂），此处
                // 落地**范围防御**（id 越界 = 设备行为异常，fail-closed）；
                // 精确 head 一致性校验登记为真机项（33 §真机填空）。
                if (elem.id as usize) >= QUEUE_MAX {
                    #[cfg(target_os = "none")]
                    eneros_runtime::println!("[blk-wait] used id OUT OF RANGE id={}", elem.id);
                    return Err(StorageError::HardwareFault {
                        detail: "virtio-blk used ring id out of range",
                    });
                }
                return Ok(elem.len);
            }
        }
        #[cfg(target_os = "none")]
        eneros_runtime::println!(
            "[blk-wait] TIMEOUT device never completed (20M spins) last_used={}",
            self.last_used.get()
        );
        Err(StorageError::Timeout {
            operation: "virtio-blk request",
            ms: 2000,
        })
    }

    /// Returns the negotiated device capacity in blocks (via a flush request
    /// probe is not possible; callers set it through `set_block_count` after
    /// device config, or rely on the configured max).
    ///
    /// 39 收尾复验（P2-1）：容量事实不得被生产代码任意覆写——仅
    /// 测试/仿真可用（release 无 mock feature，编译期隔离）。
    #[cfg(any(test, feature = "mock"))]
    pub fn set_block_count(&self, blocks: u64) {
        self.block_count.set(blocks);
    }

    /// Returns a raw pointer to the DMA pool backing store.
    pub fn pool_ptr(&self) -> *mut u8 {
        self.pool.borrow_mut().as_bytes_mut().as_mut_ptr()
    }

    /// Returns a mutable borrow of the simulated MMIO (test helper).
    #[cfg(test)]
    pub fn mmio_borrow(&self) -> core::cell::RefMut<'_, M> {
        self.mmio.borrow_mut()
    }
}

impl<M: VirtioMmio, P: DmaPool, C: ArmCache> BlockDevice for VirtioBlkDriver<M, P, C> {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        if block_idx >= self.block_count.get() {
            #[cfg(target_os = "none")]
            eneros_runtime::println!(
                "[blk] read OutOfRange block={} max={}",
                block_idx,
                self.block_count.get()
            );
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count.get(),
            });
        }
        if buf.len() != self.config.block_size {
            #[cfg(target_os = "none")]
            eneros_runtime::println!(
                "[blk] read buf-len {} != bs {}",
                buf.len(),
                self.config.block_size
            );
            return Err(StorageError::HardwareFault {
                detail: "read buffer size != block size",
            });
        }
        // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 同族修复）：读路径同样是
        // 4 KiB 栈帧——boot 安装的 `read_current` 也走本函数，一并改堆。
        let mut data = alloc::vec![0u8; 4096];
        let len = self.issue(
            VIRTIO_BLK_T_IN,
            block_idx * (self.config.block_size as u64 / SECTOR_SIZE as u64),
            &mut data[..buf.len()],
            buf.len(),
        )?;
        buf.copy_from_slice(&data[..buf.len()]);
        let _ = len;
        Ok(())
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        if block_idx >= self.block_count.get() {
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count.get(),
            });
        }
        if buf.len() != self.config.block_size {
            return Err(StorageError::HardwareFault {
                detail: "write buffer size != block size",
            });
        }
        // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 真根因修复）：旧实现用
        // `let mut data = [0u8; 4096]` 在**栈上**再放一份 4 KiB，使本函数
        // 帧 = 4096 B（实弹反汇编：`sub sp, sp, #0x1000`）。boot 安装路径的
        // 调用链（block_cmd → boot_install → BootSlotBackend → 本函数）
        // 在同一线程栈上叠加后越过 svc-storage 的 64 KiB 栈 ⇒ 栈访问触发
        // seL4 fault、线程停在内核且无人应答 ⇒ 实测"整机 idle、四个 CPU
        // 全落 idle_thread、trial 永不返回"。改为**堆缓冲**（本 crate 已
        // `extern crate alloc`，且同一文件下方 erase_block 亦为 4 KiB 帧）：
        // 帧降到数百字节，长链不再触碰栈上限。
        let mut data = alloc::vec![0u8; 4096];
        data[..buf.len()].copy_from_slice(buf);
        self.issue(
            VIRTIO_BLK_T_OUT,
            block_idx * (self.config.block_size as u64 / SECTOR_SIZE as u64),
            &mut data[..buf.len()],
            buf.len(),
        )?;
        Ok(())
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        // virtio-blk 无硬件 erase 命令；按 virtio-blk 惯例上层以零填充模拟
        // 擦除（littlefs 依赖 erase 语义）。写入全零块。
        // 同 write_block：4 KiB 走堆，避免同族栈帧叠加。
        let zeros = alloc::vec![0u8; 4096];
        self.write_block(block_idx, &zeros[..self.config.block_size])
    }

    fn block_count(&self) -> u64 {
        self.block_count.get()
    }

    fn block_size(&self) -> usize {
        self.config.block_size
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.issue(VIRTIO_BLK_T_FLUSH, 0, &mut [], 0)?;
        Ok(())
    }

    fn health_status(&self) -> DeviceHealth {
        DeviceHealth {
            total_blocks: self.block_count.get(),
            bad_blocks: 0,
            wear_level: 0,
            temperature: 0,
            remaining_life: 100,
        }
    }
}

impl<M: VirtioMmio, P: DmaPool, C: ArmCache> fmt::Debug for VirtioBlkDriver<M, P, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtioBlkDriver")
            .field("block_count", &self.block_count)
            .field("initialized", &self.initialized)
            .field("legacy", &self.legacy)
            .field("block_size", &self.config.block_size)
            .finish()
    }
}

// ============================================================================
// Host simulation backends (tests)
// ============================================================================

/// RAM-backed virtio-mmio register simulation with QEMU-like semantics.
///
/// WP-26.10（26-C §4.2）：真实 MMIO 后端（生产路径）——`read_volatile`/
/// `write_volatile` 访问真实 virtio-mmio 寄存器；映射成功 ⇔ 正确物理页由
/// FIX-BUGS-0810 两件套（alloc_device_at + node_offset=slot）保证。
pub struct VolatileMmio {
    /// 寄存器基址（已映射虚拟地址）。
    base: usize,
}

impl VolatileMmio {
    /// 构造真实 MMIO 后端。`base` 必须是已映射（且覆盖足够长度）的
    /// virtio-mmio 寄存器基址。
    pub fn new(base: usize) -> Self {
        Self { base }
    }
}

impl VirtioMmio for VolatileMmio {
    fn read_reg(&self, offset: usize) -> u32 {
        // SAFETY: base 由调用方保证指向已映射的 virtio-mmio 寄存器区；
        // offset 为 virtio 规范寄存器偏移，调用方保证界内；32 位对齐读
        // （virtio-mmio 规范要求）。
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    fn write_reg(&mut self, offset: usize, value: u32) {
        // SAFETY: 同上；32 位对齐写。
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) }
    }
}

/// Models the registers the driver touches. The backing `disk` stores sector
/// data; after the driver writes `QueueNotify`, the test drives
/// [`RamMmio::process_queues`] (the role QEMU's device model plays) to consume
/// the avail ring and produce used entries.
pub struct RamMmio {
    /// 寄存器文件覆盖到 config space（0x100..0x10C：capacity 8 字节 +
    /// size_max 4 字节——39 WP-39.5 后 capacity 是驱动事实，mock 必须提供）。
    regs: [u32; 68],
    /// Shared persistent disk storage. `Rc<RefCell<..>>` lets the test keep
    /// the "media" alive across driver instances (power-loss → remount).
    disk: alloc::rc::Rc<core::cell::RefCell<alloc::vec::Vec<u8>>>,
    /// Notify latch: set when the driver writes QueueNotify.
    notified: bool,
    /// Optional raw pointer to the driver's DMA pool, injected after
    /// `VirtioBlkDriver::new` so that `QueueNotify` can synchronously run the
    /// device model (QEMU's role).
    pool_ptr: core::cell::Cell<*mut u8>,
    /// Number of 512-byte sectors on the simulated disk.
    pub disk_blocks: u64,
    /// **故障注入面**（2026-09-14，`REG-70.15-VIRTQ-FULL-UNDER-LOAD` 红测用）：
    /// 大于 0 时接下来这么多次请求以 `VIRTIO_BLK_S_IOERR` 完成（不落盘、
    /// 不回读），用来构造"设备瞬时失败"序列。设备模型必须能失败，否则
    /// "失败路径的描述符回收"这条不变量在夹具里不可证。
    pub fail_next: core::cell::Cell<u32>,
    /// **停顿注入面**（2026-09-14，`REG-70.15-BLK-DEVICE-IOERR-SUSTAINED` 红测用）：
    /// 大于 0 时 `process_queues` **不消费**待处理条目（设备"卡住"不复位也不
    /// 完成），用来构造"设备永不完成 ⇒ 驱动 `wait_used` 超时"序列——正是
    /// 长窗实弹的首发形态（`TIMEOUT device never completed … last_used=…`）。
    pub stall_next: core::cell::Cell<u32>,
}

impl RamMmio {
    /// Creates a simulation with `blocks` 512-byte sectors (fresh disk).
    pub fn new(blocks: u64) -> Self {
        Self::with_disk(alloc::vec![0u8; (blocks as usize) * SECTOR_SIZE])
    }

    /// Creates a simulation bound to an existing persistent disk (for
    /// power-loss → remount tests).
    pub fn with_disk(disk: alloc::vec::Vec<u8>) -> Self {
        let mut regs = [0u32; 68];
        regs[VMMIO_MAGIC_VALUE / 4] = VIRTIO_MMIO_MAGIC;
        regs[VMMIO_VERSION / 4] = VIRTIO_MMIO_VERSION_1;
        regs[VMMIO_DEVICE_ID / 4] = VIRTIO_BLK_DEVICE_ID;
        regs[VMMIO_QUEUE_NUM_MAX / 4] = 128; // > QUEUE_MAX（70.7：QUEUE_MAX 16→64）
        let blocks = (disk.len() / SECTOR_SIZE) as u64;
        // config space offset 0：capacity（512B 扇区数，8 字节 LE）。
        regs[0x100 / 4] = (blocks & 0xFFFF_FFFF) as u32;
        regs[0x104 / 4] = (blocks >> 32) as u32;
        RamMmio {
            regs,
            disk: alloc::rc::Rc::new(core::cell::RefCell::new(disk)),
            notified: false,
            pool_ptr: core::cell::Cell::new(core::ptr::null_mut()),
            disk_blocks: blocks,
            fail_next: core::cell::Cell::new(0),
            stall_next: core::cell::Cell::new(0),
        }
    }

    /// Creates a simulation sharing an existing persistent disk handle
    /// (zero-copy; used by power-loss → remount tests).
    pub fn with_disk_rc(disk: alloc::rc::Rc<core::cell::RefCell<alloc::vec::Vec<u8>>>) -> Self {
        let mut regs = [0u32; 68];
        regs[VMMIO_MAGIC_VALUE / 4] = VIRTIO_MMIO_MAGIC;
        regs[VMMIO_VERSION / 4] = VIRTIO_MMIO_VERSION_1;
        regs[VMMIO_DEVICE_ID / 4] = VIRTIO_BLK_DEVICE_ID;
        regs[VMMIO_QUEUE_NUM_MAX / 4] = 128; // > QUEUE_MAX（70.7：QUEUE_MAX 16→64）
        let blocks = (disk.borrow().len() / SECTOR_SIZE) as u64;
        regs[0x100 / 4] = (blocks & 0xFFFF_FFFF) as u32;
        regs[0x104 / 4] = (blocks >> 32) as u32;
        RamMmio {
            regs,
            disk,
            notified: false,
            pool_ptr: core::cell::Cell::new(core::ptr::null_mut()),
            disk_blocks: blocks,
            fail_next: core::cell::Cell::new(0),
            stall_next: core::cell::Cell::new(0),
        }
    }

    /// Returns a clone of the shared disk handle (to bind a second driver
    /// instance to the same medium after a simulated power loss).
    pub fn disk_handle(&self) -> alloc::rc::Rc<core::cell::RefCell<alloc::vec::Vec<u8>>> {
        alloc::rc::Rc::clone(&self.disk)
    }

    /// Returns a copy of the simulated disk contents (for cross-checks).
    pub fn disk(&self) -> alloc::vec::Vec<u8> {
        self.disk.borrow().clone()
    }

    /// QEMU-device role: consume all pending avail entries and fill used.
    ///
    /// `pool` is the driver's DMA pool; physical offsets are relative to
    /// [`POOL_PHYS_BASE`].
    fn process_queues(&mut self, pool: *mut u8) {
        if pool.is_null() {
            return;
        }
        // SAFETY: the pool is the driver's exclusive DMA buffer (injected via
        // attach_pool) and is laid out per our virtqueue structs.
        let pool = unsafe { core::slice::from_raw_parts_mut(pool, 9 * 4096) };
        // SAFETY: avail/used 视图区间与 desc/数据区间不重叠，布局匹配。
        let avail = unsafe { &mut *(pool[0x0100..0x1100].as_mut_ptr() as *mut VirtqAvail) };
        let used = unsafe { &mut *(pool[0x1000..0x2000].as_mut_ptr() as *mut VirtqUsed) };
        let fail_left = self.fail_next.get();
        if self.stall_next.get() > 0 {
            // 设备"卡住"：不消费 avail 条目（既无 used 项，也不推进
            // last_avail_idx）——驱动侧观察即"永不完成"。
            self.stall_next.set(self.stall_next.get() - 1);
            return;
        }
        // Process all newly available descriptors.
        while used.idx != avail.idx {
            let head = avail.ring[used.idx as usize & QUEUE_MASK] as usize;
            // SAFETY: desc 只读视图区间合法，长度 QUEUE_MAX 不越界。
            let descs = unsafe {
                core::slice::from_raw_parts_mut(
                    pool[0x0000..0x1000].as_mut_ptr() as *mut VirtqDesc,
                    QUEUE_MAX,
                )
            };
            let req_desc = &descs[head];
            let req_off = (req_desc.addr - POOL_PHYS_BASE) as usize;
            // SAFETY: `req_off` 来自 desc.addr - 池基址，位于池内；
            // 16 字节请求头不越界（生产路径同布局）。
            let req = unsafe { &*(pool[req_off..req_off + 16].as_ptr() as *const VirtioBlkReq) };
            let is_flush = req.kind == VIRTIO_BLK_T_FLUSH;
            // FLUSH 使用 2 描述符链（req→status）；读写使用 3 描述符链。
            let data_desc;
            let status_desc;
            if is_flush {
                status_desc = &descs[req_desc.next as usize];
                data_desc = status_desc;
            } else {
                data_desc = &descs[req_desc.next as usize];
                status_desc = &descs[data_desc.next as usize];
            }
            let data_off = if is_flush {
                0
            } else {
                (data_desc.addr - POOL_PHYS_BASE) as usize
            };
            let status_off = (status_desc.addr - POOL_PHYS_BASE) as usize;
            match req.kind {
                VIRTIO_BLK_T_IN if fail_left > 0 => {
                    // 注入的瞬时失败：设备"看见"请求但报 IOERR（不改介质）。
                    pool[status_off] = VIRTIO_BLK_S_IOERR;
                }
                VIRTIO_BLK_T_IN => {
                    let start = (req.sector as usize) * SECTOR_SIZE;
                    let len = data_desc.len as usize;
                    let disk = self.disk.borrow();
                    if start + len <= disk.len() {
                        pool[data_off..data_off + len].copy_from_slice(&disk[start..start + len]);
                        pool[status_off] = VIRTIO_BLK_S_OK;
                    } else {
                        pool[status_off] = VIRTIO_BLK_S_IOERR;
                    }
                }
                VIRTIO_BLK_T_OUT if fail_left > 0 => {
                    pool[status_off] = VIRTIO_BLK_S_IOERR;
                }
                VIRTIO_BLK_T_OUT => {
                    let start = (req.sector as usize) * SECTOR_SIZE;
                    let len = data_desc.len as usize;
                    let mut disk = self.disk.borrow_mut();
                    if start + len <= disk.len() {
                        disk[start..start + len].copy_from_slice(&pool[data_off..data_off + len]);
                        pool[status_off] = VIRTIO_BLK_S_OK;
                    } else {
                        pool[status_off] = VIRTIO_BLK_S_IOERR;
                    }
                }
                VIRTIO_BLK_T_FLUSH => {
                    pool[status_off] = VIRTIO_BLK_S_OK;
                }
                _ => {
                    pool[status_off] = VIRTIO_BLK_S_IOERR;
                }
            }
            used.ring[used.idx as usize & QUEUE_MASK] = VirtqUsedElem {
                id: head as u32,
                len: if is_flush {
                    status_desc.len
                } else {
                    data_desc.len
                },
            };
            used.idx = used.idx.wrapping_add(1);
            if self.fail_next.get() > 0 {
                self.fail_next.set(self.fail_next.get() - 1);
            }
        }
    }
}

/// Physical base address of the DMA pool in host simulations.
const POOL_PHYS_BASE: u64 = 0x1000_0000;

impl VirtioMmio for RamMmio {
    fn read_reg(&self, offset: usize) -> u32 {
        // 31 DR-09：config space 读取越界防护（mock regs 只有 0x100 字节；
        // capacity 探测越界 → 0 → 上层回退占位）。
        self.regs.get(offset / 4).copied().unwrap_or(0)
    }
    fn write_reg(&mut self, offset: usize, value: u32) {
        // QueueNotify signals the (virtual) device to process the ring.
        if offset == VMMIO_QUEUE_NOTIFY {
            self.notified = true;
            self.process_queues(self.pool_ptr.get());
        }
        self.regs[offset / 4] = value;
    }
    fn post_init(&self, pool: *mut u8) {
        self.pool_ptr.set(pool);
    }
}

/// RAM-backed DMA pool for host tests.
pub struct RamPool {
    bytes: Vec<u8>,
}

impl RamPool {
    pub fn new(npages: usize) -> Self {
        Self {
            bytes: alloc::vec![0u8; npages * 4096],
        }
    }
}

impl DmaPool for RamPool {
    fn len(&self) -> usize {
        self.bytes.len()
    }
    fn phys_base(&self) -> u64 {
        POOL_PHYS_BASE
    }
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use core::cell::RefCell;

    use super::*;
    use crate::driver::StorageType;

    /// 追踪型 cache：记录 clean/invalidate 调用，用于验证 DMA cache
    /// 维护时序（v0.214.0 B1-02 压测：无 cache 一致性数据损坏）。
    #[derive(Default)]
    struct TrackingCache {
        cleans: RefCell<usize>,
        invalidates: RefCell<usize>,
        last_invalidate_va: RefCell<usize>,
    }

    impl ArmCache for TrackingCache {
        fn clean(&self, _va: usize, _len: usize) {
            *self.cleans.borrow_mut() += 1;
        }
        fn invalidate(&self, va: usize, _len: usize) {
            *self.invalidates.borrow_mut() += 1;
            *self.last_invalidate_va.borrow_mut() = va;
        }
    }

    fn config() -> StorageConfig {
        StorageConfig {
            storage_type: StorageType::VirtioBlk,
            base_addr: 0x0A00_0000,
            irq_num: 48,
            dma_channel: 0,
            block_size: 512,
            max_transfer_blocks: 256,
        }
    }

    #[test]
    fn test_device_init_magic_and_id() {
        let mmio = RamMmio::new(1024);
        assert_eq!(mmio.read_reg(VMMIO_MAGIC_VALUE), VIRTIO_MMIO_MAGIC);
        assert_eq!(mmio.read_reg(VMMIO_DEVICE_ID), VIRTIO_BLK_DEVICE_ID);
    }

    /// 39 WP-39.5（P1-14）mutation：capacity=0 必须拒绝（旧实现虚构
    /// max_transfer_blocks*16 块——猜容量即允许越界写）。
    #[test]
    fn test_capacity_zero_rejected_no_fabrication() {
        let mmio = RamMmio::new(0); // 0 扇区 → config capacity = 0
        let pool = RamPool::new(9);
        let result = VirtioBlkDriver::new(mmio, pool, NoopCache, config());
        assert!(
            matches!(result, Err(StorageError::Unsupported { .. })),
            "capacity=0 必须 fail-closed，不得虚构容量"
        );
    }

    #[test]
    fn test_write_then_read_roundtrip() {
        let mmio = RamMmio::new(1024);
        let pool = RamPool::new(9);
        let mut drv = VirtioBlkDriver::new(mmio, pool, NoopCache, config()).expect("init");
        drv.set_block_count(1024);

        let input = [0x5A; 512];
        drv.write_block(3, &input).expect("write");
        let mut out = [0u8; 512];
        drv.read_block(3, &mut out).expect("read");
        assert_eq!(out, input);
    }

    /// 2026-09-14（70.15 长窗实弹红测）：**失败请求不得泄漏 virtqueue
    /// 描述符**（`REG-70.15-VIRTQ-FULL-UNDER-LOAD`）。
    ///
    /// 缺陷形态：`issue` 在 `wait_used()?` 与 `status != OK` 两条错误出口
    /// **都没有** `desc_used -= ndesc` ⇒ 每次失败永久漏 2–3 个描述符；实测
    /// 数次瞬时失败后队列打满 ⇒ 之后每次 `issue` 都 `HardwareFault{"virtqueue
    /// full"}` ⇒ 块层整条死掉（快照落盘饿死 ⇒ 控制链"活着但不再控制"）。
    ///
    /// 判据两面：① 注入的 30 次失败必须都是**设备 IOERR**（不是队列打满——
    /// 后者说明描述符在漏）；② 故障序列结束后块层必须**仍然可用**。
    #[test]
    fn test_failed_requests_do_not_leak_virtqueue_descriptors() {
        let mmio = RamMmio::new(1024);
        mmio.fail_next.set(30);
        let pool = RamPool::new(9);
        let mut drv = VirtioBlkDriver::new(mmio, pool, NoopCache, config()).expect("init");
        drv.set_block_count(1024);
        let mut out = [0u8; 512];
        for i in 0..30u32 {
            match drv.read_block(3, &mut out) {
                Err(StorageError::HardwareFault { detail }) => assert!(
                    !detail.contains("virtqueue full"),
                    "第 {i} 次请求打成队列满——描述符在失败路径上没有被回收：{detail}"
                ),
                other => {
                    #[allow(clippy::disallowed_macros)] // 测试断言
                    {
                        panic!("第 {i} 次必须是设备 IOERR，实际 {other:?}")
                    }
                }
            }
        }
        // 注入结束：块层必须恢复可用（旧实现此处必炸 "virtqueue full"）。
        drv.read_block(3, &mut out)
            .expect("故障序列后块层必须仍可用");
        let input = [0x77u8; 512];
        drv.write_block(5, &input).expect("write");
        drv.read_block(5, &mut out).expect("read");
        assert_eq!(out, input);
    }

    /// 2026-09-14（70.15 长窗实弹红测）：**设备停顿不得把队列永久钉死**。
    ///
    /// 缺陷形态（实弹）：`[blk-wait] TIMEOUT device never completed (20M spins)
    /// last_used=38028`（该值此后冻结）⇒ 设备侧 `last_avail_idx` 与 guest 侧
    /// `avail.idx` 失步 ⇒ 之后**所有**请求静默丢弃 ⇒ 快照落盘饿死 ⇒ 控制链
    /// "活着但不再控制"。旧实现超时后既不回收描述符也不复位队列 ⇒ 一次停顿
    /// = 块层永久死。
    ///
    /// 判据两面：
    /// ① **单次停顿必须被吸收**——复位重建 + 重试 ⇒ 请求成功（旧实现：直接
    ///   Timeout，且队列带伤）；
    /// ② **持续停顿** ⇒ 干净失败为 `Timeout` 且 `desc_used` 归零（不留半用
    ///   描述符——否则队列永久 `virtqueue full`）。
    #[test]
    fn test_device_stall_recovers_queue_instead_of_bricking() {
        // ① 单次停顿：吸收（复位重建 + 重试 ⇒ 成功）。
        let mmio = RamMmio::new(1024);
        mmio.stall_next.set(1);
        let pool = RamPool::new(9);
        let mut drv = VirtioBlkDriver::new(mmio, pool, NoopCache, config()).expect("init");
        drv.set_block_count(1024);
        let input = [0x3Cu8; 512];
        drv.write_block(7, &input)
            .expect("单次设备停顿必须被队列复位 + 重试吸收（旧实现此处 Timeout）");

        // ② 持续停顿：干净失败 + 描述符归零。
        let mmio2 = RamMmio::new(1024);
        mmio2.stall_next.set(u32::MAX);
        let pool2 = RamPool::new(9);
        let mut drv2 = VirtioBlkDriver::new(mmio2, pool2, NoopCache, config()).expect("init");
        drv2.set_block_count(1024);
        let input2 = [0x3Du8; 512];
        let r = drv2.write_block(7, &input2);
        assert!(
            matches!(r, Err(StorageError::Timeout { .. })),
            "持续停顿时必须判超时，实际 {r:?}"
        );
        assert_eq!(
            drv2.desc_used.get(),
            0,
            "超时后描述符必须归零（回收 + 队列复位重建）"
        );
    }

    #[test]
    fn test_flush_and_read_other_block() {
        let mmio = RamMmio::new(1024);
        let pool = RamPool::new(9);
        let mut drv = VirtioBlkDriver::new(mmio, pool, NoopCache, config()).expect("init");
        drv.set_block_count(1024);
        drv.flush().expect("flush");
        let mut out = [0u8; 512];
        drv.read_block(0, &mut out).expect("read");
        assert_eq!(out, [0u8; 512]);
    }

    #[test]
    fn test_out_of_range_read_rejected() {
        let mmio = RamMmio::new(16);
        let pool = RamPool::new(9);
        let drv = VirtioBlkDriver::new(mmio, pool, NoopCache, config()).expect("init");
        drv.set_block_count(16);
        let mut out = [0u8; 512];
        assert!(drv.read_block(16, &mut out).is_err());
    }

    #[test]
    fn test_erase_zero_fills() {
        let mmio = RamMmio::new(1024);
        let pool = RamPool::new(9);
        let mut drv = VirtioBlkDriver::new(mmio, pool, NoopCache, config()).expect("init");
        drv.set_block_count(128);
        let input = [0xAB; 512];
        drv.write_block(5, &input).expect("write");
        drv.erase_block(5).expect("erase (zero-fill)");
        let mut out = [0u8; 512];
        drv.read_block(5, &mut out).expect("read");
        assert_eq!(out, [0u8; 512], "erase must zero-fill the block");
    }

    #[test]
    fn test_health_status() {
        let mmio = RamMmio::new(16);
        let pool = RamPool::new(9);
        let drv = VirtioBlkDriver::new(mmio, pool, NoopCache, config()).expect("init");
        drv.set_block_count(16);
        let h = drv.health_status();
        assert_eq!(h.total_blocks, 16);
        assert_eq!(h.remaining_life, 100);
    }

    // ===== v0.214.0（B1-02）：DMA cache 维护与压测 =====

    #[test]
    fn test_cache_clean_before_notify_and_invalidate_after_used() {
        // 验证 cache 维护时序：描述符/请求/OUT 数据在 notify 前 clean；
        // used ring 轮询与 status/IN 数据读回前 invalidate。
        let mmio = RamMmio::new(1024);
        let pool = RamPool::new(9);
        let cache = TrackingCache::default();
        let mut drv = VirtioBlkDriver::new(mmio, pool, cache, config()).expect("init");
        drv.set_block_count(1024);

        let input = [0xC3; 512];
        drv.write_block(1, &input).expect("write");
        // write 路径：descs/req/OUT data clean（≥1）+ used ring invalidate（≥1）
        assert!(
            *drv.cache.cleans.borrow() >= 1,
            "OUT 数据须在 notify 前 clean"
        );
        assert!(
            *drv.cache.invalidates.borrow() >= 1,
            "used ring 须在轮询前 invalidate"
        );

        let mut out = [0u8; 512];
        drv.read_block(1, &mut out).expect("read");
        // read 路径：IN 数据 invalidate 已发生
        assert!(
            *drv.cache.invalidates.borrow() >= 2,
            "IN 数据读回前须 invalidate"
        );
        assert_eq!(out, input);
    }

    #[test]
    fn test_stress_random_roundtrip_no_corruption() {
        // 压测：随机块×随机内容多次往返，验证 cache 一致性与数据无损坏。
        let mmio = RamMmio::new(1024);
        let pool = RamPool::new(9);
        let cache = TrackingCache::default();
        let mut drv = VirtioBlkDriver::new(mmio, pool, cache, config()).expect("init");
        drv.set_block_count(64);

        // 固定伪随机序列（确定性，可重算）。
        let mut seed = 0x1234_5678u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as u8
        };

        for round in 0..200u32 {
            let block = (round * 7 % 64) as u64;
            let mut input = [0u8; 512];
            for b in input.iter_mut() {
                *b = next();
            }
            drv.write_block(block, &input).expect("write");
            let mut out = [0u8; 512];
            drv.read_block(block, &mut out).expect("read");
            assert_eq!(
                out, input,
                "round {round} block {block}: cache 一致性数据损坏"
            );
        }
        // 压测全程每次 IO 都有 cache 维护动作。
        assert!(*drv.cache.cleans.borrow() >= 200);
        assert!(*drv.cache.invalidates.borrow() >= 400);
    }

    #[test]
    fn test_stress_interleaved_multi_block_no_cross_corruption() {
        // 多块交错读写：验证不同块间无串扰（cache 维护范围正确）。
        let mmio = RamMmio::new(1024);
        let pool = RamPool::new(9);
        let mut drv = VirtioBlkDriver::new(mmio, pool, NoopCache, config()).expect("init");
        drv.set_block_count(32);

        let pattern_a = [0xAA; 512];
        let pattern_b = [0x55; 512];
        for i in 0..64u32 {
            let block = (i % 4) as u64;
            if i % 2 == 0 {
                drv.write_block(block, &pattern_a).expect("write A");
            } else {
                drv.write_block(block, &pattern_b).expect("write B");
            }
            let mut out = [0u8; 512];
            drv.read_block(block, &mut out).expect("read");
            let expect = if i % 2 == 0 { pattern_a } else { pattern_b };
            assert_eq!(out, expect, "block {block} 数据串扰");
        }
    }
}
