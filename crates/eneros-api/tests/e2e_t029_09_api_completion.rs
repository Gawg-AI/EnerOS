//! T029-09: 校验/合规/规划/WhatIf/审计 API 补全集成测试
//!
//! 测试 5 个 API 端点的成功和错误场景：
//! - `POST /api/validation/check` — 系统级校验
//! - `POST /api/compliance/check` — 设备合规检查
//! - `POST /api/planning/evaluate` — 配电网规划评估
//! - `POST /api/whatif` — What-If 推演
//! - `GET /api/audit` — 审计日志查询
//!
//! 这些测试使用真实的底层 crate（非 mock），验证：
//! - 真实业务逻辑被调用（ValidationRuleEngine、ComplianceChecker 等）
//! - trace_id 集成（响应体包含 trace_id）
//! - HTTP 状态码和响应体格式正确
//! - OpenAPI 文档包含所有 5 个端点

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use eneros_api::app::{create_router, AppState};
use eneros_api::audit::{AuditEntry, AuditLog};
use eneros_api::OpenApiDoc;
use eneros_core::StructuredAction;
use eneros_runtime::constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_runtime::constraint::ConstraintEngine;
use eneros_runtime::gateway::constraint_validator::ConstraintAwareValidator;
use eneros_runtime::gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_runtime::gateway::gateway::SafetyGateway;
use utoipa::OpenApi;

// ---------------------------------------------------------------------------
// 测试辅助函数
// ---------------------------------------------------------------------------

/// 提取响应体为 JSON
async fn response_to_json(body: Body) -> serde_json::Value {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// 发送 POST 请求并返回 (StatusCode, 响应体 JSON)
async fn send_post_request(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let json = response_to_json(response.into_body()).await;
    (status, json)
}

/// 发送 GET 请求并返回 (StatusCode, 响应体 JSON)
async fn send_get_request(
    app: axum::Router,
    uri: &str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let json = response_to_json(response.into_body()).await;
    (status, json)
}

/// 构建一个简单的 NetworkSimulator 用于 WhatIf 测试
struct TestSimulator {
    always_feasible: bool,
}

impl NetworkSimulator for TestSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: self.always_feasible,
            summary: if self.always_feasible {
                "All constraints satisfied".to_string()
            } else {
                "Constraint violation".to_string()
            },
        }
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0)]
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.0)]
    }
}

/// 构建带决策管道的 AppState（用于 WhatIf 测试）
fn build_state_with_pipeline() -> AppState {
    let simulator = Arc::new(TestSimulator { always_feasible: true });
    let projector = Arc::new(FeasibilityProjector::new(simulator));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let safety_gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        safety_gateway.clone(),
    ));
    let pipeline = Arc::new(ConstrainedDecisionPipeline::new(
        projector,
        validator,
        safety_gateway,
    ));
    AppState::new().with_decision_pipeline(pipeline)
}

/// 构建带审计日志的 AppState（用于审计查询测试）
fn build_state_with_audit_log() -> AppState {
    let audit_log = Arc::new(AuditLog::new(1000));
    // 预填充一些测试数据
    audit_log.record(AuditEntry::new(
        "alice",
        "operator",
        "POST",
        "/api/actions/structured",
        "192.168.1.1",
        "success",
    ));
    audit_log.record(AuditEntry::new(
        "bob",
        "observer",
        "GET",
        "/api/agents",
        "10.0.0.1",
        "success",
    ));
    audit_log.record(AuditEntry::new(
        "alice",
        "operator",
        "POST",
        "/api/actions/structured",
        "192.168.1.1",
        "failed",
    ).with_detail("constraint violation"));
    AppState::new().with_audit_log(audit_log)
}

// ===========================================================================
// 端点 1: POST /api/validation/check — 校验电网模型
// ===========================================================================

