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
//!   （`soc -= power × hours / capacity`，clamp [0,1]），grid/market/last_update 透传。
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
pub fn apply_action(state: &mut TwinModel, action: &Action) {
    match *action {
        Action::SetDevicePower { device_id, power } => {
            if let Some(t) = state.devices.get_mut(&device_id) {
                t.state.power = if power.is_finite() { power } else { 0.0 };
            }
        }
        Action::RemoveDevice { device_id } => {
            state.devices.remove(&device_id);
        }
        Action::SetGridPower { active_power } => {
            state.grid.active_power = sanitize(active_power);
        }
        Action::SetMarketPrice { price } => {
            let price = sanitize(price);
            match state.market {
                Some(ref mut m) => m.current_price = price,
                None => {
                    state.market = Some(MarketMirror {
                        timestamp: 0,
                        current_price: price,
                    });
                }
            }
        }
    }
}

/// 计算结果指标（D11：恰好 3 条，固定顺序）.
///
/// 1. `grid_active_power`：电网有功功率终态 vs 基线。
/// 2. `total_device_power`：设备功率总和（f64 求和后 `as f32`）。
/// 3. `min_soc`：设备 SOC 最小值；空设备 → 1.0（中性，value 与 baseline 均 1.0）。
pub fn compute_outcomes(baseline: &TwinModel, final_state: &TwinModel) -> Vec<Outcome> {
    let total_power =
        |m: &TwinModel| -> f32 { m.devices.values().map(|t| t.state.power).sum::<f64>() as f32 };
    let min_soc = |m: &TwinModel| -> f32 {
        m.devices
            .values()
            .map(|t| t.state.soc)
            .fold(1.0_f64, f64::min) as f32
    };
    vec![
        Outcome {
            metric: "grid_active_power",
            value: final_state.grid.active_power,
            baseline: baseline.grid.active_power,
        },
        Outcome {
            metric: "total_device_power",
            value: total_power(final_state),
            baseline: total_power(baseline),
        },
        Outcome {
            metric: "min_soc",
            value: min_soc(final_state),
            baseline: min_soc(baseline),
        },
    ]
}

/// 风险评估（D11：取最重规则；空 outcomes → Low）.
///
/// - min_soc ≤ 0.0 → Critical
/// - min_soc < 0.2 → High
/// - grid_active_power 相对波动 > 50% → Medium
///   （|baseline| < 1e-6 时：|value − baseline| < 1e-6 → 波动 0，否则视为无穷大波动）
/// - 否则 → Low
pub fn assess_risk(outcomes: &[Outcome]) -> RiskLevel {
    if outcomes.is_empty() {
        return RiskLevel::Low;
    }
    let mut min_soc: Option<f32> = None;
    let mut grid: Option<(f32, f32)> = None;
    for o in outcomes {
        match o.metric {
            "min_soc" => min_soc = Some(o.value),
            "grid_active_power" => grid = Some((o.value, o.baseline)),
            _ => {}
        }
    }
    if let Some(s) = min_soc {
        if s <= 0.0 {
            return RiskLevel::Critical;
        }
        if s < 0.2 {
            return RiskLevel::High;
        }
    }
    if let Some((value, base)) = grid {
        let dev = if base.abs() < 1e-6 {
            if (value - base).abs() < 1e-6 {
                0.0
            } else {
                f32::INFINITY
            }
        } else {
            (value - base).abs() / base.abs()
        };
        if dev > 0.5 {
            return RiskLevel::Medium;
        }
    }
    RiskLevel::Low
}

/// 仿真模型抽象（D4：无 Send + Sync 约束）.
pub trait SimModel {
    /// 对应用动作后的孪生状态做 `duration_ms` 推演，返回终态.
    fn run(&self, state: TwinModel, duration_ms: u64) -> Result<TwinModel, WhatIfError>;
    /// 模型名（如 `"analytical"`）.
    fn name(&self) -> &'static str;
}

/// 简化解析仿真模型（D9：仅 SOC 线性推演，grid/market/last_update 透传）.
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

