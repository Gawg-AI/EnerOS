//! ARMv8-A Generic Timer HAL implementation.
//!
//! Implements [`crate::HalClock`] using the ARMv8-A physical generic timer:
//! - `CNTFRQ_EL0`: timer frequency (Hz)
//! - `CNTPCT_EL0`: physical count register (monotonic)
//! - `CNTP_TVAL_EL0` / `CNTP_CTL_EL0`: timer deadline and control

/// ARM64 Generic Timer HAL implementation.
pub struct Arm64Timer;

impl crate::HalClock for Arm64Timer {
    fn now_ns(&self) -> u64 {
        let count: u64;
        unsafe {
            core::arch::asm!(
                "mrs {}, cntpct_el0",
                out(reg) count,
                options(nostack, preserves_flags),
            );
        }
        let freq = self.frequency_hz();
        // ns = count * 1_000_000_000 / freq
        count.saturating_mul(1_000_000_000) / freq
    }

    fn frequency_hz(&self) -> u64 {
        let freq: u64;
        unsafe {
            core::arch::asm!(
                "mrs {}, cntfrq_el0",
                out(reg) freq,
                options(nostack, preserves_flags),
            );
        }
        freq
    }

    fn set_deadline(&self, ns: u64) -> Result<(), crate::HalError> {
        let freq = self.frequency_hz();
        let ticks = ns.saturating_mul(freq) / 1_000_000_000;
        unsafe {
            core::arch::asm!(
                "msr cntp_tval_el0, {}",
                in(reg) ticks,
                options(nostack, preserves_flags),
            );
            // Set ENABLE bit (bit 0) of CNTP_CTL_EL0.
            let enable: u64 = 1;
            core::arch::asm!(
                "msr cntp_ctl_el0, {}",
                in(reg) enable,
                options(nostack, preserves_flags),
            );
        }
        Ok(())
    }
}

static ARM64_TIMER: Arm64Timer = Arm64Timer;

/// Returns the ARM64 Generic Timer HAL singleton.
pub fn clock() -> &'static dyn crate::HalClock {
    &ARM64_TIMER
}
