//! SP805 hardware watchdog driver.

const WDT_LOAD: u64 = 0x00;
#[allow(dead_code)]
const WDT_VALUE: u64 = 0x04;
const WDT_CTRL: u64 = 0x08;
const WDT_INTCLR: u64 = 0x0c;
const WDT_LOCK: u64 = 0xC00;

const WDT_UNLOCK: u32 = 0x1ACCE551;
const WDT_LOCK_V: u32 = 0x1;

pub struct HwWatchdog {
    pub base: u64,
}

impl HwWatchdog {
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    #[inline]
    unsafe fn w(&self, off: u64, v: u32) {
        core::ptr::write_volatile((self.base + off) as *mut u32, v);
    }

    #[allow(dead_code)]
    #[inline]
    unsafe fn r(&self, off: u64) -> u32 {
        core::ptr::read_volatile((self.base + off) as *const u32)
    }

    pub fn init(&self, timeout_ms: u32) {
        if self.base == 0 {
            return;
        }
        unsafe {
            self.w(WDT_LOCK, WDT_UNLOCK);
            let load = timeout_ms * 1000;
            self.w(WDT_LOAD, load);
            self.w(WDT_INTCLR, 1);
            self.w(WDT_CTRL, 0x3);
            self.w(WDT_LOCK, WDT_LOCK_V);
        }
    }

    pub fn kick(&self) {
        if self.base == 0 {
            return;
        }
        unsafe {
            self.w(WDT_LOCK, WDT_UNLOCK);
            self.w(WDT_INTCLR, 1);
            self.w(WDT_LOCK, WDT_LOCK_V);
        }
    }

    pub fn stop(&self) {
        if self.base == 0 {
            return;
        }
        unsafe {
            self.w(WDT_LOCK, WDT_UNLOCK);
            self.w(WDT_CTRL, 0);
            self.w(WDT_LOCK, WDT_LOCK_V);
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.base != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_with_base() {
        let wdt = HwWatchdog::new(0x09050000);
        assert_eq!(wdt.base, 0x09050000);
    }

    #[test]
    fn test_new_with_zero_base() {
        let wdt = HwWatchdog::new(0);
        assert_eq!(wdt.base, 0);
    }

    #[test]
    fn test_is_enabled_true() {
        let wdt = HwWatchdog::new(0x09050000);
        assert!(wdt.is_enabled());
    }

    #[test]
    fn test_is_enabled_false() {
        let wdt = HwWatchdog::new(0);
        assert!(!wdt.is_enabled());
    }

    #[test]
    fn test_init_zero_base_no_panic() {
        let wdt = HwWatchdog::new(0);
        wdt.init(10_000);
    }

    #[test]
    fn test_kick_zero_base_no_panic() {
        let wdt = HwWatchdog::new(0);
        wdt.kick();
    }

    #[test]
    fn test_stop_zero_base_no_panic() {
        let wdt = HwWatchdog::new(0);
        wdt.stop();
    }
}
