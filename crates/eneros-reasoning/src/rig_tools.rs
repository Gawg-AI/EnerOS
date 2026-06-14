//! Power system Tools for rig's Tool trait.
//!
//! These tools allow rig's Agent to call EnerOS's power system analysis
//! capabilities (power flow, constraint checking, etc.) during reasoning.
//!
//! **Design**: Each tool wraps an existing EnerOS capability. When EnerOS
//! adds new analysis features, just add a new Tool here. When rig changes
//! its Tool trait, only this file needs updating.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

// ── Arg structs (available without rig feature for testing) ──────────

/// Arguments for power flow tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rig", derive(schemars::JsonSchema))]
pub struct PowerFlowArgs {
    /// Optional: specific bus IDs to analyze (empty = all buses)
    #[cfg_attr(feature = "rig", schemars(description = "Bus IDs to analyze, e.g. [1, 2, 5]. Empty means all buses."))]
    pub bus_ids: Option<Vec<u64>>,
}

/// Arguments for constraint check tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rig", derive(schemars::JsonSchema))]
pub struct ConstraintCheckArgs {
    /// Voltage lower limit in pu
    #[cfg_attr(feature = "rig", schemars(description = "Voltage lower limit in pu (default: 0.95)"))]
    pub v_min: Option<f64>,
    /// Voltage upper limit in pu
    #[cfg_attr(feature = "rig", schemars(description = "Voltage upper limit in pu (default: 1.05)"))]
    pub v_max: Option<f64>,
    /// Maximum branch loading percentage
    #[cfg_attr(feature = "rig", schemars(description = "Maximum branch loading percentage (default: 100.0)"))]
    pub max_loading: Option<f64>,
}

/// Arguments for N-1 analysis tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rig", derive(schemars::JsonSchema))]
pub struct N1AnalysisArgs {
    /// Voltage lower limit for N-1 post-contingency check (pu)
    #[cfg_attr(feature = "rig", schemars(description = "Voltage lower limit in pu for post-contingency check (default: 0.95)"))]
    pub v_min: Option<f64>,
    /// Voltage upper limit for N-1 post-contingency check (pu)
    #[cfg_attr(feature = "rig", schemars(description = "Voltage upper limit in pu for post-contingency check (default: 1.05)"))]
    pub v_max: Option<f64>,
    /// Thermal limit for N-1 post-contingency check (loading %)
    #[cfg_attr(feature = "rig", schemars(description = "Thermal loading limit in percent for post-contingency check (default: 100.0)"))]
    pub thermal_limit: Option<f64>,
}

/// Arguments for voltage stability tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rig", derive(schemars::JsonSchema))]
pub struct VoltageStabilityArgs {
    /// Reserved for future continuation-power-flow step size
    #[cfg_attr(feature = "rig", schemars(description = "Load increase step in pu for stability margin estimation (default: 0.05)"))]
    pub step_size: Option<f64>,
}

// ── PowerSystemToolSet (available without rig feature) ──────────────

/// Tool set for power system analysis — manages all available rig Tools.
///
/// Holds a shared reference to a [`eneros_network::PowerNetwork`] so that
/// each tool can perform real analysis when called by the rig Agent.
///
/// Use [`PowerSystemToolSet::all`] or [`PowerSystemToolSet::new`] to create
/// a fully-configured set with a live network. The `Default` impl creates an
/// empty set with no tools — suitable as a placeholder before a network is
/// available.
#[derive(Default)]
pub struct PowerSystemToolSet {
    /// Shared network reference
    #[cfg(feature = "rig")]
    network: Option<Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>>,
    /// Whether to include power flow tool
    pub include_power_flow: bool,
    /// Whether to include constraint check tool
    pub include_constraint_check: bool,
    /// Whether to include N-1 analysis tool
    pub include_n1_analysis: bool,
    /// Whether to include voltage stability tool
    pub include_voltage_stability: bool,
}

impl PowerSystemToolSet {
    /// Create a tool set with all power system tools enabled.
    #[cfg(feature = "rig")]
    pub fn all(network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>) -> Self {
        Self {
            network: Some(network),
            include_power_flow: true,
            include_constraint_check: true,
            include_n1_analysis: true,
            include_voltage_stability: true,
        }
    }

    /// Create a tool set with the given network and all tools enabled.
    #[cfg(feature = "rig")]
    pub fn new(network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>) -> Self {
        Self::all(network)
    }

    /// Get the shared network reference, if set.
    #[cfg(feature = "rig")]
    pub fn network(&self) -> Option<&Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>> {
        self.network.as_ref()
    }

