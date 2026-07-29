//! seL4 兼容层数据类型（v0.156.0 用户态兼容性验证）。
//!
//! 定义 seL4 ABI 兼容的核心类型：[`MessageInfo`]（消息信息结构）、
//! [`CPtr`]（能力指针）、[`Rights`]（权限位）、[`CompatError`]（兼容层
//! 错误类型）。所有类型均为 `repr(C)` 或 `repr(u8/u64)` 保证 ABI 稳定。
//!
//! 蓝图 phase3.md §v0.156.0 §4.1 数据结构原样实现。

/// seL4 能力指针类型（兼容层）。
///
/// seL4 使用 64 位 CPtr 标识能力槽位（CSpace 中的路径编码），
/// 兼容层维护 CPtr → 自研内核 [`CapSlot`](crate::cap::CapSlot) 的映射。
pub type CPtr = u64;

/// seL4 消息信息结构（兼容层）。
///
/// 对应 seL4 `seL4_MessageInfo_t`，包含消息标签、能力解包数、
/// 额外能力数与消息长度。兼容层在翻译时将此结构转换为自研内核
/// 的连续缓冲区模型（蓝图 §5 技术交底：MR ↔ buffer 转换）。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageInfo {
    /// 消息标签（seL4 `label` 字段，用于标识消息类型）。
    pub label: u64,
    /// 解包能力数（seL4 `capsUnwrapped` 字段）。
    pub caps_unwrapped: u8,
    /// 额外能力数（seL4 `extraCaps` 字段）。
    pub extra_caps: u8,
    /// 消息长度（字节数，seL4 `length` 字段）。
    pub length: u16,
}

impl MessageInfo {
    /// 创建空消息（label=0，无能力，长度=0）。
    pub const fn empty() -> Self {
        Self {
            label: 0,
            caps_unwrapped: 0,
            extra_caps: 0,
            length: 0,
        }
    }

    /// 创建带标签和长度的消息。
    pub const fn new(label: u64, length: u16) -> Self {
        Self {
            label,
            caps_unwrapped: 0,
            extra_caps: 0,
            length,
        }
    }

    /// 将 MessageInfo 序列化为 u64（seL4 ABI 寄存器传递格式）。
    ///
    /// 位布局：`[63:32] label | [31:16] length | [15:8] extra_caps | [7:0] caps_unwrapped`
    pub const fn to_word(self) -> u64 {
        (self.label << 32)
            | ((self.length as u64) << 16)
            | ((self.extra_caps as u64) << 8)
            | (self.caps_unwrapped as u64)
    }

    /// 从 u64 反序列化为 MessageInfo。
    pub const fn from_word(word: u64) -> Self {
        Self {
            label: word >> 32,
            length: ((word >> 16) & 0xFFFF) as u16,
            extra_caps: ((word >> 8) & 0xFF) as u8,
            caps_unwrapped: (word & 0xFF) as u8,
        }
    }
}

/// seL4 权限位（兼容层）。
///
/// 对应 seL4 `seL4_CapRights_t`，用于 Mint/Grant 操作时指定
/// 新能力的权限子集。值与 seL4 原始定义一致。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rights {
    /// 无权限。
    None = 0,
    /// 读权限。
    Read = 1,
    /// 写权限。
    Write = 2,
    /// 授予权限（可 mint 派生）。
    Grant = 4,
    /// 读写权限。
    ReadWrite = 3,
    /// 全部权限。
    All = 7,
}

impl Rights {
    /// 从位值构造 Rights（截断到合法范围）。
    pub const fn from_bits(bits: u64) -> Self {
        match (bits & 0x7) as u8 {
            0 => Self::None,
            1 => Self::Read,
            2 => Self::Write,
            3 => Self::ReadWrite,
            4 => Self::Grant,
            7 => Self::All,
            // 其余组合降级为 None（保守策略）。
            _ => Self::None,
        }
    }

    /// 转换为自研内核 [`CapRights`](crate::cap::CapRights) 位值。
    ///
    /// 映射关系：seL4 Read→READ(1)，Write→WRITE(2)，Grant→GRANT(8)。
    pub const fn to_cap_rights_bits(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Read => 1,        // CapRights::READ
            Self::Write => 2,       // CapRights::WRITE
            Self::Grant => 8,       // CapRights::GRANT
            Self::ReadWrite => 3,   // READ | WRITE
            Self::All => 1 | 2 | 8, // READ | WRITE | GRANT
        }
    }
}

/// 兼容层错误类型。
///
/// 蓝图 §4.4 错误处理原样：不支持的 syscall 返回 ENOSYS 语义，
/// 无效能力返回 EFAULT 语义，翻译失败为不可恢复错误。
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatError {
    /// 不支持的 seL4 调用（返回 ENOSYS，用户态可降级）。
    UnsupportedSyscall = 1,
    /// 无效能力指针（返回 EFAULT，触发用户态错误处理）。
    InvalidCapability = 2,
    /// 翻译失败（兼容层 bug，不应发生）。
    TranslationFailed = 3,
    /// 超时。
    Timeout = 4,
}

impl CompatError {
    /// 转换为 usize 错误码（syscall 返回值口径）。
    pub const fn to_code(self) -> usize {
        self as usize
    }
}

