//! EnerOS Hardware Abstraction Layer (HAL) trait specifications.
//!
//! This crate defines the complete HAL trait interface set for EnerOS.
//! It is a pure design crate (no hardware implementations); BSPs implement
//! these traits in v0.6.0+.
//!
//! 38（蓝图 38）：平台资源唯一事实（[`inventory`]）、DMA 唯一 seam
//! （[`dma`]）、平台装配唯一入口（[`platform_adapter`]）与地址强类型
//! （[`addr`]）——核心业务只消费这些模型，禁止板卡名/厂商常量分支。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod addr;
pub mod dma;
pub mod inventory;
pub mod irq_table;
pub mod platform_adapter;

pub use addr::{BusAddr, CpuVaddr, DeviceAddr, Iova, PhysAddr};
pub use dma::{DmaAllocator, DmaCoherency, DmaDirection, DmaMapping};
pub use inventory::{DeviceResources, PlatformInventory, ValidatedInventory};
pub use platform_adapter::{HalRuntime, PlatformAdapter, PlatformDescriptionProvider};

pub mod types;
pub use types::*;

/// HAL 纯数学/边界逻辑（无硬件依赖，host 可测；HAL-001/002）。
pub(crate) mod timer_math;

/// GPIO 位操作纯逻辑（无硬件依赖，host 可测；HAL-003）。
pub(crate) mod bit_logic;

