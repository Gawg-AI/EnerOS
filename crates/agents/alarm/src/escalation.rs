//! 告警升级策略（D22 单级超时升级）.

use crate::level::AlarmLevel;
use crate::record::AlarmRecord;

/// 升级策略（`from_level` → `to_level`，超时触发，D22 简化为单级升级）.
#[derive(Debug, Clone)]
pub struct EscalationPolicy {
    /// 源级别.
    pub from_level: AlarmLevel,
    /// 目标级别.
    pub to_level: AlarmLevel,
    /// 超时阈值（毫秒）.
    pub timeout_ms: u64,
}

impl EscalationPolicy {
    /// 构造升级策略.
    pub fn new(from: AlarmLevel, to: AlarmLevel, timeout_ms: u64) -> Self {
        Self {
            from_level: from,
            to_level: to,
            timeout_ms,
        }
    }

    /// 检查告警是否应被升级.
    ///
    /// - 告警级别 != `from_level` → `None`
    /// - 告警状态 != Active → `None`（仅 Active 升级，Acknowledged 不升级）
    /// - 已 ACK（`acknowledged_at_ms.is_some()`）→ `None`
    /// - 超时（`now_ms >= raised_at_ms + timeout_ms`）→ `Some(to_level)`
    pub fn check_escalation(&self, record: &AlarmRecord, now_ms: u64) -> Option<AlarmLevel> {
        if record.level != self.from_level {
            return None;
        }
        if !record.is_active() {
            return None;
        }
        if record.acknowledged_at_ms.is_some() {
            return None;
        }
        if now_ms >= record.raised_at_ms.saturating_add(self.timeout_ms) {
            return Some(self.to_level);
        }
        None
    }
}
