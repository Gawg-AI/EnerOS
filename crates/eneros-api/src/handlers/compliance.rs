//! 设备合规检查 API handler (v0.7.0 — deferred from v0.6.0 S4)。
//!
//! 暴露 `eneros-constraint` 的 `ComplianceChecker` 通过
//! `POST /api/compliance/check`。客户端提交设备规格和运行工况，
//! 并接收 `ComplianceFinding` 列表。
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
use eneros_runtime::constraint::compliance::{
    ComplianceChecker, ComplianceFinding, ComplianceStatus, EquipmentSpec, OperatingConditions,
};

// ---------------------------------------------------------------------------
// OpenAPI schema 镜像类型
// ---------------------------------------------------------------------------

/// `ComplianceStatus` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum ComplianceStatusSchema {
    /// 设备/工况符合标准
    Passed,
    /// 设备/工况违反标准
    Failed(String),
    /// 缺少必要数据，无法判定
    Inconclusive(String),
}

/// `ComplianceFinding` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ComplianceFindingSchema {
    /// 规则标识（如 "TR2_LOAD_001"）
    pub rule_id: String,
    /// 规则描述
    pub description: String,
    /// 引用的国标（如 "GB/T 6451-2023"）
    pub standard: String,
    /// 检查结果
    pub status: ComplianceStatusSchema,
    /// 测量值（若可用）
    pub measured_value: Option<f64>,
    /// 限值（若适用）
    pub limit_value: Option<f64>,
    /// 测量/限值单位
    pub unit: String,
}

/// 请求体 schema（用于 OpenAPI 文档）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ComplianceRequestSchema {
    /// 设备规格（JSON 对象，结构同 `EquipmentSpec`）
    pub spec: serde_json::Value,
    /// 当前运行工况（JSON 对象，结构同 `OperatingConditions`）
    pub operating: serde_json::Value,
}

/// 响应体 schema（用于 OpenAPI 文档）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ComplianceResponseSchema {
    /// 所有合规检查发现
    pub findings: Vec<ComplianceFindingSchema>,
    /// 是否全部检查通过
    pub all_passed: bool,
    /// 失败检查数
    pub failed_count: usize,
    /// 不可判定检查数
    pub inconclusive_count: usize,
    /// 分布式追踪 ID（T029-09）
    pub trace_id: String,
}

// ---------------------------------------------------------------------------
// 实际请求/响应类型（用于序列化/反序列化）
// ---------------------------------------------------------------------------

/// `POST /api/compliance/check` 请求体。
#[derive(Debug, Deserialize)]
pub struct ComplianceRequest {
    /// 设备规格。
    pub spec: EquipmentSpec,
    /// 当前运行工况。
    pub operating: OperatingConditions,
}

/// `POST /api/compliance/check` 响应体。
#[derive(Debug, Serialize)]
pub struct ComplianceResponse {
    /// 所有合规检查发现。
    pub findings: Vec<ComplianceFinding>,
    /// 是否全部检查通过。
    pub all_passed: bool,
    /// 失败检查数。
    pub failed_count: usize,
    /// 不可判定检查数。
    pub inconclusive_count: usize,
    /// 分布式追踪 ID（T029-09）。
    pub trace_id: String,
}

/// `POST /api/compliance/check` — 检查设备合规性（基于 GB/T 标准）。
///
/// 调用 `eneros-constraint` 的 `ComplianceChecker` 执行真实合规检查，
/// 包括变压器负载（GB/T 6451）、变压器热特性（GB/T 1094.7）、
/// 电压偏差（GB/T 12325）、电缆载流量（GB/T 12706）、
/// 断路器开断容量（GB/T 1984）等。
///
/// trace_id 从请求扩展中提取（T029-04 中间件注入），贯穿到日志和响应体。
#[utoipa::path(
    post,
    tag = "compliance",
    path = "/api/compliance/check",
    request_body = ComplianceRequestSchema,
    responses(
        (status = 200, description = "合规检查结果", body = ComplianceResponseSchema),
        (status = 400, description = "请求参数错误"),
    )
)]
pub async fn check_handler(
    State(_state): State<AppState>,
    Extension(trace_id_ext): Extension<TraceId>,
    Json(req): Json<ComplianceRequest>,
) -> axum::response::Response {
    let trace_id = trace_id_ext.0;

    tracing::info!(
        trace_id = %trace_id,
        equipment_type = %req.spec.equipment_type,
        "compliance check request received"
    );

    let findings = ComplianceChecker::check_all(&req.spec, &req.operating);

    let failed_count = findings
        .iter()
        .filter(|f| matches!(f.status, ComplianceStatus::Failed(_)))
        .count();

    let inconclusive_count = findings
        .iter()
        .filter(|f| matches!(f.status, ComplianceStatus::Inconclusive(_)))
        .count();

    let all_passed = failed_count == 0 && inconclusive_count == 0;

    tracing::info!(
        trace_id = %trace_id,
        total_findings = findings.len(),
        failed_count,
        inconclusive_count,
        all_passed,
        "compliance check completed"
    );

    let response = ComplianceResponse {
        findings,
        all_passed,
        failed_count,
        inconclusive_count,
        trace_id,
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compliance_request_deserialize() {
        let json = r#"{
            "spec": {
                "equipment_type": "transformer",
                "rated_capacity": 10.0,
                "rated_voltage_kv": 35.0,
                "normal_loading_limit_percent": 85.0
            },
            "operating": {
                "loading_percent": 75.0,
                "voltage_pu": 1.0
            }
        }"#;
        let req: ComplianceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.spec.equipment_type, "transformer");
        assert_eq!(req.spec.rated_capacity, Some(10.0));
        assert_eq!(req.operating.loading_percent, Some(75.0));
    }
}
