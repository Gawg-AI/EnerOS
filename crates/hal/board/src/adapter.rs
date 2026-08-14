//! 38（WP-38.4）：平台适配器——新 ARM 盒子唯一接入点。
//!
//! # QEMU virt 板级事实（经验证的平台声明，非猜测）
//! - virtio-mmio 设备所在系统为**缓存一致**内存（QEMU virt 机器无
//!   cache 模型；33 蓝图 durable-log 掉电证据已证明 NoopCache 路径数据
//!   真实存活——该证据即"qemu-virt 一致"的验证依据）；
//! - root 镜像 1:1 映射（物理 == 虚拟）：DMA 池 phys_base = 池内存地址
//!   （seL4 rootserver 身份映射 + BOOT-007 契约窗口，37 证据链验证）；
//! - FDT 未声明 `dma-coherent` 时，本适配器把 virtio 设备标记 Coherent；
//!   其他设备保持 FDT 事实（缺省 NonCoherent → 需要真实 cache 维护）。
//!
//! 新增 ARM 盒子 = 新增 `PlatformAdapter` 实现 + 平台描述；核心业务不
//! 分支板卡名（蓝图 38 §1 工程合同）。

extern crate alloc;

use alloc::boxed::Box;

use eneros_hal::dma::{DmaAllocator, DmaCoherency, DmaPoolAllocator};
use eneros_hal::inventory::{DescriptionSource, PlatformProvenance};
use eneros_hal::platform_adapter::{
    chosen_handles, cpu_topology_from, ClockControllerSet, DmaRuntime, HalRuntime,
    InterruptControllerHandle, PlatformAdapter, PlatformDescriptionProvider, PlatformError,
    PlatformInput, ResetControllerSet, TimerHandle,
};

use crate::device_tree::{FdtDescriptionProvider, FdtTree};

/// QEMU virt 平台适配器（AArch64 参考平台）。
#[derive(Debug, Clone, Copy, Default)]
pub struct QemuVirtAdapter {
    fdt: FdtDescriptionProvider,
}

impl QemuVirtAdapter {
    pub const fn new() -> Self {
        Self {
            fdt: FdtDescriptionProvider,
        }
    }
}

impl PlatformAdapter for QemuVirtAdapter {
    fn discover(
        &self,
        input: &PlatformInput,
    ) -> Result<eneros_hal::inventory::ValidatedInventory, PlatformError> {
        let mut inventory = self.fdt.describe(input)?;
        // QEMU virt 板级事实：virtio-mmio 设备一致内存（经 33 durable-log
        // 掉电证据验证）；FDT 无 dma-coherent 属性时由适配器声明。
        for dev in inventory.devices.iter_mut() {
            let is_virtio = dev.compatible.as_slice().iter().any(|c| c == "virtio,mmio");
            if is_virtio && dev.dma_coherency == DmaCoherency::NonCoherent {
                dev.dma_coherency = DmaCoherency::Coherent;
            }
        }
        inventory.provenance = PlatformProvenance {
            source: DescriptionSource::Fdt,
            digest: None,
            description: "qemu-virt (fdt + verified board facts)".into(),
        };
        inventory
            .validate()
            .map_err(|_| PlatformError::InvalidInventory)
    }

    fn build_hal(
        &self,
        inventory: &eneros_hal::inventory::ValidatedInventory,
    ) -> Result<HalRuntime, PlatformError> {
        // CPU 拓扑：inventory 事实（enabled 核）。
        let topo = cpu_topology_from(&inventory.inventory)?;
        // console/entropy：chosen 角色（stdout-path → 设备索引由
        // FdtDescriptionProvider 解析；无 → None 显式降级）。
        let (console, entropy) = chosen_handles(&inventory.inventory);
        // timer：架构 generic timer（aarch64 唯一合法路径；host 无时钟 →
        // None——host 不得生成 target 时钟证据）。
        let timer = timer_handle();
        // DMA：root 镜像 1:1 板级事实——池内存地址即物理地址。
        let allocator: &'static mut dyn DmaAllocator =
            Box::leak(Box::new(DmaPoolAllocator::<{ 9 * 4096 }>::new_identity()));
        Ok(HalRuntime {
            cpu: topo,
            interrupts: InterruptControllerHandle {
                domains: inventory.inventory.interrupt_domains.clone(),
            },
            timer,
            console,
            entropy,
            dma: DmaRuntime {
                allocator,
                iommu_present: !inventory.inventory.iommu_domains.is_empty(),
                default_coherency: DmaCoherency::NonCoherent,
            },
            reset: ResetControllerSet { present: false },
            clocks: ClockControllerSet { present: false },
        })
    }
}

