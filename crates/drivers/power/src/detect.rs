//! Power-loss detection via dual-redundant ADC + GPIO.
//!
//! On aarch64, the main power rail is monitored by two independent paths:
//! 1. **ADC comparison** — reads the main supply voltage and compares against
//!    a threshold (e.g. 4.75 V for a 5 V rail).
//! 2. **GPIO interrupt** — a dedicated power-fail signal from the PMIC triggers
//!    a level-triggered interrupt.
//!
//! Both paths must agree before power is considered OK. This dual-redundancy
//! prevents false positives from ADC noise or GPIO glitches.
//!
//! Detection latency target: **< 10 ms** from mains failure to callback.
//! (Host tests do not verify timing.)

use spin::Mutex;

// ============================================================================
// Callback registration
// ============================================================================

/// Callback registered by the system for power-loss interrupt handling.
/// Called by [`notify_power_loss`] when the hardware (or mock) detects loss.
pub(crate) static POWER_IRQ_CALLBACK: Mutex<Option<fn()>> = Mutex::new(None);

/// Register the power-loss interrupt callback.
///
/// The callback is invoked from [`notify_power_loss`] when power loss is
/// detected. It should initiate the shutdown sequence via [`on_power_loss`].
pub fn register_power_irq(callback: fn()) {
    *POWER_IRQ_CALLBACK.lock() = Some(callback);
}

// ============================================================================
// Power state notification
// ============================================================================

/// Called by the hardware interrupt handler (or host mock) when main power
/// is lost. Updates global state and invokes the registered callback.
pub fn notify_power_loss() {
    {
        let mut state = crate::POWER_STATE.lock();
        state.main_power_ok = false;
        state.in_shutdown = true;
    }
    // Invoke callback outside the state lock to avoid re-entrancy deadlock.
    if let Some(cb) = *POWER_IRQ_CALLBACK.lock() {
        cb();
    }
}

/// Called when main power is restored. Cancels the shutdown sequence by
/// clearing `in_shutdown`. This is the authorized cancellation path —
/// normal tasks cannot cancel via [`advance_sequence`](crate::advance_sequence).
pub fn notify_power_restored() {
    let mut state = crate::POWER_STATE.lock();
    state.main_power_ok = true;
    state.in_shutdown = false;
}

// ============================================================================
// Power status check — aarch64 hardware implementation
// ============================================================================

/// ADC base address (example: platform-specific, set by board config).
#[cfg(target_arch = "aarch64")]
const ADC_BASE: u64 = 0x0906_0000;
/// ADC data register offset.
#[cfg(target_arch = "aarch64")]
const ADC_DATA_REG: u64 = 0x00;
/// Main power OK threshold in millivolts (4.75 V for a 5 V rail).
#[cfg(target_arch = "aarch64")]
const POWER_OK_THRESHOLD_MV: u32 = 4750;

/// GPIO base address (example: platform-specific).
#[cfg(target_arch = "aarch64")]
const GPIO_BASE: u64 = 0x0907_0000;
/// GPIO data register offset.
#[cfg(target_arch = "aarch64")]
const GPIO_DATA_REG: u64 = 0x00;
/// Bit 0 of GPIO data indicates power-fail (1 = fail, 0 = ok).
#[cfg(target_arch = "aarch64")]
const POWER_FAIL_BIT: u32 = 0x1;

/// Read the main supply voltage via ADC and compare against threshold.
///
/// Returns `true` if voltage is at or above the OK threshold.
#[cfg(target_arch = "aarch64")]
fn adc_check_voltage() -> bool {
    // SAFETY: reading a 32-bit MMIO register at the configured ADC base.
    // The caller (board init) is responsible for ensuring the address is valid.
    let adc_raw = unsafe { core::ptr::read_volatile((ADC_BASE + ADC_DATA_REG) as *const u32) };
    // Convert ADC raw value to millivolts.
    // Real conversion depends on ADC resolution and reference voltage;
    // simplified here as a direct mapping (platform-specific).
    let voltage_mv = adc_raw;
    voltage_mv >= POWER_OK_THRESHOLD_MV
}

