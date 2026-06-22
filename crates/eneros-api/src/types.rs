use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use eneros_core::ElementId;

/// API response wrapper
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PowerFlowRequest {
    pub case_id: Option<String>,
    pub max_iterations: Option<u32>,
    pub tolerance: Option<f64>,
}

/// Power flow response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PowerFlowResponse {
    pub converged: bool,
    pub iterations: u32,
    pub total_losses: f64,
    pub bus_voltages: Vec<BusVoltageResponse>,
    pub branch_flows: Vec<BranchFlowResponse>,
}

/// Bus voltage response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BusVoltageResponse {
    pub bus_id: ElementId,
    pub voltage_magnitude: f64,
    pub voltage_angle: f64,
}

/// Branch flow response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TopologyDataResponse {
    pub buses: Vec<BusData>,
    pub branches: Vec<BranchData>,
    pub zones: Vec<u64>,
}

/// Bus data in topology response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BusData {
    pub id: u64,
    pub name: String,
    pub zone_id: u64,
    pub voltage_kv: f64,
}

/// Branch data in topology response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentsResponse {
    pub agent_count: usize,
    pub agents: Vec<AgentInfo>,
}

/// Agent info in agents response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ScadaLatestResponse {
    pub readings: Vec<ScadaReadingResponse>,
    pub snapshot_time: String,
}

/// Single SCADA reading response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OpfRequest {
    pub generators: Vec<GenBidRequest>,
    pub branches: Vec<BranchLimitRequest>,
    pub loads: Vec<(u64, f64)>,
    pub slack_bus: u64,
}

/// Generator bid in OPF request
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BranchLimitRequest {
    pub branch_id: u64,
    pub from_bus: u64,
    pub to_bus: u64,
    pub p_limit: f64,
    pub reactance: f64,
}

/// OPF response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
    /// Measurements (optional — if omitted and network is available, synthetic measurements are generated)
    #[serde(default)]
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
// Analysis — AC-OPF types (v0.8.0)
// ============================================================

/// AC-OPF request for POST /api/analysis/ac-opf
#[derive(Debug, Serialize, Deserialize)]
pub struct AcOpfRequest {
    /// 求解方法："newton" 或 "interior_point"（默认 newton）
    #[serde(default)]
    pub method: Option<String>,
    /// 可选：自定义发电机（若为空则从已加载网络构建）
    #[serde(default)]
    pub generators: Vec<AcGenRequest>,
    /// 可选：自定义支路（若为空则从已加载网络构建）
    #[serde(default)]
    pub branches: Vec<AcBranchRequest>,
    /// 可选：自定义母线（若为空则从已加载网络构建）
    #[serde(default)]
    pub buses: Vec<AcBusRequest>,
    /// 平衡母线 ID（默认 1）
    #[serde(default)]
    pub slack_bus: Option<u64>,
    /// 系统基准容量（MVA，默认 100）
    #[serde(default)]
    pub base_mva: Option<f64>,
}

/// AC-OPF 发电机请求
#[derive(Debug, Serialize, Deserialize)]
pub struct AcGenRequest {
    pub gen_id: u64,
    pub bus_id: u64,
    pub p_min: f64,
    pub p_max: f64,
    pub q_min: f64,
    pub q_max: f64,
    pub cost_a: f64,
    pub cost_b: f64,
    pub cost_c: f64,
}

/// AC-OPF 支路请求
#[derive(Debug, Serialize, Deserialize)]
pub struct AcBranchRequest {
    pub branch_id: u64,
    pub from_bus: u64,
    pub to_bus: u64,
    pub r_pu: f64,
    pub x_pu: f64,
    pub b_half: f64,
    pub tap_ratio: f64,
    pub s_limit_mva: f64,
}

/// AC-OPF 母线请求
#[derive(Debug, Serialize, Deserialize)]
pub struct AcBusRequest {
    pub bus_id: u64,
    pub p_load: f64,
    pub q_load: f64,
    pub v_min: f64,
    pub v_max: f64,
    pub v_init: f64,
    pub theta_init: f64,
}

/// AC-OPF 响应
#[derive(Debug, Serialize, Deserialize)]
pub struct AcOpfResponse {
    /// (gen_id, p_mw, q_mvar)
    pub generation: Vec<(u64, f64, f64)>,
    /// (bus_id, v_pu, theta_rad)
    pub bus_voltages: Vec<(u64, f64, f64)>,
    /// (branch_id, from_to_mva)
    pub branch_flows: Vec<(u64, f64)>,
    /// (bus_id, lmp_$/mwh)
    pub nodal_prices: Vec<(u64, f64)>,
    pub total_cost: f64,
    pub total_losses: f64,
    pub converged: bool,
    pub iterations: u32,
    pub warnings: Vec<String>,
}

