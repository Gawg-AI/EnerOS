//! 38（WP-38.4）：`PlatformAdapter`/`HalRuntime`——root 生产平台装配唯一入口。
//!
//! # 目标（蓝图 38 WP-38.4）
//! - root 不再直接调用 `arm64::timer::clock` 或 QEMU 常量：生产必须经
//!   adapter 构造 [`HalRuntime`]；未初始化访问返回错误/阻断，不回退默认；
//! - 新 ARM 盒子 = 新增 `PlatformAdapter` 实现 + 平台描述（FDT/ACPI/
//!   manifest），核心不分支板卡名；
//! - CPU 数来自 inventory/seL4 BootInfo 交叉验证，不固定常量；
//! - console/entropy 缺失影响 capability/readiness（缺 required → 阻断）。

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::dma::{DmaAllocator, DmaCoherency};
use crate::inventory::{
    DescriptionSource, DeviceStatus, InterruptDomain, PlatformInventory, ValidatedInventory,
};

/// 平台装配错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    /// 无平台描述（DTB 缺失/非法）。
    NoDescription,
    /// 描述解析失败。
    ParseFailed,
    /// inventory 校验失败（重叠/越界/唯一性）。
    InvalidInventory,
    /// 平台不支持（unknown compatible / 架构不符）。
    Unsupported,
    /// HAL 构建失败（缺关键资源）。
    HalBuildFailed { what: &'static str },
}

/// 平台装配输入（root 启动期事实；`ProductManifest` 契约在 36 构建期校验，
/// 此处 FDT 来自签名介质区域，BootInfo 交叉验证由装配层执行）。
#[derive(Debug, Clone, Default)]
pub struct PlatformInput<'a> {
    /// 运行时 FDT 切片（已验证介质内；None = 无描述）。
    pub runtime_dtb: Option<&'a [u8]>,
    /// seL4 BootInfo 报告的 CPU 数（交叉验证用；0 = 未知）。
    pub bootinfo_cpu_count: u32,
}

/// 平台描述提供者（WP-38.2/38.3）：FDT/ACPI/manifest 统一 seam。
///
/// 核心不判断描述来源；未来 ACPI/固件 manifest 实现同一 trait。
pub trait PlatformDescriptionProvider {
    /// 解析描述 → 归一化 `PlatformInventory`（**只读**：不触发设备写/
    /// 格式化/业务 I/O——固定不变量 1）。
    fn describe(&self, input: &PlatformInput) -> Result<PlatformInventory, PlatformError>;
}

/// 平台适配器（新盒子唯一接入点）。
pub trait PlatformAdapter {
    /// 发现并校验平台事实（`describe` + `validate`）。
    fn discover(&self, input: &PlatformInput) -> Result<ValidatedInventory, PlatformError>;
    /// 由已验证 inventory 构建 HAL 运行时（资源句柄集合）。
    fn build_hal(&self, inventory: &ValidatedInventory) -> Result<HalRuntime, PlatformError>;
}

/// CPU 拓扑（来自 inventory；核心不硬编码核数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuTopology {
    pub core_count: u32,
    pub enabled_cores: Vec<u32>,
}

/// 定时器句柄（frequency 校验非零后由装配层注入 time 子系统）。
#[derive(Clone, Copy)]
pub struct TimerHandle {
    /// 单调时钟实现（BSP 注入）。
    pub clock: &'static dyn crate::HalClock,
    /// 时钟频率（Hz；构造时校验非零——0 拒绝构造）。
    pub frequency_hz: u64,
}

impl TimerHandle {
    /// 构造；`frequency_hz == 0` → None（WP-38.6：timer frequency 必须
    /// 来自架构寄存器/描述并校验非零）。
    pub fn new(clock: &'static dyn crate::HalClock, frequency_hz: u64) -> Option<Self> {
        if frequency_hz == 0 {
            return None;
        }
        Some(Self {
            clock,
            frequency_hz,
        })
    }
}

/// 控制台句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleHandle {
    pub name: String,
}

/// 熵源句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntropyHandle {
    pub name: String,
}

/// 中断控制器句柄（域集合；SMP 注册边界由 WP-38.5 类型化保证）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptControllerHandle {
    pub domains: Vec<InterruptDomain>,
}

/// DMA 运行时（分配器唯一来源；IOMMU 存在性影响地址路径）。
pub struct DmaRuntime {
    pub allocator: &'static mut dyn DmaAllocator,
    pub iommu_present: bool,
    pub default_coherency: DmaCoherency,
}

