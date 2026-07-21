//! 实时状态（D4：包装 v0.67.0 SystemState + current_price + load_demand）.

use alloc::vec::Vec;

use eneros_safety_validator::state::SystemState;

/// 实时状态（D4：包装 v0.67.0 SystemState + current_price + load_demand）.
///
/// 蓝图引用 `current_price` / `load_demand` 但 v0.67.0 `SystemState` 仅含电气字段
/// （voltage_v / current_a / frequency_hz / soc_pct / timestamp_ms），无电价/负荷。
/// 本类型包装 `SystemState` 并补齐电价与负荷需求字段。
#[derive(Debug, Clone)]
pub struct RealtimeState {
    /// 系统电气状态（v0.67.0：voltage_v/current_a/frequency_hz/soc_pct/timestamp_ms）.
    pub system: SystemState,
    /// 当前电价（元/kWh）.
    pub current_price: f64,
    /// 各时段负荷需求（kW，可选）.
    pub load_demand: Option<Vec<f64>>,
}

impl Default for RealtimeState {
    fn default() -> Self {
        Self {
            system: SystemState::default(),
            current_price: 0.5,
            load_demand: None,
        }
    }
}
