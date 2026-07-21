//! GGUF 元数据值类型与值（D6）.
//!
//! `GgufValueType` 枚举 GGUF 规范的 13 种值类型标签；`GgufValue` 为对应的
//! 动态值（使用 `alloc::string::String` / `alloc::vec::Vec`，no_std 兼容）。

use alloc::string::String;
use alloc::vec::Vec;

/// GGUF 元数据值类型标签（与 GGUF 规范的 value_type 值一一对应）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GgufValueType {
    /// 无符号 8-bit 整数.
    Uint8 = 0,
    /// 有符号 8-bit 整数.
    Int8 = 1,
    /// 无符号 16-bit 整数.
    Uint16 = 2,
    /// 有符号 16-bit 整数.
    Int16 = 3,
    /// 无符号 32-bit 整数.
    Uint32 = 4,
    /// 有符号 32-bit 整数.
    Int32 = 5,
    /// 32-bit 浮点.
    Float32 = 6,
    /// 布尔.
    Bool = 7,
    /// UTF-8 字符串.
    String = 8,
    /// 同类型数组.
    Array = 9,
    /// 无符号 64-bit 整数.
    Uint64 = 10,
    /// 有符号 64-bit 整数.
    Int64 = 11,
    /// 64-bit 浮点.
    Float64 = 12,
}

impl GgufValueType {
    /// 从 u32 原始值构造 `GgufValueType`，非法值返回 `None`.
    pub fn from_u32(value: u32) -> Option<GgufValueType> {
        match value {
            0 => Some(Self::Uint8),
            1 => Some(Self::Int8),
            2 => Some(Self::Uint16),
            3 => Some(Self::Int16),
            4 => Some(Self::Uint32),
            5 => Some(Self::Int32),
            6 => Some(Self::Float32),
            7 => Some(Self::Bool),
            8 => Some(Self::String),
            9 => Some(Self::Array),
            10 => Some(Self::Uint64),
            11 => Some(Self::Int64),
            12 => Some(Self::Float64),
            _ => None,
        }
    }
}

/// GGUF 元数据动态值.
#[derive(Debug, Clone)]
pub enum GgufValue {
    /// 无符号 8-bit 整数.
    Uint8(u8),
    /// 有符号 8-bit 整数.
    Int8(i8),
    /// 无符号 16-bit 整数.
    Uint16(u16),
    /// 有符号 16-bit 整数.
    Int16(i16),
    /// 无符号 32-bit 整数.
    Uint32(u32),
    /// 有符号 32-bit 整数.
    Int32(i32),
    /// 32-bit 浮点.
    Float32(f32),
    /// 布尔.
    Bool(bool),
    /// UTF-8 字符串.
    String(String),
    /// 同类型数组.
    Array(Vec<GgufValue>),
    /// 无符号 64-bit 整数.
    Uint64(u64),
    /// 有符号 64-bit 整数.
    Int64(i64),
    /// 64-bit 浮点.
    Float64(f64),
}
