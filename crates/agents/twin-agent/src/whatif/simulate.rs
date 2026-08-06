//! WhatIf 模拟引擎（ARCH-G03：interface 与 implementation 分离）。

use super::*;

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
