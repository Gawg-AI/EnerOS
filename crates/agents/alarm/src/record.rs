//! 告警记录与状态.

use alloc::string::String;

use crate::level::AlarmLevel;

/// 告警唯一标识（自增分配）.
pub type AlarmId = u64;

/// 告警生命周期状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmState {
    /// 活跃（已生成未确认）.
    Active,
    /// 已确认（运维已 ACK，停止升级）.
    Acknowledged,
    /// 已清除（故障恢复，归档）.
    Cleared,
}

/// 告警记录（全生命周期可追溯）.
///
/// `raised_at_ms` 为注入的 u64 毫秒时间戳（D25）。
#[derive(Debug, Clone)]
pub struct AlarmRecord {
    /// 告警 ID.
    pub id: AlarmId,
    /// 告警级别.
    pub level: AlarmLevel,
    /// 告警源（设备/点/Agent 标识）.
    pub source: String,
    /// 告警描述.
    pub description: String,
    /// 生成时间（u64 毫秒，D25）.
    pub raised_at_ms: u64,
    /// 确认时间.
    pub acknowledged_at_ms: Option<u64>,
    /// 清除时间.
    pub cleared_at_ms: Option<u64>,
    /// 升级前原始级别（`None` 表示未升级）.
    pub escalated_from: Option<AlarmLevel>,
    /// 当前状态.
    pub state: AlarmState,
}

impl AlarmRecord {
    /// 构造新告警（`state = Active`，仅 `raised_at_ms` 有值）.
    pub fn new(
        id: AlarmId,
        level: AlarmLevel,
        source: &str,
        description: &str,
        now_ms: u64,
    ) -> Self {
        Self {
            id,
            level,
            source: String::from(source),
            description: String::from(description),
            raised_at_ms: now_ms,
            acknowledged_at_ms: None,
            cleared_at_ms: None,
            escalated_from: None,
            state: AlarmState::Active,
        }
    }

    /// 是否处于 Active 状态.
    pub fn is_active(&self) -> bool {
        self.state == AlarmState::Active
    }

    /// 是否已升级.
    pub fn is_escalated(&self) -> bool {
        self.escalated_from.is_some()
    }
}