impl WhatIfAnalyzer {
    /// 分析场景（只读：输入 `model` 不被修改，分析在 clone 上进行）.
    ///
    /// 流程（D10）：clone → 逐 action [`apply_action`] → `sim_model.run`
    /// → 发散转 `Ok`（Critical + 空 outcomes）→ 不可用透传 `Err`
    /// → [`compute_outcomes`] → [`assess_risk`] → 返回（scenario 名回显）。
    pub fn analyze(
        &self,
        scenario: &Scenario,
        model: &TwinModel,
    ) -> Result<ScenarioResult, WhatIfError> {
        let mut sim = model.clone();
        for a in &scenario.actions {
            apply_action(&mut sim, a);
        }
        let final_state = match self.sim_model.run(sim, scenario.duration_ms) {
            Ok(s) => s,
            Err(WhatIfError::Diverged) => {
                return Ok(ScenarioResult {
                    scenario: scenario.name,
                    outcomes: Vec::new(),
                    risk_level: RiskLevel::Critical,
                });
            }
            Err(e) => return Err(e),
        };
        let outcomes = compute_outcomes(model, &final_state);
        let risk_level = assess_risk(&outcomes);
        Ok(ScenarioResult {
            scenario: scenario.name,
            outcomes,
            risk_level,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceTwin;

    // ===== 测试辅助 =====

    /// 构造含单设备的孪生模型（其余字段默认）.
    fn model_with_device(id: u64, soc: f64, power: f64) -> TwinModel {
        let mut m = TwinModel::default();
        let mut t = DeviceTwin {
            device_id: id,
            ..DeviceTwin::default()
        };
        t.state.soc = soc;
        t.state.power = power;
        m.devices.insert(id, t);
        m
    }

    /// 恒定发散的仿真模型（T34 故障注入）.
    struct DivergingSimModel;

    impl SimModel for DivergingSimModel {
        fn run(&self, _state: TwinModel, _duration_ms: u64) -> Result<TwinModel, WhatIfError> {
            Err(WhatIfError::Diverged)
        }

        fn name(&self) -> &'static str {
            "diverging"
        }
    }

    /// 恒定不可用的仿真模型（T35 故障注入）.
    struct UnavailableSimModel;

    impl SimModel for UnavailableSimModel {
        fn run(&self, _state: TwinModel, _duration_ms: u64) -> Result<TwinModel, WhatIfError> {
            Err(WhatIfError::ModelUnavailable)
        }

        fn name(&self) -> &'static str {
            "unavailable"
        }
    }

    // ===== T1: Action 4 变体构造 + Copy + PartialEq 相等/不等 =====
    #[test]
    fn t1_action_variants_copy_eq() {
        let a1 = Action::SetDevicePower {
            device_id: 1,
            power: 2.5,
        };
        let a2 = Action::RemoveDevice { device_id: 1 };
        let a3 = Action::SetGridPower { active_power: 8.0 };
        let a4 = Action::SetMarketPrice { price: 0.65 };
        let a1c = a1; // Copy
        assert_eq!(a1, a1c);
        assert_eq!(
            a1,
            Action::SetDevicePower {
                device_id: 1,
                power: 2.5
            }
        );
        assert_eq!(a2, Action::RemoveDevice { device_id: 1 });
        assert_eq!(a3, Action::SetGridPower { active_power: 8.0 });
        assert_eq!(a4, Action::SetMarketPrice { price: 0.65 });
        assert_ne!(a1, a2);
        assert_ne!(a3, a4);
        assert_ne!(
            a1,
            Action::SetDevicePower {
                device_id: 2,
                power: 2.5
            }
        );
    }

    // ===== T2: Scenario 构造 + Clone 回显 =====
    #[test]
    fn t2_scenario_clone_echo() {
        let s = Scenario {
            name: "heavy-discharge",
            actions: vec![
                Action::SetDevicePower {
                    device_id: 1,
                    power: 50.0,
                },
                Action::SetGridPower { active_power: 16.0 },
            ],
            duration_ms: 3_600_000,
        };
        let s2 = s.clone();
        assert_eq!(s2.name, "heavy-discharge");
        assert_eq!(s2.actions.len(), 2);
        assert_eq!(s2.actions, s.actions);
        assert_eq!(s2.duration_ms, 3_600_000);
    }

    // ===== T3: Outcome 3 字段 + Copy + 相等判定 =====
    #[test]
    fn t3_outcome_fields_copy_eq() {
        let o = Outcome {
            metric: "min_soc",
            value: 0.7,
            baseline: 0.8,
        };
        let oc = o; // Copy
        assert_eq!(o, oc);
        assert_eq!(o.metric, "min_soc");
        assert!((o.value - 0.7).abs() < 1e-6);
        assert!((o.baseline - 0.8).abs() < 1e-6);
        assert_ne!(
            o,
            Outcome {
                metric: "min_soc",
                value: 0.6,
                baseline: 0.8
            }
        );
    }

    // ===== T4: RiskLevel 序 + Default == Low + Copy =====
    #[test]
    fn t4_risk_level_ordering_default() {
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
        assert_eq!(RiskLevel::default(), RiskLevel::Low);
        let r = RiskLevel::High;
        let rc = r; // Copy
        assert_eq!(r, rc);
    }

    // ===== T5: ScenarioResult 构造 + Clone + risk_level 回显 =====
    #[test]
    fn t5_scenario_result_clone() {
        let r = ScenarioResult {
            scenario: "stable",
            outcomes: vec![Outcome {
                metric: "min_soc",
                value: 0.9,
                baseline: 0.9,
            }],
            risk_level: RiskLevel::Low,
        };
        let r2 = r.clone();
        assert_eq!(r2.scenario, "stable");
        assert_eq!(r2.outcomes, r.outcomes);
        assert_eq!(r2.risk_level, RiskLevel::Low);
    }

    // ===== T6: summary_json 可解析 + 含 scenario/risk_level/outcomes 数组 =====
    #[test]
    fn t6_summary_json_parseable() {
        let r = ScenarioResult {
            scenario: "grid-shift",
            outcomes: vec![
                Outcome {
                    metric: "grid_active_power",
                    value: 16.0,
                    baseline: 10.0,
                },
                Outcome {
                    metric: "min_soc",
                    value: 0.9,
                    baseline: 0.9,
                },
            ],
            risk_level: RiskLevel::Medium,
        };
        let s = r.summary_json();
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert_eq!(v["scenario"], "grid-shift");
        assert_eq!(v["risk_level"], "Medium");
        let arr = v["outcomes"].as_array().expect("outcomes array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["metric"], "grid_active_power");
    }

    // ===== T7: WhatIfError 两变体 Debug + PartialEq 不等 =====
    #[test]
    fn t7_whatif_error_variants() {
        let e1 = WhatIfError::ModelUnavailable;
        let e2 = WhatIfError::Diverged;
        assert_ne!(e1, e2);
        assert_eq!(e1, WhatIfError::ModelUnavailable);
        assert_eq!(e2, WhatIfError::Diverged);
        let dbg = alloc::format!("{:?}", e1);
        assert!(dbg.contains("ModelUnavailable"));
    }

    // ===== T8: apply_action SetDevicePower 存在设备更新 =====
    #[test]
    fn t8_apply_set_device_power_existing() {
        let mut m = model_with_device(1, 0.8, 1.0);
        apply_action(
            &mut m,
            &Action::SetDevicePower {
                device_id: 1,
                power: 2.5,
            },
        );
        let t = m.devices.get(&1).expect("device exists");
        assert!((t.state.power - 2.5).abs() < 1e-12);
        assert!((t.state.soc - 0.8).abs() < 1e-12);
    }

    // ===== T9: apply_action SetDevicePower 不存在设备无副作用 =====
    #[test]
    fn t9_apply_set_device_power_missing_noop() {
        let mut m = model_with_device(1, 0.8, 1.0);
        apply_action(
            &mut m,
            &Action::SetDevicePower {
                device_id: 99,
                power: 5.0,
            },
        );
        assert_eq!(m.devices.len(), 1);
        let t = m.devices.get(&1).expect("device exists");
        assert!((t.state.power - 1.0).abs() < 1e-12);
        assert!((t.state.soc - 0.8).abs() < 1e-12);
    }

    // ===== T10: apply_action SetDevicePower NaN / +Inf → 0.0（D12）=====
    #[test]
    fn t10_apply_set_device_power_non_finite() {
        let mut m = model_with_device(1, 0.8, 1.0);
        apply_action(
            &mut m,
            &Action::SetDevicePower {
                device_id: 1,
                power: f64::NAN,
            },
        );
        assert!(m.devices.get(&1).expect("d").state.power.abs() < 1e-12);
        apply_action(
            &mut m,
            &Action::SetDevicePower {
                device_id: 1,
                power: f64::INFINITY,
            },
        );
        assert!(m.devices.get(&1).expect("d").state.power.abs() < 1e-12);
    }

    // ===== T11: apply_action RemoveDevice 移除 + 重复移除不 panic =====
    #[test]
    fn t11_apply_remove_device() {
        let mut m = model_with_device(1, 0.8, 1.0);
        apply_action(&mut m, &Action::RemoveDevice { device_id: 1 });
        assert!(!m.devices.contains_key(&1));
        assert_eq!(m.devices.len(), 0);
        // 重复移除无副作用
        apply_action(&mut m, &Action::RemoveDevice { device_id: 1 });
        assert_eq!(m.devices.len(), 0);
    }

    // ===== T12: apply_action SetGridPower 更新 + 其余字段不变 =====
    #[test]
    fn t12_apply_set_grid_power() {
        let mut m = TwinModel::default();
        m.grid.frequency = 50.0;
        m.grid.active_power = 10.0;
        m.grid.timestamp = 777;
        apply_action(&mut m, &Action::SetGridPower { active_power: 8.0 });
        assert!((m.grid.active_power - 8.0).abs() < 1e-6);
        assert!((m.grid.frequency - 50.0).abs() < 1e-6);
        assert_eq!(m.grid.timestamp, 777);
    }

    // ===== T13: apply_action SetGridPower NaN → 0.0 =====
    #[test]
    fn t13_apply_set_grid_power_nan() {
        let mut m = TwinModel::default();
        m.grid.active_power = 10.0;
        apply_action(
            &mut m,
            &Action::SetGridPower {
                active_power: f32::NAN,
            },
        );
        assert!(m.grid.active_power.abs() < 1e-6);
    }

    // ===== T14: apply_action SetMarketPrice None→Some / 已有→覆盖 =====
    #[test]
    fn t14_apply_set_market_price() {
        let mut m = TwinModel::default();
        assert!(m.market.is_none());
        apply_action(&mut m, &Action::SetMarketPrice { price: 0.65 });
        let mk = m.market.expect("market created");
        assert!((mk.current_price - 0.65).abs() < 1e-6);
        assert_eq!(mk.timestamp, 0);
        // 已有 → 覆盖 price，timestamp 保留
        m.market = Some(MarketMirror {
            timestamp: 7,
            current_price: 0.65,
        });
        apply_action(&mut m, &Action::SetMarketPrice { price: 0.9 });
        let mk = m.market.expect("market exists");
        assert!((mk.current_price - 0.9).abs() < 1e-6);
        assert_eq!(mk.timestamp, 7);
    }

    // ===== T15: AnalyticalSimModel::new 钳制（D12）=====
    #[test]
    fn t15_analytical_new_clamp() {
        assert!((AnalyticalSimModel::new(f64::NAN).battery_capacity_kwh - 100.0).abs() < 1e-12);
        assert!((AnalyticalSimModel::new(0.0).battery_capacity_kwh - 100.0).abs() < 1e-12);
        assert!((AnalyticalSimModel::new(-5.0).battery_capacity_kwh - 100.0).abs() < 1e-12);
        assert!(
            (AnalyticalSimModel::new(f64::INFINITY).battery_capacity_kwh - 100.0).abs() < 1e-12
        );
        assert!((AnalyticalSimModel::new(50.0).battery_capacity_kwh - 50.0).abs() < 1e-12);
    }

    // ===== T16: SOC 放电推演 0.8 → 0.7 =====
    #[test]
    fn t16_soc_discharge() {
        let sim = AnalyticalSimModel::new(100.0);
        let m = model_with_device(1, 0.8, 10.0);
        let final_state = sim.run(m, 3_600_000).expect("run ok");
        let soc = final_state.devices.get(&1).expect("d").state.soc;
        assert!((soc - 0.7).abs() < 1e-9);
    }

    // ===== T17: 充电（负功率）soc +0.05 =====
    #[test]
    fn t17_soc_charge_negative_power() {
        let sim = AnalyticalSimModel::new(100.0);
        let m = model_with_device(1, 0.8, -5.0);
        let final_state = sim.run(m, 3_600_000).expect("run ok");
        let soc = final_state.devices.get(&1).expect("d").state.soc;
        assert!((soc - 0.85).abs() < 1e-9);
    }

    // ===== T18: clamp 双界：放超 → 0.0；充超 → 1.0 =====
    #[test]
    fn t18_soc_clamp_bounds() {
        let sim = AnalyticalSimModel::new(100.0);
        let m = model_with_device(1, 0.1, 50.0);
        let f = sim.run(m, 3_600_000).expect("run ok");
        assert!(f.devices.get(&1).expect("d").state.soc.abs() < 1e-12);
        let m2 = model_with_device(1, 0.9, -50.0);
        let f2 = sim.run(m2, 3_600_000).expect("run ok");
        assert!((f2.devices.get(&1).expect("d").state.soc - 1.0).abs() < 1e-12);
    }

    // ===== T19: duration_ms = 0 → soc 不变 =====
    #[test]
    fn t19_zero_duration_noop() {
        let sim = AnalyticalSimModel::new(100.0);
        let m = model_with_device(1, 0.8, 10.0);
        let f = sim.run(m, 0).expect("run ok");
        assert!((f.devices.get(&1).expect("d").state.soc - 0.8).abs() < 1e-12);
    }

    // ===== T20: 多设备各自推演 + grid/market/last_update 透传 + name =====
    #[test]
    fn t20_multi_device_passthrough() {
        let sim = AnalyticalSimModel::new(100.0);
        assert_eq!(sim.name(), "analytical");
        let mut m = model_with_device(1, 0.8, 10.0);
        let mut t2 = DeviceTwin {
            device_id: 2,
            ..DeviceTwin::default()
        };
        t2.state.soc = 0.6;
        t2.state.power = 5.0;
        m.devices.insert(2, t2);
        m.grid.active_power = 12.0;
        m.market = Some(MarketMirror {
            timestamp: 9,
            current_price: 0.7,
        });
        m.last_update = 42;
        let f = sim.run(m, 3_600_000).expect("run ok");
        assert!((f.devices.get(&1).expect("d1").state.soc - 0.7).abs() < 1e-9);
        assert!((f.devices.get(&2).expect("d2").state.soc - 0.55).abs() < 1e-9);
        assert!((f.grid.active_power - 12.0).abs() < 1e-6);
        let mk = f.market.expect("market passthrough");
        assert!((mk.current_price - 0.7).abs() < 1e-6);
        assert_eq!(mk.timestamp, 9);
        assert_eq!(f.last_update, 42);
    }

    // ===== T21: 设备 power NaN → sanitize 0.0 → soc 不变（D12）=====
    #[test]
    fn t21_nan_power_soc_unchanged() {
        let sim = AnalyticalSimModel::new(100.0);
        let m = model_with_device(1, 0.8, f64::NAN);
        let f = sim.run(m, 3_600_000).expect("run ok");
        assert!((f.devices.get(&1).expect("d").state.soc - 0.8).abs() < 1e-12);
    }

    // ===== T22: compute_outcomes 恰好 3 条 + 固定顺序 =====
    #[test]
    fn t22_outcomes_three_fixed_order() {
        let b = model_with_device(1, 0.8, 1.0);
        let f = b.clone();
        let outcomes = compute_outcomes(&b, &f);
        assert_eq!(outcomes.len(), 3);
        let metrics: Vec<&'static str> = outcomes.iter().map(|o| o.metric).collect();
        assert_eq!(
            metrics,
            ["grid_active_power", "total_device_power", "min_soc"]
        );
    }

    // ===== T23: outcomes 数值正确（grid 10→16；Σ 3.0→3.0；min_soc 0.8→0.7）=====
    #[test]
    fn t23_outcomes_values() {
        let mut baseline = model_with_device(1, 0.8, 1.0);
        let mut t2 = DeviceTwin {
            device_id: 2,
            ..DeviceTwin::default()
        };
        t2.state.soc = 0.9;
        t2.state.power = 2.0;
        baseline.devices.insert(2, t2.clone());
        baseline.grid.active_power = 10.0;
        let mut final_state = baseline.clone();
        final_state.grid.active_power = 16.0;
        final_state.devices.get_mut(&1).expect("d1").state.soc = 0.7;
        let outcomes = compute_outcomes(&baseline, &final_state);
        assert!((outcomes[0].value - 16.0).abs() < 1e-6);
        assert!((outcomes[0].baseline - 10.0).abs() < 1e-6);
        assert!((outcomes[1].value - 3.0).abs() < 1e-6);
        assert!((outcomes[1].baseline - 3.0).abs() < 1e-6);
        assert!((outcomes[2].value - 0.7).abs() < 1e-6);
        assert!((outcomes[2].baseline - 0.8).abs() < 1e-6);
    }

    // ===== T24: 空设备 → min_soc value/baseline 均 1.0 =====
    #[test]
    fn t24_empty_devices_min_soc_neutral() {
        let b = TwinModel::default();
        let f = TwinModel::default();
        let outcomes = compute_outcomes(&b, &f);
        assert!((outcomes[2].value - 1.0).abs() < 1e-6);
        assert!((outcomes[2].baseline - 1.0).abs() < 1e-6);
    }

    // ===== T25: assess_risk min_soc == 0.0 → Critical =====
    #[test]
    fn t25_risk_min_soc_zero_critical() {
        let outcomes = vec![Outcome {
            metric: "min_soc",
            value: 0.0,
            baseline: 0.8,
        }];
        assert_eq!(assess_risk(&outcomes), RiskLevel::Critical);
    }

    // ===== T26: assess_risk min_soc == 0.15 → High =====
    #[test]
    fn t26_risk_min_soc_low_high() {
        let outcomes = vec![Outcome {
            metric: "min_soc",
            value: 0.15,
            baseline: 0.8,
        }];
        assert_eq!(assess_risk(&outcomes), RiskLevel::High);
    }

    // ===== T27: 边界 min_soc 0.2 平稳 → Low；0.19 → High =====
    #[test]
    fn t27_risk_min_soc_boundary() {
        let stable = vec![
            Outcome {
                metric: "grid_active_power",
                value: 10.0,
                baseline: 10.0,
            },
            Outcome {
                metric: "min_soc",
                value: 0.2,
                baseline: 0.8,
            },
        ];
        assert_eq!(assess_risk(&stable), RiskLevel::Low);
        let below = vec![Outcome {
            metric: "min_soc",
            value: 0.19,
            baseline: 0.8,
        }];
        assert_eq!(assess_risk(&below), RiskLevel::High);
    }

    // ===== T28: min_soc 安全 + grid 10→16（>50%）→ Medium =====
    #[test]
    fn t28_risk_grid_deviation_medium() {
        let outcomes = vec![
            Outcome {
                metric: "grid_active_power",
                value: 16.0,
                baseline: 10.0,
            },
            Outcome {
                metric: "min_soc",
                value: 0.9,
                baseline: 0.9,
            },
        ];
        assert_eq!(assess_risk(&outcomes), RiskLevel::Medium);
    }

    // ===== T29: 波动恰 50%（10→15）→ Low；全平稳 → Low =====
    #[test]
    fn t29_risk_exact_50pct_low() {
        let exact = vec![
            Outcome {
                metric: "grid_active_power",
                value: 15.0,
                baseline: 10.0,
            },
            Outcome {
                metric: "min_soc",
                value: 0.9,
                baseline: 0.9,
            },
        ];
        assert_eq!(assess_risk(&exact), RiskLevel::Low);
        let flat = vec![
            Outcome {
                metric: "grid_active_power",
                value: 10.0,
                baseline: 10.0,
            },
            Outcome {
                metric: "total_device_power",
                value: 3.0,
                baseline: 3.0,
            },
            Outcome {
                metric: "min_soc",
                value: 0.8,
                baseline: 0.8,
            },
        ];
        assert_eq!(assess_risk(&flat), RiskLevel::Low);
    }

    // ===== T30: 空 outcomes → Low =====
    #[test]
    fn t30_risk_empty_low() {
        assert_eq!(assess_risk(&[]), RiskLevel::Low);
    }

    // ===== T31: 取最重：min_soc 0.0 + grid 大波动 → Critical =====
    #[test]
    fn t31_risk_worst_wins() {
        let outcomes = vec![
            Outcome {
                metric: "grid_active_power",
                value: 100.0,
                baseline: 10.0,
            },
            Outcome {
                metric: "min_soc",
                value: 0.0,
                baseline: 0.8,
            },
        ];
        assert_eq!(assess_risk(&outcomes), RiskLevel::Critical);
    }

    // ===== T32: 重放电端到端 → Ok + min_soc 0.0 + Critical =====
    #[test]
    fn t32_heavy_discharge_critical() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(AnalyticalSimModel::new(100.0)),
        };
        let model = model_with_device(1, 0.3, 0.0);
        let scenario = Scenario {
            name: "heavy-discharge",
            actions: vec![Action::SetDevicePower {
                device_id: 1,
                power: 50.0,
            }],
            duration_ms: 3_600_000,
        };
        let r = analyzer.analyze(&scenario, &model).expect("analyze ok");
        let min_soc = r
            .outcomes
            .iter()
            .find(|o| o.metric == "min_soc")
            .expect("min_soc outcome");
        assert!(min_soc.value.abs() < 1e-6);
        assert_eq!(r.risk_level, RiskLevel::Critical);
    }