#[tokio::test]
async fn test_validation_check_success() {
    let state = AppState::new();
    let app = create_router(state);

    // 提交一个包含电压偏差超标的系统状态快照
    let body = serde_json::json!({
        "state": {
            "buses": [
                {
                    "bus_id": "B1",
                    "nominal_kv": 10.0,
                    "measured_kv": 10.5,
                    "thd_percent": 3.0,
                    "plt": 0.5
                }
            ],
            "frequency": {
                "nominal_hz": 50.0,
                "measured_hz": 50.1
            },
            "contingencies": [],
            "short_circuits": []
        },
        "checks": ["voltage", "frequency"]
    });

    let (status, json) = send_post_request(app, "/api/validation/check", body).await;

    assert_eq!(status, StatusCode::OK);
    // 应包含 findings 数组
    assert!(json["findings"].is_array(), "findings 应为数组");
    // 应包含 summary 对象
    assert!(json["summary"].is_object(), "summary 应为对象");
    // 应包含 trace_id
    assert!(
        json["trace_id"].is_string(),
        "trace_id 应为字符串: {}",
        json
    );
    // 电压和频率检查都应通过
    assert!(
        json["summary"]["passed"].as_u64() >= Some(2),
        "至少 2 项检查通过: {}",
        json["summary"]
    );
    assert_eq!(
        json["summary"]["failed"].as_u64(),
        Some(0),
        "无失败项: {}",
        json["summary"]
    );
}

#[tokio::test]
async fn test_validation_check_voltage_failure() {
    let state = AppState::new();
    let app = create_router(state);

    // 提交一个电压偏差超标的系统状态（11.5kV vs 10kV 标称 = 15% 偏差 > 7% 限值）
    let body = serde_json::json!({
        "state": {
            "buses": [
                {
                    "bus_id": "B1",
                    "nominal_kv": 10.0,
                    "measured_kv": 11.5
                }
            ],
            "frequency": null,
            "contingencies": [],
            "short_circuits": []
        },
        "checks": ["voltage"]
    });

    let (status, json) = send_post_request(app, "/api/validation/check", body).await;

    assert_eq!(status, StatusCode::OK);
    assert!(json["findings"].is_array());
    assert_eq!(
        json["findings"].as_array().unwrap().len(),
        1,
        "应有 1 条电压校验结果"
    );
    // 应有 1 个失败
    assert_eq!(
        json["summary"]["failed"].as_u64(),
        Some(1),
        "应有 1 项失败: {}",
        json["summary"]
    );
    // trace_id 应存在
    assert!(json["trace_id"].is_string());
}

#[tokio::test]
async fn test_validation_check_all_rules() {
    let state = AppState::new();
    let app = create_router(state);

    // 提交一个包含所有类型观测的系统状态，不指定 checks（运行全部）
    let body = serde_json::json!({
        "state": {
            "buses": [
                {
                    "bus_id": "B1",
                    "nominal_kv": 10.0,
                    "measured_kv": 10.3,
                    "thd_percent": 3.0,
                    "plt": 0.5
                }
            ],
            "frequency": {
                "nominal_hz": 50.0,
                "measured_hz": 50.1
            },
            "contingencies": [
                {
                    "branch_id": "L1",
                    "max_voltage_deviation_pu": 0.05,
                    "max_loading_percent": 80.0,
                    "bus_collapse": false
                }
            ],
            "short_circuits": [
                {
                    "bus_id": "B1",
                    "nominal_kv": 10.0,
                    "ik_3ph_ka": 20.0,
                    "breaker_capacity_ka": 25.0,
                    "fault_clearing_time_s": 0.15
                }
            ]
        }
    });

    let (status, json) = send_post_request(app, "/api/validation/check", body).await;

    assert_eq!(status, StatusCode::OK);
    // 应运行全部 7 类规则：电压 + 频率 + 谐波 + 闪变 + N-1 + 短路容量 + 故障清除时间
    let findings = json["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 7, "应运行全部 7 类规则");
    // 全部通过
    assert_eq!(
        json["summary"]["passed"].as_u64(),
        Some(7),
        "全部 7 项应通过: {}",
        json["summary"]
    );
}

// ===========================================================================
// 端点 2: POST /api/compliance/check — 合规检查
// ===========================================================================

#[tokio::test]
async fn test_compliance_check_transformer_pass() {
    let state = AppState::new();
    let app = create_router(state);

    // 变压器在正常负载下应通过合规检查
    let body = serde_json::json!({
        "spec": {
            "equipment_type": "transformer",
            "rated_capacity": 10.0,
            "rated_voltage_kv": 10.0,
            "normal_loading_limit_percent": 85.0,
            "max_temp_c": 140.0
        },
        "operating": {
            "loading_percent": 70.0,
            "ambient_temp_c": 35.0,
            "voltage_pu": 1.03
        }
    });

    let (status, json) = send_post_request(app, "/api/compliance/check", body).await;

    assert_eq!(status, StatusCode::OK);
    assert!(json["findings"].is_array(), "findings 应为数组");
    // 变压器应检查：负载 + 热特性 + 电压偏差 = 3 项
    assert_eq!(
        json["findings"].as_array().unwrap().len(),
        3,
        "变压器应有 3 项检查"
    );
    assert!(
        json["all_passed"].as_bool() == Some(true),
        "应全部通过: {}",
        json
    );
    assert_eq!(
        json["failed_count"].as_u64(),
        Some(0),
        "无失败项: {}",
        json
    );
    assert!(json["trace_id"].is_string(), "trace_id 应存在");
}

#[tokio::test]
async fn test_compliance_check_transformer_loading_fail() {
    let state = AppState::new();
    let app = create_router(state);

    // 变压器过载：95% > 85% 限值
    let body = serde_json::json!({
        "spec": {
            "equipment_type": "transformer",
            "rated_capacity": 10.0,
            "rated_voltage_kv": 10.0,
            "normal_loading_limit_percent": 85.0
        },
        "operating": {
            "loading_percent": 95.0,
            "voltage_pu": 1.0
        }
    });

    let (status, json) = send_post_request(app, "/api/compliance/check", body).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        json["all_passed"].as_bool() == Some(false),
        "应不全部通过: {}",
        json
    );
    assert!(
        json["failed_count"].as_u64() >= Some(1),
        "至少 1 项失败: {}",
        json["failed_count"]
    );
    assert!(json["trace_id"].is_string());
}

