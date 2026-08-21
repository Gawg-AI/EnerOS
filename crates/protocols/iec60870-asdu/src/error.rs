//! ASDU 编解码错误（蓝图 48 WP-48.0 抽取）。
//!
//! 变体集 = 原 iec104-slave asdu.rs 实际使用的全部错误（Decode ×20、
//! InvalidTypeId ×1）；slave 以 `From<AsduError> for Iec104Error`
//! 无损映射（行为零变更）。

use core::fmt;

/// ASDU 编解码错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsduError {
    /// 解码失败（长度不足/格式不符——原 Iec104Error::Decode 同义）。
    Decode,
    /// 非法类型标识（TypeId::from_u8 未命中——原 Iec104Error::InvalidTypeId 同义）。
    InvalidTypeId,
}

impl fmt::Display for AsduError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode => write!(f, "asdu decode error"),
            Self::InvalidTypeId => write!(f, "asdu invalid type id"),
        }
    }
}

impl core::error::Error for AsduError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_all_variants() {
        assert_eq!(format!("{}", AsduError::Decode), "asdu decode error");
        assert_eq!(
            format!("{}", AsduError::InvalidTypeId),
            "asdu invalid type id"
        );
    }

    #[test]
    fn eq_and_clone() {
        assert_eq!(AsduError::Decode, AsduError::Decode);
        assert_ne!(AsduError::Decode, AsduError::InvalidTypeId);
        let c = AsduError::InvalidTypeId;
        assert_eq!(c.clone(), AsduError::InvalidTypeId);
    }
}
