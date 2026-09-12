//! EnerOS virtio-core — virtio-mmio 共享原语（蓝图 39 WP-39.5/WP-39.7）。
//!
//! 从 `eneros-storage::driver::virtio_blk` 抽取的**传输层公共部分**：
//! 寄存器偏移、设备状态位、virtqueue 环形结构、寄存器访问 trait 与 DMA 池
//! trait。块驱动（storage）与网络驱动（net）共用同一套 virtio 原语，
//! 禁止各自复制一份偏移常量或 ring 布局（35-01 §3 同义类型纪律）。
//!
//! 本 crate 零依赖、`no_std`；不含任何 virtio-blk/virtio-net 业务语义。
//!
//! # 布局（legacy virtqueue，与 `root_task.c` 的 VMMIO_* 一致）
//!
//! ```text
//! 描述符表：    [VirtqDesc; QUEUE_MAX]      (16 × 16 B)
//! 可用环：      VirtqAvail                  (flags + idx + ring[16] + used_event)
//! 已用环：      VirtqUsed                   (flags + idx + ring[16] + avail_event)
//! ```

#![cfg_attr(not(test), no_std)]

// ============================================================================
// virtio-mmio register offsets (matching root_task.c VMMIO_*)
// ============================================================================

/// Magic value register — must read 0x7472_6976 ("virt").
pub const VMMIO_MAGIC_VALUE: usize = 0x000;
/// Device version (1 = legacy, 2 = modern).
pub const VMMIO_VERSION: usize = 0x004;
/// Device ID (2 = block, 1 = net).
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
/// Guest page size (legacy) — QEMU/Linux UAPI 偏移 0x028（与 C 参考
/// `root_task.c VMMIO_GUEST_PAGE_SIZE` 一致）。写 4096 → QEMU
/// `guest_page_shift=ctz32(4096)=12`，QueuePFN 写入后左移该值得到
/// desc 表物理地址。旧值 0x08C 是无效偏移，QEMU 忽略 → shift 恒 0 →
/// desc 地址 = PFN 值本身（缺 <<12），virtio-blk 请求超时（41 根因，
/// QEMU 9.2.4 `virtio-mmio.c` 实测）。
pub const VMMIO_GUEST_PAGE_SIZE: usize = 0x028;
/// Queue descriptor low/high (modern).
pub const VMMIO_QUEUE_DESC_LOW: usize = 0x080;
pub const VMMIO_QUEUE_DESC_HIGH: usize = 0x084;
/// Queue driver (avail) low/high (modern).
pub const VMMIO_QUEUE_DRIVER_LOW: usize = 0x090;
pub const VMMIO_QUEUE_DRIVER_HIGH: usize = 0x094;
/// Queue device (used) low/high (modern).
pub const VMMIO_QUEUE_DEVICE_LOW: usize = 0x0A0;
pub const VMMIO_QUEUE_DEVICE_HIGH: usize = 0x0A4;
/// Queue align (legacy) — QEMU/Linux UAPI 偏移 0x03C（与 C 参考
/// `root_task.c VMMIO_QUEUE_ALIGN` 一致）。旧值 0x0AC 是无效偏移
/// （QEMU 该处为 SHM_SEL），写 4096 被忽略；QEMU 复位默认
/// align=4096，故布局未受影响，但必须按规范修正。
pub const VMMIO_QUEUE_ALIGN: usize = 0x03C;

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

/// Descriptor flags.
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

/// Queue capacity (16 descriptors, matching C reference).
/// 70.7 登记：QUEUE_MAX=64 实验曾尝试绕开 #16 读挂死——伴随 stress
/// 测试 sim 索引越界与布局连锁，未采纳（回归）。
pub const QUEUE_MAX: usize = 16;
pub const QUEUE_MASK: usize = QUEUE_MAX - 1;

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

// ============================================================================
// Transport / memory abstractions
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 布局不变量：描述符表/可用环/已用环在 pool 内的固定偏移。
    /// （与 root_task.c 的 virtio 队列布局一致——被 blk 与 net 驱动共用。）
    #[test]
    fn ring_layout_offsets_are_stable() {
        assert_eq!(core::mem::size_of::<VirtqDesc>(), 16);
        // avail = flags(2)+idx(2)+ring(2×16)+used_event(2) = 38（align 2）。
        assert_eq!(core::mem::size_of::<VirtqAvail>(), 38);
        assert_eq!(core::mem::size_of::<VirtqUsedElem>(), 8);
        // used = flags(2)+idx(2)+ring(8×16)+avail_event(2) = 134 → align 4 补齐。
        assert_eq!(core::mem::size_of::<VirtqUsed>(), 136);
        // 池内偏移：desc 表 0x0000，avail 0x0100，used 0x0200（各 1 页内）。
        assert_eq!(core::mem::size_of::<VirtqDesc>() * QUEUE_MAX, 0x100);
    }

    #[test]
    fn status_bits_match_spec() {
        assert_eq!(VIRTIO_STATUS_ACKNOWLEDGE, 1);
        assert_eq!(VIRTIO_STATUS_DRIVER, 2);
        assert_eq!(VIRTIO_STATUS_FEATURES_OK, 8);
        assert_eq!(VIRTIO_STATUS_DRIVER_OK, 4);
    }
}
