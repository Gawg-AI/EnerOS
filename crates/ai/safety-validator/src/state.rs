//! 最小系统状态（D2：本地定义）.
//!
//! 蓝图使用 `SystemState` 但未定义，HMI crate 的 `SystemState` 是显示状态
//! （agent_states / storage_usage / network），与电气安全校验无关。
//! 本 crate 定义最小满足校验需求的电气状态类型。
//!
//! # 蓝图 40 WP-40.1（可信采集边界）
//!
//! `SystemState` **不再实现**可用于控制的 `Default`——旧实现返回"典型运行点"
//! （380V/0A/50Hz/50%SOC），观测缺失时被当作真实健康状态继续决策（P0-09）。
//! 控制输入必须来自 [`crate::trusted`]（`TrustedTelemetry`）的评估器；测试/
//! 演示构造一律走 [`TestSystemStateBuilder`]，且 `unknown()` 形态为 NaN
//! （fail-closed：任何误用都会在安全校验/求解中失败，不产生控制）。

/// 最小系统状态（D2）.
///
/// 包含电气校验所需的最小字段集合：电压 / 电流 / 频率 / SOC / 时间戳。
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

impl SystemState {
    /// 显式构造（唯一公开构造路径——生产值只能来自可信遥测评估器或
    /// 显式测试/演示装配，不允许无参默认值进入控制链）。
    pub const fn new(
        voltage_v: f64,
        current_a: f64,
        frequency_hz: f64,
        soc_pct: f64,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            voltage_v,
            current_a,
            frequency_hz,
            soc_pct,
            timestamp_ms,
        }
    }

    /// 无观测形态：所有电气值为 NaN、时间戳 0。
    ///
    /// fail-closed：任何把该形态当作真实状态的误用，都会在安全校验/求解
    /// 中传播为拒绝（NaN 比较不为真），不产生控制——不是"健康默认值"。
    /// 仅用于"当前无观测"的显式占位（如解析器构造期初始状态，决策前必须
    /// 经 `set_state` 覆写为受信任状态）。
    pub const fn unknown() -> Self {
        Self {
            voltage_v: f64::NAN,
            current_a: f64::NAN,
            frequency_hz: f64::NAN,
            soc_pct: f64::NAN,
            timestamp_ms: 0,
        }
    }
}

/// 测试/演示专用构造器（蓝图 40 WP-40.1：测试构造放
/// `TestSystemStateBuilder`；`unknown()` 为 NaN fail-closed 形态）。
///
/// **不得**用于生产控制输入——生产路径只经
/// `TelemetryTrustEvaluator::evaluate` 产出 `SystemState`。
#[derive(Debug, Clone)]
pub struct TestSystemStateBuilder {
    state: SystemState,
}

impl TestSystemStateBuilder {
    /// 未知状态：所有电气值为 NaN、时间戳 0——误入控制链会 fail-closed
    /// （NaN 经校验/求解传播为拒绝），不构成"健康默认值"。
    pub fn unknown() -> Self {
        Self {
            state: SystemState::unknown(),
        }
    }

    /// 显式构造（测试固定电气工况）。
    pub fn new(
        voltage_v: f64,
        current_a: f64,
        frequency_hz: f64,
        soc_pct: f64,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            state: SystemState::new(voltage_v, current_a, frequency_hz, soc_pct, timestamp_ms),
        }
    }

    /// 覆盖母线电压（V）。
    pub fn with_voltage_v(mut self, v: f64) -> Self {
        self.state.voltage_v = v;
        self
    }

    /// 覆盖母线电流（A）。
    pub fn with_current_a(mut self, v: f64) -> Self {
        self.state.current_a = v;
        self
    }

    /// 覆盖系统频率（Hz）。
    pub fn with_frequency_hz(mut self, v: f64) -> Self {
        self.state.frequency_hz = v;
        self
    }

    /// 覆盖储能 SOC（0.0~1.0）。
    pub fn with_soc_pct(mut self, v: f64) -> Self {
        self.state.soc_pct = v;
        self
    }

    /// 覆盖时间戳（ms）。
    pub fn with_timestamp_ms(mut self, v: u64) -> Self {
        self.state.timestamp_ms = v;
        self
    }

    /// 产出状态。
    pub fn build(self) -> SystemState {
        self.state
    }
}
