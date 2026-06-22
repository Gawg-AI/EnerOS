//! 配电网规划评估 API handler (v0.7.0 — deferred from v0.6.0 S4)。
//!
//! 暴露 `eneros-analysis` 的 `PlanningEvaluator` 通过
//! `POST /api/planning/evaluate`。客户端提交供电区分类、电压等级、
//! 当前运行指标，并接收候选规划方案和限值信息。
//!
//! T029-09: 集成 trace_id（从请求扩展获取），贯穿到日志和响应体。
//! T029-09: 完善 OpenAPI 文档（request_body + response body schema）。

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::app::AppState;
use crate::middleware::TraceId;
use eneros_runtime::analysis::planning::{
    CandidatePlan, LoadingLimits, PlanningEvaluator, SupplyRadius, SupplyAreaClass, VoltageLimits,
};

// ---------------------------------------------------------------------------
// OpenAPI schema 镜像类型
// ---------------------------------------------------------------------------

/// `SupplyAreaClass` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum SupplyAreaClassSchema {
    /// A类：中心城区，高可靠性要求
    A,
    /// B类：一般城区
    B,
    /// C类：郊区
    C,
    /// D类：农村地区
    D,
    /// E类：偏远地区
    E,
}

/// `VoltageLimits` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VoltageLimitsSchema {
    /// 额定电压 (kV)
    pub rated_voltage_kv: f64,
    /// 正偏差限值 (%)
    pub positive_deviation_percent: f64,
    /// 负偏差限值 (%)
    pub negative_deviation_percent: f64,
}

/// `LoadingLimits` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LoadingLimitsSchema {
    /// 正常运行负载率限值 (%)
    pub normal_percent: f64,
    /// 经济运行负载率范围 (min%, max%)
    pub economic_range: (f64, f64),
    /// N-1 事故短时负载率限值 (%)
    pub n1_emergency_percent: f64,
    /// 紧急负载率限值 (%)
    pub emergency_percent: f64,
}

/// `SupplyRadius` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SupplyRadiusSchema {
    /// 电缆线路最大供电半径 (km)
    pub cable_max_km: f64,
    /// 架空线路最大供电半径 (km)
    pub overhead_max_km: f64,
}

/// `CandidatePlan` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CandidatePlanSchema {
    /// 候选方案动作类型
    pub action: String,
    /// 方案描述
    pub description: String,
    /// 预估造价（百万元）
    pub estimated_cost_million_cny: f64,
    /// 触发条件
    pub trigger_condition: String,
}

/// 请求体 schema（用于 OpenAPI 文档）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PlanningRequestSchema {
    /// 供电区分类（A/B/C/D/E）
    pub area_class: SupplyAreaClassSchema,
    /// 额定电压等级 (kV)
    pub voltage_kv: f64,
    /// 当前变压器负载率 (%)
    pub current_loading_percent: f64,
    /// 当前电压偏差 (%)
    pub voltage_deviation_percent: f64,
    /// 光伏渗透率 (%)
    #[serde(default)]
    pub pv_penetration_percent: f64,
    /// 电动汽车渗透率 (%)
    #[serde(default)]
    pub ev_penetration_percent: f64,
}

/// 响应体 schema（用于 OpenAPI 文档）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PlanningResponseSchema {
    /// 适用电压限值
    pub voltage_limits: VoltageLimitsSchema,
    /// 适用变压器负载率限值
    pub transformer_loading_limits: LoadingLimitsSchema,
    /// 适用线路负载率限值
    pub line_loading_limits: LoadingLimitsSchema,
    /// 适用供电半径
    pub supply_radius: SupplyRadiusSchema,
    /// 生成的候选方案
    pub candidate_plans: Vec<CandidatePlanSchema>,
    /// N-1 通过率要求 (%)
    pub n1_pass_rate_percent: f64,
    /// 供电可靠性 RS-1 要求 (%)
    pub reliability_rs1_percent: f64,
    /// 分布式追踪 ID（T029-09）
    pub trace_id: String,
}

// ---------------------------------------------------------------------------
// 实际请求/响应类型（用于序列化/反序列化）
// ---------------------------------------------------------------------------