#[tokio::test]
async fn test_compliance_check_breaker() {
    let state = AppState::new();
    let app = create_router(state);

    // 断路器开断容量检查：故障电流 20kA < 额定开断 25kA → 通过
    let body = serde_json::json!({
        "spec": {
            "equipment_type": "switchgear",
            "rated_voltage_kv": 10.0,
            "rated_breaking_current_ka": 25.0
        },
        "operating": {
            "short_circuit_current_ka": 20.0,
            "voltage_pu": 1.0
        }
    });

    let (status, json) = send_post_request(app, "/api/compliance/check", body).await;

    assert_eq!(status, StatusCode::OK);
    // 断路器应检查：开断容量 + 电压偏差 = 2 项
    assert_eq!(
        json["findings"].as_array().unwrap().len(),
        2,
        "断路器应有 2 项检查"
    );
    assert!(
        json["all_passed"].as_bool() == Some(true),
        "应全部通过: {}",
        json
    );
}

// ===========================================================================
// 端点 3: POST /api/planning/evaluate — 规划评估
// ===========================================================================

#[tokio::test]
async fn test_planning_evaluate_normal_operation() {
    let state = AppState::new();
    let app = create_router(state);

    // 正常运行工况：无候选方案
    let body = serde_json::json!({
        "area_class": "B",
        "voltage_kv": 10.0,
        "current_loading_percent": 50.0,
        "voltage_deviation_percent": 3.0,
        "pv_penetration_percent": 10.0,
        "ev_penetration_percent": 5.0
    });

    let (status, json) = send_post_request(app, "/api/planning/evaluate", body).await;

    assert_eq!(status, StatusCode::OK);
    // 应返回电压限值
    assert!(json["voltage_limits"].is_object(), "voltage_limits 应为对象");
    assert_eq!(
        json["voltage_limits"]["rated_voltage_kv"].as_f64(),
        Some(10.0)
    );
    // 应返回变压器负载率限值
    assert!(
        json["transformer_loading_limits"].is_object(),
        "transformer_loading_limits 应为对象"
    );
    // 应返回线路负载率限值
    assert!(
        json["line_loading_limits"].is_object(),
        "line_loading_limits 应为对象"
    );
    // 应返回供电半径
    assert!(json["supply_radius"].is_object(), "supply_radius 应为对象");
    // 正常运行 → 无候选方案
    assert_eq!(
        json["candidate_plans"].as_array().unwrap().len(),
        0,
        "正常运行应无候选方案"
    );
    // N-1 通过率要求
    assert_eq!(
        json["n1_pass_rate_percent"].as_f64(),
        Some(100.0),
        "B 类区域 N-1 通过率要求 100%"
    );
    // trace_id
    assert!(json["trace_id"].is_string());
}

