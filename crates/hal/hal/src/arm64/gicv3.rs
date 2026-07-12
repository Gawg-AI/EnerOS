//! ARM Generic Interrupt Controller v3 (GICv3) HAL implementation.
//!
//! Implements [`crate::HalIrq`] for a GICv3 interrupt controller with
//! memory-mapped Distributor (GICD) and Redistributor (GICR) frames plus the
//! ICC system-register CPU interface.
//!
//! The default base addresses target the QEMU `virt` machine:
//! - GICD at `0x0800_0000`
//! - GICR at `0x080A_0000`

// ---------------------------------------------------------------------------
// GICD (Distributor) register offsets
// ---------------------------------------------------------------------------
const GICD_CTLR: u64 = 0x00;
const GICD_ISENABLER: u64 = 0x100;
const GICD_ICENABLER: u64 = 0x180;
#[allow(dead_code)] // GICv3 register map; pending-clear not needed yet in v0.6.0
const GICD_ICPENDR: u64 = 0x280;
const GICD_PRI: u64 = 0x400;

// ---------------------------------------------------------------------------
// GICR (Redistributor) register offsets
// ---------------------------------------------------------------------------
#[allow(dead_code)] // GICv3 register map; redistributor CTLR not used yet in v0.6.0
const GICR_CTLR: u64 = 0x00;
const GICR_WAKER: u64 = 0x14;
#[allow(dead_code)] // GICv3 register map; TYPER probe deferred to multi-core support
const GICR_TYPER: u64 = 0x08;
#[allow(dead_code)] // GICv3 register map; group config not used yet in v0.6.0
const GICR_IGROUPR0: u64 = 0x100;

// GICR_WAKER bit definitions.
const GICR_WAKER_PROCESSING_SLEEP: u32 = 1 << 1; // bit 1
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2; // bit 2

/// Maximum number of IRQs supported.
const MAX_IRQ: u32 = 256;

/// ARM64 GICv3 interrupt controller HAL implementation.
pub struct Arm64Gic {
    gicd_base: u64,
    gicr_base: u64,
}

/// Static IRQ handler table.
///
/// Stored in a `static mut` because [`crate::HalIrq::register`] takes `&self`
/// (not `&mut self`); registration is expected to happen during boot before
/// concurrency is enabled.
#[allow(static_mut_refs)]
static mut IRQ_HANDLERS: [Option<crate::IrqHandler>; MAX_IRQ as usize] = [None; MAX_IRQ as usize];

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

impl Arm64Gic {
    /// Create a new GICv3 HAL instance with the given base addresses.
    pub const fn new(gicd_base: u64, gicr_base: u64) -> Self {
        Self {
            gicd_base,
            gicr_base,
        }
    }

    /// Initialize the GIC: distributor, redistributor, and CPU interface.
    ///
    /// # Safety
    /// Touches memory-mapped hardware registers and ICC system registers.
    pub fn init(&self) {
        // SAFETY: MMIO writes and system-register accesses are required to
        // bring the GIC online; caller must ensure base addresses are valid.
        unsafe {
            // Enable GICD: ARE_NS (bit 4) + EnableGrp1 (bit 1) + Enable (bit 0).
            w32(self.gicd_base, GICD_CTLR, 1 | (1 << 4) | (1 << 1));
            self.init_redistributor();
            // Enable CPU interface via ICC system registers.
            let igrpen1: u64 = 1;
            core::arch::asm!(
                "msr icc_igrpen1_el1, {}",
                in(reg) igrpen1,
                options(nostack, preserves_flags),
            );
            let pmr: u64 = 0xff;
            core::arch::asm!(
                "msr icc_pmr_el1, {}",
                in(reg) pmr,
                options(nostack, preserves_flags),
            );
            // Default priority (0x80) for SGIs/PPIs (IRQ 0..32).
            for i in 0..32u32 {
                w32(self.gicd_base, GICD_PRI + (i as u64) * 4, 0x80);
            }
        }
    }