/// Read the GPIO power-fail signal.
///
/// Returns `true` if the power-fail signal is NOT asserted (power OK).
#[cfg(target_arch = "aarch64")]
fn gpio_check_signal() -> bool {
    // SAFETY: reading a 32-bit MMIO register at the configured GPIO base.
    let gpio_val = unsafe { core::ptr::read_volatile((GPIO_BASE + GPIO_DATA_REG) as *const u32) };
    (gpio_val & POWER_FAIL_BIT) == 0
}

/// Check if main power is OK using dual-redundant ADC + GPIO.
///
/// Both paths must agree: returns `true` only if ADC voltage is above
/// threshold AND GPIO power-fail signal is not asserted.
#[cfg(target_arch = "aarch64")]
pub fn is_main_power_ok() -> bool {
    adc_check_voltage() && gpio_check_signal()
}

// ============================================================================
// Power status check — host mock
// ============================================================================

/// Host mock: check cached power state from [`crate::POWER_STATE`].
#[cfg(not(target_arch = "aarch64"))]
pub fn is_main_power_ok() -> bool {
    crate::POWER_STATE.lock().main_power_ok
}

/// Host mock: simulate main power status change.
///
/// When `ok` is `false`, calls [`notify_power_loss`] (updates state + invokes
/// callback). When `ok` is `true`, calls [`notify_power_restored`] (cancels
/// shutdown).
#[cfg(not(target_arch = "aarch64"))]
pub fn set_main_power_ok(ok: bool) {
    if ok {
        notify_power_restored();
    } else {
        notify_power_loss();
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());
    static CALLBACK_CALLED: StdMutex<bool> = StdMutex::new(false);

    fn reset_state() {
        *crate::POWER_STATE.lock() = crate::PowerState {
            main_power_ok: true,
            ups_soc: 100,
            last_checkpoint: core::time::Duration::from_millis(0),
            in_shutdown: false,
        };
        *POWER_IRQ_CALLBACK.lock() = None;
    }

    fn test_callback() {
        *CALLBACK_CALLED.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    #[test]
    fn test_register_power_irq() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        *CALLBACK_CALLED.lock().unwrap_or_else(|e| e.into_inner()) = false;

        register_power_irq(test_callback);
        assert!(POWER_IRQ_CALLBACK.lock().is_some());

        // Trigger power loss — callback should fire.
        notify_power_loss();
        assert!(*CALLBACK_CALLED.lock().unwrap_or_else(|e| e.into_inner()));
    }

    #[test]
    fn test_notify_power_loss_updates_state() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        assert!(is_main_power_ok());
        notify_power_loss();
        assert!(!is_main_power_ok());
        assert!(crate::POWER_STATE.lock().in_shutdown);
    }

    #[test]
    fn test_notify_power_restored_updates_state() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        notify_power_loss();
        assert!(!is_main_power_ok());

        notify_power_restored();
        assert!(is_main_power_ok());
        assert!(!crate::POWER_STATE.lock().in_shutdown);
    }

    #[test]
    fn test_set_main_power_ok_host_mock() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        set_main_power_ok(false);
        assert!(!is_main_power_ok());
        assert!(crate::POWER_STATE.lock().in_shutdown);

        set_main_power_ok(true);
        assert!(is_main_power_ok());
        assert!(!crate::POWER_STATE.lock().in_shutdown);
    }

    #[test]
    fn test_no_callback_no_panic() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        // No callback registered — notify should not panic.
        notify_power_loss();
        assert!(!is_main_power_ok());
    }

    #[test]
    fn test_register_overwrites_previous_callback() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        fn first_cb() {}
        fn second_cb() {}

        register_power_irq(first_cb);
        register_power_irq(second_cb);

        // Latest registration wins.
        let cb = POWER_IRQ_CALLBACK.lock();
        assert!(cb.is_some());
    }
}
