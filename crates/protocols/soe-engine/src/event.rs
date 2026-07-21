//! SOE 事件数据模型.

use alloc::string::String;

use eneros_telemetry_model::QualityFlag;
use eneros_upa_model::{DeviceId, PointId, PointValue};

/// SOE 事件类型（11 变体）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoeEventType {
    /// 遥信变位（开关状态变化）.
    DigitalChange,
    /// 遥测越限（超过上限/下限）.
    AnalogOverLimit,
    /// 遥测恢复（越限恢复）.
    AnalogRecovery,
    /// 品质变化（Good→Invalid 等）.
    QualityChange,
    /// 遥控执行.
    ControlExecute,
    /// 遥控完成.
    ControlDone,
    /// 遥控失败.
    ControlFailed,
    /// 人工置数.
    ManualSet,
    /// 设备通信中断.
    CommLost,
    /// 设备通信恢复.
    CommRestore,
    /// 自定义事件.
    Custom(u16),
}

/// 事件优先级（值越小优先级越高）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventPriority {
    /// 紧急（保护动作/严重故障）.
    Critical = 0,
    /// 高（告警/越限）.
    High = 1,
    /// 中（状态变化）.
    Medium = 2,
    /// 低（品质变化/信息）.
    Low = 3,
}

/// SOE 事件.
///
/// `timestamp_ms`/`system_time_ms` 均为 `u64` 毫秒（D1/D9）。
/// `event_id` 由引擎分配，构造时置 0 占位。
#[derive(Debug, Clone)]
pub struct SoeEvent {
    /// 事件 ID（全局唯一，由引擎分配）.
    pub event_id: u64,
    /// 事件时标（ms 级精度，单调时钟，D1）.
    pub timestamp_ms: u64,
    /// 系统时间（用于显示与同步，D9）.
    pub system_time_ms: u64,
    /// 关联点 ID.
    pub point_id: PointId,
    /// 关联设备 ID.
    pub device_id: DeviceId,
    /// 事件类型.
    pub event_type: SoeEventType,
    /// 事件前值.
    pub old_value: PointValue,
    /// 事件后值.
    pub new_value: PointValue,
    /// 事件品质.
    pub quality: QualityFlag,
    /// 事件优先级.
    pub priority: EventPriority,
    /// 事件描述.
    pub description: String,
}

impl SoeEvent {
    /// 构造事件（`event_id` 置 0 占位，由引擎分配）.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        point_id: PointId,
        device_id: DeviceId,
        event_type: SoeEventType,
        old_value: PointValue,
        new_value: PointValue,
        quality: QualityFlag,
        priority: EventPriority,
        description: &str,
        now_ms: u64,
    ) -> Self {
        Self {
            event_id: 0,
            timestamp_ms: now_ms,
            system_time_ms: now_ms,
            point_id,
            device_id,
            event_type,
            old_value,
            new_value,
            quality,
            priority,
            description: String::from(description),
        }
    }

    /// 是否为紧急事件.
    pub fn is_critical(&self) -> bool {
        self.priority == EventPriority::Critical
    }
}
