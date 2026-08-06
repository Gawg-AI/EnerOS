//! EnerOS seL4 syscall FFI bindings (no_std, ADR-0023).
//!
//! Two backends, selected by `build.rs` via cfg flags:
//!
//! - `sel4_target` (set when `target_os = "none"`, i.e. the seL4 userland
//!   target `aarch64-unknown-none`): real libsel4 link. `build.rs` compiles
//!   `shim/sel4_shim.c` against the generated libsel4 headers and links
//!   `libsel4.a` + `libgcc.a`. The [`ffi`] module declares the extern "C"
//!   bindings to the shim; [`types`] holds the matching Rust types.
//!
//! - `host_build` (set for any other target, e.g. `cargo test` on x86_64):
//!   in-process no-op stubs. No C compiler or libsel4 needed. Lets the
//!   workspace build and unit-test on the host without a seL4 toolchain.
//!
//! See `docs/adr/ADR-0023-sel4-kernel-baseline.md` for the seL4 baseline
//! decision and `shim/sel4_shim.h` for the C-side wrapper contract.

#![no_std]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

extern crate alloc;

pub mod types;
pub mod safe;

#[cfg(sel4_target)]
pub mod ffi;

// Re-export common types at crate root for ergonomic access.
pub use types::*;

// ----------------------------------------------------------------------------
// Serial output — the one API the runtime console actually uses.
//
// `seL4_put_char` routes to the real `seL4_DebugPutChar` via the C shim on
// the seL4 target; on host builds it is a no-op stub so `cargo test` works
// without a seL4 toolchain.
// ----------------------------------------------------------------------------

/// Write one byte to the seL4 debug serial port.
///
/// Returns 0 on success, mirroring the seL4 syscall return convention.
/// On host builds this is a no-op returning 0.
#[cfg(sel4_target)]
pub fn seL4_put_char(c: u8) -> isize {
    // The C shim's `eneros_sel4_debug_put_char` wraps `seL4_DebugPutChar`,
    // which takes a `char`. On aarch64 AAPCS64 `char` is unsigned, matching
    // `u8` — no sign conversion needed.
    let _ = unsafe { ffi::eneros_sel4_debug_put_char(c) };
    0
}

/// Host-build stub: no-op, returns 0.
#[cfg(host_build)]
pub fn seL4_put_char(_c: u8) -> isize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_char_returns_zero_on_host() {
        assert_eq!(seL4_put_char(b'A'), 0);
        assert_eq!(seL4_put_char(0x0A), 0);
    }
}
