use async_trait::async_trait;
use eneros_core::Result;
use eneros_powerflow::PowerFlowSolver;
use eneros_constraint::ConstraintEngine;
use std::collections::HashMap;
use eneros_core::ElementId;

use crate::tool::{Tool, ToolOutput};

/// Power flow calculation tool
pub struct PowerFlowTool {
    solver: PowerFlowSolver,
}

impl PowerFlowTool {
    /// Create a new PowerFlowTool with default solver
    pub fn new() -> Self {
        Self {
            solver: PowerFlowSolver::default_solver(),
        }
    }

    /// Create with custom solver parameters
    pub fn with_solver(max_iterations: u32, tolerance: f64) -> Self {
        Self {
            solver: PowerFlowSolver::new(max_iterations, tolerance),
        }
    }
}

impl Default for PowerFlowTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PowerFlowTool {
    fn name(&self) -> &str {
        "power_flow"
    }

    fn description(&self) -> &str {
        "Execute Newton-Raphson power flow calculation on a power network"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "network": { "type": "string", "description": "Network identifier (e.g., 'ieee14')" },
            },
            "required": ["network"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolOutput> {
        let network = params.get("network").and_then(|v| v.as_str()).unwrap_or("ieee14");

        match network {
            "ieee14" => {
                let data = eneros_powerflow::ieee14();
                let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();
                let v_initial: Vec<f64> = data.buses.iter().map(|b| b.v_pu).collect();

                match self.solver.solve_with_initial(&ybus, &p_spec, &q_spec, &bus_types, Some(&v_initial)) {
                    Ok(result) => {
                        let summary = serde_json::json!({
                            "converged": result.converged,
                            "iterations": result.iterations,
                            "total_losses": result.total_losses,
                            "bus_count": result.bus_results.len(),
                            "branch_count": result.branch_results.len(),
                        });
                        Ok(ToolOutput::ok(summary, &format!(
                            "Power flow {} in {} iterations, losses: {:.2} MW",
                            if result.converged { "converged" } else { "did NOT converge" },
                            result.iterations,
                            result.total_losses
                        )))
                    }
                    Err(e) => Ok(ToolOutput::err(&format!("Power flow failed: {}", e))),
                }
            }
            _ => Ok(ToolOutput::err(&format!("Unknown network: {}", network))),
        }
    }
}

/// N-1 contingency analysis tool
pub struct N1AnalysisTool;

impl N1AnalysisTool {
    /// Create a new N1AnalysisTool
    pub fn new() -> Self {
        Self
    }
}

impl Default for N1AnalysisTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for N1AnalysisTool {
    fn name(&self) -> &str {
        "n1_analysis"
    }

    fn description(&self) -> &str {
        "Perform N-1 contingency analysis on a power network"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "network": { "type": "string", "description": "Network identifier" },
            },
            "required": ["network"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolOutput> {
        let network = params.get("network").and_then(|v| v.as_str()).unwrap_or("ieee14");

        match network {
            "ieee14" => {
                let data = eneros_powerflow::ieee14();
                let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();

                let bus_map: HashMap<ElementId, usize> = data
                    .buses
                    .iter()
                    .enumerate()
                    .map(|(idx, bus)| (bus.bus_id as ElementId, idx))
                    .collect();

                let branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = data
                    .branches
                    .iter()
                    .map(|br| (br.from_bus as ElementId, br.to_bus as ElementId, br.r_pu, br.x_pu, br.b_pu, br.tap_ratio))
                    .collect();

                let solver = PowerFlowSolver::new(100, 1e-8);
                let engine = ConstraintEngine::new();

                let results = engine.check_n1_analysis(
                    &ybus, &p_spec, &q_spec, &bus_types,
                    &branches, &bus_map, &solver,
                    None, None, None,
                );

                let converged = results.iter().filter(|r| r.converged).count();
                let violations = results.iter().filter(|r| !r.voltage_violations.is_empty() || !r.thermal_violations.is_empty()).count();

                let summary = serde_json::json!({
                    "total_contingencies": results.len(),
                    "converged": converged,
                    "with_violations": violations,
                    "critical": results.iter().filter(|r| !r.converged).count(),
                });

                Ok(ToolOutput::ok(summary, &format!(
                    "N-1: {}/{} converged, {} with violations",
                    converged, results.len(), violations
                )))
            }
            _ => Ok(ToolOutput::err(&format!("Unknown network: {}", network))),
        }
    }
}

/// Constraint check tool
pub struct ConstraintCheckTool;

impl ConstraintCheckTool {
    /// Create a new ConstraintCheckTool
    pub fn new() -> Self {
        Self
    }
}

impl Default for ConstraintCheckTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ConstraintCheckTool {
    fn name(&self) -> &str {
        "constraint_check"
    }

