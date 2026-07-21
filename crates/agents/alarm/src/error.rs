//! 告警管理错误类型.

/// 告警管理错误.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlarmError {
    /// 告警未找到（ID 不存在于活跃表）.
    NotFound,
    /// 告警已被确认（非 Active 状态再次 ACK）.
    AlreadyAcknowledged,
    /// 告警已被清除.
    AlreadyCleared,
    /// 告警已升级（重复升级）.
    AlreadyEscalated,
    /// 告警级别非法（无匹配升级策略）.
    InvalidLevel,
    /// 告警被抑制（同源窗口内超阈值）.
    Suppressed,
}