/// seL4 syscall 编号常量（兼容层翻译表使用）。
///
/// 编号参照 seL4 内核源码 `libsel4/include/sel4/syscall.h`，
/// 仅列出 Phase 1-2 用户态实际使用到的核心调用（约 9 个）。
pub mod sel4_syscall_nums {
    /// seL4_Mint：能力派生（badge + rights 缩减）。
    pub const SEL4_MINT: u32 = 1;
    /// seL4_Send：IPC 发送（不等待回复）。
    pub const SEL4_SEND: u32 = 2;
    /// seL4_Recv：IPC 接收（阻塞）。
    pub const SEL4_RECV: u32 = 3;
    /// seL4_Wait：通知等待（阻塞）。
    pub const SEL4_WAIT: u32 = 4;
    /// seL4_Yield：让出时间片。
    pub const SEL4_YIELD: u32 = 5;
    /// seL4_Call：IPC 发送并等待回复（同步往返）。
    pub const SEL4_CALL: u32 = 6;
    /// seL4_Move：能力移动。
    pub const SEL4_MOVE: u32 = 7;
    /// seL4_Copy：能力复制。
    pub const SEL4_COPY: u32 = 8;
    /// seL4_Delete：能力删除。
    pub const SEL4_DELETE: u32 = 9;
    /// seL4_Signal：通知发送（非阻塞）。
    pub const SEL4_SIGNAL: u32 = 10;
    /// seL4_NBSend：非阻塞 IPC 发送。
    pub const SEL4_NBSEND: u32 = 11;
    /// seL4_NBRecv：非阻塞 IPC 接收。
    pub const SEL4_NBRECV: u32 = 12;
    /// seL4_Reply：IPC 回复。
    pub const SEL4_REPLY: u32 = 13;
    /// seL4_ReplyRecv：回复并接收（原子组合）。
    pub const SEL4_REPLY_RECV: u32 = 14;
    /// seL4_Retype：Untyped 重类型化（兼容层跳过，内核自动管理）。
    pub const SEL4_RETYPE: u32 = 15;
    /// seL4_TCB_Configure：线程配置（与 Retype 聚合为 ThreadCreate）。
    pub const SEL4_TCB_CONFIGURE: u32 = 16;
    /// seL4_TCB_SetPriority：线程优先级设置。
    pub const SEL4_TCB_SET_PRIORITY: u32 = 17;
    /// seL4_TCB_Resume：线程恢复。
    pub const SEL4_TCB_RESUME: u32 = 18;
    /// seL4_TCB_Suspend：线程挂起。
    pub const SEL4_TCB_SUSPEND: u32 = 19;
    /// seL4_Untyped_Retype：Untyped 重类型化（同 RETYPE，兼容层跳过）。
    pub const SEL4_UNTYPED_RETYPE: u32 = 20;

    /// 翻译表容量（覆盖编号 1~20）。
    pub const TABLE_SIZE: usize = 32;
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// T1：MessageInfo 序列化/反序列化往返一致。
    #[test]
    fn t1_message_info_roundtrip() {
        let msg = MessageInfo {
            label: 0xDEAD_BEEF,
            caps_unwrapped: 2,
            extra_caps: 1,
            length: 128,
        };
        let word = msg.to_word();
        let restored = MessageInfo::from_word(word);
        assert_eq!(msg, restored);
    }

    /// T2：MessageInfo::empty() 全零。
    #[test]
    fn t2_message_info_empty() {
        let msg = MessageInfo::empty();
        assert_eq!(msg.to_word(), 0);
    }

    /// T3：MessageInfo::new() 仅设 label + length。
    #[test]
    fn t3_message_info_new() {
        let msg = MessageInfo::new(42, 64);
        assert_eq!(msg.label, 42);
        assert_eq!(msg.length, 64);
        assert_eq!(msg.caps_unwrapped, 0);
        assert_eq!(msg.extra_caps, 0);
    }

    /// T4：Rights::from_bits 合法值映射。
    #[test]
    fn t4_rights_from_bits() {
        assert_eq!(Rights::from_bits(0), Rights::None);
        assert_eq!(Rights::from_bits(1), Rights::Read);
        assert_eq!(Rights::from_bits(2), Rights::Write);
        assert_eq!(Rights::from_bits(3), Rights::ReadWrite);
        assert_eq!(Rights::from_bits(4), Rights::Grant);
        assert_eq!(Rights::from_bits(7), Rights::All);
    }

    /// T5：Rights::from_bits 非法值降级为 None。
    #[test]
    fn t5_rights_from_bits_invalid() {
        assert_eq!(Rights::from_bits(5), Rights::None);
        assert_eq!(Rights::from_bits(6), Rights::None);
    }

    /// T6：Rights::to_cap_rights_bits 映射正确。
    #[test]
    fn t6_rights_to_cap_rights() {
        assert_eq!(Rights::None.to_cap_rights_bits(), 0);
        assert_eq!(Rights::Read.to_cap_rights_bits(), 1);
        assert_eq!(Rights::Write.to_cap_rights_bits(), 2);
        assert_eq!(Rights::Grant.to_cap_rights_bits(), 8);
        assert_eq!(Rights::ReadWrite.to_cap_rights_bits(), 3);
        assert_eq!(Rights::All.to_cap_rights_bits(), 11);
    }

    /// T7：CompatError::to_code 值正确。
    #[test]
    fn t7_compat_error_codes() {
        assert_eq!(CompatError::UnsupportedSyscall.to_code(), 1);
        assert_eq!(CompatError::InvalidCapability.to_code(), 2);
        assert_eq!(CompatError::TranslationFailed.to_code(), 3);
        assert_eq!(CompatError::Timeout.to_code(), 4);
    }

    /// T8：MessageInfo 结构体尺寸为 16 字节（repr(C) 对齐）。
    #[test]
    fn t8_message_info_size() {
        // label(8) + caps_unwrapped(1) + extra_caps(1) + length(2) + padding(4) = 16
        assert_eq!(core::mem::size_of::<MessageInfo>(), 16);
    }
}