// ============================================================
// Analysis — Transient Stability types (v0.8.0)
// ============================================================

/// 暂态稳定仿真请求 for POST /api/analysis/transient
#[derive(Debug, Serialize, Deserialize)]
pub struct TransientRequest {
    /// 仿真模式："simulate"（单次仿真）、"cct"（CCT 二分搜索）、"equal_area"（等面积法则）
    #[serde(default = "default_transient_mode")]
    pub mode: String,
    /// 发电机动态参数（若为空则使用默认经典模型）
    #[serde(default)]
    pub generators: Vec<GenDynamicRequest>,
    /// 母线 ID 列表（若为空则从网络构建）
    #[serde(default)]
    pub buses: Vec<u64>,
    /// 支路列表 (from, to, r, x, b, tap)（若为空则从网络构建）
    #[serde(default)]
    pub branches: Vec<(u64, u64, f64, f64, f64, f64)>,
    /// 基准容量 MVA（默认 100）
    #[serde(default = "default_base_mva")]
    pub base_mva: f64,
    /// 负荷列表 (bus_id, P_pu, Q_pu)
    #[serde(default)]
    pub loads: Vec<(u64, f64, f64)>,
    /// 故障类型："three_phase" 或 "line_outage"
    #[serde(default = "default_fault_type")]
    pub fault_type: String,
    /// 故障母线 ID（三相短路）
    #[serde(default)]
    pub fault_bus: Option<u64>,
    /// 故障阻抗（p.u.，三相短路）
    #[serde(default)]
    pub fault_impedance: Option<f64>,
    /// 故障支路 ID（断线）
    #[serde(default)]
    pub fault_branch: Option<u64>,
    /// 仿真参数
    #[serde(default)]
    pub params: Option<TransientParamsRequest>,
    /// CCT 搜索下限 (s)
    #[serde(default = "default_cct_min")]
    pub cct_min: f64,
    /// CCT 搜索上限 (s)
    #[serde(default = "default_cct_max")]
    pub cct_max: f64,
    /// CCT 搜索容差 (s)
    #[serde(default = "default_cct_tol")]
    pub cct_tolerance: f64,
    /// 等面积法则参数（单机无穷大系统）
    #[serde(default)]
    pub equal_area: Option<EqualAreaRequest>,
}

fn default_transient_mode() -> String { "simulate".to_string() }
fn default_base_mva() -> f64 { 100.0 }
fn default_fault_type() -> String { "three_phase".to_string() }
fn default_cct_min() -> f64 { 0.05 }
fn default_cct_max() -> f64 { 0.50 }
fn default_cct_tol() -> f64 { 0.005 }

/// 发电机动态参数请求
#[derive(Debug, Serialize, Deserialize)]
pub struct GenDynamicRequest {
    pub gen_id: u64,
    pub bus_id: u64,
    /// 模型："classical" 或 "fourth_order"
    #[serde(default = "default_gen_model")]
    pub model: String,
    pub h: f64,
    #[serde(default)]
    pub d: f64,
    pub xd_prime: f64,
    #[serde(default)]
    pub xd: f64,
    #[serde(default)]
    pub efd: f64,
    pub pm: f64,
    #[serde(default)]
    pub ka: f64,
    #[serde(default)]
    pub ta: f64,
}

fn default_gen_model() -> String { "classical".to_string() }

/// 仿真参数请求
#[derive(Debug, Serialize, Deserialize)]
pub struct TransientParamsRequest {
    #[serde(default = "default_t_start")]
    pub t_start: f64,
    #[serde(default = "default_t_end")]
    pub t_end: f64,
    #[serde(default = "default_dt")]
    pub dt: f64,
    #[serde(default = "default_t_fault")]
    pub t_fault: f64,
    #[serde(default = "default_t_clear")]
    pub t_clear: f64,
    /// "rk4" 或 "implicit_trapezoidal"
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default = "default_frequency")]
    pub frequency: f64,
}

fn default_t_start() -> f64 { 0.0 }
fn default_t_end() -> f64 { 2.0 }
fn default_dt() -> f64 { 0.01 }
fn default_t_fault() -> f64 { 0.1 }
fn default_t_clear() -> f64 { 0.2 }
fn default_method() -> String { "rk4".to_string() }
fn default_frequency() -> f64 { 50.0 }