    /// Returns true if any tools are enabled and a network is available.
    #[cfg(feature = "rig")]
    pub fn has_tools(&self) -> bool {
        self.network.is_some()
            && (self.include_power_flow
                || self.include_constraint_check
                || self.include_n1_analysis
                || self.include_voltage_stability)
    }

    /// Get the list of tool descriptions for prompt construction.
    pub fn tool_descriptions(&self) -> Vec<(&'static str, &'static str)> {
        let mut tools = Vec::new();
        if self.include_power_flow {
            tools.push(("power_flow", "Run power flow analysis on the current network state. Returns bus voltages, angles, and branch flows."));
        }
        if self.include_constraint_check {
            tools.push(("constraint_check", "Check the current network state for constraint violations (voltage, thermal). Returns list of violations."));
        }
        if self.include_n1_analysis {
            tools.push(("n1_analysis", "Run N-1 contingency analysis. Simulates single-element outages and reports security violations."));
        }
        if self.include_voltage_stability {
            tools.push(("voltage_stability", "Check voltage stability margin. Reports proximity to voltage collapse."));
        }
        tools
    }

    /// Build a rig ToolSet containing all enabled tools.
    ///
    /// Panics if no network has been set (i.e. this is the `Default` placeholder).
    #[cfg(feature = "rig")]
    pub fn to_toolset(&self) -> rig_core::tool::ToolSet {
        let network = self
            .network
            .clone()
            .expect("PowerSystemToolSet::to_toolset requires a network — use ::all(network) to create");
        let mut builder = rig_core::tool::ToolSet::builder();
        if self.include_power_flow {
            builder = builder.static_tool(PowerFlowTool {
                network: Arc::clone(&network),
            });
        }
        if self.include_constraint_check {
            builder = builder.static_tool(ConstraintCheckTool {
                network: Arc::clone(&network),
            });
        }
        if self.include_n1_analysis {
            builder = builder.static_tool(N1AnalysisTool {
                network: Arc::clone(&network),
            });
        }
        if self.include_voltage_stability {
            builder = builder.static_tool(VoltageStabilityTool {
                network: Arc::clone(&network),
            });
        }
        builder.build()
    }
}

// ── Helper: serialize analysis results to JSON ──────────────────────
// The result types in eneros-powerflow / eneros-constraint don't derive
// Serialize, so we build serde_json::Value manually.
// These helpers are only needed when the rig feature is enabled.

#[cfg(feature = "rig")]
fn power_flow_result_to_json(result: &eneros_powerflow::PowerFlowResult) -> serde_json::Value {
    let buses: Vec<serde_json::Value> = result
        .bus_results
        .iter()
        .map(|b| {
            serde_json::json!({
                "bus_id": b.bus_id,
                "voltage_pu": (b.voltage_magnitude * 1000.0).round() / 1000.0,
                "angle_deg": (b.voltage_angle.to_degrees() * 100.0).round() / 100.0,
                "p_injection_mw": (b.p_injection * 100.0).round() / 100.0,
                "q_injection_mvar": (b.q_injection * 100.0).round() / 100.0,
            })
        })
        .collect();

    let branches: Vec<serde_json::Value> = result
        .branch_results
        .iter()
        .map(|br| {
            serde_json::json!({
                "branch_id": br.branch_id,
                "from_bus": br.from_bus,
                "to_bus": br.to_bus,
                "p_from_mw": (br.p_from * 100.0).round() / 100.0,
                "p_to_mw": (br.p_to * 100.0).round() / 100.0,
                "loss_mw": (br.loss_mw * 1000.0).round() / 1000.0,
                "loading_percent": (br.loading_percent * 10.0).round() / 10.0,
            })
        })
        .collect();

    serde_json::json!({
        "converged": result.converged,
        "iterations": result.iterations,
        "total_losses_mw": (result.total_losses * 100.0).round() / 100.0,
        "buses": buses,
        "branches": branches,
    })
}

#[cfg(feature = "rig")]
fn violations_to_json(violations: &[eneros_constraint::Violation]) -> serde_json::Value {
    let items: Vec<serde_json::Value> = violations
        .iter()
        .map(|v| {
            serde_json::json!({
                "constraint_id": v.constraint_id,
                "element_id": v.element_id,
                "constraint_type": format!("{:?}", v.constraint_type),
                "actual_value": v.actual_value,
                "limit_min": v.limit_min,
                "limit_max": v.limit_max,
                "severity": format!("{:?}", v.severity),
                "violation_percent": (v.violation_percent() * 10.0).round() / 10.0,
            })
        })
        .collect();
    serde_json::json!(items)
}

