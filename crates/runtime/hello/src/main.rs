//! EnerOS Hello — First Rust user-space component (v0.4.0)
//!
//! This is the first seL4 user-space Rust program. It initializes the
//! runtime and prints a greeting message to the serial console, then
//! halts in an infinite loop.

// On the bare-metal target (`aarch64-unknown-none`) this crate is `no_std + no_main`
// with its own panic handler / lang items. On host builds (`cargo build --workspace`)
// we compile as a regular std binary so the host linker has a `main` entry point.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_os = "none", feature(lang_items))]
#![allow(internal_features)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

extern crate eneros_runtime;

use eneros_runtime::{init, println};

/// Entry point called by seL4 after loading the user-space image.
///
/// Initializes the runtime, prints the hello banner, and halts.
#[cfg(target_os = "none")]
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

/// Host-side entry point — exists only so `cargo build --workspace` can link
/// this crate on the host target. The real entry point is `_start` above on
/// `aarch64-unknown-none`.
#[cfg(not(target_os = "none"))]
fn main() {
    init();
    println!("Hello from Rust on seL4!");
    println!("EnerOS Phase 0 - first userland component.");
    println!("== Userland component alive ==");
    println!("Target: aarch64-unknown-none");
}

/// Personality function required by the Rust compiler for unwind handling.
/// In no_std + panic=abort, this is never called but must be defined.
#[cfg(target_os = "none")]
#[lang = "eh_personality"]
extern "C" fn eh_personality() {}

/// Panic handler — outputs a [PANIC] prefix and halts.
///
/// Note: Full PanicInfo formatting requires alloc (core::fmt::Display for
/// PanicInfo is not available in no_std without alloc). This handler
/// outputs a simple prefix; future versions may add location info.
#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println!("[PANIC] user-space component panic");
    loop {
        core::hint::spin_loop();
    }
}
