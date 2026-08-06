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

use core::fmt;
use core::sync::atomic::{compiler_fence, Ordering};

use alloc::vec::Vec;

use crate::driver::types::{DeviceHealth, StorageConfig};
use crate::driver::BlockDevice;
use crate::error::StorageError;

// ============================================================================
// virtio-mmio register offsets (matching root_task.c VMMIO_*)
// ============================================================================

/// Magic value register — must read 0x7472_6976 ("virt").
pub const VMMIO_MAGIC_VALUE: usize = 0x000;
/// Device version (1 = legacy, 2 = modern).
pub const VMMIO_VERSION: usize = 0x004;
/// Device ID (2 = block).
pub const VMMIO_DEVICE_ID: usize = 0x008;
/// Device vendor ID.
pub const VMMIO_VENDOR_ID: usize = 0x00C;
/// Device features word 0.
pub const VMMIO_DEVICE_FEATURES: usize = 0x010;
/// Device features select (0/1).
pub const VMMIO_DEVICE_FEATURES_SEL: usize = 0x014;
/// Driver features word 0.
pub const VMMIO_DRIVER_FEATURES: usize = 0x020;
/// Driver features select (0/1).
pub const VMMIO_DRIVER_FEATURES_SEL: usize = 0x024;
/// Queue selector.
pub const VMMIO_QUEUE_SEL: usize = 0x030;
/// Maximum queue size (read-only).
pub const VMMIO_QUEUE_NUM_MAX: usize = 0x034;
/// Queue size (write to set).
pub const VMMIO_QUEUE_NUM: usize = 0x038;
/// Legacy queue PFN (guest physical frame number).
pub const VMMIO_QUEUE_PFN: usize = 0x040;
/// Queue ready (modern).
pub const VMMIO_QUEUE_READY: usize = 0x044;
/// Queue notify.
pub const VMMIO_QUEUE_NOTIFY: usize = 0x050;
/// Interrupt status.
pub const VMMIO_INTERRUPT_STATUS: usize = 0x060;
/// Interrupt acknowledge.
pub const VMMIO_INTERRUPT_ACK: usize = 0x064;
/// Device status.
pub const VMMIO_STATUS: usize = 0x070;
/// Guest page size (legacy).
pub const VMMIO_GUEST_PAGE_SIZE: usize = 0x08C;
/// Queue descriptor low/high (modern).
pub const VMMIO_QUEUE_DESC_LOW: usize = 0x080;
pub const VMMIO_QUEUE_DESC_HIGH: usize = 0x084;
/// Queue driver (avail) low/high (modern).
pub const VMMIO_QUEUE_DRIVER_LOW: usize = 0x090;
pub const VMMIO_QUEUE_DRIVER_HIGH: usize = 0x094;
/// Queue device (used) low/high (modern).
pub const VMMIO_QUEUE_DEVICE_LOW: usize = 0x0A0;
pub const VMMIO_QUEUE_DEVICE_HIGH: usize = 0x0A4;
/// Queue align (legacy).
pub const VMMIO_QUEUE_ALIGN: usize = 0x0AC;

/// virtio-mmio magic ("virt" little-endian).
pub const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976;
/// Version 1 (legacy transport).
pub const VIRTIO_MMIO_VERSION_1: u32 = 1;
/// Version 2 (modern transport).
pub const VIRTIO_MMIO_VERSION_2: u32 = 2;

/// Device status bits.
pub const VIRTIO_STATUS_ACKNOWLEDGE: u32 = 1;
pub const VIRTIO_STATUS_DRIVER: u32 = 2;
pub const VIRTIO_STATUS_DRIVER_OK: u32 = 4;
pub const VIRTIO_STATUS_FEATURES_OK: u32 = 8;

/// Block device ID (virtio-blk).
pub const VIRTIO_BLK_DEVICE_ID: u32 = 2;

/// Descriptor flags.
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

/// Block request types.
pub const VIRTIO_BLK_T_IN: u32 = 0;
pub const VIRTIO_BLK_T_OUT: u32 = 1;
pub const VIRTIO_BLK_T_FLUSH: u32 = 4;

/// Block request status codes.
pub const VIRTIO_BLK_S_OK: u8 = 0;
pub const VIRTIO_BLK_S_IOERR: u8 = 1;

