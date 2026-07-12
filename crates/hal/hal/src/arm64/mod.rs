//! ARM64 (aarch64) HAL implementation.
//!
//! Concrete implementations of the EnerOS HAL traits for the ARMv8-A /
//! aarch64 architecture, targeting the seL4-based EnerOS kernel on the
//! QEMU `virt` platform.
//!
//! # Submodules
//! - [`cpu`]: [`crate::HalCpu`] via DAIF / MPIDR_EL1 / WFI.
//! - [`gicv3`]: [`crate::HalIrq`] via the ARM Generic Interrupt Controller v3.
//! - [`timer`]: [`crate::HalClock`] via the ARMv8-A Generic Timer.
//! - [`uart_pl011`]: [`crate::HalSerial`] via the ARM PrimeCell PL011 UART.
//! - [`gpio`]: [`crate::HalGpio`] via a generic memory-mapped GPIO controller.
//! - [`net_mmio`]: register-level Ethernet MAC MMIO access (PHY/MAC).
//! - [`provider`]: [`crate::HalProvider`] glue exposing the ARM64 HAL singletons.

pub mod cpu;
pub mod gicv3;
pub mod gpio;
pub mod net_mmio;
pub mod provider;
pub mod timer;
pub mod uart_pl011;
