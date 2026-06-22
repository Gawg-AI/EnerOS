//! 系统级校验 API handler (v0.7.0 — deferred from v0.6.0 S4)。
//!
//! 暴露 `eneros-constraint` 的 `ValidationRuleEngine` 通过
//! `POST /api/validation/check`。客户端提交 `SystemStateSnapshot`
//! 并接收 `ValidationFinding` 列表。
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
use eneros_runtime::constraint::validation_rules::{
    SystemStateSnapshot, ValidationFinding, ValidationRuleEngine, ValidationSummary,
};

// ---------------------------------------------------------------------------
// OpenAPI schema 镜像类型
// ---------------------------------------------------------------------------

/// `ValidationStatus` 的 OpenAPI schema 镜像。
///
/// `eneros-constraint` 中的 `ValidationStatus` 未派生 `ToSchema`，
/// 此处镜像其结构用于 OpenAPI 文档生成。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum ValidationStatusSchema {
    /// 校验通过
    Passed,
    /// 校验失败
    Failed { detail: String },
    /// 数据不足，无法判定
    Inconclusive { detail: String },
}

/// `ValidationFinding` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ValidationFindingSchema {
    /// 规则标识（如 "VQ_DEV_001"）
    pub rule_id: String,
    /// 规则描述
    pub description: String,
    /// 引用的国标
    pub standard: String,
    /// 校验结果
    pub status: ValidationStatusSchema,
    /// 测量值（若可用）
    pub measured_value: Option<f64>,
    /// 限值（若适用）
    pub limit_value: Option<f64>,
    /// 测量/限值单位
    pub unit: String,
}

/// `ValidationSummary` 的 OpenAPI schema 镜像。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ValidationSummarySchema {
    /// 通过数
    pub passed: usize,
    /// 失败数
    pub failed: usize,
    /// 不可判定数
    pub inconclusive: usize,
}

/// 请求体 schema（用于 OpenAPI 文档）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ValidationRequestSchema {
    /// 系统状态快照（JSON 对象，结构同 `SystemStateSnapshot`）
    pub state: serde_json::Value,
    /// 可选：仅运行指定检查（如 ["voltage", "frequency"]）。
    /// 省略则运行全部检查。
    #[serde(default)]
    pub checks: Vec<String>,
}

/// 响应体 schema（用于 OpenAPI 文档）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ValidationResponseSchema {
    /// 所有校验发现
    pub findings: Vec<ValidationFindingSchema>,
    /// 校验结果汇总
    pub summary: ValidationSummarySchema,
    /// 分布式追踪 ID（T029-09）
    pub trace_id: String,
}

// ---------------------------------------------------------------------------
// 实际请求/响应类型（用于序列化/反序列化）
// ---------------------------------------------------------------------------

/// `POST /api/validation/check` 请求体。
#[derive(Debug, Deserialize)]
pub struct ValidationRequest {
    /// 待校验的系统状态快照。
    pub state: SystemStateSnapshot,
    /// 可选：仅运行特定检查（如 ["voltage", "frequency"]）。
    /// 省略则运行全部检查。
    #[serde(default)]
    pub checks: Vec<String>,
}

/// `POST /api/validation/check` 响应体。
#[derive(Debug, Serialize)]
pub struct ValidationResponse {
    /// 所有校验发现。
    pub findings: Vec<ValidationFinding>,
    /// 校验结果汇总。
    pub summary: ValidationSummary,
    /// 分布式追踪 ID（T029-09）。
    pub trace_id: String,
}

/// `POST /api/validation/check` — 运行系统级校验规则。
///
/// 调用 `eneros-constraint` 的 `ValidationRuleEngine` 执行真实校验逻辑，
/// 包括电压偏差（GB/T 12325）、频率偏差（GB/T 15945）、谐波（GB/T 14549）、
/// 闪变（GB/T 12326）、N-1 安全（GB/T 38306）、短路容量（GB/T 15544）等。
///
/// trace_id 从请求扩展中提取（T029-04 中间件注入），贯穿到日志和响应体。
#[utoipa::path(
    post,
    tag = "validation",
    path = "/api/validation/check",
    request_body = ValidationRequestSchema,
    responses(
        (status = 200, description = "校验结果", body = ValidationResponseSchema),
        (status = 400, description = "请求参数错误"),
    )
)]
pub async fn check_handler(
    State(_state): State<AppState>,
    Extension(trace_id_ext): Extension<TraceId>,
    Json(req): Json<ValidationRequest>,
) -> axum::response::Response {
    let trace_id = trace_id_ext.0;

    tracing::info!(
        trace_id = %trace_id,
        checks = ?req.checks,
        bus_count = req.state.buses.len(),
        "validation check request received"
    );

    let engine = ValidationRuleEngine::new();

    let findings = if req.checks.is_empty() {
        engine.validate_all(&req.state)
    } else {
        let mut all_findings = Vec::new();
        for check in &req.checks {
            let found = match check.as_str() {
                "voltage" => engine.check_voltage_deviation(&req.state),
                "frequency" => engine.check_frequency_deviation(&req.state),
                "harmonics" => engine.check_harmonics(&req.state),
                "flicker" => engine.check_flicker(&req.state),
                "n1" | "n-1" => engine.check_n1_security(&req.state),
                "short_circuit" | "short-circuit" => engine.check_short_circuit_capacity(&req.state),
                "fault_clearing" | "fault-clearing" => engine.check_fault_clearing_time(&req.state),
                _ => Vec::new(),
            };
            all_findings.extend(found);
        }
        all_findings
    };

    let summary = ValidationRuleEngine::summarize(&findings);

    tracing::info!(
        trace_id = %trace_id,
        passed = summary.passed,
        failed = summary.failed,
        inconclusive = summary.inconclusive,
        "validation check completed"
    );

    let response = ValidationResponse {
        findings,
        summary,
        trace_id,
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_request_deserialize() {
        let json = r#"{"state": {"buses": [], "frequency": null, "contingencies": [], "short_circuits": []}, "checks": ["voltage"]}"#;
        let req: ValidationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.checks.len(), 1);
        assert_eq!(req.checks[0], "voltage");
    }

    #[test]
    fn test_validation_request_defaults() {
        let json = r#"{"state": {"buses": [], "frequency": null, "contingencies": [], "short_circuits": []}}"#;
        let req: ValidationRequest = serde_json::from_str(json).unwrap();
        assert!(req.checks.is_empty());
    }
}