/// 复位控制器集合（present 与否影响 capability/readiness）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetControllerSet {
    pub present: bool,
}

/// 时钟控制器集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockControllerSet {
    pub present: bool,
}

/// HAL 运行时——root 装配后的平台资源唯一事实。
pub struct HalRuntime {
    pub cpu: CpuTopology,
    pub interrupts: InterruptControllerHandle,
    pub timer: Option<TimerHandle>,
    pub console: Option<ConsoleHandle>,
    pub entropy: Option<EntropyHandle>,
    pub dma: DmaRuntime,
    pub reset: ResetControllerSet,
    pub clocks: ClockControllerSet,
}

/// 从 inventory 推导 CPU 拓扑（enabled 核；无 → Unsupported）。
pub fn cpu_topology_from(inventory: &PlatformInventory) -> Result<CpuTopology, PlatformError> {
    let enabled: Vec<u32> = inventory
        .cpus
        .iter()
        .filter(|c| c.enabled)
        .map(|c| c.id)
        .collect();
    if enabled.is_empty() {
        return Err(PlatformError::Unsupported);
    }
    Ok(CpuTopology {
        core_count: enabled.len() as u32,
        enabled_cores: enabled,
    })
}

/// 与 BootInfo CPU 数交叉验证（不一致 → Unsupported；BootInfo 0 = 未知跳过）。
pub fn cross_check_cpu_count(
    topology: &CpuTopology,
    bootinfo_cpu_count: u32,
) -> Result<(), PlatformError> {
    if bootinfo_cpu_count != 0 && bootinfo_cpu_count < topology.core_count {
        return Err(PlatformError::Unsupported);
    }
    Ok(())
}

/// 从 inventory 推导 chosen 控制台/熵源（缺 → None，由 readiness 显式降级）。
pub fn chosen_handles(
    inventory: &PlatformInventory,
) -> (Option<ConsoleHandle>, Option<EntropyHandle>) {
    let console = inventory.chosen.console.and_then(|i| {
        inventory
            .devices
            .get(i)
            .filter(|d| d.status == DeviceStatus::Ok)
            .map(|d| ConsoleHandle {
                name: d.compatible.first().clone(),
            })
    });
    let entropy = inventory.chosen.entropy.and_then(|i| {
        inventory
            .devices
            .get(i)
            .filter(|d| d.status == DeviceStatus::Ok)
            .map(|d| EntropyHandle {
                name: d.compatible.first().clone(),
            })
    });
    (console, entropy)
}