/// `POST /api/planning/evaluate` 请求体。
#[derive(Debug, Deserialize)]
pub struct PlanningRequest {
    /// 供电区分类 (A/B/C/D/E)。
    pub area_class: SupplyAreaClass,
    /// 额定电压等级 (kV)。
    pub voltage_kv: f64,
    /// 当前变压器负载率 (%)。
    pub current_loading_percent: f64,
    /// 当前电压偏差 (%)。
    pub voltage_deviation_percent: f64,
    /// 光伏渗透率 (%)。
    #[serde(default)]
    pub pv_penetration_percent: f64,
    /// 电动汽车渗透率 (%)。
    #[serde(default)]
    pub ev_penetration_percent: f64,
}

/// `POST /api/planning/evaluate` 响应体。
#[derive(Debug, Serialize)]
pub struct PlanningResponse {
    /// 适用电压限值。
    pub voltage_limits: VoltageLimits,
    /// 适用变压器负载率限值。
    pub transformer_loading_limits: LoadingLimits,
    /// 适用线路负载率限值。
    pub line_loading_limits: LoadingLimits,
    /// 适用供电半径。
    pub supply_radius: SupplyRadius,
    /// 生成的候选方案。
    pub candidate_plans: Vec<CandidatePlan>,
    /// N-1 通过率要求 (%)。
    pub n1_pass_rate_percent: f64,
    /// 供电可靠性 RS-1 要求 (%)。
    pub reliability_rs1_percent: f64,
    /// 分布式追踪 ID（T029-09）。
    pub trace_id: String,
}

/// `POST /api/planning/evaluate` — 配电网规划评估。
///
/// 调用 `eneros-analysis` 的 `PlanningEvaluator` 执行真实规划评估，
/// 基于 GB/T 12325（电压偏差）、DL/T 5729（供电区分类）、
/// GB/T 38306（N-1 安全）等标准生成候选规划方案。
///
/// trace_id 从请求扩展中提取（T029-04 中间件注入），贯穿到日志和响应体。
#[utoipa::path(
    post,
    tag = "planning",
    path = "/api/planning/evaluate",
    request_body = PlanningRequestSchema,
    responses(
        (status = 200, description = "配电网规划评估结果", body = PlanningResponseSchema),
        (status = 400, description = "请求参数错误"),
    )
)]
pub async fn evaluate_handler(
    State(_state): State<AppState>,
    Extension(trace_id_ext): Extension<TraceId>,
    Json(req): Json<PlanningRequest>,
) -> axum::response::Response {
    let trace_id = trace_id_ext.0;

    tracing::info!(
        trace_id = %trace_id,
        area_class = ?req.area_class,
        voltage_kv = req.voltage_kv,
        current_loading_percent = req.current_loading_percent,
        "planning evaluate request received"
    );

    let evaluator = PlanningEvaluator::new(req.area_class, req.voltage_kv);

    let voltage_limits = evaluator.voltage_limits();
    let transformer_loading_limits = evaluator.transformer_loading_limits();
    let line_loading_limits = evaluator.line_loading_limits();
    let supply_radius = evaluator.supply_radius();

    let candidate_plans = evaluator.generate_candidates(
        req.current_loading_percent,
        req.voltage_deviation_percent,
        req.pv_penetration_percent,
        req.ev_penetration_percent,
    );

    let n1_pass_rate_percent = req.area_class.n1_pass_rate_percent();
    let reliability_rs1_percent = req.area_class.reliability_rs1_percent();

    tracing::info!(
        trace_id = %trace_id,
        candidate_count = candidate_plans.len(),
        n1_pass_rate_percent,
        "planning evaluate completed"
    );

    let response = PlanningResponse {
        voltage_limits,
        transformer_loading_limits,
        line_loading_limits,
        supply_radius,
        candidate_plans,
        n1_pass_rate_percent,
        reliability_rs1_percent,
        trace_id,
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planning_request_deserialize() {
        let json = r#"{
            "area_class": "A",
            "voltage_kv": 10.0,
            "current_loading_percent": 75.0,
            "voltage_deviation_percent": 3.0,
            "pv_penetration_percent": 20.0,
            "ev_penetration_percent": 5.0
        }"#;
        let req: PlanningRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.area_class, SupplyAreaClass::A);
        assert_eq!(req.voltage_kv, 10.0);
        assert_eq!(req.current_loading_percent, 75.0);
    }

    #[test]
    fn test_planning_request_defaults() {
        let json = r#"{
            "area_class": "B",
            "voltage_kv": 35.0,
            "current_loading_percent": 50.0,
            "voltage_deviation_percent": 2.0
        }"#;
        let req: PlanningRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.pv_penetration_percent, 0.0);
        assert_eq!(req.ev_penetration_percent, 0.0);
    }
}