/// 等面积法则参数请求
#[derive(Debug, Serialize, Deserialize)]
pub struct EqualAreaRequest {
    /// 无穷大母线电压 (p.u.)
    pub v_inf: f64,
    /// 故障前总电抗 (p.u.)
    pub x_pre_fault: f64,
    /// 故障期间总电抗 (p.u.)
    pub x_fault: f64,
    /// 故障后总电抗 (p.u.)
    pub x_post_fault: f64,
    /// 系统频率 (Hz)
    #[serde(default = "default_frequency")]
    pub frequency: f64,
}

/// 暂态稳定仿真响应
#[derive(Debug, Serialize, Deserialize)]
pub struct TransientResponse {
    /// 仿真模式
    pub mode: String,
    /// 是否稳定
    pub stable: bool,
    /// 最大功角差 (度)
    pub max_angle_spread_deg: f64,
    /// 时间序列（仅 simulate 模式）
    pub time_series: Vec<TimeStepResponse>,
    /// 警告信息
    pub warnings: Vec<String>,
    /// CCT 结果（仅 cct 模式）
    pub cct: Option<CctResponse>,
    /// 等面积法则结果（仅 equal_area 模式）
    pub equal_area: Option<EqualAreaResponse>,
}

/// 单个时间步结果
#[derive(Debug, Serialize, Deserialize)]
pub struct TimeStepResponse {
    pub t: f64,
    pub rotor_angles: Vec<(u64, f64)>,
    pub rotor_speeds: Vec<(u64, f64)>,
    pub bus_voltages: Vec<(u64, f64)>,
}

/// CCT 结果
#[derive(Debug, Serialize, Deserialize)]
pub struct CctResponse {
    pub cct: f64,
    pub tolerance: f64,
    pub iterations: u32,
    pub max_angle_spread_at_cct_deg: f64,
}

/// 等面积法则结果
#[derive(Debug, Serialize, Deserialize)]
pub struct EqualAreaResponse {
    pub delta_0: f64,
    pub delta_c_critical: f64,
    pub delta_max: f64,
    pub a_accel: f64,
    pub a_decel: f64,
    pub cct: f64,
    pub stable: bool,
    pub pmax_pre: f64,
    pub pmax_fault: f64,
    pub pmax_post: f64,
}

// ============================================================
// Analysis — Observability types (v0.8.0)
// ============================================================

/// 可观测性分析请求 for POST /api/analysis/observability
#[derive(Debug, Serialize, Deserialize)]
pub struct ObservabilityRequest {
    /// 分析方法："numerical" 或 "topological"（默认 numerical）
    #[serde(default = "default_obs_method")]
    pub method: String,
    /// 测量列表（若为空则从网络潮流结果生成合成测量）
    #[serde(default)]
    pub measurements: Vec<MeasRequest>,
    /// 平衡母线 ID（默认 1）
    #[serde(default)]
    pub slack_bus: Option<u64>,
    /// 是否计算最小 PMU 配置
    #[serde(default)]
    pub compute_pmu_placement: bool,
    /// 已有 PMU 母线列表
    #[serde(default)]
    pub existing_pmu_buses: Vec<u64>,
}

fn default_obs_method() -> String { "numerical".to_string() }

/// 可观测性分析响应
#[derive(Debug, Serialize, Deserialize)]
pub struct ObservabilityResponse {
    pub observable: bool,
    pub observable_buses: Vec<u64>,
    pub unobservable_buses: Vec<u64>,
    pub observable_islands: Vec<Vec<u64>>,
    pub jacobian_rank: usize,
    pub state_dimension: usize,
    pub missing_measurements: Vec<MissingMeasResponse>,
    pub method: String,
    pub pmu_placement: Option<PmuPlacementResponse>,
}

/// 缺失测量建议响应
#[derive(Debug, Serialize, Deserialize)]
pub struct MissingMeasResponse {
    pub bus_id: u64,
    pub suggested_measurement: String,
    pub reason: String,
}

/// PMU 配置响应
#[derive(Debug, Serialize, Deserialize)]
pub struct PmuPlacementResponse {
    pub pmu_buses: Vec<u64>,
    pub coverage: f64,
    pub covered_buses: Vec<u64>,
    pub pmu_count: usize,
    pub total_buses: usize,
}

// ============================================================
// Analysis — Bad Data Detection types (v0.8.0)
// ============================================================

