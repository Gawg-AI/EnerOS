use crate::network::PowerNetwork;
use eneros_constraint::projector::WhatIfResult;
use eneros_constraint::rules::ConstraintType;
use eneros_core::StructuredAction;

pub(super) fn simulate_base_case(net: &PowerNetwork) -> WhatIfResult {
    solve_and_check(net)
}

pub(super) fn solve_and_check(net: &PowerNetwork) -> WhatIfResult {
    match net.solve() {
        Ok(result) => {
            let violations = net.check_constraints(&result);
            let voltage_violations: Vec<(u64, f64, f64)> = violations
                .iter()
                .filter(|v| v.constraint_type == ConstraintType::Voltage)
                .map(|v| (v.element_id, v.actual_value, v.limit_max))
                .collect();
            let thermal_violations: Vec<(u64, f64, f64)> = violations
                .iter()
                .filter(|v| v.constraint_type == ConstraintType::Thermal)
                .map(|v| (v.element_id, v.actual_value, v.limit_max))
                .collect();

            WhatIfResult {
                applicable: true,
                converged: result.converged,
                voltage_violations,
                thermal_violations,
                all_constraints_satisfied: violations.is_empty(),
                summary: if violations.is_empty() {
                    "OK".to_string()
                } else {
                    format!("{} violations", violations.len())
                },
            }
        }
        Err(e) => WhatIfResult {
            applicable: true,
            converged: false,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: false,
            summary: format!("Power flow failed: {}", e),
        },
    }
}

pub(super) fn inapplicable_result(summary: String) -> WhatIfResult {
    WhatIfResult {
        applicable: false,
        converged: true,
        voltage_violations: vec![],
        thermal_violations: vec![],
        all_constraints_satisfied: false,
        summary,
    }
}

pub(super) fn conservative_switching_reject(action: &StructuredAction) -> WhatIfResult {
    WhatIfResult {
        applicable: true,
        converged: true,
        voltage_violations: vec![],
        thermal_violations: vec![],
        all_constraints_satisfied: false,
        summary: format!(
            "switching action {:?}: topology physics not modeled, conservative reject",
            action
        ),
    }
}
