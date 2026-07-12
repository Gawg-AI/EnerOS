//! Mock HAL implementation for compile-time interface validation and unit tests.
//!
//! Enabled via the `mock` cargo feature.

use crate::*;

/// Mock HAL implementation. All methods return trivial values.
pub struct MockHal;

impl HalCpu for MockHal {
    fn enable_irq(&self) {}
    fn disable_irq(&self) {}
    fn current_core(&self) -> u32 {
        0
    }
    fn core_count(&self) -> u32 {
        1
    }
    fn halt(&self) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }
    fn wfi(&self) {}
}

impl HalMem for MockHal {
    fn map(&self, _pa: u64, _va: u64, _flags: MemFlags) -> Result<(), HalError> {
        Ok(())
    }
    fn unmap(&self, _va: u64) -> Result<(), HalError> {
        Ok(())
    }
    fn translate(&self, _va: u64) -> Option<u64> {
        None
    }
    fn set_domain(&self, _va: u64, _domain: u32) -> Result<(), HalError> {
        Ok(())
    }
}

impl HalIrq for MockHal {
    fn register(
        &self,
        _irq: u32,
        _trigger: IrqTrigger,
        _handler: IrqHandler,
    ) -> Result<(), HalError> {
        Ok(())
    }
    fn unregister(&self, _irq: u32) -> Result<(), HalError> {
        Ok(())
    }
    fn enable(&self, _irq: u32) {}
    fn disable(&self, _irq: u32) {}
    fn eoi(&self, _irq: u32) {}
}

impl HalClock for MockHal {
    fn now_ns(&self) -> u64 {
        0
    }
    fn frequency_hz(&self) -> u64 {
        1000
    }
    fn set_deadline(&self, _ns: u64) -> Result<(), HalError> {
        Ok(())
    }
}

impl HalSerial for MockHal {
    fn write(&self, data: &[u8]) -> Result<usize, HalError> {
        Ok(data.len())
    }
    fn read(&self, _buf: &mut [u8]) -> Result<usize, HalError> {
        Ok(0)
    }
    fn flush(&self) -> Result<(), HalError> {
        Ok(())
    }
}

impl HalGpio for MockHal {
    fn set_dir(&self, _config: GpioConfig) -> Result<(), HalError> {
        Ok(())
    }
    fn set(&self, _pin: u32, _val: bool) -> Result<(), HalError> {
        Ok(())
    }
    fn get(&self, _pin: u32) -> Result<bool, HalError> {
        Ok(false)
    }
    fn toggle(&self, _pin: u32) -> Result<(), HalError> {
        Ok(())
    }
}

/// Mock HAL provider for testing the singleton injection pattern.
pub struct MockHalProvider;

static MOCK_HAL: MockHal = MockHal;

impl HalProvider for MockHalProvider {
    fn cpu(&self) -> &'static dyn HalCpu {
        &MOCK_HAL
    }
    fn mem(&self) -> &'static dyn HalMem {
        &MOCK_HAL
    }
    fn irq(&self) -> &'static dyn HalIrq {
        &MOCK_HAL
    }
    fn clock(&self) -> &'static dyn HalClock {
        &MOCK_HAL
    }
    fn serial(&self) -> &'static dyn HalSerial {
        &MOCK_HAL
    }
    fn gpio(&self) -> &'static dyn HalGpio {
        &MOCK_HAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_hal;

    #[test]
    fn mock_cpu_current_core() {
        let hal = MockHal;
        assert_eq!(hal.current_core(), 0);
    }

    #[test]
    fn mock_cpu_core_count() {
        let hal = MockHal;
        assert_eq!(hal.core_count(), 1);
    }

    #[test]
    fn mock_clock_now_ns() {
        let hal = MockHal;
        assert_eq!(hal.now_ns(), 0);
    }

    #[test]
    fn mock_clock_frequency() {
        let hal = MockHal;
        assert_eq!(hal.frequency_hz(), 1000);
    }

    #[test]
    fn mock_serial_write_returns_len() {
        let hal = MockHal;
        let data = b"hello";
        assert_eq!(hal.write(data).unwrap(), 5);
    }

    #[test]
    fn mock_serial_read_returns_zero() {
        let hal = MockHal;
        let mut buf = [0u8; 8];
        assert_eq!(hal.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn mock_serial_flush_ok() {
        let hal = MockHal;
        assert!(hal.flush().is_ok());
    }

    #[test]
    fn mock_gpio_get_returns_false() {
        let hal = MockHal;
        assert!(!hal.get(0).unwrap());
    }

    #[test]
    fn mock_mem_translate_returns_none() {
        let hal = MockHal;
        assert_eq!(hal.translate(0xDEAD_0000), None);
    }

    #[test]
    fn mock_irq_register_ok() {
        let handler: IrqHandler = |_irq: u32| IrqAction::Handled;
        let hal = MockHal;
        assert!(hal.register(32, IrqTrigger::Edge, handler).is_ok());
    }

    #[test]
    fn irq_handler_type_assignable_and_callable() {
        let h: IrqHandler = |irq: u32| {
            if irq == 0 {
                IrqAction::Handled
            } else {
                IrqAction::Disabled
            }
        };
        assert_eq!(h(0), IrqAction::Handled);
        assert_eq!(h(1), IrqAction::Disabled);
    }

    #[test]
    fn mock_provider_injection() {
        static PROVIDER: MockHalProvider = MockHalProvider;
        init_hal(&PROVIDER);
        // After injection, hal() should return the provider and cpu().current_core() == 0.
        assert_eq!(hal().cpu().current_core(), 0);
        assert_eq!(hal().cpu().core_count(), 1);
        assert_eq!(hal().clock().now_ns(), 0);
        assert_eq!(hal().serial().write(b"test").unwrap(), 4);
    }
}
