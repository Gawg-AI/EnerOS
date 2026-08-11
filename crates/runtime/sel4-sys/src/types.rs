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
// seL4_UserContext register indices (aarch64, seL4 14.0.0).
//
// 权威布局（本仓库构建同源：libsel4/sel4_arch_include/aarch64/sel4/sel4_arch/
// types.h 的 seL4_UserContext 字段序 + 内核 src/arch/arm/64/machine/
// registerset.c 的 frameRegisters/gpRegisters 顺序）——**X 寄存器不连续**
// （帧寄存器组 X0..X8/X16..X18/X29/X30 在前，其余 X9..X15/X19..X28 在后）：
//
//   index 0   = PC（FaultIP）
//   index 1   = SP（SP_EL0）
//   index 2   = SPSR（PSTATE——aarch64 UserContext **无独立 NZCV 字段**，
//               NZCV 是 SPSR 的位）
//   index 3..11  = X0..X8
//   index 12..14 = X16..X18
//   index 15..16 = X29..X30
//   index 17..23 = X9..X15
//   index 24..33 = X19..X28
//   index 34..35 = TPIDR_EL0 / TPIDRRO_EL0
//   共 36 words = 288 B（sel4_client.h `assert_size_correct(..., 288)`）。
//
// 30 RT-012 纠偏（相对上一轮错误修正）：上一轮把 X0 从 3 改为 2、X30 从
// 33 改为 32、引入 REG_NZCV=33——与权威布局矛盾（index 2 实为 SPSR、
// index 3 实为 X0、index 16 实为 X30、index 33 实为 X28）。spawn 的
// `initial_registers`（X0=ep cap 槽、X1=IPC buffer vaddr）会把 ep 写进
// SPSR、buffer 写进 x0 → 服务 `_start(ep, ipc_buffer)` 参数错位（NULL
// IPC buffer + 无效 cptr → fail-closed 挂起）。本修正恢复权威索引
// （X0=3/X1=4...），`set_x` 改表驱动映射（X 索引不连续，禁止线性 +n）。
//
// These are needed by AGT-003 to set the entry point + stack of a spawned
// Agent thread (TCB_WriteRegisters).
// ----------------------------------------------------------------------------
pub const REG_PC: usize = 0;
pub const REG_SP: usize = 1;
pub const REG_SPSR: usize = 2;
pub const REG_X0: usize = 3;
pub const REG_X1: usize = 4;
pub const REG_X2: usize = 5;
pub const REG_X3: usize = 6;
pub const REG_X4: usize = 7;
pub const REG_X5: usize = 8;
pub const REG_X6: usize = 9;
pub const REG_X7: usize = 10;
pub const REG_X8: usize = 11;
pub const REG_X16: usize = 12;
pub const REG_X17: usize = 13;
pub const REG_X18: usize = 14;
pub const REG_X29: usize = 15;
pub const REG_X30: usize = 16;
pub const REG_X9: usize = 17;
pub const REG_X10: usize = 18;
pub const REG_X11: usize = 19;
pub const REG_X12: usize = 20;
pub const REG_X13: usize = 21;
pub const REG_X14: usize = 22;
pub const REG_X15: usize = 23;
pub const REG_X19: usize = 24;
pub const REG_X20: usize = 25;
pub const REG_X21: usize = 26;
pub const REG_X22: usize = 27;
pub const REG_X23: usize = 28;
pub const REG_X24: usize = 29;
pub const REG_X25: usize = 30;
pub const REG_X26: usize = 31;
pub const REG_X27: usize = 32;
pub const REG_X28: usize = 33;
pub const REG_TPIDR_EL0: usize = 34;
pub const REG_TPIDRRO_EL0: usize = 35;

