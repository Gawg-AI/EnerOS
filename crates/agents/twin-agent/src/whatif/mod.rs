//! What-if 分析 — Action / Scenario / Outcome / RiskLevel / ScenarioResult / WhatIfError
//! + SimModel trait + AnalyticalSimModel + WhatIfAnalyzer（v0.91.0）.
//!
//! - **D2**：`name` / `scenario` / `metric` 全部 `&'static str`（无堆分配，同 v0.90.0 D2）。
//! - **D3**：`duration_ms: u64` 替代 `Duration`（全 crate 统一 u64 ms 外部时间注入惯例）。
//! - **D4**：[`SimModel`] 不要求 Send + Sync（no_std 单线程惯例）。
//! - **D7**：[`Action`] 4 变体覆盖 [`TwinModel`] 设备/电网/市场三面（蓝图未定义 `Action`，本地定义）。
//! - **D8**：蓝图 `sim_state.apply(action)`（`TwinModel` 无此方法）→ 自由函数 [`apply_action`]
//!   （model.rs 零改动，Surgical）。
//! - **D9**：[`AnalyticalSimModel`] 简化解析模型——仅 SOC 线性推演
//!   （`soc -= power × hours / capacity`，clamp `[0,1]`），grid/market/last_update 透传。
//! - **D10**：仿真发散 → [`WhatIfError::Diverged`] → analyze 转 `Ok` 且 `risk_level = Critical`
//!   + outcomes 空；模型不可用 → [`WhatIfError::ModelUnavailable`] → analyze 透传 `Err` 拒绝分析。
//! - **D11**：[`compute_outcomes`] 固定 3 指标（`grid_active_power` / `total_device_power` /
//!   `min_soc`，空设备 min_soc=1.0 中性）；[`assess_risk`] 取最重规则。
//! - **D12**：NaN/Inf 防御（v0.88.0 C140 教训）——action/sim 中非有限功率复用
//!   [`model_forecast::sanitize`](crate::model_forecast::sanitize) 按 0.0；
//!   `battery_capacity_kwh` 非有限或 ≤ 0 → 默认 100.0。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use serde::Serialize;

use crate::model::{MarketMirror, TwinModel};
use crate::model_forecast::sanitize;

/// 假设动作（D7：覆盖 TwinModel 设备/电网/市场三面）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// 设定设备功率（kW，f64；不存在设备无副作用；非有限按 0.0，D12）.
    SetDevicePower { device_id: u64, power: f64 },
    /// 移除设备.
    RemoveDevice { device_id: u64 },
    /// 设定电网有功功率（kW）.
    SetGridPower { active_power: f32 },
    /// 设定市场电价（元/kWh）.
    SetMarketPrice { price: f32 },
}

/// What-if 场景（名称 + 动作序列 + 推演时长；D2/D3）.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// 场景名（D2：&'static str）.
    pub name: &'static str,
    /// 假设动作序列（按序应用）.
    pub actions: Vec<Action>,
    /// 推演时长（ms，D3）.
    pub duration_ms: u64,
}

/// 单条结果指标（value 为仿真终态值，baseline 为原始模型值；D2）.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Outcome {
    /// 指标名（`grid_active_power` / `total_device_power` / `min_soc`）.
    pub metric: &'static str,
    /// 仿真终态值.
    pub value: f32,
    /// 基线（原始模型）值.
    pub baseline: f32,
}

/// 风险等级（序即严重度：Low < Medium < High < Critical）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize)]
pub enum RiskLevel {
    /// 低风险（默认）.
    #[default]
    Low,
    /// 中风险（grid 功率相对波动 > 50%）.
    Medium,
    /// 高风险（min_soc < 0.2）.
    High,
    /// 严重风险（min_soc ≤ 0 或仿真发散，D10）.
    Critical,
}

/// 场景分析结果（scenario 名回显 + 3 指标 + 风险等级）.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioResult {
    /// 场景名（回显 [`Scenario::name`]）.
    pub scenario: &'static str,
    /// 结果指标序列（D11：恰好 3 条，固定顺序）.
    pub outcomes: Vec<Outcome>,
    /// 风险等级（取最重，D11）.
    pub risk_level: RiskLevel,
}

impl ScenarioResult {
    /// 序列化为 JSON 摘要（仿 v0.89.0 `TwinSnapshot::summary_json` 模式）.
    ///
    /// serde_json 对纯数据 DTO 序列化不会失败；失败时兜底返回 `"{}"`。
    pub fn summary_json(&self) -> String {
        match serde_json::to_string(self) {
            Ok(s) => s,
            Err(_) => String::from("{}"),
        }
    }
}

/// What-if 分析错误（D10：两变体，无 DDS 透传——本版本不触总线）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhatIfError {
    /// 仿真模型不可用 → analyze 拒绝分析（透传 Err）.
    ModelUnavailable,
    /// 仿真发散 → analyze 转 Ok 结果（risk_level = Critical，outcomes 空）.
    Diverged,
}

/// 应用假设动作到孪生模型（D8：自由函数，model.rs 零改动）.
///
/// - `SetDevicePower`：仅更新存在设备，非有限功率按 0.0（D12）；不存在设备无副作用。
/// - `RemoveDevice`：移除设备（重复移除无副作用）。
/// - `SetGridPower`：`grid.active_power = sanitize`，其余 grid 字段不变。
/// - `SetMarketPrice`：`None` → 新建 `MarketMirror { timestamp: 0, current_price }`；
///   `Some` → 仅覆盖 price（timestamp 保留）。
pub struct AnalyticalSimModel {
    /// 电池容量（kWh，构造时非有限或 ≤ 0 → 100.0，D12）.
    pub battery_capacity_kwh: f64,
}

impl AnalyticalSimModel {
    /// 创建解析模型：`battery_capacity_kwh` 非有限或 ≤ 0 → 100.0（D12）.
    pub fn new(battery_capacity_kwh: f64) -> Self {
        let battery_capacity_kwh = if battery_capacity_kwh.is_finite() && battery_capacity_kwh > 0.0
        {
            battery_capacity_kwh
        } else {
            100.0
        };
        Self {
            battery_capacity_kwh,
        }
    }
}

impl SimModel for AnalyticalSimModel {
    fn run(&self, mut state: TwinModel, duration_ms: u64) -> Result<TwinModel, WhatIfError> {
        let hours = duration_ms as f64 / 3_600_000.0;
        for twin in state.devices.values_mut() {
            let p = sanitize(twin.state.power as f32) as f64;
            twin.state.soc =
                (twin.state.soc - p * hours / self.battery_capacity_kwh).clamp(0.0, 1.0);
        }
        Ok(state)
    }

    fn name(&self) -> &'static str {
        "analytical"
    }
}

/// What-if 分析器（D4/D10：clone 模型 → 逐 action 应用 → 仿真推演 → 结果/风险评估）.
pub struct WhatIfAnalyzer {
    /// 仿真模型（D4：Box<dyn SimModel>，无 Send + Sync 约束）.
    pub sim_model: Box<dyn SimModel>,
}

/// 模拟/评估大实现（ARCH-G03 拆分）。
mod simulate;

pub use simulate::{apply_action, assess_risk, compute_outcomes, SimModel};

#[cfg(test)]
mod tests;