    // ===== T33: 平稳场景 → Low + 3 条 outcomes + scenario 名回显 =====
    #[test]
    fn t33_stable_scenario_low() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(AnalyticalSimModel::new(100.0)),
        };
        let model = model_with_device(1, 0.8, 0.0);
        let scenario = Scenario {
            name: "mild",
            actions: vec![Action::SetDevicePower {
                device_id: 1,
                power: 1.0,
            }],
            duration_ms: 1_800_000,
        };
        let r = analyzer.analyze(&scenario, &model).expect("analyze ok");
        assert_eq!(r.risk_level, RiskLevel::Low);
        assert_eq!(r.outcomes.len(), 3);
        assert_eq!(r.scenario, "mild");
    }

    // ===== T34: DivergingSimModel → Ok + outcomes 空 + Critical =====
    #[test]
    fn t34_diverged_to_critical() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(DivergingSimModel),
        };
        let model = model_with_device(1, 0.8, 1.0);
        let scenario = Scenario {
            name: "diverge",
            actions: vec![],
            duration_ms: 1_000,
        };
        let r = analyzer.analyze(&scenario, &model).expect("diverged -> ok");
        assert!(r.outcomes.is_empty());
        assert_eq!(r.risk_level, RiskLevel::Critical);
        assert_eq!(r.scenario, "diverge");
    }

    // ===== T35: UnavailableSimModel → Err(ModelUnavailable) =====
    #[test]
    fn t35_unavailable_rejects() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(UnavailableSimModel),
        };
        let model = model_with_device(1, 0.8, 1.0);
        let scenario = Scenario {
            name: "unavail",
            actions: vec![],
            duration_ms: 1_000,
        };
        let r = analyzer.analyze(&scenario, &model);
        assert_eq!(r.err(), Some(WhatIfError::ModelUnavailable));
    }

    // ===== T36: 确定性：两次 analyze 逐字段一致 =====
    #[test]
    fn t36_deterministic() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(AnalyticalSimModel::new(100.0)),
        };
        let model = model_with_device(1, 0.8, 10.0);
        let scenario = Scenario {
            name: "det",
            actions: vec![Action::SetGridPower { active_power: 12.0 }],
            duration_ms: 3_600_000,
        };
        let r1 = analyzer.analyze(&scenario, &model).expect("ok1");
        let r2 = analyzer.analyze(&scenario, &model).expect("ok2");
        assert_eq!(r1.scenario, r2.scenario);
        assert_eq!(r1.outcomes, r2.outcomes);
        assert_eq!(r1.risk_level, r2.risk_level);
    }

    // ===== T37: 只读：analyze 后 model 与事前 clone 逐字段相等 =====
    #[test]
    fn t37_read_only_model() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(AnalyticalSimModel::new(100.0)),
        };
        let mut model = model_with_device(1, 0.3, 1.0);
        model.grid.active_power = 10.0;
        model.market = Some(MarketMirror {
            timestamp: 5,
            current_price: 0.6,
        });
        model.last_update = 123;
        let before = model.clone();
        let scenario = Scenario {
            name: "readonly",
            actions: vec![
                Action::SetDevicePower {
                    device_id: 1,
                    power: 50.0,
                },
                Action::SetGridPower { active_power: 16.0 },
            ],
            duration_ms: 3_600_000,
        };
        let _ = analyzer.analyze(&scenario, &model).expect("ok");
        assert_eq!(model.devices.len(), before.devices.len());
        for (id, twin) in &before.devices {
            let now = model.devices.get(id).expect("device still present");
            assert!((now.state.soc - twin.state.soc).abs() < 1e-12);
            assert!((now.state.power - twin.state.power).abs() < 1e-12);
        }
        assert!((model.grid.active_power - before.grid.active_power).abs() < 1e-6);
        assert_eq!(model.market, before.market);
        assert_eq!(model.last_update, before.last_update);
    }

    // ===== T38: 组合动作 SetDevicePower{1,20} + RemoveDevice{2} =====
    #[test]
    fn t38_combined_actions() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(AnalyticalSimModel::new(100.0)),
        };
        let mut model = model_with_device(1, 0.9, 0.0);
        let mut t2 = DeviceTwin {
            device_id: 2,
            ..DeviceTwin::default()
        };
        t2.state.soc = 0.5;
        t2.state.power = 3.0;
        model.devices.insert(2, t2);
        let scenario = Scenario {
            name: "combined",
            actions: vec![
                Action::SetDevicePower {
                    device_id: 1,
                    power: 20.0,
                },
                Action::RemoveDevice { device_id: 2 },
            ],
            duration_ms: 3_600_000,
        };
        let r = analyzer.analyze(&scenario, &model).expect("ok");
        let total = r
            .outcomes
            .iter()
            .find(|o| o.metric == "total_device_power")
            .expect("total outcome");
        assert!((total.value - 20.0).abs() < 1e-6);
        let min_soc = r
            .outcomes
            .iter()
            .find(|o| o.metric == "min_soc")
            .expect("min_soc outcome");
        assert!((min_soc.value - 0.7).abs() < 1e-6);
    }

    // ===== T39: 空 actions + duration 0 → value == baseline 全等 + Low =====
    #[test]
    fn t39_noop_scenario() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(AnalyticalSimModel::new(100.0)),
        };
        let mut model = model_with_device(1, 0.8, 2.0);
        model.grid.active_power = 10.0;
        let scenario = Scenario {
            name: "noop",
            actions: vec![],
            duration_ms: 0,
        };
        let r = analyzer.analyze(&scenario, &model).expect("ok");
        for o in &r.outcomes {
            assert!((o.value - o.baseline).abs() < 1e-6);
        }
        assert_eq!(r.risk_level, RiskLevel::Low);
    }

    // ===== T40: SetGridPower{16.0}（baseline 10.0）→ outcome 回显 + Medium =====
    #[test]
    fn t40_grid_shift_medium() {
        let analyzer = WhatIfAnalyzer {
            sim_model: Box::new(AnalyticalSimModel::new(100.0)),
        };
        let mut model = model_with_device(1, 0.9, 0.0);
        model.grid.active_power = 10.0;
        let scenario = Scenario {
            name: "grid-shift",
            actions: vec![Action::SetGridPower { active_power: 16.0 }],
            duration_ms: 0,
        };
        let r = analyzer.analyze(&scenario, &model).expect("ok");
        let grid = r
            .outcomes
            .iter()
            .find(|o| o.metric == "grid_active_power")
            .expect("grid outcome");
        assert!((grid.value - 16.0).abs() < 1e-6);
        assert!((grid.baseline - 10.0).abs() < 1e-6);
        assert_eq!(r.risk_level, RiskLevel::Medium);
    }
}