    fn description(&self) -> &str {
        "Check power system constraints (voltage, thermal, frequency)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "bus_voltages": {
                    "type": "array",
                    "items": { "type": "object", "properties": { "bus_id": { "type": "integer" }, "voltage": { "type": "number" } } }
                },
                "branch_loadings": {
                    "type": "array",
                    "items": { "type": "object", "properties": { "branch_id": { "type": "integer" }, "loading": { "type": "number" } } }
                },
                "frequency": { "type": "number" }
            }
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolOutput> {
        let bus_voltages: Vec<(eneros_core::ElementId, f64)> = params
            .get("bus_voltages")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        Some((
                            item.get("bus_id")?.as_u64()? as eneros_core::ElementId,
                            item.get("voltage")?.as_f64()?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let branch_loadings: Vec<(eneros_core::ElementId, f64)> = params
            .get("branch_loadings")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        Some((
                            item.get("branch_id")?.as_u64()? as eneros_core::ElementId,
                            item.get("loading")?.as_f64()?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let frequency = params.get("frequency").and_then(|v| v.as_f64()).unwrap_or(50.0);

        let engine = ConstraintEngine::new();
        let violations = engine.check_all(&bus_voltages, &branch_loadings, frequency);

        let summary = serde_json::json!({
            "violation_count": violations.len(),
            "violations": violations.iter().map(|v| serde_json::json!({
                "constraint_id": v.constraint_id,
                "element_id": v.element_id,
                "actual_value": v.actual_value,
            })).collect::<Vec<_>>(),
        });

        Ok(ToolOutput::ok(summary, &format!(
            "Constraint check: {} violations found", violations.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolEngine;

    #[tokio::test]
    async fn test_powerflow_tool_ieee14() {
        let tool = PowerFlowTool::new();
        let params = serde_json::json!({ "network": "ieee14" });
        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        assert!(result.data["converged"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_n1_analysis_tool_ieee14() {
        let tool = N1AnalysisTool::new();
        let params = serde_json::json!({ "network": "ieee14" });
        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        assert!(result.data["total_contingencies"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_constraint_check_tool() {
        let tool = ConstraintCheckTool::new();
        let params = serde_json::json!({
            "bus_voltages": [{ "bus_id": 1, "voltage": 0.85 }],
            "branch_loadings": [],
            "frequency": 50.0
        });
        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_tool_engine_register_and_execute() {
        let mut engine = ToolEngine::new();
        engine.register(Box::new(PowerFlowTool::new()));
        engine.register(Box::new(N1AnalysisTool::new()));

        assert_eq!(engine.tool_count(), 2);
        assert!(engine.has_tool("power_flow"));
        assert!(engine.has_tool("n1_analysis"));
        assert!(!engine.has_tool("unknown"));

        let result = engine.execute("power_flow", serde_json::json!({ "network": "ieee14" })).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_tool_engine_unknown_tool() {
        let engine = ToolEngine::new();
        let result = engine.execute("unknown", serde_json::json!({})).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_tool_engine_list_tools() {
        let mut engine = ToolEngine::new();
        engine.register(Box::new(PowerFlowTool::new()));
        let tools = engine.list_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "power_flow");
    }
}
