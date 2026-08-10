//! seL4 base types and constants (aarch64 / seL4 14.0.0, ADR-0023).
//!
//! These mirror `libsel4/include/sel4/types.h` and the generated
//! `shared_types_gen.h`. The C-side shim (`shim/sel4_shim.c`) re-exports
//! platform-specific enum values via getter functions; we call those getters
//! at runtime so we never hardcode a value that depends on seL4 config.

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

// ----------------------------------------------------------------------------
// Base scalar types (seL4 64-bit).
// ----------------------------------------------------------------------------
/// `seL4_Word` — the seL4 machine word. 64-bit on aarch64.
pub type Word = u64;

/// `seL4_CPtr` — a capability handle. Opaque integer passed in a register.
pub type CPtr = u64;

/// `seL4_Error` — syscall return code. `0` = `seL4_NoError`.
pub type Error = i64;

/// `seL4_ObjectType` — object kind for `Untyped_Retype`. Platform-specific;
/// resolve via `object_type` getters rather than hardcoding.
pub type ObjectType = u64;

/// `seL4_CapRights` — rights bitmask for capability mint/copy.
pub type CapRights = u64;

/// `seL4_MessageInfo` — packed IPC message metadata
/// (label / length / capsUnwrapped / extraCaps). See [`message_info_new`].
pub type MessageInfo = u64;

/// `seL4_Bool` — C99 `_Bool`; maps to `u8` on aarch64 AAPCS64.
pub type Bool = u8;

/// `seL4_ARM_VMAttributes` — page mapping attributes. Resolve via
/// `arm_default_attributes`.
pub type ArmVmAttributes = u64;

// ----------------------------------------------------------------------------
// Error codes (stable across platforms — seL4 shared_types.bf).
// ----------------------------------------------------------------------------
pub const NO_ERROR: Error = 0;
pub const INVALID_ARGUMENT: Error = 1;
pub const INVALID_CAPABILITY: Error = 2;
pub const ILLEGAL_OPERATION: Error = 3;
pub const RANGE_ERROR: Error = 4;
pub const ALIGNMENT_ERROR: Error = 5;
pub const FAILED_LOOKUP: Error = 6;
pub const TRUNCATED_MESSAGE: Error = 7;
pub const DELETE_FIRST: Error = 8;
pub const REVOKE_FIRST: Error = 9;
pub const NOT_ENOUGH_MEMORY: Error = 10;

/// Human-readable error name. Used by the safe wrappers and by tests.
pub fn error_name(e: Error) -> &'static str {
    match e {
        NO_ERROR => "seL4_NoError",
        INVALID_ARGUMENT => "seL4_InvalidArgument",
        INVALID_CAPABILITY => "seL4_InvalidCapability",
        ILLEGAL_OPERATION => "seL4_IllegalOperation",
        RANGE_ERROR => "seL4_RangeError",
        ALIGNMENT_ERROR => "seL4_AlignmentError",
        FAILED_LOOKUP => "seL4_FailedLookup",
        TRUNCATED_MESSAGE => "seL4_TruncatedMessage",
        DELETE_FIRST => "seL4_DeleteFirst",
        REVOKE_FIRST => "seL4_RevokeFirst",
        NOT_ENOUGH_MEMORY => "seL4_NotEnoughMemory",
        _ => "seL4_UnknownError",
    }
}

// ----------------------------------------------------------------------------
// seL4_UserContext register indices (aarch64).
//
// seL4_UserContext on aarch64 is a flat array of seL4_Word. The first N
// entries have fixed meanings (see libsel4 sel4_arch_include/aarch64/types.h):
//   index 0  = PC
//   index 1  = SP
//   index 2..32 = X0..X30
//   index 33 = NZCV (PSTATE flags)
//
// 30 RT-012：索引与注释对齐——旧代码 X 寄存器偏移 +1（X0=3...X30=33）
// 且 REG_X30 与 REG_NZCV 同为 33（碰撞，二者只能其一正确）；现修正为
// X0=2...X30=32（NZCV=33 不变）。REG_X0 消费方（spawn initial_registers）
// 同步修正（index 2 = x0——旧值 3 实为 x1，服务参数错位被 QEMU 运行
// 掩盖：svc 不依赖参数）。
//
// These are needed by AGT-003 to set the entry point + stack of a spawned
// Agent thread (TCB_WriteRegisters).
// ----------------------------------------------------------------------------
pub const REG_PC: usize = 0;
pub const REG_SP: usize = 1;
pub const REG_X0: usize = 2;
pub const REG_X1: usize = 3;
pub const REG_X2: usize = 4;
pub const REG_X3: usize = 5;
pub const REG_X4: usize = 6;
pub const REG_X5: usize = 7;
pub const REG_X6: usize = 8;
pub const REG_X7: usize = 9;
pub const REG_X8: usize = 10;
pub const REG_X9: usize = 11;
pub const REG_X10: usize = 12;
pub const REG_X11: usize = 13;
pub const REG_X12: usize = 14;
pub const REG_X13: usize = 15;
pub const REG_X14: usize = 16;
pub const REG_X15: usize = 17;
pub const REG_X16: usize = 18;
pub const REG_X17: usize = 19;
pub const REG_X18: usize = 20;
pub const REG_X19: usize = 21;
pub const REG_X20: usize = 22;
pub const REG_X21: usize = 23;
pub const REG_X22: usize = 24;
pub const REG_X23: usize = 25;
pub const REG_X24: usize = 26;
pub const REG_X25: usize = 27;
pub const REG_X26: usize = 28;
pub const REG_X27: usize = 29;
pub const REG_X28: usize = 30;
pub const REG_X29: usize = 31; // FP
pub const REG_X30: usize = 32; // LR
pub const REG_NZCV: usize = 33;

