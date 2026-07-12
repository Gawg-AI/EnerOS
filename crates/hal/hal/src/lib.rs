//! EnerOS Hardware Abstraction Layer (HAL) trait specifications.
//!
//! This crate defines the complete HAL trait interface set for EnerOS.
//! It is a pure design crate (no hardware implementations); BSPs implement
//! these traits in v0.6.0+.

#![cfg_attr(not(test), no_std)]

pub mod types;
pub use types::*;

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
    /// Returns the memory HAL implementation.
    fn mem(&self) -> &'static dyn HalMem;
    /// Returns the interrupt controller HAL implementation.
    fn irq(&self) -> &'static dyn HalIrq;
    /// Returns the clock HAL implementation.
    fn clock(&self) -> &'static dyn HalClock;
    /// Returns the serial HAL implementation.
    fn serial(&self) -> &'static dyn HalSerial;
    /// Returns the GPIO HAL implementation.
    fn gpio(&self) -> &'static dyn HalGpio;
}

/// Global HAL reference (injected by BSP during early boot).
///
/// # Safety
/// This is a `static mut` accessed only from [`init_hal`] (write-once)
/// and [`hal()`] (read-after-init). No concurrent mutation occurs.
#[allow(static_mut_refs)]
static mut HAL: Option<&'static dyn HalProvider> = None;

/// Initialize the global HAL with a BSP-provided implementation.
///
/// Must be called exactly once during early boot, before any HAL usage.
///
/// # Safety
/// This function writes to a `static mut`. It must be called from a
/// single-threaded boot context before the scheduler starts.
pub fn init_hal(provider: &'static dyn HalProvider) {
    unsafe {
        HAL = Some(provider);
    }
}

/// Returns the global HAL provider.
///
/// # Panics
/// Panics if [`init_hal`] has not been called yet.
pub fn hal() -> &'static dyn HalProvider {
    unsafe { HAL.expect("HAL not initialized: call init_hal() during boot first") }
}

#[cfg(feature = "mock")]
pub mod mock;