#[tokio::test]
async fn test_planning_evaluate_with_candidates() {
    let state = AppState::new();
    let app = create_router(state);

    // 高负载 + 电压偏差 + 高光伏 + 高 EV → 4 个候选方案
    let body = serde_json::json!({
        "area_class": "B",
        "voltage_kv": 10.0,
        "current_loading_percent": 85.0,
        "voltage_deviation_percent": 8.0,
        "pv_penetration_percent": 30.0,
        "ev_penetration_percent": 20.0
    });

    let (status, json) = send_post_request(app, "/api/planning/evaluate", body).await;

    assert_eq!(status, StatusCode::OK);
    let plans = json["candidate_plans"].as_array().unwrap();
    assert_eq!(plans.len(), 4, "应有 4 个候选方案: {}", plans.len());
    // 应包含变压器增容方案
    assert!(
        plans.iter().any(|p| p["action"].as_str().unwrap_or("").contains("TransformerUpgrade")),
        "应包含变压器增容方案"
    );
    // 应包含馈线加固方案
    assert!(
        plans.iter().any(|p| p["action"].as_str().unwrap_or("").contains("FeederReinforcement")),
        "应包含馈线加固方案"
    );
    assert!(json["trace_id"].is_string());
}

#[tokio::test]
async fn test_planning_evaluate_area_class_a() {
    let state = AppState::new();
    let app = create_router(state);

    // A 类区域：更高可靠性要求
    let body = serde_json::json!({
        "area_class": "A",
        "voltage_kv": 10.0,
        "current_loading_percent": 50.0,
        "voltage_deviation_percent": 3.0
    });

    let (status, json) = send_post_request(app, "/api/planning/evaluate", body).await;

    assert_eq!(status, StatusCode::OK);
    // A 类区域 N-1 通过率要求 100%
    assert_eq!(
        json["n1_pass_rate_percent"].as_f64(),
        Some(100.0),
        "A 类区域 N-1 通过率要求 100%"
    );
    // A 类区域 RS-1 可靠性要求 99.99%
    assert_eq!(
        json["reliability_rs1_percent"].as_f64(),
        Some(99.99),
        "A 类区域 RS-1 可靠性要求 99.99%"
    );
}

// ===========================================================================
// 端点 4: POST /api/whatif — What-If 推演
// ===========================================================================

#[tokio::test]
async fn test_whatif_feasible_action() {
    let state = build_state_with_pipeline();
    let app = create_router(state);

    // 提交一个可行的发电机启动动作
    let body = serde_json::json!({
        "action": {
            "StartGenerator": {
                "gen_id": 1,
                "target_mw": 100.0
            }
        }
    });

    let (status, json) = send_post_request(app, "/api/whatif", body).await;

    assert_eq!(status, StatusCode::OK);
    // TestSimulator 总是返回 feasible
    assert!(
        json["feasible"].as_bool() == Some(true),
        "动作应可行: {}",
        json
    );
    assert!(
        json["projected"].as_bool() == Some(false),
        "动作无需投影: {}",
        json
    );
    assert!(
        json["infeasible"].as_bool() == Some(false),
        "动作不应不可行: {}",
        json
    );
    assert!(json["summary"].is_string(), "summary 应为字符串");
    assert!(json["projection"].is_object(), "projection 应为对象");
    assert!(json["trace_id"].is_string(), "trace_id 应存在");
}

#[tokio::test]
async fn test_whatif_without_pipeline_returns_503() {
    // 未配置决策管道 → 503
    let state = AppState::new();
    let app = create_router(state);

    let body = serde_json::json!({
        "action": {
            "StartGenerator": {
                "gen_id": 1,
                "target_mw": 100.0
            }
        }
    });

    let (status, _json) = send_post_request(app, "/api/whatif", body).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_whatif_generator_over_capacity_projected() {
    let state = build_state_with_pipeline();
    let app = create_router(state);

    // 发电机目标 300MW > 上限 200MW → 应被投影
    let body = serde_json::json!({
        "action": {
            "StartGenerator": {
                "gen_id": 1,
                "target_mw": 300.0
            }
        }
    });

    let (status, json) = send_post_request(app, "/api/whatif", body).await;

    assert_eq!(status, StatusCode::OK);
    // 应被投影（裁剪到 200MW）或可行
    assert!(
        json["feasible"].as_bool() == Some(true) || json["projected"].as_bool() == Some(true),
        "应可行或被投影: {}",
        json
    );
    assert!(json["trace_id"].is_string());
}

// ===========================================================================
// 端点 5: GET /api/audit — 审计日志查询
// ===========================================================================

#[tokio::test]
async fn test_audit_query_all_entries() {
    let state = build_state_with_audit_log();
    let app = create_router(state);

    let (status, json) = send_get_request(app, "/api/audit").await;

    assert_eq!(status, StatusCode::OK);
    assert!(json["entries"].is_array(), "entries 应为数组");
    // 预填充了 3 条记录
    assert_eq!(
        json["total"].as_u64(),
        Some(3),
        "总条目数应为 3: {}",
        json["total"]
    );
    // 默认 limit=100，应返回全部 3 条
    assert_eq!(
        json["entries"].as_array().unwrap().len(),
        3,
        "应返回 3 条记录"
    );
    assert!(json["trace_id"].is_string(), "trace_id 应存在");
}

#[tokio::test]
async fn test_audit_query_by_actor() {
    let state = build_state_with_audit_log();
    let app = create_router(state);

    // 按 actor=alice 过滤（应有 2 条）
    let (status, json) = send_get_request(app, "/api/audit?actor=alice").await;

    assert_eq!(status, StatusCode::OK);
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "alice 应有 2 条记录");
    // 所有条目都应是 alice 的
    for entry in entries {
        assert_eq!(entry["actor"].as_str(), Some("alice"));
    }
    assert_eq!(json["total"].as_u64(), Some(3), "总条目数仍为 3");
}

