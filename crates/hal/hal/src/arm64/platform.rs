//! 平台配置（BSP-001）：外设基址与 CPU 拓扑按平台选择。
//!
//! 平台清单：
//! - [`QEMU_VIRT`]：seL4 QEMU `virt`（S2/S3 验证平台，4 核，PL011 0x0900_0000）；
//! - [`PHYTIUM_D2000`]：飞腾 D2000（**真机验证 BLOCKED 缺硬件**；外设基址基于
//!   公开数据手册，真机就绪后以 D2000 设备树/实测校准——不虚构地址）。
//!
//! CPU 拓扑：`core_count` 不再硬编码 4——按平台声明（QEMU=4，D2000=8）；
//! 运行时仍可经 MPIDR Aff 字段核对（`cpu::current_core`）。

/// 平台外设基址与 CPU 拓扑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformConfig {
    /// CPU 核心数（cluster 拓扑合计）。
    pub core_count: u32,
    /// PL011 UART 基址。
    pub uart_base: u64,
    /// GIC 分配器（GICD）基址。
    pub gicd_base: u64,
    /// GIC 重分配器（GICR）基址（GICv3）。
    pub gicr_base: u64,
    /// 通用定时器中断号（PPI 16 = 0x1B 减去 16 的 SPI 编号规则，arm 标准）。
    pub timer_irq: u32,
    /// 网络 MMIO 基址（30 RT-025：NetMmio 单例注入用；无网卡平台为 0）。
    pub net_base: u64,
}

/// QEMU `virt` 平台（S2/S3 验证环境，4 核）。
pub const QEMU_VIRT: PlatformConfig = PlatformConfig {
    core_count: 4,
    uart_base: 0x0900_0000,
    gicd_base: 0x0800_0000,
    gicr_base: 0x080A_0000,
    timer_irq: 27,         // PPI 11（EL1 物理定时器）
    net_base: 0x0A00_0000, // QEMU virt virtio-mmio 区（virtio-net 槽 1）
};

/// 飞腾 D2000 平台（**BLOCKED 缺硬件**；基址基于公开数据手册，
/// 真机后以设备树校准——本常量用于地址定义先行，不用于伪造验证）。
pub const PHYTIUM_D2000: PlatformConfig = PlatformConfig {
    core_count: 8,          // D2000 8 核（2×4 cluster 拓扑）
    uart_base: 0x2800_0000, // D2000 UART0
    gicd_base: 0x2980_0000, // D2000 GICD（公开资料）
    gicr_base: 0x2982_0000, // D2000 GICR（公开资料）
    timer_irq: 27,          // ARM 通用定时器 PPI（与 QEMU 相同，arm 规范）
    net_base: 0,            // D2000 网卡基址待真机设备树校准（不虚构）
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bsp001_platform_configs_distinct() {
        assert_ne!(QEMU_VIRT.core_count, PHYTIUM_D2000.core_count);
        assert_eq!(QEMU_VIRT.core_count, 4, "QEMU virt 4 核");
        assert_eq!(PHYTIUM_D2000.core_count, 8, "D2000 8 核（cluster 拓扑）");
        assert_ne!(QEMU_VIRT.uart_base, PHYTIUM_D2000.uart_base);
        assert_ne!(QEMU_VIRT.gicd_base, PHYTIUM_D2000.gicd_base);
        // 地址非零且对齐
        for cfg in [QEMU_VIRT, PHYTIUM_D2000] {
            assert!(cfg.uart_base % 0x1000 == 0);
            assert!(cfg.gicd_base % 0x1000 == 0);
        }
    }
}
