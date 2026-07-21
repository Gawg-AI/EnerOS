//! 最小系统状态（D2：本地定义）.
//!
//! 蓝图使用 `SystemState` 但未定义，HMI crate 的 `SystemState` 是显示状态
//! （agent_states / storage_usage / network），与电气安全校验无关。
//! 本 crate 定义最小满足校验需求的电气状态类型。

/// 最小系统状态（D2）.
///
/// 包含电气校验所需的最小字段集合：电压 / 电流 / 频率 / SOC / 时间戳。
/// `Default` 返回典型运行点：380V / 0A / 50Hz / 50% SOC / 0ms。
#[derive(Debug, Clone)]
pub struct SystemState {
    /// 母线电压（V）。
    pub voltage_v: f64,
    /// 母线电流（A）。
    pub current_a: f64,
    /// 系统频率（Hz）。
    pub frequency_hz: f64,
    /// 储能 SOC（0.0~1.0）。
    pub soc_pct: f64,
    /// 时间戳（ms）。
    pub timestamp_ms: u64,
}

impl Default for SystemState {
    /// 典型运行点：380V / 0A / 50Hz / 0.5 SOC / 0ms（D2）。
    fn default() -> Self {
        Self {
            voltage_v: 380.0,
            current_a: 0.0,
            frequency_hz: 50.0,
            soc_pct: 0.5,
            timestamp_ms: 0,
        }
    }
}
