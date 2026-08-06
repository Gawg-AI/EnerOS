//! ARM64 core HAL provider.
//!
//! Implements [`crate::HalProvider`] exposing the CPU, IRQ, Clock, and Serial
//! subsystems. HAL-004/HAL-006：`mem()` 与 `gpio()` 已从 trait 移除——
//! QEMU virt 无 GPIO 控制器（旧 0x0902_0000 是 fw-cfg），`HalMem` 无
//! 可用实现（旧 `panic!` 占位删除）。BSP 有真实设备时扩展。

/// ARM64 core HAL provider: CPU + IRQ + Clock + Serial.
pub struct Arm64HalCoreProvider;

impl crate::HalProvider for Arm64HalCoreProvider {
    fn cpu(&self) -> &'static dyn crate::HalCpu {
        crate::arm64::cpu::cpu()
    }

    fn irq(&self) -> &'static dyn crate::HalIrq {
        crate::arm64::gicv3::irq()
    }

    fn clock(&self) -> &'static dyn crate::HalClock {
        crate::arm64::timer::clock()
    }

    fn serial(&self) -> &'static dyn crate::HalSerial {
        crate::arm64::uart_pl011::serial()
    }
}

static ARM64_HAL_CORE: Arm64HalCoreProvider = Arm64HalCoreProvider;

/// Returns the ARM64 core HAL provider singleton.
pub fn core_provider() -> &'static dyn crate::HalProvider {
    &ARM64_HAL_CORE
}
