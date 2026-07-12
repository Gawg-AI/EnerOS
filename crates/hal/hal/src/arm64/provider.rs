//! ARM64 core HAL provider.
//!
//! Implements [`crate::HalProvider`] exposing the CPU, IRQ, Clock, Serial,
//! and GPIO subsystems. `HalMem` is deferred: the v0.8.0 `mm` crate
//! implements `AddressSpace` with `&mut self` semantics, which is
//! incompatible with `HalMem`'s `&self` signature. Adaptation is
//! postponed to v0.10.0 (partition isolation verification).

/// ARM64 partial HAL provider: CPU + IRQ + Clock + Serial + GPIO.
pub struct Arm64HalCoreProvider;

// The `mem` accessor intentionally panics until v0.10.0 wires up a real
// implementation. `clippy::disallowed_macros` (see clippy.toml) forbids
// `panic!` in non-test code; this stub is a documented, temporary exemption.
#[allow(clippy::disallowed_macros)]
impl crate::HalProvider for Arm64HalCoreProvider {
    fn cpu(&self) -> &'static dyn crate::HalCpu {
        crate::arm64::cpu::cpu()
    }

    fn mem(&self) -> &'static dyn crate::HalMem {
        panic!("not implemented: HalMem will be added in v0.10.0")
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

    fn gpio(&self) -> &'static dyn crate::HalGpio {
        crate::arm64::gpio::gpio()
    }
}

static ARM64_HAL_CORE: Arm64HalCoreProvider = Arm64HalCoreProvider;

/// Returns the ARM64 core HAL provider singleton.
pub fn core_provider() -> &'static dyn crate::HalProvider {
    &ARM64_HAL_CORE
}
