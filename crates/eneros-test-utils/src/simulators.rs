//! Mock 网络模拟器集合
//!
//! 这些模拟器实现 `eneros_constraint::projector::NetworkSimulator` trait，
//! 用于在测试中提供确定性的 What-If 分析结果。每个模拟器对应一种
//! 典型场景（可行、违例、可投影、不收敛等），便于覆盖可行性投影器
//! 与决策管线的各个分支。

use eneros_constraint::projector::{NetworkSimulator, WhatIfResult};
use eneros_core::StructuredAction;

/// 始终返回可行结果的 mock 模拟器（与既有单元测试一致）
pub struct FeasibleMockSimulator;

impl NetworkSimulator for FeasibleMockSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: true,
            summary: "OK".to_string(),
        }
    }
    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0), (2, 0.0, 150.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02), (2, 0.98)]
    }
}

/// 始终返回电压+热力违例的 mock 模拟器
pub struct ViolatingMockSimulator;

impl NetworkSimulator for ViolatingMockSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![(2, 0.88, 0.95)],
            thermal_violations: vec![(5, 110.0, 100.0)],
            all_constraints_satisfied: false,
            summary: "Voltage and thermal violations".to_string(),
        }
    }
    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0), (2, 0.0, 150.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02), (2, 0.88)]
    }
}

/// 对原始动作返回违例、对削减后动作返回可行的 mock 模拟器。
///
/// 模拟"投影"流程：StartGenerator 且 target_mw > 100 不可行，
/// 但 target_mw <= 100 可行。
pub struct ProjectingMockSimulator;

impl NetworkSimulator for ProjectingMockSimulator {
    fn simulate_action(&self, action: &StructuredAction) -> WhatIfResult {
        match action {
            StructuredAction::StartGenerator { target_mw, .. } if *target_mw > 100.0 => {
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![(2, 0.88, 0.95)],
                    thermal_violations: vec![],
                    all_constraints_satisfied: false,
                    summary: "Voltage violation".to_string(),
                }
            }
            _ => WhatIfResult {
                applicable: true,
                converged: true,
                voltage_violations: vec![],
                thermal_violations: vec![],
                all_constraints_satisfied: true,
                summary: "OK".to_string(),
            },
        }
    }
    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0), (2, 0.0, 150.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02), (2, 0.98)]
    }
}

/// 始终返回不收敛结果的 mock 模拟器
pub struct NonConvergentMockSimulator;

impl NetworkSimulator for NonConvergentMockSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: false,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: false,
            summary: "Power flow did not converge".to_string(),
        }
    }
    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02)]
    }
}

/// 仅返回电压违例的 mock 模拟器
pub struct VoltageViolationMockSimulator;

impl NetworkSimulator for VoltageViolationMockSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![(3, 0.85, 0.95), (4, 0.87, 0.95)],
            thermal_violations: vec![],
            all_constraints_satisfied: false,
            summary: "Voltage violations".to_string(),
        }
    }
    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02), (3, 0.85)]
    }
}

/// 仅返回热力违例的 mock 模拟器
pub struct ThermalViolationMockSimulator;

impl NetworkSimulator for ThermalViolationMockSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![(5, 120.0, 100.0), (6, 115.0, 100.0)],
            all_constraints_satisfied: false,
            summary: "Thermal violations".to_string(),
        }
    }
    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02)]
    }
}
