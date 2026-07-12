//! EnerOS Hello — First Rust user-space component (v0.4.0)
//!
//! This is the first seL4 user-space Rust program. It initializes the
//! runtime and prints a greeting message to the serial console, then
//! halts in an infinite loop.

#![no_std]
#![no_main]
#![feature(lang_items)]
#![allow(internal_features)]
#![allow(clippy::empty_loop)]

use core::panic::PanicInfo;

extern crate eneros_runtime;

use eneros_runtime::{init, println};

/// Entry point called by seL4 after loading the user-space image.
///
/// Initializes the runtime, prints the hello banner, and halts.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    init();
    println!("Hello from Rust on seL4!");
    println!("EnerOS Phase 0 - first userland component.");
    println!("== Userland component alive ==");
    println!("Target: aarch64-unknown-none");
    loop {
        core::hint::spin_loop();
    }
}

/// Personality function required by the Rust compiler for unwind handling.
/// In no_std + panic=abort, this is never called but must be defined.
#[lang = "eh_personality"]
extern "C" fn eh_personality() {}

/// Panic handler — outputs a [PANIC] prefix and halts.
///
/// Note: Full PanicInfo formatting requires alloc (core::fmt::Display for
/// PanicInfo is not available in no_std without alloc). This handler
/// outputs a simple prefix; future versions may add location info.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println!("[PANIC] user-space component panic");
    loop {
        core::hint::spin_loop();
    }
}
