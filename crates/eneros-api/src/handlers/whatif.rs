//! WhatIf 假设计算 API handler (v0.7.0 — deferred from v0.6.0 S4)。
//!
//! 暴露 `FeasibilityProjector` 的 What-If 分析通过
//! `POST /api/whatif`。客户端提交 `StructuredAction` 并接收
//! `ProjectionResult`，指示动作是否可行、需要投影或完全不可行。
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
use eneros_core::StructuredAction;

// ---------------------------------------------------------------------------
// OpenAPI schema 镜像类型
// ---------------------------------------------------------------------------

/// `StructuredAction` 的 OpenAPI schema 镜像。
///
/// 镜像 `eneros-core::StructuredAction` 的结构用于 OpenAPI 文档生成。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum StructuredActionWhatIfSchema {
    /// 执行设备操作
    ExecuteDevice { device_id: u64, operation: String, value: f64 },
    /// 切除负荷
    ShedLoad { zone_id: u32, amount_mw: f64 },
    /// 启动/调整发电机
    StartGenerator { gen_id: u64, target_mw: f64 },
    /// 通知 Agent
    NotifyAgent { agent_id: String, message: String },
    /// 隔离故障区段
    IsolateFault { upstream_switch: u64, downstream_switch: u64 },
    /// 合上联络开关恢复供电
    CloseTieSwitch { switch_id: u64 },
}

/// 请求体 schema（用于 OpenAPI 文档）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WhatIfRequestSchema {
    /// 待评估的动作
    pub action: StructuredActionWhatIfSchema,
}

/// 响应体 schema（用于 OpenAPI 文档）
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WhatIfResponseSchema {
    /// 动作是否原样可行
    pub feasible: bool,
    /// 动作是否被投影（修改）后可行
    pub projected: bool,
    /// 动作是否完全不可行
    pub infeasible: bool,
    /// 人类可读的摘要
    pub summary: String,
    /// 投影结果详情（JSON）
    pub projection: serde_json::Value,
    /// 分布式追踪 ID（T029-09）
    pub trace_id: String,
}

// ---------------------------------------------------------------------------
// 实际请求/响应类型（用于序列化/反序列化）
// ---------------------------------------------------------------------------

/// `POST /api/whatif` 请求体。
#[derive(Debug, Deserialize)]
pub struct WhatIfRequest {
    /// 待评估的动作。
    pub action: StructuredAction,
}

/// `POST /api/whatif` 响应体。
#[derive(Debug, Serialize)]
pub struct WhatIfResponse {
    /// 动作是否原样可行。
    pub feasible: bool,
    /// 动作是否被投影（修改）后可行。
    pub projected: bool,
    /// 动作是否完全不可行。
    pub infeasible: bool,
    /// 人类可读的摘要。
    pub summary: String,
    /// 投影结果详情（JSON）。
    pub projection: serde_json::Value,
    /// 分布式追踪 ID（T029-09）。
    pub trace_id: String,
}

/// `POST /api/whatif` — 通过 What-If 分析评估动作可行性。
///
/// 调用 `eneros-gateway` 的 `ConstrainedDecisionPipeline::project()` 执行
/// 真实可行性投影，包括设备硬限值裁剪、潮流仿真验证、约束满足检查。
/// 不执行实际动作，仅做 What-If 推演。
///
/// trace_id 从请求扩展中提取（T029-04 中间件注入），贯穿到日志和响应体。
#[utoipa::path(
    post,
    tag = "whatif",
    path = "/api/whatif",
    request_body = WhatIfRequestSchema,
    responses(
        (status = 200, description = "What-If 分析结果", body = WhatIfResponseSchema),
        (status = 503, description = "决策管道未配置"),
    )
)]
pub async fn whatif_handler(
    State(state): State<AppState>,
    Extension(trace_id_ext): Extension<TraceId>,
    Json(req): Json<WhatIfRequest>,
) -> axum::response::Response {
    let trace_id = trace_id_ext.0;

    tracing::info!(
        trace_id = %trace_id,
        action = ?req.action,
        "whatif analysis request received"
    );

    let pipeline = match &state.decision_pipeline {
        Some(p) => p,
        None => {
            tracing::warn!(
                trace_id = %trace_id,
                "decision pipeline not configured"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "decision pipeline not configured",
            )
                .into_response();
        }
    };

    let projection = pipeline.project(&req.action);

    let (feasible, projected, infeasible) = (
        projection.is_feasible(),
        projection.is_projected(),
        projection.is_infeasible(),
    );

    let summary = if feasible {
        "Action is feasible as proposed".to_string()
    } else if projected {
        "Action was projected to the nearest feasible point".to_string()
    } else {
        "Action is infeasible".to_string()
    };

    tracing::info!(
        trace_id = %trace_id,
        feasible,
        projected,
        infeasible,
        "whatif analysis completed"
    );

    let projection_json = serde_json::to_value(&projection).unwrap_or(serde_json::json!({}));

    let response = WhatIfResponse {
        feasible,
        projected,
        infeasible,
        summary,
        projection: projection_json,
        trace_id,
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whatif_response_serialization() {
        let resp = WhatIfResponse {
            feasible: true,
            projected: false,
            infeasible: false,
            summary: "Action is feasible".to_string(),
            projection: serde_json::json!({"status": "feasible"}),
            trace_id: "test-trace-id".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"feasible\":true"));
        assert!(json.contains("\"infeasible\":false"));
        assert!(json.contains("\"trace_id\":\"test-trace-id\""));
    }
}