/// HAL-005：有界忙等——轮询 `cond` 直到为 true 或超过 `max_iters`。
///
/// 旧代码为无限 `while` + `spin_loop()` 忙等（外设挂死即永久阻塞）。
/// 返回 `Err(HalError::Timeout)` 供调用方上报。
#[allow(dead_code)]
pub(crate) fn wait_until<F: FnMut() -> bool>(mut cond: F, max_iters: u32) -> Result<(), HalError> {
    for _ in 0..max_iters {
        if cond() {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(HalError::Timeout)
}

#[cfg(target_arch = "aarch64")]
pub mod arm64;

/// CPU operations HAL trait.
///
/// Provides core-level CPU control: interrupt masking, core identification,
/// and power management (halt/wfi).
pub trait HalCpu {
    /// Enable CPU interrupts (unmask DAIF).
    fn enable_irq(&self);

    /// Disable CPU interrupts (mask DAIF).
    fn disable_irq(&self);

    /// Returns the current CPU core ID (0-indexed).
    fn current_core(&self) -> u32;

    /// Returns the total number of CPU cores.
    fn core_count(&self) -> u32;

    /// Halt the CPU permanently (never returns).
    fn halt(&self) -> !;

    /// Wait for interrupt (enter low-power state until an IRQ fires).
    fn wfi(&self);
}

/// Memory management HAL trait.
///
/// Provides virtual memory mapping, unmapping, and address translation.
pub trait HalMem {
    /// Map a physical address `pa` to virtual address `va` with `flags`.
    fn map(&self, pa: u64, va: u64, flags: MemFlags) -> Result<(), HalError>;

    /// Unmap the virtual address `va`.
    fn unmap(&self, va: u64) -> Result<(), HalError>;

    /// Translate a virtual address to its physical address.
    /// Returns `None` if not mapped.
    fn translate(&self, va: u64) -> Option<u64>;

    /// Set the protection domain for a virtual address range.
    fn set_domain(&self, va: u64, domain: u32) -> Result<(), HalError>;
}

/// Interrupt controller HAL trait.
///
/// Provides interrupt registration, enable/disable, and end-of-interrupt.
pub trait HalIrq {
    /// Register a handler for IRQ `irq` with trigger type `trigger`.
    fn register(&self, irq: u32, trigger: IrqTrigger, handler: IrqHandler) -> Result<(), HalError>;

    /// Unregister the handler for IRQ `irq`.
    fn unregister(&self, irq: u32) -> Result<(), HalError>;

    /// Enable IRQ `irq`.
    fn enable(&self, irq: u32);

    /// Disable IRQ `irq`.
    fn disable(&self, irq: u32);

    /// Signal end-of-interrupt for IRQ `irq`.
    fn eoi(&self, irq: u32);
}

/// Clock and timer HAL trait.
///
/// Provides a monotonic nanosecond clock and timer deadline setting.
pub trait HalClock {
    /// Returns the current monotonic time in nanoseconds.
    fn now_ns(&self) -> u64;

    /// Returns the clock frequency in Hz.
    fn frequency_hz(&self) -> u64;

    /// Set a timer deadline at `ns` nanoseconds.
    fn set_deadline(&self, ns: u64) -> Result<(), HalError>;
}

/// Serial port HAL trait.
///
/// Provides byte-level serial I/O (e.g. UART).
pub trait HalSerial {
    /// Write `data` to the serial port. Returns number of bytes written.
    fn write(&self, data: &[u8]) -> Result<usize, HalError>;

    /// Read bytes into `buf`. Returns number of bytes read.
    fn read(&self, buf: &mut [u8]) -> Result<usize, HalError>;

    /// Flush any buffered output.
    fn flush(&self) -> Result<(), HalError>;
}

/// GPIO HAL trait.
///
/// Provides GPIO pin direction configuration and read/write/toggle.
pub trait HalGpio {
    /// Configure a GPIO pin with `config`.
    fn set_dir(&self, config: GpioConfig) -> Result<(), HalError>;

    /// Set GPIO `pin` to `val` (true=high, false=low).
    fn set(&self, pin: u32, val: bool) -> Result<(), HalError>;

    /// Read the current value of GPIO `pin` (true=high, false=low).
    fn get(&self, pin: u32) -> Result<bool, HalError>;

    /// Toggle GPIO `pin` value.
    fn toggle(&self, pin: u32) -> Result<(), HalError>;
}

/// HAL provider trait — the BSP injection point.
///
/// A Board Support Package implements this trait to expose all six HAL
/// subsystems. The implementation is registered via [`init_hal`] during
/// early boot, then accessed via [`hal()`].
pub trait HalProvider {
    /// Returns the CPU HAL implementation.
    fn cpu(&self) -> &'static dyn HalCpu;
    /// Returns the interrupt controller HAL implementation.
    fn irq(&self) -> &'static dyn HalIrq;
    /// Returns the clock HAL implementation.
    fn clock(&self) -> &'static dyn HalClock;
    /// Returns the serial HAL implementation.
    fn serial(&self) -> &'static dyn HalSerial;
    // HAL-004/HAL-006：`mem()` 与 `gpio()` 已从 trait 移除——
    // QEMU virt 平台没有内存映射 HAL 与 GPIO 控制器（虚构 MMIO 地址
    // 0x0901_0000/0x0902_0000 分别是 PL031 RTC 与 fw-cfg，非网卡/GPIO）；
    // 原 `mem()` 占位 `panic!` 已删除。BSP 有真实设备时再扩展 trait。
}

/// Global HAL reference (injected by BSP during early boot).
///
/// 30 RT-027：`static mut` → `OnceCell`（write-once 语义由类型表达；
/// 旧实现依赖调用约定 + 注释，`hal()` 未初始化时 expect panic——语义保留
/// 但消除 `static mut` 面）。
static HAL: spin::Once<&'static (dyn HalProvider + Sync)> = spin::Once::new();

/// Initialize the global HAL with a BSP-provided implementation.
///
/// Must be called exactly once during early boot, before any HAL usage.
/// （重复调用静默忽略——Once 语义；调用方为 boot 装配期单线程上下文。）
pub fn init_hal(provider: &'static (dyn HalProvider + Sync)) {
    let _ = HAL.call_once(|| provider);
}

/// Returns the global HAL provider.
///
/// # Panics
/// Panics if [`init_hal`] has not been called yet（启动顺序 bug，尽早暴露）。
pub fn hal() -> &'static (dyn HalProvider + Sync) {
    *HAL.get()
        .expect("HAL not initialized: call init_hal() during boot first")
}

#[cfg(feature = "mock")]
pub mod mock;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_until_immediately_true() {
        assert_eq!(wait_until(|| true, 10), Ok(()));
    }

    #[test]
    fn wait_until_true_after_n_iters() {
        let mut n = 0;
        let r = wait_until(
            || {
                n += 1;
                n >= 3
            },
            10,
        );
        assert_eq!(r, Ok(()));
        assert_eq!(n, 3);
    }

    #[test]
    fn wait_until_timeout() {
        // 永不满足的条件 → Timeout（HAL-005：旧代码无限忙等，新代码有界）
        let mut n = 0;
        let r = wait_until(
            || {
                n += 1;
                false
            },
            5,
        );
        assert_eq!(r, Err(HalError::Timeout));
        assert_eq!(n, 5);
    }
}