/// Xn → UserContext 索引（aarch64 X 寄存器组不连续，30 RT-012 纠偏后
/// 唯一映射入口——禁止线性 `REG_X0 + n`）。
/// 非法 n（>30）→ None。
pub const fn reg_x_index(n: usize) -> Option<usize> {
    match n {
        0..=8 => Some(REG_X0 + n),           // X0..X8 @ 3..=11
        9..=15 => Some(REG_X9 + (n - 9)),    // X9..X15 @ 17..=23
        16..=18 => Some(REG_X16 + (n - 16)), // X16..X18 @ 12..=14
        19..=28 => Some(REG_X19 + (n - 19)), // X19..X28 @ 24..=33
        29 => Some(REG_X29),                 // @ 15
        30 => Some(REG_X30),                 // @ 16
        _ => None,
    }
}

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
        // 30 RT-012 纠偏：X 寄存器在 seL4 aarch64 UserContext 中**不连续**
        // （帧寄存器组 X0..X8/X16..X18/X29/X30 在前，X9..X15/X19..X28 在
        // 后），经 reg_x_index 表映射——禁止线性 `REG_X0 + n`（X0=3 起
        // 连续 +8 后跳到 X16=12，X9=17）。
        if let Some(idx) = reg_x_index(n) {
            self.regs[idx] = val;
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
    /// 30 RT-012 判据（权威布局，seL4 14.0.0 aarch64）：REG 常量锚定 +
    /// 36 索引两两不等 + `reg_x_index` 与常量表一致（X 不连续映射）。
    #[test]
    fn reg_indices_match_authoritative_aarch64_layout() {
        // 锚定（libsel4 types.h 字段序 + 内核 registerset.c frameRegisters/
        // gpRegisters 顺序；spawn 服务参数链依赖 X0/X1）。
        assert_eq!(REG_PC, 0);
        assert_eq!(REG_SP, 1);
        assert_eq!(REG_SPSR, 2, "index 2 = SPSR（aarch64 无独立 NZCV 字段）");
        assert_eq!(REG_X0, 3, "x0 必须位于 index 3（spawn 服务参数依赖）");
        assert_eq!(REG_X1, 4);
        assert_eq!(REG_X8, 11);
        assert_eq!(REG_X16, 12);
        assert_eq!(REG_X17, 13);
        assert_eq!(REG_X18, 14);
        assert_eq!(REG_X29, 15);
        assert_eq!(REG_X30, 16, "x30 位于 index 16（帧寄存器组末尾）");
        assert_eq!(REG_X9, 17, "x9 位于 index 17（其他整数寄存器组）");
        assert_eq!(REG_X28, 33);
        assert_eq!(REG_TPIDR_EL0, 34);
        assert_eq!(REG_TPIDRRO_EL0, 35);
        // 唯一性：全部 36 个索引两两不等。
        let all = [
            REG_PC, REG_SP, REG_SPSR, REG_X0, REG_X1, REG_X2, REG_X3, REG_X4, REG_X5, REG_X6,
            REG_X7, REG_X8, REG_X9, REG_X10, REG_X11, REG_X12, REG_X13, REG_X14, REG_X15, REG_X16,
            REG_X17, REG_X18, REG_X19, REG_X20, REG_X21, REG_X22, REG_X23, REG_X24, REG_X25,
            REG_X26, REG_X27, REG_X28, REG_X29, REG_X30, REG_TPIDR_EL0, REG_TPIDRRO_EL0,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "REG 索引碰撞：index {i} 与 {j} 同为 {a}");
                }
            }
        }
        // reg_x_index 与常量表一致（Xn 全部命中且索引正确；X31 非法）。
        for n in 0..=30usize {
            let expect = match n {
                0..=8 => 3 + n,
                9..=15 => 17 + (n - 9),
                16..=18 => 12 + (n - 16),
                19..=28 => 24 + (n - 19),
                29 => 15,
                30 => 16,
                _ => unreachable!(),
            };
            assert_eq!(reg_x_index(n), Some(expect), "X{n} 映射错误");
        }
        assert_eq!(reg_x_index(31), None, "X31 不存在");
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

    /// 30 RT-012 回归锚点：spawn `initial_registers` 契约（固定索引 3/4，
    /// 不引用常量）——服务 `_start(ep, ipc_buffer)` 经 aarch64 ABI 从
    /// x0/x1 读参；索引错位（x0 被写进 spsr=2、buffer 被写进 x0=3）时
    /// 服务收到 NULL IPC buffer + 无效 cptr → fail-closed 挂起。
    #[test]
    fn spawn_register_contract_x0_ep_x1_buffer() {
        let mut regs = UserContext::zeroed();
        regs.set_pc(0x202_024);
        regs.set_sp(0x221_000);
        regs.set_x(0, 0); // ep cap 槽 0
        regs.set_x(1, 0x221_000); // IPC buffer vaddr
        assert_eq!(regs.regs[0], 0x202_024, "pc 位于 index 0");
        assert_eq!(regs.regs[1], 0x221_000, "sp 位于 index 1");
        assert_eq!(regs.regs[2], 0, "spsr=0（合法 EL0 PSTATE，不得被污染）");
        assert_eq!(regs.regs[3], 0, "x0 必须是 ep（index 3）");
        assert_eq!(regs.regs[4], 0x221_000, "x1 必须是 buffer vaddr（index 4）");
    }
}
