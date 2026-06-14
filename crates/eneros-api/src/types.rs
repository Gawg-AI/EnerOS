use std::collections::HashMap;
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

// ============================================================
// Topology types (existing)
// ============================================================

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

// ============================================================
// Power flow types (existing, enhanced)
// ============================================================

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

// ============================================================
// Constraint types (existing)
// ============================================================

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

// ============================================================
// Topology data response
// ============================================================

/// Topology data response for GET /api/topology
#[derive(Debug, Serialize, Deserialize)]
pub struct TopologyDataResponse {
    pub buses: Vec<BusData>,
    pub branches: Vec<BranchData>,
    pub zones: Vec<u64>,
}

/// Bus data in topology response
#[derive(Debug, Serialize, Deserialize)]
pub struct BusData {
    pub id: u64,
    pub name: String,
    pub zone_id: u64,
    pub voltage_kv: f64,
}

/// Branch data in topology response
#[derive(Debug, Serialize, Deserialize)]
pub struct BranchData {
    pub id: u64,
    pub from_bus: u64,
    pub to_bus: u64,
    pub reactance: f64,
}

// ============================================================
// Agent types
// ============================================================

/// Agents response for GET /api/agents
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentsResponse {
    pub agent_count: usize,
    pub agents: Vec<AgentInfo>,
}

/// Agent info in agents response
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub agent_type: String,
    pub authority: String,
    pub status: String,
}

// ============================================================
// SCADA types
// ============================================================

/// SCADA latest readings response for GET /api/scada/latest
#[derive(Debug, Serialize, Deserialize)]
pub struct ScadaLatestResponse {
    pub readings: Vec<ScadaReadingResponse>,
    pub snapshot_time: String,
}

/// Single SCADA reading response
#[derive(Debug, Serialize, Deserialize)]
pub struct ScadaReadingResponse {
    pub element_id: u64,
    pub parameter: String,
    pub value: f64,
    pub quality: String,
}

// ============================================================
// Analysis — OPF types
// ============================================================

/// OPF request for POST /api/analysis/opf
#[derive(Debug, Serialize, Deserialize)]
pub struct OpfRequest {
    pub generators: Vec<GenBidRequest>,
    pub branches: Vec<BranchLimitRequest>,
    pub loads: Vec<(u64, f64)>,
    pub slack_bus: u64,
}

/// Generator bid in OPF request
#[derive(Debug, Serialize, Deserialize)]
pub struct GenBidRequest {
    pub gen_id: u64,
    pub bus_id: u64,
    pub p_min: f64,
    pub p_max: f64,
    pub cost_a: f64,
    pub cost_b: f64,
    pub cost_c: f64,
}

/// Branch limit in OPF request
#[derive(Debug, Serialize, Deserialize)]
pub struct BranchLimitRequest {
    pub branch_id: u64,
    pub from_bus: u64,
    pub to_bus: u64,
    pub p_limit: f64,
    pub reactance: f64,
}

/// OPF response
#[derive(Debug, Serialize, Deserialize)]
pub struct OpfResponse {
    pub generation: Vec<(u64, f64)>,
    pub total_cost: f64,
    pub nodal_prices: Vec<(u64, f64)>,
    pub converged: bool,
}

// ============================================================
// Analysis — State Estimation types
// ============================================================

/// State estimation request for POST /api/analysis/state-estimation
#[derive(Debug, Serialize, Deserialize)]
pub struct SeRequest {
    pub measurements: Vec<MeasRequest>,
    pub bus_count: usize,
    pub slack_bus: u64,
}

/// Measurement in state estimation request
#[derive(Debug, Serialize, Deserialize)]
pub struct MeasRequest {
    pub meas_type: String,
    pub element_id: u64,
    pub value: f64,
    pub sigma: f64,
}

/// State estimation response
#[derive(Debug, Serialize, Deserialize)]
pub struct SeResponse {
    pub bus_voltages: Vec<(u64, f64, f64)>,
    pub bad_data: Vec<u64>,
    pub converged: bool,
}

// ============================================================
// Analysis — Short Circuit types
// ============================================================

/// Short circuit request for POST /api/analysis/short-circuit
#[derive(Debug, Serialize, Deserialize)]
pub struct ScRequest {
    pub bus_id: u64,
    pub fault_type: String,
    pub fault_impedance_real: f64,
    pub fault_impedance_imag: f64,
}

/// Short circuit response
#[derive(Debug, Serialize, Deserialize)]
pub struct ScResponse {
    pub fault_current_real: f64,
    pub fault_current_imag: f64,
    pub bus_voltages: Vec<(u64, f64, f64)>,
}

// ============================================================
// Dashboard types
// ============================================================

