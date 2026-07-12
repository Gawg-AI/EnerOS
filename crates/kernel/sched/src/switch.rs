//! ARM64 context switch — Phase 0 P0-F (v0.18.0).
//!
//! Provides [`context_switch`] (a naked function with aarch64 inline asm)
//! and [`thread_switch`] (a safe wrapper over `Tcb`).
//!
//! Per D3, aarch64 asm is cfg-gated; host builds get a stub.

use crate::tcb::Tcb;

/// ARM64 context switch (naked function).
///
/// Saves callee-saved registers (x19-x30) to the current stack, stores
/// the current sp to `*from_sp` (x0), loads sp from `*to_sp` (x1),
/// restores the target's callee-saved registers, and `ret`s.
///
/// Per the aarch64 C ABI, `from_sp` is in x0 and `to_sp` is in x1 on
/// entry. The `naked_asm!` macro (nightly-2026-04-04) does not allow
/// register operands, so the asm references x0/x1 directly.
///
/// # Safety
///
/// This is a naked function — the compiler emits no prologue/epilogue.
/// The asm must be self-contained. Callers must ensure `from_sp` and
/// `to_sp` point to valid `u64` stack-pointer storage.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
pub unsafe extern "C" fn context_switch(from_sp: *mut u64, to_sp: *const u64) {
    core::arch::naked_asm!(
        "stp x29, x30, [sp, #-16]!",
        "stp x27, x28, [sp, #-16]!",
        "stp x25, x26, [sp, #-16]!",
        "stp x23, x24, [sp, #-16]!",
        "stp x21, x22, [sp, #-16]!",
        "stp x19, x20, [sp, #-16]!",
        "mov x2, sp",   // x2 = current sp (can't str/ldr sp directly)
        "str x2, [x0]", // *from_sp = saved sp (x0 = first arg)
        "ldr x2, [x1]", // x2 = *to_sp (x1 = second arg)
        "mov sp, x2",   // sp = x2
        "ldp x19, x20, [sp], #16",
        "ldp x21, x22, [sp], #16",
        "ldp x23, x24, [sp], #16",
        "ldp x25, x26, [sp], #16",
        "ldp x27, x28, [sp], #16",
        "ldp x29, x30, [sp], #16",
        "ret",
    );
}

/// Host-side stub: real context switch is impossible without aarch64.
///
/// Calling this on a non-aarch64 target panics. The stub exists so the
/// crate compiles on host for unit testing of the API surface.
///
/// # Safety
///
/// This function is unsafe because it is a stub — on host targets it
/// always panics. Callers must ensure they only invoke the real
/// [`context_switch`] on `aarch64` targets.
#[cfg(not(target_arch = "aarch64"))]
#[allow(clippy::disallowed_macros)]
pub unsafe extern "C" fn context_switch(_from_sp: *mut u64, _to_sp: *const u64) {
    panic!("context_switch requires aarch64; not available on host target");
}

/// Switch from `from` thread to `to` thread.
///
/// Saves `from`'s context (sp/pc updated in the Tcb), loads `to`'s
/// context. On host this panics (see D3).
///
/// # Panics
///
/// Panics on non-aarch64 targets (host testing only).
pub fn thread_switch(from: &mut Tcb, to: &Tcb) {
    // SAFETY: On aarch64, the naked function handles register save/restore.
    // On host, this panics — thread_switch must not be called in host tests.
    unsafe { context_switch(&mut from.sp as *mut u64, &to.sp as *const u64) };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_switch_signature() {
        // Verify the function exists and has the right signature.
        // We don't call it directly — on host it panics (extern "C" can't
        // unwind, so catch_unwind can't intercept it). The signature check
        // confirms the API surface compiles correctly.
        let _f: unsafe extern "C" fn(*mut u64, *const u64) = context_switch;
    }

    #[test]
    fn test_thread_switch_signature() {
        // Verify thread_switch exists and has the right signature.
        // We don't call it on host — context_switch panics in extern "C"
        // (non-unwinding), which would abort the test process.
        let _f: fn(&mut crate::tcb::Tcb, &crate::tcb::Tcb) = thread_switch;
    }
}
