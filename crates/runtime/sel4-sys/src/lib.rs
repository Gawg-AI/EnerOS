//! EnerOS seL4 syscall FFI bindings (no_std, ADR-0023).
//!
//! Two backends, selected by `build.rs` via cfg flags:
//!
//! - `sel4_target` (set when `target_os = "none"`, i.e. the seL4 userland
//!   target `aarch64-unknown-none`): real libsel4 link. `build.rs` compiles
//!   `shim/sel4_shim.c` against the generated libsel4 headers and links
//!   `libsel4.a` + `libgcc.a`. The `ffi` module declares the extern "C"
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

pub mod safe;
pub mod types;

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
    // SAFETY: `eneros_sel4_debug_put_char` 为 C shim（host 为 stub）
    // 封装，参数为 u8 纯值，无指针，无内存安全风险。
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

// Host-build FFI stubs (host_build): in-process MR array + no-op calls.
// Lets agent-sdk ipc_sel4 tests run on the host (蓝图/20 R2-2).
#[cfg(host_build)]
pub mod ffi {
    use core::sync::atomic::{AtomicU64, Ordering};

    /// Host-side message-register store (64 slots, per-process).
    static HOST_MR: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];

    /// Write one MR slot (host stub; index bounds-checked).
    ///
    /// # Safety
    /// Host stub：越界索引被忽略，无内存安全风险（与目标 shim 行为一致）。
    pub unsafe fn eneros_sel4_set_mr(idx: u64, val: u64) {
        if (idx as usize) < HOST_MR.len() {
            HOST_MR[idx as usize].store(val, Ordering::SeqCst);
        }
    }

    /// Read one MR slot (host stub).
    ///
    /// # Safety
    /// Host stub：越界索引返回 0，无内存安全风险。
    pub unsafe fn eneros_sel4_get_mr(idx: u64) -> u64 {
        if (idx as usize) < HOST_MR.len() {
            HOST_MR[idx as usize].load(Ordering::SeqCst)
        } else {
            0
        }
    }

    /// Build a seL4 message-info word (host stub: length-only packing).
    ///
    /// 30 RT-013：编码与 types.rs 位段统一（length 在 bits 0-7）——旧实现
    /// `(length & 0xFF) << 8` 与 `message_info_length`（取 bits 0-7）解码
    /// 方向不一致，host 自回环读回错误 length。
    ///
    /// # Safety
    /// Host stub：纯值构造，无指针参数。
    pub unsafe fn eneros_sel4_message_info_new(
        _label: u64,
        _caps_unwrapped: u64,
        _extra_caps: u64,
        length: u64,
    ) -> u64 {
        crate::types::message_info_new(0, 0, 0, length)
    }

    /// Synchronous call (host stub: returns immediately, no peer).
    ///
    /// # Safety
    /// Host stub：无实际能力操作，调用方负责目标构建下传参合法。
    pub unsafe fn eneros_sel4_call(_dest_cap: u64, _msg_info: u64) -> u64 {
        0
    }

    /// Fire-and-forget send (host stub).
    ///
    /// # Safety
    /// Host stub：无实际能力操作，调用方负责目标构建下传参合法。
    pub unsafe fn eneros_sel4_send(_dest_cap: u64, _msg_info: u64) {}
}
