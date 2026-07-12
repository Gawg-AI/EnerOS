//! EnerOS Kernel Crate (Phase 0 Placeholder)
//!
//! This crate serves as the no_std kernel placeholder for Phase 0.
//! Actual kernel functionality will be added in subsequent versions.

#![no_std]
#![feature(lang_items)]
#![allow(internal_features)]

/// Kernel initialization placeholder.
///
/// Kernel init placeholder — board crate provides hardware boot support.
/// For now, it provides a minimal entry point that does nothing.
pub fn init() -> ! {
    // Phase 0 placeholder: board crate (eneros-board) provides boot support;
    // kernel init will be implemented in subsequent versions.
    loop {
        core::hint::spin_loop();
    }
}

#[lang = "eh_personality"]
extern "C" fn eh_personality() {}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
