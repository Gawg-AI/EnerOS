//! DMA 内存池与 cache 维护抽象（P0-6，v0.196.0）.
//!
//! 以太网 DMA 描述符环需要**连续物理内存**（描述符数组 + TX/RX 缓冲），
//! 以及 DMA 传输前后的 cache 一致性维护。本模块提供与
//! `eneros-storage` 的 virtio-blk 一致的两层抽象：
//!
//! - [`DmaPool`]：连续 DMA 内存（host 测试用 `Vec<u8>` 竞技场，真机接
//!   seL4 untyped 帧或引导时预留的物理内存）。
//! - [`ArmCache`]：`clean`（设备读前写回）/`invalidate`（设备写后失效）。
//!   aarch64 真机用 `dc cvac`/`dc ivac` + `dsb ish`；host 仿真为 no-op。

/// 连续 DMA 内存池。
///
/// 布局（由 `DmaPool::layout` 描述）：
/// - TX 描述符数组（`tx_count × 16 B`）
/// - RX 描述符数组（`rx_count × 16 B`）
/// - TX 帧缓冲（`tx_count × mtu`）
/// - RX 帧缓冲（`rx_count × mtu`）
pub trait DmaPool {
    /// 返回池总字节数。
    fn len(&self) -> usize;
    /// 池是否为空。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// 返回池的物理基址（描述符/缓冲的物理地址 = base + offset）。
    fn phys_base(&self) -> u64;
    /// 返回池内容（只读）。
    fn as_bytes(&self) -> &[u8];
    /// 返回池内容（可变）。
    fn as_bytes_mut(&mut self) -> &mut [u8];
}

/// ARM64 dcache 维护（DMA 一致性）。
pub trait ArmCache {
    /// 设备读前 clean（写回）指定虚拟地址区间。
    fn clean(&self, va: usize, len: usize);
    /// 设备写后 invalidate 指定虚拟地址区间。
    fn invalidate(&self, va: usize, len: usize);
}

/// Host 仿真 / cache 一致平台的 no-op cache。
pub struct NoopCache;

impl ArmCache for NoopCache {
    fn clean(&self, _va: usize, _len: usize) {}
    fn invalidate(&self, _va: usize, _len: usize) {}
}

/// aarch64 真实 dcache 维护（`dc cvac`/`dc ivac` + `dsb ish`，64 B cache line）。
#[cfg(target_arch = "aarch64")]
pub struct Aarch64Cache;

#[cfg(target_arch = "aarch64")]
impl ArmCache for Aarch64Cache {
    fn clean(&self, va: usize, len: usize) {
        // SAFETY: `dc cvac` 作用于调用方保证已映射且长度合法的虚拟地址。
        unsafe {
            let mut addr = va;
            let end = va.wrapping_add(len);
            while addr < end {
                core::arch::asm!("dc cvac, {addr}", addr = in(reg) addr, options(nostack));
                addr = addr.wrapping_add(64);
            }
            core::arch::asm!("dsb ish", options(nostack));
        }
    }

    fn invalidate(&self, va: usize, len: usize) {
        // SAFETY: `dc ivac` 作用于调用方保证已映射且长度合法的虚拟地址。
        unsafe {
            let mut addr = va;
            let end = va.wrapping_add(len);
            while addr < end {
                core::arch::asm!("dc ivac, {addr}", addr = in(reg) addr, options(nostack));
                addr = addr.wrapping_add(64);
            }
            core::arch::asm!("dsb ish", options(nostack));
        }
    }
}

/// 非 aarch64 回退：cache 一致 host 仿真。
#[cfg(not(target_arch = "aarch64"))]
pub type Aarch64Cache = NoopCache;

/// host 测试用 RAM DMA 池。
#[cfg(test)]
pub struct RamDmaPool {
    bytes: alloc::vec::Vec<u8>,
    phys_base: u64,
}

#[cfg(test)]
impl RamDmaPool {
    /// 创建 `size` 字节的 RAM 池，物理基址 `phys_base`。
    pub fn new(size: usize, phys_base: u64) -> Self {
        Self {
            bytes: alloc::vec![0u8; size],
            phys_base,
        }
    }
}

#[cfg(test)]
impl DmaPool for RamDmaPool {
    fn len(&self) -> usize {
        self.bytes.len()
    }
    fn phys_base(&self) -> u64 {
        self.phys_base
    }
    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ram_pool_layout() {
        let pool = RamDmaPool::new(4096, 0x1000_0000);
        assert_eq!(pool.len(), 4096);
        assert_eq!(pool.phys_base(), 0x1000_0000);
        assert!(!pool.is_empty());
    }

    #[test]
    fn test_noop_cache_is_noop() {
        let cache = NoopCache;
        cache.clean(0x1000, 128);
        cache.invalidate(0x1000, 128);
        // 不应 panic；host 无 cache 语义。
    }
}