#[tokio::test]
async fn test_audit_query_by_result() {
    let state = build_state_with_audit_log();
    let app = create_router(state);

    // 按 result=failed 过滤（应有 1 条）
    let (status, json) = send_get_request(app, "/api/audit?result=failed").await;

    assert_eq!(status, StatusCode::OK);
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "failed 应有 1 条记录");
    assert_eq!(entries[0]["result"].as_str(), Some("failed"));
    assert_eq!(entries[0]["actor"].as_str(), Some("alice"));
}

#[tokio::test]
async fn test_audit_query_with_limit() {
    let state = build_state_with_audit_log();
    let app = create_router(state);

    // limit=2 → 应返回 2 条（最新在前）
    let (status, json) = send_get_request(app, "/api/audit?limit=2").await;

    assert_eq!(status, StatusCode::OK);
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "应返回 2 条记录");
    assert_eq!(json["total"].as_u64(), Some(3), "总条目数仍为 3");
    assert_eq!(json["returned"].as_u64(), Some(2), "returned 应为 2");
}

#[tokio::test]
async fn test_audit_query_without_log_returns_503() {
    // 未配置审计日志 → 503
    let state = AppState::new();
    let app = create_router(state);

    let (status, _json) = send_get_request(app, "/api/audit").await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

// ===========================================================================
// OpenAPI 文档验证
// ===========================================================================

#[tokio::test]
async fn test_openapi_contains_t029_09_endpoints() {
    let openapi = OpenApiDoc::openapi();
    let json = serde_json::to_value(&openapi).expect("OpenAPI 序列化为 JSON 应成功");

    let paths = json["paths"]
        .as_object()
        .expect("paths 应为 JSON 对象");

    // 验证 5 个新端点都在 OpenAPI 中注册
    assert!(
        paths.contains_key("/api/validation/check"),
        "OpenAPI 应包含 POST /api/validation/check"
    );
    assert!(
        paths.contains_key("/api/compliance/check"),
        "OpenAPI 应包含 POST /api/compliance/check"
    );
    assert!(
        paths.contains_key("/api/planning/evaluate"),
        "OpenAPI 应包含 POST /api/planning/evaluate"
    );
    assert!(
        paths.contains_key("/api/whatif"),
        "OpenAPI 应包含 POST /api/whatif"
    );
    assert!(
        paths.contains_key("/api/audit"),
        "OpenAPI 应包含 GET /api/audit"
    );

    // 验证 HTTP 方法正确
    assert!(json["paths"]["/api/validation/check"]["post"].is_object());
    assert!(json["paths"]["/api/compliance/check"]["post"].is_object());
    assert!(json["paths"]["/api/planning/evaluate"]["post"].is_object());
    assert!(json["paths"]["/api/whatif"]["post"].is_object());
    assert!(json["paths"]["/api/audit"]["get"].is_object());

    // 验证 schema 已注册
    let schemas = json["components"]["schemas"]
        .as_object()
        .expect("components.schemas 应为 JSON 对象");

    // 校验 API schema
    assert!(schemas.contains_key("ValidationRequestSchema"), "缺少 ValidationRequestSchema");
    assert!(schemas.contains_key("ValidationResponseSchema"), "缺少 ValidationResponseSchema");
    assert!(schemas.contains_key("ValidationFindingSchema"), "缺少 ValidationFindingSchema");
    assert!(schemas.contains_key("ValidationStatusSchema"), "缺少 ValidationStatusSchema");
    assert!(schemas.contains_key("ValidationSummarySchema"), "缺少 ValidationSummarySchema");

    // 合规 API schema
    assert!(schemas.contains_key("ComplianceRequestSchema"), "缺少 ComplianceRequestSchema");
    assert!(schemas.contains_key("ComplianceResponseSchema"), "缺少 ComplianceResponseSchema");
    assert!(schemas.contains_key("ComplianceFindingSchema"), "缺少 ComplianceFindingSchema");
    assert!(schemas.contains_key("ComplianceStatusSchema"), "缺少 ComplianceStatusSchema");

    // 规划 API schema
    assert!(schemas.contains_key("PlanningRequestSchema"), "缺少 PlanningRequestSchema");
    assert!(schemas.contains_key("PlanningResponseSchema"), "缺少 PlanningResponseSchema");
    assert!(schemas.contains_key("SupplyAreaClassSchema"), "缺少 SupplyAreaClassSchema");
    assert!(schemas.contains_key("VoltageLimitsSchema"), "缺少 VoltageLimitsSchema");
    assert!(schemas.contains_key("LoadingLimitsSchema"), "缺少 LoadingLimitsSchema");
    assert!(schemas.contains_key("SupplyRadiusSchema"), "缺少 SupplyRadiusSchema");
    assert!(schemas.contains_key("CandidatePlanSchema"), "缺少 CandidatePlanSchema");

    // WhatIf API schema
    assert!(schemas.contains_key("WhatIfRequestSchema"), "缺少 WhatIfRequestSchema");
    assert!(schemas.contains_key("WhatIfResponseSchema"), "缺少 WhatIfResponseSchema");
    assert!(schemas.contains_key("StructuredActionWhatIfSchema"), "缺少 StructuredActionWhatIfSchema");

    // 审计 API schema
    assert!(schemas.contains_key("AuditEntryResponse"), "缺少 AuditEntryResponse");
    assert!(schemas.contains_key("AuditQueryResponseSchema"), "缺少 AuditQueryResponseSchema");
}

#[tokio::test]
async fn test_openapi_tags_contain_t029_09() {
    let openapi = OpenApiDoc::openapi();
    let json = serde_json::to_value(&openapi).expect("OpenAPI 序列化为 JSON 应成功");

    let tags = json["tags"]
        .as_array()
        .expect("tags 应为数组");

    let tag_names: Vec<&str> = tags
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    assert!(tag_names.contains(&"validation"), "缺少 validation tag");
    assert!(tag_names.contains(&"compliance"), "缺少 compliance tag");
    assert!(tag_names.contains(&"planning"), "缺少 planning tag");
    assert!(tag_names.contains(&"whatif"), "缺少 whatif tag");
    assert!(tag_names.contains(&"audit"), "缺少 audit tag");
}

// ===========================================================================
// trace_id 集成验证
// ===========================================================================

#[tokio::test]
async fn test_trace_id_propagated_to_validation_response() {
    let state = AppState::new();
    let app = create_router(state);

    let upstream_trace_id = "550e8400-e29b-41d4-a716-446655440000";
    let body = serde_json::json!({
        "state": {
            "buses": [],
            "frequency": null,
            "contingencies": [],
            "short_circuits": []
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/validation/check")
                .header("content-type", "application/json")
                .header("x-trace-id", upstream_trace_id)
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // 响应头应包含 X-Trace-Id
    assert_eq!(
        response
            .headers()
            .get("x-trace-id")
            .and_then(|v| v.to_str().ok()),
        Some(upstream_trace_id)
    );

    let json = response_to_json(response.into_body()).await;
    // 响应体应包含 trace_id
    assert_eq!(
        json["trace_id"].as_str(),
        Some(upstream_trace_id),
        "响应体应包含上游传入的 trace_id"
    );
}

#[tokio::test]
async fn test_trace_id_propagated_to_compliance_response() {
    let state = AppState::new();
    let app = create_router(state);

    let upstream_trace_id = "test-compliance-trace-id";
    let body = serde_json::json!({
        "spec": {
            "equipment_type": "transformer",
            "rated_voltage_kv": 10.0,
            "normal_loading_limit_percent": 85.0
        },
        "operating": {
            "loading_percent": 70.0,
            "voltage_pu": 1.0
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/compliance/check")
                .header("content-type", "application/json")
                .header("x-trace-id", upstream_trace_id)
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = response_to_json(response.into_body()).await;
    assert_eq!(
        json["trace_id"].as_str(),
        Some(upstream_trace_id),
        "响应体应包含上游传入的 trace_id"
    );
}