#[cfg(feature = "rig")]
fn n1_results_to_json(results: &[eneros_constraint::N1Result]) -> serde_json::Value {
    let items: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            let voltage_violations: Vec<serde_json::Value> = r
                .voltage_violations
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "element_id": v.element_id,
                        "type": format!("{:?}", v.violation_type),
                        "actual": v.actual_value,
                        "limit": v.limit_value,
                    })
                })
                .collect();
            let thermal_violations: Vec<serde_json::Value> = r
                .thermal_violations
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "element_id": v.element_id,
                        "type": format!("{:?}", v.violation_type),
                        "actual": v.actual_value,
                        "limit": v.limit_value,
                    })
                })
                .collect();
            serde_json::json!({
                "outage_branch_id": r.branch_id,
                "converged": r.converged,
                "severity": format!("{:?}", r.severity),
                "voltage_violations": voltage_violations,
                "thermal_violations": thermal_violations,
            })
        })
        .collect();
    serde_json::json!(items)
}

#[cfg(feature = "rig")]
fn stability_result_to_json(result: &eneros_constraint::StabilityResult) -> serde_json::Value {
    let margins: Vec<serde_json::Value> = result
        .voltage_margins
        .iter()
        .map(|m| {
            serde_json::json!({
                "bus_id": m.bus_id,
                "voltage_pu": (m.voltage_pu * 1000.0).round() / 1000.0,
                "margin": (m.margin * 1000.0).round() / 1000.0,
            })
        })
        .collect();
    serde_json::json!({
        "stable": result.stable,
        "critical_buses": result.critical_buses,
        "voltage_margins": margins,
    })
}

// ── rig Tool implementations ────────────────────────────────────────

/// Error type for power system tools.
#[cfg(feature = "rig")]
#[derive(Debug, thiserror::Error)]
pub enum ToolCallError {
    #[error("Tool execution error: {0}")]
    ExecutionError(String),
}

/// Power flow analysis tool for rig Agent.
#[cfg(feature = "rig")]
pub struct PowerFlowTool {
    pub network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>,
}

#[cfg(feature = "rig")]
impl rig_core::tool::Tool for PowerFlowTool {
    const NAME: &'static str = "power_flow";
    type Error = ToolCallError;
    type Args = PowerFlowArgs;
    type Output = serde_json::Value;

    fn name(&self) -> String {
        "power_flow".to_string()
    }

    async fn definition(&self, _prompt: String) -> rig_core::completion::ToolDefinition {
        rig_core::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Run power flow analysis on the current network. Returns bus voltages (pu), angles (degrees), branch flows (MW), and total losses.".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(PowerFlowArgs))
                .unwrap_or_else(|_| serde_json::json!({"type": "object"})),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(bus_ids = ?args.bus_ids, "Power flow tool called via rig Agent");

        let net = self.network.read();
        match net.solve() {
            Ok(result) => {
                if !result.converged {
                    return Ok(serde_json::json!({
                        "error": "Power flow did not converge",
                        "iterations": result.iterations,
                    }));
                }
                let mut json = power_flow_result_to_json(&result);
                // If specific bus IDs were requested, filter the bus results
                if let Some(ref bus_ids) = args.bus_ids {
                    if let Some(buses) = json.get_mut("buses").and_then(|b| b.as_array_mut()) {
                        let id_set: std::collections::HashSet<u64> =
                            bus_ids.iter().copied().collect();
                        buses.retain(|b| {
                            b.get("bus_id")
                                .and_then(|v| v.as_u64())
                                .map_or(false, |id| id_set.contains(&id))
                        });
                    }
                }
                Ok(json)
            }
            Err(e) => Ok(serde_json::json!({
                "error": format!("Power flow solve failed: {}", e),
            })),
        }
    }
}

/// Constraint check tool for rig Agent.
#[cfg(feature = "rig")]
pub struct ConstraintCheckTool {
    pub network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>,
}

#[cfg(feature = "rig")]
impl rig_core::tool::Tool for ConstraintCheckTool {
    const NAME: &'static str = "constraint_check";
    type Error = ToolCallError;
    type Args = ConstraintCheckArgs;
    type Output = serde_json::Value;

    fn name(&self) -> String {
        "constraint_check".to_string()
    }

    async fn definition(&self, _prompt: String) -> rig_core::completion::ToolDefinition {
        rig_core::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Check the current network for constraint violations: voltage limits and thermal limits.".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(ConstraintCheckArgs))
                .unwrap_or_else(|_| serde_json::json!({"type": "object"})),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            v_min = ?args.v_min,
            v_max = ?args.v_max,
            "Constraint check tool called via rig Agent"
        );