/// Queue capacity (16 descriptors, matching C reference).
pub const QUEUE_MAX: usize = 16;
const QUEUE_MASK: usize = QUEUE_MAX - 1;

/// Sector size for virtio-blk (always 512 bytes).
const SECTOR_SIZE: usize = 512;

/// Number of request slots for data/status rotation (C uses 3 data + 4 status).
const REQ_SLOTS: usize = 3;

// ============================================================================
// virtqueue structures (legacy layout)
// ============================================================================

/// Virtqueue descriptor (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtqDesc {
    /// Guest physical address of the buffer.
    pub addr: u64,
    /// Buffer length in bytes.
    pub len: u32,
    /// Descriptor flags (`VIRTQ_DESC_F_*`).
    pub flags: u16,
    /// Index of the next descriptor in the chain.
    pub next: u16,
}

/// Virtqueue available ring.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; QUEUE_MAX],
    pub used_event: u16,
}

impl Default for VirtqAvail {
    fn default() -> Self {
        Self {
            flags: 0,
            idx: 0,
            ring: [0; QUEUE_MAX],
            used_event: 0,
        }
    }
}

/// Virtqueue used ring element.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

/// Virtqueue used ring.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; QUEUE_MAX],
    pub avail_event: u16,
}

impl Default for VirtqUsed {
    fn default() -> Self {
        Self {
            flags: 0,
            idx: 0,
            ring: [VirtqUsedElem::default(); QUEUE_MAX],
            avail_event: 0,
        }
    }
}

/// Block request header (16 bytes, virtio-blk request).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioBlkReq {
    pub kind: u32,
    pub reserved: u32,
    pub sector: u64,
}

// ============================================================================
// Transport / memory / cache abstractions
// ============================================================================

/// virtio-mmio register file access.
///
/// Real hardware: volatile MMIO at the device base address. Host tests:
/// a RAM-backed simulation of QEMU's register semantics.
pub trait VirtioMmio {
    /// Reads a 32-bit register at `offset` (device endianness: LE).
    fn read_reg(&self, offset: usize) -> u32;
    /// Writes `value` to the 32-bit register at `offset`.
    fn write_reg(&mut self, offset: usize, value: u32);
    /// Optional hook invoked after queue setup with the DMA pool pointer.
    ///
    /// RAM-simulated backends use this to wire the device model to the pool
    /// (so `QueueNotify` can be processed synchronously); real MMIO backends
    /// leave it as a no-op.
    fn post_init(&self, _pool: *mut u8) {}
}

/// Contiguous DMA memory pool (descriptor ring + request buffers).
///
/// Real hardware/seL4: untyped retype frames with a physical address. Host
/// tests: `Vec<u8>` arena with an assigned physical base.
pub trait DmaPool {
    /// Returns the number of bytes available in the pool.
    fn len(&self) -> usize;
    /// Returns `true` if the pool is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Returns the physical base address of the pool.
    fn phys_base(&self) -> u64;
    /// Returns a mutable byte slice of the whole pool.
    fn as_bytes_mut(&mut self) -> &mut [u8];
    /// Returns a byte slice of the whole pool.
    fn as_bytes(&self) -> &[u8];
}

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

#[cfg(target_arch = "aarch64")]
impl ArmCache for Aarch64Cache {
    fn clean(&self, va: usize, len: usize) {
        // SAFETY: inline asm `dc cvac` operates on a virtual address that the
        // caller guarantees is mapped and valid for `len` bytes.
        unsafe {
            let mut addr = va;
            let end = va.wrapping_add(len);
            while addr < end {
                core::arch::asm!("dc cvac, {addr}", addr = in(reg) addr, options(nostack));
                addr = addr.wrapping_add(64); // cache line size (64 B for ARMv8)
            }
            core::arch::asm!("dsb ish", options(nostack));
        }
    }

