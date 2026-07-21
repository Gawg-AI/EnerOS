//! v0.78.0 消息编解码元数据 tag（不实现实际编解码，D6）。

use core::fmt;

/// 消息序列化格式 tag（仅元数据，不实现编解码，D6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecKind {
    /// DDS 标准 CDR.
    Cdr,
    /// Rust-only bincode.
    Bincode,
    /// 调试用.
    Json,
}

impl CodecKind {
    /// 编码为 `u8` tag.
    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Cdr => 0,
            Self::Bincode => 1,
            Self::Json => 2,
        }
    }

    /// 从 `u8` tag 解码，非法值返回 `None`.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Cdr),
            1 => Some(Self::Bincode),
            2 => Some(Self::Json),
            _ => None,
        }
    }
}

/// 编解码错误（保留供未来实际编解码使用，本版本不触发）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// 不支持的编解码格式.
    Unsupported(CodecKind),
    /// 数据非法.
    InvalidData,
    /// 缓冲区过短.
    BufferTooShort,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(c) => write!(f, "unsupported codec: {:?}", c),
            Self::InvalidData => write!(f, "invalid data"),
            Self::BufferTooShort => write!(f, "buffer too short"),
        }
    }
}

impl core::error::Error for CodecError {}