/// Topology SVG response for GET /api/dashboard/topology-svg
#[derive(Debug, Serialize, Deserialize)]
pub struct TopologySvgResponse {
    pub svg: String,
    pub bus_count: usize,
    pub branch_count: usize,
}

/// Flow heatmap response for GET /api/dashboard/flow-heatmap
#[derive(Debug, Serialize, Deserialize)]
pub struct FlowHeatmapResponse {
    pub bus_colors: HashMap<u64, String>,
    pub branch_widths: HashMap<u64, f64>,
    pub branch_colors: HashMap<u64, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_response_success_serialization() {
        let response: ApiResponse<String> = ApiResponse::success("hello".to_string());
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"data\":\"hello\""));
        assert!(json.contains("\"error\":null"));
    }

    #[test]
    fn test_api_response_error_serialization() {
        let response: ApiResponse<String> = ApiResponse::error("something went wrong".to_string());
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"success\":false"));
        assert!(json.contains("\"data\":null"));
        assert!(json.contains("\"error\":\"something went wrong\""));
    }

    #[test]
    fn test_power_flow_request_deserialization() {
        let json = r#"{"case_id":"ieee14","max_iterations":50,"tolerance":1e-6}"#;
        let req: PowerFlowRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.case_id, Some("ieee14".to_string()));
        assert_eq!(req.max_iterations, Some(50));
        assert!((req.tolerance.unwrap() - 1e-6).abs() < f64::EPSILON);
    }

    #[test]
    fn test_power_flow_request_defaults() {
        let json = r#"{}"#;
        let req: PowerFlowRequest = serde_json::from_str(json).unwrap();
        assert!(req.case_id.is_none());
        assert!(req.max_iterations.is_none());
        assert!(req.tolerance.is_none());
    }

    #[test]
    fn test_topology_data_response_serialization() {
        let response = TopologyDataResponse {
            buses: vec![BusData { id: 1, name: "Bus 1".to_string(), zone_id: 0, voltage_kv: 138.0 }],
            branches: vec![BranchData { id: 1, from_bus: 1, to_bus: 2, reactance: 0.1 }],
            zones: vec![0],
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: TopologyDataResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.buses.len(), 1);
        assert_eq!(parsed.branches.len(), 1);
    }

    #[test]
    fn test_agents_response_serialization() {
        let response = AgentsResponse {
            agent_count: 2,
            agents: vec![
                AgentInfo { name: "DispatchAgent".to_string(), agent_type: "Dispatcher".to_string(), authority: "System".to_string(), status: "available".to_string() },
                AgentInfo { name: "OperationAgent".to_string(), agent_type: "Operator".to_string(), authority: "Zone".to_string(), status: "available".to_string() },
            ],
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: AgentsResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_count, 2);
    }

    #[test]
    fn test_scada_latest_response_serialization() {
        let response = ScadaLatestResponse {
            readings: vec![ScadaReadingResponse {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                value: 1.02,
                quality: "Good".to_string(),
            }],
            snapshot_time: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: ScadaLatestResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.readings.len(), 1);
    }

    #[test]
    fn test_opf_request_deserialization() {
        let json = r#"{
            "generators": [{"gen_id":1,"bus_id":1,"p_min":0.0,"p_max":200.0,"cost_a":0.005,"cost_b":10.0,"cost_c":100.0}],
            "branches": [{"branch_id":1,"from_bus":1,"to_bus":2,"p_limit":200.0,"reactance":0.1}],
            "loads": [[3, 100.0]],
            "slack_bus": 1
        }"#;
        let req: OpfRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.generators.len(), 1);
        assert_eq!(req.branches.len(), 1);
        assert_eq!(req.loads.len(), 1);
        assert_eq!(req.slack_bus, 1);
    }

    #[test]
    fn test_se_request_deserialization() {
        let json = r#"{
            "measurements": [{"meas_type":"voltage","element_id":0,"value":1.02,"sigma":0.01}],
            "bus_count": 3,
            "slack_bus": 0
        }"#;
        let req: SeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.measurements.len(), 1);
        assert_eq!(req.bus_count, 3);
    }

    #[test]
    fn test_sc_request_deserialization() {
        let json = r#"{
            "bus_id": 1,
            "fault_type": "3ph",
            "fault_impedance_real": 0.0,
            "fault_impedance_imag": 0.0
        }"#;
        let req: ScRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.bus_id, 1);
        assert_eq!(req.fault_type, "3ph");
    }

    #[test]
    fn test_constraint_violation_response_serialization() {
        let response = ConstraintViolationResponse {
            constraint_id: "v1".to_string(),
            element_id: 1,
            actual_value: 0.9,
            limit_min: 0.95,
            limit_max: 1.05,
            severity: "Major".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: ConstraintViolationResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.constraint_id, "v1");
        assert!((parsed.actual_value - 0.9).abs() < f64::EPSILON);
    }
}
