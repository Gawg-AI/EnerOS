//! ARM64 CPU operations HAL implementation.
//!
//! Implements [`crate::HalCpu`] using ARMv8-A system registers and
//! instructions:
//! - DAIF (interrupt mask via `daifclr` / `daifset`)
//! - MPIDR_EL1 (core identification)
//! - WFI (wait-for-interrupt low-power state)

// BSP-001：核心数按平台配置（QEMU virt=4，飞腾 D2000=8 cluster 拓扑）；
// 运行时设备树探测为后续项（真机验证 BLOCKED 缺硬件）。
use super::platform::QEMU_VIRT;
const CORE_COUNT: u32 = QEMU_VIRT.core_count;

/// ARM64 CPU HAL implementation.
pub struct Arm64Cpu;

impl crate::HalCpu for Arm64Cpu {
    fn enable_irq(&self) {
        // DAIFClr #0xf clears all DAIF bits (unmasks IRQ/FIQ/ABT/DBG).
        // SAFETY: `msr daifclr` 为无内存访问的系统寄存器写指令；副作用是
        // 解除中断屏蔽，本 HAL 单线程串行使用，不引入数据竞争。
        unsafe {
            core::arch::asm!("msr daifclr, #0xf", options(nostack, preserves_flags));
        }
    }

    fn disable_irq(&self) {
        // DAIFSet #0xf sets all DAIF bits (masks IRQ/FIQ/ABT/DBG).
        // SAFETY: `msr daifset` 为无内存访问的系统寄存器写指令；副作用是
        // 屏蔽中断，本 HAL 单线程串行使用，不引入数据竞争。
        unsafe {
            core::arch::asm!("msr daifset, #0xf", options(nostack, preserves_flags));
        }
    }

    fn current_core(&self) -> u32 {
        let id: u64;
        // SAFETY: `mrs mpidr_el1` 为无内存访问的系统寄存器读指令；输出
        // 寄存器由 `out(reg)` 约束分配，`id` 在 asm 后完全初始化。
        unsafe {
            core::arch::asm!(
                "mrs {}, mpidr_el1",
                out(reg) id,
                options(nostack, preserves_flags),
            );
        }
        // Aff0 (low 8 bits) is the core ID within a cluster.
        (id & 0xff) as u32
    }

    fn core_count(&self) -> u32 {
        CORE_COUNT
    }

    fn halt(&self) -> ! {
        loop {
            self.wfi();
        }
    }

    fn wfi(&self) {
        // SAFETY: `wfi` 不访问内存、无输出约束；副作用仅为进入低功耗等待，
        // 不改变任何可观察内存状态。
        unsafe {
            core::arch::asm!("wfi", options(nostack, preserves_flags));
        }
    }
}

static ARM64_CPU: Arm64Cpu = Arm64Cpu;

/// Returns the ARM64 CPU HAL singleton.
pub fn cpu() -> &'static dyn crate::HalCpu {
    &ARM64_CPU
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_count_constant() {
        assert_eq!(CORE_COUNT, 4);
    }
}
