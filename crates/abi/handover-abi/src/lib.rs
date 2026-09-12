//! 句柄移交 ABI 单源（SPEC-SERIES-2026-08-28/C WP-C.3）。
//!
//! 历史病灶（体检候选 13）：设备移交的固定虚拟地址常量在 root-task
//! `spawn.rs`（`STORAGE_MMIO_VADDR`）与消费侧 `drv-svc/blk_backend.rs`
//!（`SVC_STORAGE_MMIO_VADDR`）同值不同名，两侧注释各自自称"单一事实源"
//! ——root-task 是 bin，消费侧无法依赖，靠注释对齐（eipc_grants 治过的
//! 病的孪生兄弟）。本 crate 把 vaddr 常量与寄存器序收编一处：
//!
//! - **寄存器/内存布局逐字节不变**（纯重定位，不改 ABI）；
//! - mint 侧（spawn）与消费侧（drv-svc 等）共用同一常量；
//! - 新增一种移交的完整结构（Handover 类型 + SpawnedHandle 表驱动）为
//!   C.3 续作（需 boot 链 QEMU 验证，见规格执行状态），本卷先治常量双源。

#![no_std]

/// 服务 VSpace 中 virtio-blk MMIO 页的固定虚拟地址（spawn 映射 →
/// svc-storage `_start` X3；QEMU virt 板级事实）。
///
/// 全仓唯一：`spawn.rs` 与 `blk_backend.rs` 的历史同名双源（0x4000_0000）
/// 已收编至本常量（SPEC-B/C）。
pub const STORAGE_MMIO_VADDR: u64 = 0x4000_0000;

/// 服务 VSpace 中 DMA 池（9 页连续物理帧）的固定虚拟地址（svc-storage
/// `_start` X4）。历史双源（0x4001_0000）同收编。
pub const STORAGE_DMA_VADDR: u64 = 0x4001_0000;

/// DMA 池页数（virtio-blk 队列下限：desc/avail/used + req + data/status）。
pub const STORAGE_DMA_PAGES: usize = 9;

/// 协议服务 VSpace 中 virtio-net MMIO 页的固定虚拟地址（每服务 VSpace
/// 独立——各服务可复用同一 vaddr 映射各自槽位物理页；两页区
/// 0x4002_0000..0x4002_2000）。SPEC-R4 WP-R4.3 收编（历史 spawn.rs 本地
/// 定义，与 storage 面同病）。
pub const NET_MMIO_VADDR: u64 = 0x4002_0000;

/// 协议服务 DMA 池页数（virtio-net 队列下限，= VIRTIO_NET_POOL_SIZE/4K）。
/// SPEC-R4 WP-R4.3 收编（历史双源：spawn.rs 与 net_transport.rs 各定义
/// 一次，消费侧注释自称"单一事实源"）。
pub const NET_DMA_PAGES: usize = 9;

/// 协议服务 VSpace 中 PL011 UART MMIO 页的固定虚拟地址（svc-modbus-rtu
/// `_start` **X7** 接收；SPEC-D WP-D.1）。区段 0x4003_0000..0x4003_1000
/// 与 STORAGE_DMA（0x4001_0000+9 页）、NET 两页区互不重叠（root-task
/// `uart_mmio_vaddr_region_disjoint_from_other_device_regions` 冻结）。
pub const UART_MMIO_VADDR: u64 = 0x4003_0000;

/// QEMU virt virtio-mmio 区域基址（板级事实：32 槽 × 0x200；blk/net 在
/// 低槽）。SPEC-R4 WP-R4.3 收编（历史三抄：spawn `STORAGE_VIRTIO_REGION_BASE`
/// / driver_probe `VIRTIO_REGION_BASE` / crypto entropy 默认值）。
pub const VIRTIO_MMIO_REGION_BASE: u64 = 0x0a00_0000;

/// 70.7（WP-70.7.2）：svc-storage 启动盘（virtio **slot 0**，GPT 产品盘）
/// DMA 池页数——与数据盘同规格（virtio-blk 9 页布局）。
///
/// **连续性契约**：启动盘池由 spawn 在数据盘池分配之后**背靠背**连续
/// 分配（同一 untyped 顺序切，物理连续）——svc-storage 侧启动盘 DMA
/// 基址 = 数据盘 dma_phys + STORAGE_DMA_PAGES×4096（X5 推导，零新寄存器）。
/// spawn 侧分配后校验连续性，断裂 → 启动后端诚实降级。
pub const SVC_BOOT_DMA_PAGES: usize = 9;

/// 启动盘 = virtio slot 0（MMIO = STORAGE_MMIO_VADDR + 0×0x200——数据盘
/// 移交页已覆盖 slot 0-7，启动盘寄存器**零新增映射**）。
pub const BOOT_MMIO_SLOT: u64 = 0;

/// virtio-mmio 槽步长（legacy 传输）。
pub const VIRTIO_MMIO_SLOT_STRIDE: u64 = 0x200;

// 移交寄存器序（spawn mint 与 `_start` 消费的对齐契约，文档化单源）：
//
// | 服务 | 移交 | 寄存器 |
// |------|------|--------|
// | svc-storage | virtio-blk | `_start(ep, buffer, ready, mmio_vaddr=X3, dma_vaddr=X4, dma_phys=X5, wall_epoch=X6)` |
// | svc-modbus-tcp | virtio-net | `_start(ep, buffer, ready, mmio_vaddr, dma_vaddr, dma_phys, wall_epoch)`（同序） |
// | svc-modbus-rtu | UART | X7（UART vaddr；SPEC-D） |
//
// 禁止改动（QEMU 全启动链冻结；改动 MUST 走 image raw 链验证）。

#[cfg(test)]
mod tests {
    use super::*;

    /// 单源值冻结（53 卷 QEMU 基线的装配事实——改值即 ABI 变更）。
    #[test]
    fn vaddr_values_frozen() {
        assert_eq!(STORAGE_MMIO_VADDR, 0x4000_0000);
        assert_eq!(STORAGE_DMA_VADDR, 0x4001_0000);
        assert_eq!(STORAGE_DMA_PAGES, 9);
        // 两地址不重叠（MMIO 页与 DMA 池各占一页）。
        assert_ne!(STORAGE_MMIO_VADDR, STORAGE_DMA_VADDR);
        // SPEC-R4 WP-R4.3：收编面冻结（值与收编前各散抄点逐字节一致）。
        assert_eq!(NET_MMIO_VADDR, 0x4002_0000);
        assert_eq!(NET_DMA_PAGES, 9);
        assert_eq!(UART_MMIO_VADDR, 0x4003_0000);
        assert_eq!(VIRTIO_MMIO_REGION_BASE, 0x0a00_0000);
        assert_eq!(VIRTIO_MMIO_SLOT_STRIDE, 0x200);
    }
}