    /// Wake the current core's redistributor and wait for it to be ready.
    ///
    /// # Safety
    /// Touches memory-mapped GICR registers.
    unsafe fn init_redistributor(&self) {
        let gicr = self.locate_current_core_gicr();
        // Clear ProcessorSleep to wake the redistributor.
        let mut waker = r32(gicr, GICR_WAKER);
        waker &= !GICR_WAKER_PROCESSING_SLEEP;
        w32(gicr, GICR_WAKER, waker);
        // Wait until ChildrenAsleep clears (re-distributor ready).
        while r32(gicr, GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP != 0 {
            core::hint::spin_loop();
        }
    }

    /// Locate the GICR base for the current core.
    ///
    /// Simplified single-core implementation: returns the single GICR frame.
    /// Multi-core support will iterate GICR frames by MPIDR in a future version.
    fn locate_current_core_gicr(&self) -> u64 {
        self.gicr_base
    }

    /// Acknowledge and dispatch the highest-priority pending IRQ.
    ///
    /// Reads ICC_IAR1_EL1 to acknowledge the IRQ, looks up the registered
    /// handler (if any), then signals EOI. Spurious IRQs (1023) are ignored
    /// without EOI per the GICv3 specification.
    pub fn dispatch_irq(&self) {
        let irq: u64;
        // SAFETY: reads the interrupt acknowledge register.
        unsafe {
            core::arch::asm!(
                "mrs {}, icc_iar1_el1",
                out(reg) irq,
                options(nostack, preserves_flags),
            );
        }
        // Spurious interrupt (1023): do not EOI, just return.
        if irq == 1023 {
            return;
        }
        let irq = irq as u32;
        // SAFETY: bounds-checked read of the handler table.
        unsafe {
            if let Some(handler) = IRQ_HANDLERS[irq as usize] {
                let _ = handler(irq);
            }
        }
        // Always EOI (including for unknown IRQs) to clear the active state.
        // Use fully-qualified syntax: `eoi` is a `HalIrq` trait method, which
        // is not in scope inside this inherent `impl Arm64Gic` block.
        crate::HalIrq::eoi(self, irq);
    }
}

impl crate::HalIrq for Arm64Gic {
    fn register(
        &self,
        irq: u32,
        _trigger: crate::IrqTrigger,
        handler: crate::IrqHandler,
    ) -> Result<(), crate::HalError> {
        if irq >= MAX_IRQ {
            return Err(crate::HalError::InvalidParam);
        }
        // SAFETY: single-threaded boot registration; IRQ_HANDLERS bounds-checked.
        unsafe {
            IRQ_HANDLERS[irq as usize] = Some(handler);
        }
        Ok(())
    }

    fn unregister(&self, irq: u32) -> Result<(), crate::HalError> {
        if irq >= MAX_IRQ {
            return Err(crate::HalError::InvalidParam);
        }
        // SAFETY: see `register`.
        unsafe {
            IRQ_HANDLERS[irq as usize] = None;
        }
        Ok(())
    }

    fn enable(&self, irq: u32) {
        // SAFETY: MMIO write to GICD_ISENABLER to enable `irq`.
        unsafe {
            w32(
                self.gicd_base,
                GICD_ISENABLER + ((irq / 32) as u64) * 4,
                1 << (irq % 32),
            );
        }
    }

    fn disable(&self, irq: u32) {
        // SAFETY: MMIO write to GICD_ICENABLER to disable `irq`.
        unsafe {
            w32(
                self.gicd_base,
                GICD_ICENABLER + ((irq / 32) as u64) * 4,
                1 << (irq % 32),
            );
        }
    }

    fn eoi(&self, irq: u32) {
        // GICv3 system-register mode: write ICC_EOIR1_EL1 (not GICC_EOIR MMIO).
        let irq_id: u64 = irq as u64;
        // SAFETY: system-register write; does not touch NZCV.
        unsafe {
            core::arch::asm!(
                "msr icc_eoir1_el1, {}",
                in(reg) irq_id,
                options(nostack, preserves_flags),
            );
        }
    }
}

static ARM64_GIC: Arm64Gic = Arm64Gic::new(0x08000000, 0x080A0000);

/// Returns the ARM64 GICv3 IRQ HAL singleton.
pub fn irq() -> &'static dyn crate::HalIrq {
    &ARM64_GIC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gicd_offsets() {
        assert_eq!(GICD_CTLR, 0x00);
        assert_eq!(GICD_ISENABLER, 0x100);
        assert_eq!(GICD_ICENABLER, 0x180);
        assert_eq!(GICD_ICPENDR, 0x280);
        assert_eq!(GICD_PRI, 0x400);
    }

    #[test]
    fn gicr_offsets() {
        assert_eq!(GICR_CTLR, 0x00);
        assert_eq!(GICR_WAKER, 0x14);
        assert_eq!(GICR_TYPER, 0x08);
        assert_eq!(GICR_IGROUPR0, 0x100);
    }

    #[test]
    fn gicr_waker_bits() {
        assert_eq!(GICR_WAKER_PROCESSING_SLEEP, 2);
        assert_eq!(GICR_WAKER_CHILDREN_ASLEEP, 4);
    }

    #[test]
    fn max_irq_constant() {
        assert_eq!(MAX_IRQ, 256);
    }
}
