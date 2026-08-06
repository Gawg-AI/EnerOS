//! IEC 104 从站协议错误类型.

use core::fmt;

/// IEC 104 协议错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Iec104Error {
    /// 编码失败
    Encode,
    /// 解码失败
    Decode,
    /// 传输层错误
    Transport,
    /// 序列号错误
    Sequence,
    /// 非法帧（起始字节/长度/格式不符）
    InvalidFrame,
    /// 超时
    Timeout,
    /// 连接已关闭
    ConnectionClosed,
    /// 点不存在（IOA 未注册）
    PointNotFound,
    /// 非法类型标识
    InvalidTypeId,
    /// 序号不连续/重放（v0.219.0）
    SeqMismatch {
        /// 期望序号
        expected: u16,
        /// 实际序号
        actual: u16,
    },
    /// k 窗口打满（v0.219.0）
    WindowFull,
}

impl fmt::Display for Iec104Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode => write!(f, "iec104 encode error"),
            Self::Decode => write!(f, "iec104 decode error"),
            Self::Transport => write!(f, "iec104 transport error"),
            Self::Sequence => write!(f, "iec104 sequence error"),
            Self::InvalidFrame => write!(f, "iec104 invalid frame"),
            Self::Timeout => write!(f, "iec104 timeout"),
            Self::ConnectionClosed => write!(f, "iec104 connection closed"),
            Self::PointNotFound => write!(f, "iec104 point not found"),
            Self::InvalidTypeId => write!(f, "iec104 invalid type id"),
            Self::SeqMismatch { expected, actual } => write!(
                f,
                "iec104 sequence mismatch: expected {expected}, got {actual}"
            ),
            Self::WindowFull => write!(f, "iec104 k-window full"),
        }
    }
}

impl core::error::Error for Iec104Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_all_variants() {
        assert_eq!(format!("{}", Iec104Error::Encode), "iec104 encode error");
        assert_eq!(format!("{}", Iec104Error::Decode), "iec104 decode error");
        assert_eq!(
            format!("{}", Iec104Error::Transport),
            "iec104 transport error"
        );
        assert_eq!(
            format!("{}", Iec104Error::Sequence),
            "iec104 sequence error"
        );
        assert_eq!(
            format!("{}", Iec104Error::InvalidFrame),
            "iec104 invalid frame"
        );
        assert_eq!(format!("{}", Iec104Error::Timeout), "iec104 timeout");
        assert_eq!(
            format!("{}", Iec104Error::ConnectionClosed),
            "iec104 connection closed"
        );
        assert_eq!(
            format!("{}", Iec104Error::PointNotFound),
            "iec104 point not found"
        );
        assert_eq!(
            format!("{}", Iec104Error::InvalidTypeId),
            "iec104 invalid type id"
        );
    }

    #[test]
    fn test_eq() {
        assert_eq!(Iec104Error::Timeout, Iec104Error::Timeout);
        assert_ne!(Iec104Error::Timeout, Iec104Error::Encode);
    }

    #[test]
    fn test_clone() {
        let e = Iec104Error::Transport;
        let cloned = e.clone();
        assert_eq!(e, cloned);
    }
}
