//! Boot information structures for EnerOS hardware boot.
//!
//! 依据蓝图 `phase0.md` §v0.3.0 §4.1：定义由 bootloader 传递给 seL4 的
//! 启动信息结构，以及多板支持的 `BoardConfig` trait（设计决策 D2）。

/// Boot stage identifier (蓝图 §v0.3.0 接口交付物)
///
/// 标识系统当前所处的启动阶段，从 ROM 初始化到 seL4 内核运行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    /// ROM 初始化阶段
    RomInit,
    /// U-Boot/启动加载器阶段
    Bootloader,
    /// seL4 镜像已加载
    Sel4Loaded,
    /// seL4 内核运行
    Sel4Running,
}

/// Boot info passed from bootloader to seL4 (蓝图 §4.1)
///
/// 描述板级硬件的关键参数，由 bootloader 在启动时传递给 seL4 内核。
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    /// 板名
    pub board_name: &'static str,
    /// 物理内存基址
    pub ram_base: u64,
    /// 内存大小（字节）
    pub ram_size: u64,
    /// 串口寄存器基址
    pub serial_base: u64,
    /// CPU 核数
    pub cpu_count: u32,
    /// CPU 频率（MHz）
    pub freq_mhz: u32,
}

/// Board configuration trait for multi-board support (设计决策 D2)
///
/// 每种板型实现该 trait，提供自身的 `BootInfo`，从而支持多板型适配。
pub trait BoardConfig {
    /// 返回该板型的启动信息
    fn boot_info() -> BootInfo;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 `BootInfo` 全部 6 个字段的构造与读取。
    #[test]
    fn test_boot_info_construction() {
        let info = BootInfo {
            board_name: "qemu-virt",
            ram_base: 0x4000_0000,
            ram_size: 0x0800_0000,
            serial_base: 0x0900_0000,
            cpu_count: 4,
            freq_mhz: 1500,
        };
        assert_eq!(info.board_name, "qemu-virt");
        assert_eq!(info.ram_base, 0x4000_0000);
        assert_eq!(info.ram_size, 0x0800_0000);
        assert_eq!(info.serial_base, 0x0900_0000);
        assert_eq!(info.cpu_count, 4);
        assert_eq!(info.freq_mhz, 1500);
    }

    /// 测试 `BootStage` 全部 4 个变体的匹配。
    #[test]
    fn test_boot_stage_variants() {
        let stages = [
            BootStage::RomInit,
            BootStage::Bootloader,
            BootStage::Sel4Loaded,
            BootStage::Sel4Running,
        ];
        let mut labels: [&str; 4] = [""; 4];
        for (i, s) in stages.iter().enumerate() {
            labels[i] = match s {
                BootStage::RomInit => "rom",
                BootStage::Bootloader => "uboot",
                BootStage::Sel4Loaded => "loaded",
                BootStage::Sel4Running => "running",
            };
        }
        assert_eq!(labels, ["rom", "uboot", "loaded", "running"]);
    }

    /// 测试 `BootStage` 的 PartialEq/Eq 语义。
    #[test]
    fn test_boot_stage_equality() {
        assert_eq!(BootStage::RomInit, BootStage::RomInit);
        assert_eq!(BootStage::Sel4Running, BootStage::Sel4Running);
        assert_ne!(BootStage::RomInit, BootStage::Bootloader);
        assert_ne!(BootStage::Sel4Loaded, BootStage::Sel4Running);
    }

    /// 测试 `BootInfo` 的 Clone 行为：克隆后字段一致且独立。
    #[test]
    fn test_boot_info_clone() {
        let info = BootInfo {
            board_name: "raspi4b",
            ram_base: 0x0,
            ram_size: 0x4000_0000,
            serial_base: 0xfe20_1000,
            cpu_count: 4,
            freq_mhz: 1500,
        };
        // BootInfo implements Copy, so assignment creates an independent copy.
        let cloned = info;
        assert_eq!(cloned.board_name, info.board_name);
        assert_eq!(cloned.ram_base, info.ram_base);
        assert_eq!(cloned.ram_size, info.ram_size);
        assert_eq!(cloned.serial_base, info.serial_base);
        assert_eq!(cloned.cpu_count, info.cpu_count);
        assert_eq!(cloned.freq_mhz, info.freq_mhz);
    }

    /// 测试 `BootInfo` 的 Copy 行为：按值传递后原变量仍可用。
    #[test]
    fn test_boot_info_copy() {
        let info = BootInfo {
            board_name: "phytium-d2000",
            ram_base: 0x8000_0000,
            ram_size: 0x4000_0000,
            serial_base: 0x2800_1000,
            cpu_count: 8,
            freq_mhz: 2000,
        };
        // Copy 语义：赋值后 info 仍然可用
        let copied = info;
        assert_eq!(copied.cpu_count, 8);
        assert_eq!(info.cpu_count, 8);
        assert_eq!(copied.board_name, info.board_name);
    }

    /// 测试 `BoardConfig` trait 的实现与 `boot_info()` 返回值。
    #[test]
    fn test_board_config_trait() {
        struct QemuVirt;
        impl BoardConfig for QemuVirt {
            fn boot_info() -> BootInfo {
                BootInfo {
                    board_name: "qemu-virt",
                    ram_base: 0x4000_0000,
                    ram_size: 0x0800_0000,
                    serial_base: 0x0900_0000,
                    cpu_count: 4,
                    freq_mhz: 1500,
                }
            }
        }
        let info = QemuVirt::boot_info();
        assert_eq!(info.board_name, "qemu-virt");
        assert_eq!(info.ram_base, 0x4000_0000);
        assert_eq!(info.serial_base, 0x0900_0000);
        assert_eq!(info.cpu_count, 4);
    }

    /// 测试 `BootStage` 的 Clone/Copy 行为。
    #[test]
    fn test_boot_stage_clone_copy() {
        let s1 = BootStage::Bootloader;
        // BootStage implements Copy, so assignment creates a copy.
        let s2 = s1;
        assert_eq!(s1, s2);
        // Copy 语义
        let s3 = s1;
        assert_eq!(s1, s3);
    }
}