        let net = self.network.read();
        match net.solve() {
            Ok(result) => {
                if !result.converged {
                    return Ok(serde_json::json!({
                        "error": "Power flow did not converge — cannot check constraints",
                        "iterations": result.iterations,
                    }));
                }
                let violations = net.check_constraints(&result);
                Ok(serde_json::json!({
                    "converged": true,
                    "violation_count": violations.len(),
                    "limits": {
                        "v_min": args.v_min.unwrap_or(0.95),
                        "v_max": args.v_max.unwrap_or(1.05),
                        "max_loading": args.max_loading.unwrap_or(100.0),
                    },
                    "violations": violations_to_json(&violations),
                }))
            }
            Err(e) => Ok(serde_json::json!({
                "error": format!("Power flow solve failed: {}", e),
            })),
        }
    }
}

/// N-1 contingency analysis tool for rig Agent.
#[cfg(feature = "rig")]
pub struct N1AnalysisTool {
    pub network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>,
}

#[cfg(feature = "rig")]
impl rig_core::tool::Tool for N1AnalysisTool {
    const NAME: &'static str = "n1_analysis";
    type Error = ToolCallError;
    type Args = N1AnalysisArgs;
    type Output = serde_json::Value;

    fn name(&self) -> String {
        "n1_analysis".to_string()
    }

    async fn definition(&self, _prompt: String) -> rig_core::completion::ToolDefinition {
        rig_core::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Run N-1 contingency analysis. Simulates single-branch outages and reports voltage and thermal violations for each contingency.".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(N1AnalysisArgs))
                .unwrap_or_else(|_| serde_json::json!({"type": "object"})),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            v_min = ?args.v_min,
            v_max = ?args.v_max,
            thermal_limit = ?args.thermal_limit,
            "N-1 analysis tool called via rig Agent"
        );

        let v_min = args.v_min.unwrap_or(0.95);
        let v_max = args.v_max.unwrap_or(1.05);
        let thermal = args.thermal_limit.unwrap_or(100.0);

        let net = self.network.read();
        let results = net.check_n1_with_limits(v_min, v_max, thermal);

        let total = results.len();
        let converged = results.iter().filter(|r| r.converged).count();
        let with_violations = results
            .iter()
            .filter(|r| !r.voltage_violations.is_empty() || !r.thermal_violations.is_empty())
            .count();

        Ok(serde_json::json!({
            "total_contingencies": total,
            "converged": converged,
            "with_violations": with_violations,
            "limits": {
                "v_min": v_min,
                "v_max": v_max,
                "thermal_limit": thermal,
            },
            "contingencies": n1_results_to_json(&results),
        }))
    }
}

/// Voltage stability check tool for rig Agent.
#[cfg(feature = "rig")]
pub struct VoltageStabilityTool {
    pub network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>,
}

#[cfg(feature = "rig")]
impl rig_core::tool::Tool for VoltageStabilityTool {
    const NAME: &'static str = "voltage_stability";
    type Error = ToolCallError;
    type Args = VoltageStabilityArgs;
    type Output = serde_json::Value;

    fn name(&self) -> String {
        "voltage_stability".to_string()
    }

    async fn definition(&self, _prompt: String) -> rig_core::completion::ToolDefinition {
        rig_core::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Check voltage stability margin. Reports per-bus voltage margins and critical buses near voltage collapse.".to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(VoltageStabilityArgs))
                .unwrap_or_else(|_| serde_json::json!({"type": "object"})),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            step_size = ?args.step_size,
            "Voltage stability tool called via rig Agent"
        );

        let net = self.network.read();
        match net.solve() {
            Ok(result) => {
                if !result.converged {
                    return Ok(serde_json::json!({
                        "error": "Power flow did not converge — cannot check stability",
                        "iterations": result.iterations,
                    }));
                }
                let stability = net.check_stability(&result);
                Ok(stability_result_to_json(&stability))
            }
            Err(e) => Ok(serde_json::json!({
                "error": format!("Power flow solve failed: {}", e),
            })),
        }
    }
}