/// Conservative upper bound on `seL4_UserContext` register count across
/// supported seL4 aarch64 configs. The actual count is returned by
/// `user_context_n_registers` (resolved from the C shim).
pub const USER_CONTEXT_MAX_REGS: usize = 64;

/// `seL4_UserContext` — the register file read/written by TCB_ReadRegisters
/// / TCB_WriteRegisters. Represented as a fixed-size array sized to the
/// maximum across configs; only the first `n_registers()` entries are live.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UserContext {
    pub regs: [Word; USER_CONTEXT_MAX_REGS],
}

impl UserContext {
    /// Create a zeroed context (all registers 0). This is the safe default
    /// for a freshly spawned thread whose registers haven't been set yet.
    pub const fn zeroed() -> Self {
        Self {
            regs: [0; USER_CONTEXT_MAX_REGS],
        }
    }

    /// Set the program counter (entry point).
    pub fn set_pc(&mut self, pc: Word) {
        self.regs[REG_PC] = pc;
    }

    /// Set the stack pointer.
    pub fn set_sp(&mut self, sp: Word) {
        self.regs[REG_SP] = sp;
    }

    /// Set a general-purpose register Xn (0..=30).
    pub fn set_x(&mut self, n: usize, val: Word) {
        if n <= 30 {
            self.regs[REG_X0 + n] = val;
        }
    }

    /// Read the program counter.
    pub fn pc(&self) -> Word {
        self.regs[REG_PC]
    }

    /// Read the stack pointer.
    pub fn sp(&self) -> Word {
        self.regs[REG_SP]
    }
}

impl Default for UserContext {
    fn default() -> Self {
        Self::zeroed()
    }
}

// ----------------------------------------------------------------------------
// seL4_BootInfo — opaque handle.
//
// The real `seL4_BootInfo` struct is large and config-specific (it describes
// the untyped caps, user-image frames, device-frame caps etc. handed to the
// root task at boot). For the Rust FFI we treat it as an opaque blob and let
// the root task parse it via offset accessors as needed. This keeps the
// binding stable across seL4 versions.
// ----------------------------------------------------------------------------
#[repr(C)]
pub struct BootInfo {
    _opaque: [Word; 0],
}

// ----------------------------------------------------------------------------
// MessageInfo bitfield packing (aarch64, seL4 14).
//
// seL4_MessageInfo is a 64-bit packed word:
//   bits 0..7       = length (message words in IPC buffer beyond MR registers)
//   bits 8..15      = extraCaps (caps transferred)
//   bits 16..23     = capsUnwrapped (bits set if cap was unwrapped)
//   bits 24..63     = label (seL4_Fault tag or user-defined)
//
// The exact bit layout is in libsel4's `types_gen.h`. We pack the same way
// here; the C shim's `seL4_MessageInfo_new` is the authoritative packer and
// we also expose a Rust-side packer for convenience.
// ----------------------------------------------------------------------------
pub const MSG_LABEL_BITS: u32 = 40;
pub const MSG_CAPS_UNWRAPPED_BITS: u32 = 8;
pub const MSG_EXTRA_CAPS_BITS: u32 = 8;
pub const MSG_LENGTH_BITS: u32 = 8;

