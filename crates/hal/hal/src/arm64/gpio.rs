//! ARM64 generic GPIO controller HAL implementation.
//!
//! Implements [`crate::HalGpio`] for a simple memory-mapped GPIO controller
//! with direction, data, and pull-up/down registers. Targets the QEMU `virt`
//! platform GPIO region (base `0x0902_0000`, 32 pins).

// ---------------------------------------------------------------------------
// GPIO register offsets
// ---------------------------------------------------------------------------
const GPIO_DIR: u64 = 0x04; // Direction register
const GPIO_DATA: u64 = 0x40; // Data register
const GPIO_PUD: u64 = 0x94; // Pull-up/down

/// ARM64 generic GPIO controller HAL implementation.
pub struct Arm64Gpio {
    base: u64,
    pin_count: u32,
}

impl Arm64Gpio {
    /// Create a new GPIO HAL instance at the given MMIO base address with
    /// `pin_count` available pins.
    pub const fn new(base: u64, pin_count: u32) -> Self {
        Self { base, pin_count }
    }

    /// Write a 32-bit MMIO register.
    #[inline]
    unsafe fn w32(base: u64, off: u64, v: u32) {
        core::ptr::write_volatile((base + off) as *mut u32, v);
    }

    /// Read a 32-bit MMIO register.
    #[inline]
    unsafe fn r32(base: u64, off: u64) -> u32 {
        core::ptr::read_volatile((base + off) as *const u32)
    }
}

impl crate::HalGpio for Arm64Gpio {
    fn set_dir(&self, config: crate::GpioConfig) -> Result<(), crate::HalError> {
        if config.pin >= self.pin_count {
            return Err(crate::HalError::InvalidParam);
        }
        let mut dir = unsafe { Self::r32(self.base, GPIO_DIR) };
        let mask = 1u32 << config.pin;
        match config.dir {
            crate::GpioDir::Output => dir |= mask,
            crate::GpioDir::Input => dir &= !mask,
        }
        unsafe { Self::w32(self.base, GPIO_DIR, dir) };
        // Configure pull resistor. PullMode has no #[repr(u32)], so convert
        // manually rather than using `as u32`.
        let pull_val: u32 = match config.pull {
            crate::PullMode::None => 0,
            crate::PullMode::Up => 1,
            crate::PullMode::Down => 2,
        };
        unsafe {
            Self::w32(
                self.base,
                GPIO_PUD + ((config.pin / 16) as u64) * 4,
                pull_val,
            );
        }
        Ok(())
    }

    fn set(&self, pin: u32, val: bool) -> Result<(), crate::HalError> {
        if pin >= self.pin_count {
            return Err(crate::HalError::InvalidParam);
        }
        let mask = 1u32 << pin;
        let v = if val { mask } else { 0 };
        unsafe { Self::w32(self.base, GPIO_DATA, v) };
        Ok(())
    }

    fn get(&self, pin: u32) -> Result<bool, crate::HalError> {
        if pin >= self.pin_count {
            return Err(crate::HalError::InvalidParam);
        }
        let data = unsafe { Self::r32(self.base, GPIO_DATA) };
        Ok(data & (1u32 << pin) != 0)
    }

    fn toggle(&self, pin: u32) -> Result<(), crate::HalError> {
        let cur = self.get(pin)?;
        self.set(pin, !cur)
    }
}

static ARM64_GPIO: Arm64Gpio = Arm64Gpio::new(0x09020000, 32);

/// Returns the ARM64 GPIO HAL singleton.
pub fn gpio() -> &'static dyn crate::HalGpio {
    &ARM64_GPIO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpio_register_offsets() {
        assert_eq!(GPIO_DIR, 0x04);
        assert_eq!(GPIO_DATA, 0x40);
        assert_eq!(GPIO_PUD, 0x94);
    }
}