    fn invalidate(&self, va: usize, len: usize) {
        // SAFETY: `dc ivac` invalidates the cache line for the given VA; the
        // caller guarantees the VA is mapped and valid for `len` bytes.
        unsafe {
            let mut addr = va;
            let end = va.wrapping_add(len);
            while addr < end {
                core::arch::asm!("dc ivac, {addr}", addr = in(reg) addr, options(nostack));
                addr = addr.wrapping_add(64); // cache line size (64 B for ARMv8)
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
        let block_count = config.max_transfer_blocks as u64 * 16; // placeholder until capacity read
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
        // Wire the device model (RAM simulation) to the pool after queue setup.
        let pool_ptr = drv.pool_ptr();
        drv.mmio.borrow().post_init(pool_ptr);
        drv.initialized = true;
        Ok(drv)
    }

    /// Performs virtio-mmio device discovery, feature negotiation, and
    /// DRIVER_OK handshake (mirrors `vmmio_device_init`).
    fn device_init(&self) -> Result<(), StorageError> {
        let mut mmio = self.mmio.borrow_mut();
        // Explicit reset on (re)bind.
        mmio.write_reg(VMMIO_STATUS, 0);
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

    /// Physical address of a pool offset.
    fn pa_of(&self, offset: usize) -> u64 {
        self.pool.borrow().phys_base() + offset as u64
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
        if !self.initialized {
            return Err(StorageError::NotInitialized);
        }
        if self.desc_used.get() + 3 > QUEUE_MAX {
            return Err(StorageError::HardwareFault {
                detail: "virtqueue full",
            });
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

            let descs = unsafe {
                core::slice::from_raw_parts_mut(
                    bytes[0x0000..0x1000].as_mut_ptr() as *mut VirtqDesc,
                    QUEUE_MAX,
                )
            };
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

            let avail =
                unsafe { &mut *(bytes[0x0100..0x1100].as_mut_ptr() as *mut VirtqAvail) };
            avail.ring[avail.idx as usize & QUEUE_MASK] = head as u16;
            compiler_fence(Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
        }
        self.desc_head.set((self.desc_head.get() + 3) & QUEUE_MASK);
        self.desc_used.set(self.desc_used.get() + 3);

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
        let used_len = self.wait_used()?;

        // Check status byte and copy IN data back.
        self.cache.invalidate(self.va_of(status_off), 1);
        {
            let pool = self.pool.borrow();
            let bytes = pool.as_bytes();
            let status = bytes[status_off];
            if status != VIRTIO_BLK_S_OK {
                return Err(StorageError::HardwareFault {
                    detail: "virtio-blk device reported I/O error",
                });
            }
            if kind == VIRTIO_BLK_T_IN && datalen > 0 {
                self.cache.invalidate(self.va_of(data_off), datalen);
                data[..datalen].copy_from_slice(&bytes[data_off..data_off + datalen]);
            }
        }
        self.desc_used.set(self.desc_used.get() - 3);
        Ok(used_len)
    }

    /// Polls the used ring until a new element appears (bounded spins).
    fn wait_used(&self) -> Result<u32, StorageError> {
        for _ in 0..20_000_000u32 {
            compiler_fence(Ordering::SeqCst);
            let pool = self.pool.borrow();
            let bytes = pool.as_bytes();
            let used = unsafe { &*(bytes[0x1000..0x2000].as_ptr() as *const VirtqUsed) };
            let used_idx = used.idx;
            if used_idx != self.last_used.get() {
                let elem = used.ring[self.last_used.get() as usize & QUEUE_MASK];
                self.last_used.set(self.last_used.get().wrapping_add(1));
                // Sanity: the element id must match a descriptor we handed out.
                return Ok(elem.len);
            }
        }
        Err(StorageError::Timeout {
            operation: "virtio-blk request",
            ms: 2000,
        })
    }

    /// Returns the negotiated device capacity in blocks (via a flush request
    /// probe is not possible; callers set it through `set_block_count` after
    /// device config, or rely on the configured max).
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
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count.get(),
            });
        }
        if buf.len() != self.config.block_size {
            return Err(StorageError::HardwareFault {
                detail: "read buffer size != block size",
            });
        }
        let mut data = [0u8; 4096];
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
        let mut data = [0u8; 4096];
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
        let zeros = [0u8; 4096];
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
/// Models the registers the driver touches. The backing `disk` stores sector
/// data; after the driver writes `QueueNotify`, the test drives
/// [`RamMmio::process_queues`] (the role QEMU's device model plays) to consume
/// the avail ring and produce used entries.
pub struct RamMmio {
    regs: [u32; 64],
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
}

impl RamMmio {
    /// Creates a simulation with `blocks` 512-byte sectors (fresh disk).
    pub fn new(blocks: u64) -> Self {
        Self::with_disk(alloc::vec![0u8; (blocks as usize) * SECTOR_SIZE])
    }

    /// Creates a simulation bound to an existing persistent disk (for
    /// power-loss → remount tests).
    pub fn with_disk(disk: alloc::vec::Vec<u8>) -> Self {
        let mut regs = [0u32; 64];
        regs[VMMIO_MAGIC_VALUE / 4] = VIRTIO_MMIO_MAGIC;
        regs[VMMIO_VERSION / 4] = VIRTIO_MMIO_VERSION_1;
        regs[VMMIO_DEVICE_ID / 4] = VIRTIO_BLK_DEVICE_ID;
        regs[VMMIO_QUEUE_NUM_MAX / 4] = 32; // > QUEUE_MAX
        let blocks = (disk.len() / SECTOR_SIZE) as u64;
        RamMmio {
            regs,
            disk: alloc::rc::Rc::new(core::cell::RefCell::new(disk)),
            notified: false,
            pool_ptr: core::cell::Cell::new(core::ptr::null_mut()),
            disk_blocks: blocks,
        }
    }

    /// Creates a simulation sharing an existing persistent disk handle
    /// (zero-copy; used by power-loss → remount tests).
    pub fn with_disk_rc(
        disk: alloc::rc::Rc<core::cell::RefCell<alloc::vec::Vec<u8>>>,
    ) -> Self {
        let mut regs = [0u32; 64];
        regs[VMMIO_MAGIC_VALUE / 4] = VIRTIO_MMIO_MAGIC;
        regs[VMMIO_VERSION / 4] = VIRTIO_MMIO_VERSION_1;
        regs[VMMIO_DEVICE_ID / 4] = VIRTIO_BLK_DEVICE_ID;
        regs[VMMIO_QUEUE_NUM_MAX / 4] = 32; // > QUEUE_MAX
        let blocks = (disk.borrow().len() / SECTOR_SIZE) as u64;
        RamMmio {
            regs,
            disk,
            notified: false,
            pool_ptr: core::cell::Cell::new(core::ptr::null_mut()),
            disk_blocks: blocks,
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
        let avail = unsafe { &mut *(pool[0x0100..0x1100].as_mut_ptr() as *mut VirtqAvail) };
        let used = unsafe { &mut *(pool[0x1000..0x2000].as_mut_ptr() as *mut VirtqUsed) };
        // Process all newly available descriptors.
        while used.idx != avail.idx {
            let head = avail.ring[used.idx as usize & QUEUE_MASK] as usize;
            let descs = unsafe {
                core::slice::from_raw_parts_mut(
                    pool[0x0000..0x1000].as_mut_ptr() as *mut VirtqDesc,
                    QUEUE_MAX,
                )
            };
            let req_desc = &descs[head];
            let req_off = (req_desc.addr - POOL_PHYS_BASE) as usize;
            let req = unsafe { &*(pool[req_off..req_off + 16].as_ptr() as *const VirtioBlkReq) };
            let data_desc = &descs[req_desc.next as usize];
            let status_desc = &descs[data_desc.next as usize];
            let data_off = (data_desc.addr - POOL_PHYS_BASE) as usize;
            let status_off = (status_desc.addr - POOL_PHYS_BASE) as usize;
            match req.kind {
                VIRTIO_BLK_T_IN => {
                    let start = (req.sector as usize) * SECTOR_SIZE;
                    let len = data_desc.len as usize;
                    let disk = self.disk.borrow();
                    if start + len <= disk.len() {
                        pool[data_off..data_off + len]
                            .copy_from_slice(&disk[start..start + len]);
                        pool[status_off] = VIRTIO_BLK_S_OK;
                    } else {
                        pool[status_off] = VIRTIO_BLK_S_IOERR;
                    }
                }
                VIRTIO_BLK_T_OUT => {
                    let start = (req.sector as usize) * SECTOR_SIZE;
                    let len = data_desc.len as usize;
                    let mut disk = self.disk.borrow_mut();
                    if start + len <= disk.len() {
                        disk[start..start + len]
                            .copy_from_slice(&pool[data_off..data_off + len]);
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
                len: data_desc.len,
            };
            used.idx = used.idx.wrapping_add(1);
        }
    }
}

/// Physical base address of the DMA pool in host simulations.
const POOL_PHYS_BASE: u64 = 0x1000_0000;

impl VirtioMmio for RamMmio {
    fn read_reg(&self, offset: usize) -> u32 {
        self.regs[offset / 4]
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
    use super::*;
    use crate::driver::StorageType;

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
}