/// 架构 generic timer（aarch64）；频率校验非零（WP-38.6）——零 → None
/// （fail-closed，不猜时钟）。
#[cfg(target_arch = "aarch64")]
fn timer_handle() -> Option<TimerHandle> {
    let clock: &'static dyn eneros_hal::HalClock = eneros_hal::arm64::timer::clock();
    let freq = clock.frequency_hz();
    TimerHandle::new(clock, freq)
}

#[cfg(not(target_arch = "aarch64"))]
fn timer_handle() -> Option<TimerHandle> {
    // host 构建：无目标时钟（host 测试不得伪装 target 时钟事实）。
    None
}

impl FdtTree {
    /// 解析 chosen.stdout-path → 设备节点索引（console 角色；无 → None）。
    ///
    /// stdout-path 格式：`/path/to/device[:options]`——只取路径段。
    pub fn console_device_index(&self) -> Option<usize> {
        let stdout = self.stdout_path.as_ref()?;
        let path = stdout.split(':').next().unwrap_or("");
        self.nodes.iter().position(|n| n.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QEMU_VIRT_DTB: &[u8] = include_bytes!("../tests/fixtures/qemu-virt.dtb");

    #[test]
    fn adapter_discovers_qemu_virt() {
        let adapter = QemuVirtAdapter::new();
        let input = PlatformInput {
            runtime_dtb: Some(QEMU_VIRT_DTB),
            bootinfo_cpu_count: 2,
        };
        let inv = adapter
            .discover(&PlatformInput {
                runtime_dtb: Some(QEMU_VIRT_DTB),
                bootinfo_cpu_count: 2,
            })
            .expect("qemu-virt 描述必须成功");
        assert!(!inv.inventory.cpus.is_empty());
        assert!(inv.inventory.validate().is_ok());
        assert_eq!(inv.inventory.provenance.source, DescriptionSource::Fdt);
    }

    #[test]
    fn adapter_no_description_fails_closed() {
        let adapter = QemuVirtAdapter::new();
        let r = adapter.discover(&PlatformInput::default());
        assert_eq!(r, Err(PlatformError::NoDescription));
    }

    #[test]
    fn adapter_build_hal_on_host_has_no_timer() {
        // host 构建无目标时钟 → timer None（诚实；target 由 aarch64 路径）。
        let adapter = QemuVirtAdapter::new();
        let inv = adapter
            .discover(&PlatformInput {
                runtime_dtb: Some(QEMU_VIRT_DTB),
                bootinfo_cpu_count: 2,
            })
            .expect("discover");
        let hal = adapter.build_hal(&inv).expect("build_hal");
        if cfg!(target_arch = "aarch64") {
            assert!(hal.timer.is_some(), "aarch64 必须注入非零频率 timer");
        } else {
            assert!(hal.timer.is_none(), "host 无目标时钟");
        }
        assert!(!hal.cpu.enabled_cores.is_empty());
    }

    #[test]
    fn stdout_path_resolves_console_device() {
        use crate::device_tree::parse_fdt_tree;
        let tree = parse_fdt_tree(QEMU_VIRT_DTB).expect("parse");
        // fixture 无 stdout-path → None（不猜）。
        assert!(tree.console_device_index().is_none());
    }
}