/// Pack a MessageInfo word. Mirrors `seL4_MessageInfo_new`.
pub const fn message_info_new(
    label: Word,
    caps_unwrapped: Word,
    extra_caps: Word,
    length: Word,
) -> MessageInfo {
    let length = length & ((1u64 << MSG_LENGTH_BITS) - 1);
    let extra_caps = extra_caps & ((1u64 << MSG_EXTRA_CAPS_BITS) - 1);
    let caps_unwrapped = caps_unwrapped & ((1u64 << MSG_CAPS_UNWRAPPED_BITS) - 1);
    let label = label & ((1u64 << MSG_LABEL_BITS) - 1);
    (label << (MSG_CAPS_UNWRAPPED_BITS + MSG_EXTRA_CAPS_BITS + MSG_LENGTH_BITS))
        | (caps_unwrapped << (MSG_EXTRA_CAPS_BITS + MSG_LENGTH_BITS))
        | (extra_caps << MSG_LENGTH_BITS)
        | length
}

/// Extract the length field from a MessageInfo word.
pub const fn message_info_length(info: MessageInfo) -> Word {
    info & ((1u64 << MSG_LENGTH_BITS) - 1)
}

/// Extract the extraCaps field from a MessageInfo word.
pub const fn message_info_extra_caps(info: MessageInfo) -> Word {
    (info >> MSG_LENGTH_BITS) & ((1u64 << MSG_EXTRA_CAPS_BITS) - 1)
}

/// Extract the label field from a MessageInfo word.
pub const fn message_info_label(info: MessageInfo) -> Word {
    info >> (MSG_CAPS_UNWRAPPED_BITS + MSG_EXTRA_CAPS_BITS + MSG_LENGTH_BITS)
}

// ----------------------------------------------------------------------------
// Object-type and rights getters.
//
// These call the C shim, which resolves the platform-specific enum values
// from the generated libsel4 headers. On host builds they return 0 (the
// host stub never spawns real seL4 objects).
// ----------------------------------------------------------------------------
/// Object-type enum resolver. Returned values are platform-specific; use
/// these instead of hardcoding.
pub struct ObjectTypeTable {
    pub untyped: ObjectType,
    pub tcb: ObjectType,
    pub endpoint: ObjectType,
    pub notification: ObjectType,
    pub captable: ObjectType,
    pub arm_frame: ObjectType,
    pub arm_page_table: ObjectType,
    pub arm_vspace: ObjectType,
}

/// Rights enum resolver.
pub struct RightsTable {
    pub none: CapRights,
    pub read: CapRights,
    pub write: CapRights,
    pub grant: CapRights,
    pub all: CapRights,
}

#[cfg(test)]
mod tests {
    /// 30 RT-012 判据：36 个 REG 索引常量互不相等（X30 与 NZCV 碰撞回归
    /// 防护）；X0=2 / X30=32 / NZCV=33 与 seL4 aarch64 布局一致。
    #[test]
    fn reg_indices_unique() {
        let all = [
            REG_PC, REG_SP, REG_X0, REG_X1, REG_X2, REG_X3, REG_X4, REG_X5, REG_X6, REG_X7,
            REG_X8, REG_X9, REG_X10, REG_X11, REG_X12, REG_X13, REG_X14, REG_X15, REG_X16,
            REG_X17, REG_X18, REG_X19, REG_X20, REG_X21, REG_X22, REG_X23, REG_X24, REG_X25,
            REG_X26, REG_X27, REG_X28, REG_X29, REG_X30, REG_NZCV,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "REG 索引碰撞：index {i} 与 {j} 同为 {a}");
                }
            }
        }
        assert_eq!(REG_PC, 0);
        assert_eq!(REG_SP, 1);
        assert_eq!(REG_X0, 2, "x0 必须位于 index 2（seL4 aarch64 布局）");
        assert_eq!(REG_X30, 32, "x30 必须位于 index 32（非 33）");
        assert_eq!(REG_NZCV, 33);
    }

    use super::*;

    #[test]
    fn error_name_covers_all_codes() {
        assert_eq!(error_name(NO_ERROR), "seL4_NoError");
        assert_eq!(error_name(NOT_ENOUGH_MEMORY), "seL4_NotEnoughMemory");
        assert_eq!(error_name(999), "seL4_UnknownError");
    }

    #[test]
    fn message_info_round_trips() {
        let info = message_info_new(0xABCDEF, 1, 2, 3);
        assert_eq!(message_info_length(info), 3);
        assert_eq!(message_info_extra_caps(info), 2);
        assert_eq!(message_info_label(info), 0xABCDEF);
    }

    #[test]
    fn user_context_pc_sp() {
        let mut ctx = UserContext::zeroed();
        ctx.set_pc(0x4000_0000);
        ctx.set_sp(0x7000_0000);
        ctx.set_x(0, 0x1234);
        assert_eq!(ctx.pc(), 0x4000_0000);
        assert_eq!(ctx.sp(), 0x7000_0000);
        assert_eq!(ctx.regs[REG_X0], 0x1234);
    }
}