/// 不良数据检测请求 for POST /api/analysis/bad-data
#[derive(Debug, Serialize, Deserialize)]
pub struct BadDataRequest {
    /// 测量列表（若为空则从网络潮流结果生成合成测量）
    #[serde(default)]
    pub measurements: Vec<MeasRequest>,
    /// 平衡母线 ID（默认 1）
    #[serde(default)]
    pub slack_bus: Option<u64>,
    /// 归一化残差阈值（默认 3.0）
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    /// 显著性水平（默认 0.05）
    #[serde(default = "default_alpha")]
    pub significance_level: f64,
    /// 是否执行迭代剔除
    #[serde(default = "default_eliminate")]
    pub eliminate: bool,
    /// 最大剔除轮次
    #[serde(default = "default_max_rounds")]
    pub max_elimination_rounds: u32,
}

fn default_threshold() -> f64 { 3.0 }
fn default_alpha() -> f64 { 0.05 }
fn default_eliminate() -> bool { true }
fn default_max_rounds() -> u32 { 10 }

/// 不良数据检测响应
#[derive(Debug, Serialize, Deserialize)]
pub struct BadDataResponse {
    pub has_bad_data: bool,
    pub chi_square_test: ChiSquareTestResponse,
    pub bad_data_items: Vec<BadDataItemResponse>,
    pub topology_errors: Vec<TopologyErrorResponse>,
    pub threshold: f64,
    pub elimination_rounds: u32,
    /// 剔除后的状态估计结果（若执行剔除）
    pub cleaned_bus_voltages: Option<Vec<(u64, f64, f64)>>,
    pub converged: bool,
}

/// χ² 检测结果响应
#[derive(Debug, Serialize, Deserialize)]
pub struct ChiSquareTestResponse {
    pub objective: f64,
    pub degrees_of_freedom: usize,
    pub significance_level: f64,
    pub critical_value: f64,
    pub rejected: bool,
}

/// 不良数据项响应
#[derive(Debug, Serialize, Deserialize)]
pub struct BadDataItemResponse {
    pub meas_type: String,
    pub element_id: u64,
    pub to_element_id: Option<u64>,
    pub measured_value: f64,
    pub estimated_value: f64,
    pub residual: f64,
    pub normalized_residual: f64,
    pub sensitivity: f64,
}

/// 拓扑错误响应
#[derive(Debug, Serialize, Deserialize)]
pub struct TopologyErrorResponse {
    pub from_bus: u64,
    pub to_bus: u64,
    pub error_type: String,
    pub confidence: f64,
    pub evidence_residuals: Vec<(u64, f64)>,
}

// ============================================================
// Analysis — Asymmetric Short Circuit types (v0.8.0)
// ============================================================

/// 不对称短路分析请求 for POST /api/analysis/short-circuit/asymmetric
#[derive(Debug, Serialize, Deserialize)]
pub struct AsymmetricScRequest {
    /// 故障母线 ID
    pub bus_id: u64,
    /// 故障类型："slg"（单相接地）、"ll"（两相短路）、"dlg"（两相接地）
    #[serde(default = "default_asym_fault")]
    pub fault_type: String,
    /// 故障阻抗实部
    #[serde(default)]
    pub fault_impedance_real: f64,
    /// 故障阻抗虚部
    #[serde(default)]
    pub fault_impedance_imag: f64,
    /// 正序网络 Z-bus（可选，若不提供则从网络 Y-bus 构建）
    #[serde(default)]
    pub z_positive: Option<Vec<Vec<(f64, f64)>>>,
    /// 负序网络 Z-bus（可选，若不提供则假设 z2 = z1）
    #[serde(default)]
    pub z_negative: Option<Vec<Vec<(f64, f64)>>>,
    /// 零序网络 Z-bus（可选，若不提供则使用典型零序阻抗）
    #[serde(default)]
    pub z_zero: Option<Vec<Vec<(f64, f64)>>>,
}

fn default_asym_fault() -> String { "slg".to_string() }

/// 不对称短路分析响应
#[derive(Debug, Serialize, Deserialize)]
pub struct AsymmetricScResponse {
    pub fault_current_real: f64,
    pub fault_current_imag: f64,
    pub fault_current_magnitude_ka: f64,
    pub bus_voltages: Vec<(u64, f64, f64)>,
    pub branch_currents: Vec<(u64, f64, f64)>,
    pub fault_type: String,
    pub method: String,
}

// ============================================================
// Dashboard types
// ============================================================

/// Topology SVG response for GET /api/dashboard/topology-svg
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TopologySvgResponse {
    pub svg: String,
    pub bus_count: usize,
    pub branch_count: usize,
}

/// Flow heatmap response for GET /api/dashboard/flow-heatmap
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
