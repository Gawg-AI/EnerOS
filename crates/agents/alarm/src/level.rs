//! 告警级别（4 级，由低到高）.

/// 告警级别.
///
/// 派生 `Ord` 以便级别比较与排序：`Info < Warning < Critical < Emergency`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlarmLevel {
    /// 信息（最低级）.
    Info = 0,
    /// 警告.
    Warning = 1,
    /// 严重.
    Critical = 2,
    /// 紧急（最高级）.
    Emergency = 3,
}

impl AlarmLevel {
    /// 转为 `u8`.
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    /// 从 `u8` 转换.
    pub fn from_u8(v: u8) -> Option<AlarmLevel> {
        match v {
            0 => Some(AlarmLevel::Info),
            1 => Some(AlarmLevel::Warning),
            2 => Some(AlarmLevel::Critical),
            3 => Some(AlarmLevel::Emergency),
            _ => None,
        }
    }
}
