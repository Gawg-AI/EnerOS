//! EnerOS seL4 syscall FFI bindings (no_std, minimal).
//!
//! Provides minimal seL4 syscall bindings for aarch64 targets via inline
//! assembly (`svc #0`), and host-side stub implementations for unit testing.

#![no_std]
// seL4 API uses camelCase naming (e.g. seL4_put_char); mirror it in FFI bindings.
#![allow(non_snake_case)]

/// seL4 capability endpoint handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// Capability slot value.
    pub cap: u64,
}

/// Syscall number: output a single character to the debug serial port.
pub const SYSCALL_PUT_CHAR: u64 = 0;
/// Syscall number: send a message on an endpoint.
pub const SYSCALL_SEND: u64 = 1;
/// Syscall number: receive a message on an endpoint.
pub const SYSCALL_RECV: u64 = 2;

// ---------------------------------------------------------------------------
// aarch64 implementation (inline asm via `svc #0`)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "aarch64")]
/// Writes a single byte to the seL4 debug serial output.
///
/// # Safety
/// Performs a raw `svc #0` syscall which is only valid in a seL4 user-space
/// context with a compatible kernel.
pub fn seL4_put_char(c: u8) -> isize {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "mov x7, {nr}",
            "svc #0",
            nr = in(reg) SYSCALL_PUT_CHAR,
            inout("x0") c as u64 => ret,
            options(nostack, preserves_flags),
        );
    }
    ret as isize
}

#[cfg(target_arch = "aarch64")]
/// Sends a 64-bit message on the given endpoint.
///
/// # Safety
/// Performs a raw `svc #0` syscall; the endpoint capability must be valid.
pub fn seL4_send(ep: Endpoint, msg: u64) -> isize {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "mov x7, {nr}",
            "svc #0",
            nr = in(reg) SYSCALL_SEND,
            inout("x0") ep.cap => ret,
            in("x1") msg,
            options(nostack, preserves_flags),
        );
    }
    ret as isize
}

#[cfg(target_arch = "aarch64")]
/// Receives a 64-bit message on the given endpoint.
///
/// # Safety
/// Performs a raw `svc #0` syscall; the endpoint capability must be valid.
pub fn seL4_recv(ep: Endpoint) -> u64 {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "mov x7, {nr}",
            "svc #0",
            nr = in(reg) SYSCALL_RECV,
            inout("x0") ep.cap => ret,
            options(nostack, preserves_flags),
        );
    }
    ret
}

// ---------------------------------------------------------------------------
// Host stub implementation (non-aarch64) — returns 0 for unit testing.
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "aarch64"))]
/// Host stub: returns 0. On aarch64 this would output `c` via seL4 syscall.
pub fn seL4_put_char(_c: u8) -> isize {
    0
}

#[cfg(not(target_arch = "aarch64"))]
/// Host stub: returns 0. On aarch64 this would send `msg` on `ep`.
pub fn seL4_send(_ep: Endpoint, _msg: u64) -> isize {
    0
}

#[cfg(not(target_arch = "aarch64"))]
/// Host stub: returns 0. On aarch64 this would receive a message on `ep`.
pub fn seL4_recv(_ep: Endpoint) -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_construction_and_field_access() {
        let ep = Endpoint { cap: 42 };
        assert_eq!(ep.cap, 42);
    }

    #[test]
    fn endpoint_copy_behavior() {
        let ep = Endpoint { cap: 7 };
        let ep_copy = ep;
        assert_eq!(ep, ep_copy);
        // Verify both copies are independent values (Copy semantics).
        let ep_other = Endpoint { cap: 99 };
        assert_ne!(ep, ep_other);
    }

    #[test]
    fn host_stub_put_char_returns_zero() {
        assert_eq!(seL4_put_char(b'A'), 0);
        assert_eq!(seL4_put_char(0x0A), 0);
    }

    #[test]
    fn host_stub_send_returns_zero() {
        let ep = Endpoint { cap: 1 };
        assert_eq!(seL4_send(ep, 0xDEAD_BEEF), 0);
    }

    #[test]
    fn host_stub_recv_returns_zero() {
        let ep = Endpoint { cap: 2 };
        assert_eq!(seL4_recv(ep), 0);
    }

    #[test]
    fn syscall_constants_are_distinct() {
        assert_ne!(SYSCALL_PUT_CHAR, SYSCALL_SEND);
        assert_ne!(SYSCALL_SEND, SYSCALL_RECV);
        assert_ne!(SYSCALL_PUT_CHAR, SYSCALL_RECV);
        assert_eq!(SYSCALL_PUT_CHAR, 0);
        assert_eq!(SYSCALL_SEND, 1);
        assert_eq!(SYSCALL_RECV, 2);
    }
}
