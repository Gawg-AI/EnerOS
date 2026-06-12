use serde::{Deserialize, Serialize};
use eneros_core::ElementId;

/// API response wrapper
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(error: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
        }
    }
}

/// Topology query request
#[derive(Debug, Serialize, Deserialize)]
pub struct TopologyQuery {
    pub from_bus: ElementId,
    pub to_bus: ElementId,
}

/// Topology query response
#[derive(Debug, Serialize, Deserialize)]
pub struct TopologyResponse {
    pub connected: bool,
    pub path: Option<Vec<ElementId>>,
}

/// Power flow request
#[derive(Debug, Serialize, Deserialize)]
pub struct PowerFlowRequest {
    pub case_id: Option<String>,
    pub max_iterations: Option<u32>,
    pub tolerance: Option<f64>,
}

/// Power flow response
#[derive(Debug, Serialize, Deserialize)]
pub struct PowerFlowResponse {
    pub converged: bool,
    pub iterations: u32,
    pub total_losses: f64,
    pub bus_voltages: Vec<BusVoltageResponse>,
    pub branch_flows: Vec<BranchFlowResponse>,
}

/// Bus voltage response
#[derive(Debug, Serialize, Deserialize)]
pub struct BusVoltageResponse {
    pub bus_id: ElementId,
    pub voltage_magnitude: f64,
    pub voltage_angle: f64,
}

/// Branch flow response
#[derive(Debug, Serialize, Deserialize)]
pub struct BranchFlowResponse {
    pub branch_id: ElementId,
    pub from_bus: ElementId,
    pub to_bus: ElementId,
    pub active_power_mw: f64,
    pub reactive_power_mvar: f64,
    pub loading_percent: f64,
}

/// Constraint check request
#[derive(Debug, Serialize, Deserialize)]
pub struct ConstraintCheckRequest {
    pub bus_voltages: Vec<(ElementId, f64)>,
    pub branch_loadings: Vec<(ElementId, f64)>,
    pub frequency: f64,
}

/// Constraint violation response
#[derive(Debug, Serialize, Deserialize)]
pub struct ConstraintViolationResponse {
    pub constraint_id: String,
    pub element_id: ElementId,
    pub actual_value: f64,
    pub limit_min: f64,
    pub limit_max: f64,
    pub severity: String,
}