// ── Tests (no rig feature required) ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_flow_args_schema() {
        let args = PowerFlowArgs {
            bus_ids: Some(vec![1, 2, 5]),
        };
        let json = serde_json::to_value(&args).unwrap();
        assert!(json["bus_ids"].is_array());
    }

    #[test]
    fn test_constraint_check_args_schema() {
        let args = ConstraintCheckArgs {
            v_min: Some(0.95),
            v_max: Some(1.05),
            max_loading: Some(100.0),
        };
        let json = serde_json::to_value(&args).unwrap();
        assert_eq!(json["v_min"], 0.95);
    }

    #[test]
    fn test_n1_analysis_args_schema() {
        let args = N1AnalysisArgs {
            v_min: Some(0.90),
            v_max: Some(1.10),
            thermal_limit: Some(120.0),
        };
        let json = serde_json::to_value(&args).unwrap();
        assert_eq!(json["thermal_limit"], 120.0);
    }

    #[test]
    fn test_voltage_stability_args_schema() {
        let args = VoltageStabilityArgs {
            step_size: Some(0.05),
        };
        let json = serde_json::to_value(&args).unwrap();
        assert_eq!(json["step_size"], 0.05);
    }
}

// ── Tests that require the rig feature ───────────────────────────────

#[cfg(all(test, feature = "rig"))]
mod rig_tests {
    use super::*;
    use rig_core::tool::Tool;
    use std::sync::Arc;

    fn ieee14_network() -> Arc<parking_lot::RwLock<eneros_network::PowerNetwork>> {
        Arc::new(parking_lot::RwLock::new(
            eneros_network::PowerNetwork::from_ieee14(),
        ))
    }

    #[tokio::test]
    async fn test_power_flow_tool_call() {
        let tool = PowerFlowTool {
            network: ieee14_network(),
        };
        let result = tool
            .call(PowerFlowArgs { bus_ids: None })
            .await
            .expect("power flow tool should succeed");
        assert!(result.get("converged").and_then(|v| v.as_bool()).unwrap());
        assert_eq!(result.get("buses").unwrap().as_array().unwrap().len(), 14);
    }

    #[tokio::test]
    async fn test_power_flow_tool_filtered_buses() {
        let tool = PowerFlowTool {
            network: ieee14_network(),
        };
        let result = tool
            .call(PowerFlowArgs {
                bus_ids: Some(vec![1, 2]),
            })
            .await
            .expect("power flow tool should succeed");
        assert_eq!(result.get("buses").unwrap().as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_constraint_check_tool_call() {
        let tool = ConstraintCheckTool {
            network: ieee14_network(),
        };
        let result = tool
            .call(ConstraintCheckArgs {
                v_min: Some(0.95),
                v_max: Some(1.05),
                max_loading: Some(100.0),
            })
            .await
            .expect("constraint check tool should succeed");
        assert!(result.get("converged").and_then(|v| v.as_bool()).unwrap());
        assert!(result.get("violations").is_some());
    }

    #[tokio::test]
    async fn test_n1_analysis_tool_call() {
        let tool = N1AnalysisTool {
            network: ieee14_network(),
        };
        let result = tool
            .call(N1AnalysisArgs {
                v_min: Some(0.95),
                v_max: Some(1.05),
                thermal_limit: Some(100.0),
            })
            .await
            .expect("N-1 analysis tool should succeed");
        assert!(result.get("total_contingencies").is_some());
        assert_eq!(result.get("contingencies").unwrap().as_array().unwrap().len(), 20);
    }

    #[tokio::test]
    async fn test_voltage_stability_tool_call() {
        let tool = VoltageStabilityTool {
            network: ieee14_network(),
        };
        let result = tool
            .call(VoltageStabilityArgs { step_size: None })
            .await
            .expect("voltage stability tool should succeed");
        assert!(result.get("stable").and_then(|v| v.as_bool()).unwrap());
        assert_eq!(result.get("voltage_margins").unwrap().as_array().unwrap().len(), 14);
    }

    #[test]
    fn test_tool_set_all() {
        let set = PowerSystemToolSet::all(ieee14_network());
        assert!(set.include_power_flow);
        assert!(set.include_constraint_check);
        assert!(set.include_n1_analysis);
        assert!(set.include_voltage_stability);
    }

    #[test]
    fn test_tool_set_descriptions() {
        let set = PowerSystemToolSet::all(ieee14_network());
        let descs = set.tool_descriptions();
        assert_eq!(descs.len(), 4);
        assert!(descs.iter().any(|(name, _)| *name == "power_flow"));
        assert!(descs.iter().any(|(name, _)| *name == "constraint_check"));
        assert!(descs.iter().any(|(name, _)| *name == "n1_analysis"));
        assert!(descs.iter().any(|(name, _)| *name == "voltage_stability"));
    }

    #[test]
    fn test_tool_set_to_toolset() {
        let set = PowerSystemToolSet::all(ieee14_network());
        let toolset = set.to_toolset();
        assert!(toolset.contains("power_flow"));
        assert!(toolset.contains("constraint_check"));
        assert!(toolset.contains("n1_analysis"));
        assert!(toolset.contains("voltage_stability"));
    }
}