/// 描述来源描述串（日志/evidence）。
pub fn source_name(s: DescriptionSource) -> &'static str {
    match s {
        DescriptionSource::Fdt => "fdt",
        DescriptionSource::Acpi => "acpi",
        DescriptionSource::ProductManifest => "product-manifest",
        DescriptionSource::Synthetic => "synthetic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::PhysAddr;
    use crate::dma::{DmaBuffer, DmaDirection, DmaMapping, DmaRequest};
    use crate::inventory::{
        ChosenResources, CpuNode, DeviceId, DeviceResources, InventoryBuilder, MemoryRegion,
        MmioRegion, NonEmptyVec, PlatformProvenance,
    };
    use crate::types::HalError;

    /// host 时钟 mock（频率校验路径）。
    struct MockClock(u64);
    impl crate::HalClock for MockClock {
        fn now_ns(&self) -> u64 {
            0
        }
        fn frequency_hz(&self) -> u64 {
            self.0
        }
        fn set_deadline(&self, _ns: u64) -> Result<(), HalError> {
            Ok(())
        }
    }
    static MOCK_CLOCK_1MHZ: MockClock = MockClock(1_000_000);
    static MOCK_CLOCK_ZERO: MockClock = MockClock(0);

    /// 测试 DMA 分配器（空实现：仅占位能力）。
    struct TestAllocator;
    impl DmaAllocator for TestAllocator {
        fn allocate(&mut self, _req: DmaRequest) -> Result<DmaBuffer, crate::dma::DmaError> {
            Err(crate::dma::DmaError::Unsupported)
        }
        fn map(
            &mut self,
            _b: &DmaBuffer,
            _d: DeviceId,
            _dir: DmaDirection,
        ) -> Result<DmaMapping, crate::dma::DmaError> {
            Err(crate::dma::DmaError::Unsupported)
        }
        fn sync_for_device(
            &self,
            _m: &DmaMapping,
            _r: core::ops::Range<usize>,
        ) -> Result<(), crate::dma::DmaError> {
            Err(crate::dma::DmaError::Unsupported)
        }
        fn sync_for_cpu(
            &self,
            _m: &DmaMapping,
            _r: core::ops::Range<usize>,
        ) -> Result<(), crate::dma::DmaError> {
            Err(crate::dma::DmaError::Unsupported)
        }
        fn unmap(&mut self, _m: DmaMapping) -> Result<(), crate::dma::DmaError> {
            Err(crate::dma::DmaError::Unsupported)
        }
    }

    fn test_inventory() -> PlatformInventory {
        let pl011 = DeviceResources {
            id: DeviceId(1),
            compatible: NonEmptyVec::new(vec!["arm,pl011".into()]).unwrap(),
            status: DeviceStatus::Ok,
            regs: vec![MmioRegion {
                phys: PhysAddr::new(0x0900_0000),
                size: 0x1000,
            }],
            interrupts: vec![],
            clocks: vec![],
            resets: vec![],
            gpios: vec![],
            dmas: vec![],
            iommu: None,
            dma_coherency: DmaCoherency::Coherent,
            properties: Default::default(),
        };
        InventoryBuilder {
            cpus: vec![
                CpuNode {
                    id: 0,
                    enabled: true,
                },
                CpuNode {
                    id: 1,
                    enabled: true,
                },
            ],
            memory: vec![MemoryRegion {
                phys: PhysAddr::new(0x4000_0000),
                size: 0x1000_0000,
            }],
            devices: vec![pl011],
            chosen: ChosenResources {
                console: Some(0),
                ..Default::default()
            },
            ..Default::default()
        }
        .build(PlatformProvenance {
            source: DescriptionSource::Synthetic,
            digest: None,
            description: "test".into(),
        })
    }

    #[test]
    fn timer_handle_rejects_zero_frequency() {
        assert!(TimerHandle::new(&MOCK_CLOCK_ZERO, 0).is_none());
        assert!(TimerHandle::new(&MOCK_CLOCK_1MHZ, 1_000_000).is_some());
    }

    #[test]
    fn cpu_topology_from_inventory() {
        let inv = test_inventory();
        let topo = cpu_topology_from(&inv).unwrap();
        assert_eq!(topo.core_count, 2);
        // BootInfo 交叉验证：bootinfo 报告更少 → 拒绝。
        assert!(cross_check_cpu_count(&topo, 1).is_err());
        assert!(cross_check_cpu_count(&topo, 0).is_ok(), "未知跳过");
        assert!(cross_check_cpu_count(&topo, 2).is_ok());
        assert!(cross_check_cpu_count(&topo, 8).is_ok(), "bootinfo 可更大");
    }

    #[test]
    fn cpu_topology_rejects_all_disabled() {
        let mut inv = test_inventory();
        inv.cpus[0].enabled = false;
        inv.cpus[1].enabled = false;
        assert_eq!(cpu_topology_from(&inv), Err(PlatformError::Unsupported));
    }

    #[test]
    fn chosen_handles_derived() {
        let inv = test_inventory();
        let (console, entropy) = chosen_handles(&inv);
        assert_eq!(console.unwrap().name, "arm,pl011");
        assert!(entropy.is_none(), "无熵源 → None（显式降级，不猜）");
    }

    #[test]
    fn hal_runtime_assembles_with_validated_inventory() {
        // 装配层模式：discover → validate → build（无硬件时用 mock 注入）。
        let inv = test_inventory();
        let validated = inv.validate().unwrap();
        let topo = cpu_topology_from(&validated.inventory).unwrap();
        let (console, entropy) = chosen_handles(&validated.inventory);
        // DmaRuntime 需要 'static mut；此处仅验证类型组合（装配层用 Box::leak）。
        let _rt = HalRuntime {
            cpu: topo,
            interrupts: InterruptControllerHandle {
                domains: validated.inventory.interrupt_domains.clone(),
            },
            timer: TimerHandle::new(&MOCK_CLOCK_1MHZ, 1_000_000),
            console,
            entropy,
            dma: DmaRuntime {
                allocator: Box::leak(Box::new(TestAllocator)),
                iommu_present: false,
                default_coherency: DmaCoherency::Coherent,
            },
            reset: ResetControllerSet { present: false },
            clocks: ClockControllerSet { present: false },
        };
        assert!(_rt.timer.is_some());
    }
}
