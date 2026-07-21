//! DegradeConfig — 端到端降级流程配置.
//!
//! [`DegradeConfig`] 集中管理心跳周期、超时阈值、恢复过渡时长、看门狗硬复位超时、
//! 以及功率设定值/命令点的 PointId（D9：由调用方配置，不硬编码常量）。

use eneros_upa_model::PointId;

/// 端到端降级流程配置.
///
/// 所有时间字段使用 `u64` 毫秒（D5：拒绝 `Duration` 类型）。
#[derive(Debug, Clone, Copy)]
pub struct DegradeConfig {
    /// 心跳周期（毫秒，默认 1000 = 1s）。
    pub heartbeat_period_ms: u64,
    /// 心跳超时阈值（连续超时次数，默认 3 = 3s 后判定 Dead）。
    pub heartbeat_timeout_count: u8,
    /// 恢复过渡时长（毫秒，默认 30000 = 30s 线性插值）。
    pub recovery_transition_ms: u64,
    /// 看门狗硬复位超时（毫秒，默认 10000 = 10s）。
    pub watchdog_hard_timeout_ms: u32,
    /// 功率设定值点 ID（D9：由调用方配置）。
    pub power_setpoint_point: PointId,
    /// 功率命令点 ID（D9：由调用方配置）。
    pub power_cmd_point: PointId,
}

impl Default for DegradeConfig {
    fn default() -> Self {
        Self {
            heartbeat_period_ms: 1000,
            heartbeat_timeout_count: 3,
            recovery_transition_ms: 30000,
            watchdog_hard_timeout_ms: 10000,
            power_setpoint_point: 0,
            power_cmd_point: 0,
        }
    }
}
