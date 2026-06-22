//! A05:2021 — 安全配置错误 (Security Misconfiguration)
//!
//! 测试 EnerOS 的安全配置，验证：
//! - 默认配置安全
//! - 调试端点在生产中关闭
//! - CORS 配置合理

#[cfg(test)]
use eneros_api::app::{create_router, AppState};

/// 验证默认 AppState 不包含敏感引擎（最小权限原则）
#[test]
fn test_default_state_is_minimal() {
    let state = AppState::new();

    // 默认状态应为空（最小权限）
    assert!(state.topology_engine.is_none(), "默认不应注入 topology");
    assert!(state.powerflow_solver.is_none(), "默认不应注入 powerflow");
    assert!(state.constraint_engine.is_none(), "默认不应注入 constraint");
    assert!(state.network.is_none(), "默认不应注入 network");
    assert!(state.auth_manager.is_none(), "默认不应注入 auth_manager");
    assert!(
        state.agent_orchestrator.is_none(),
        "默认不应注入 agent_orchestrator"
    );
}

/// 验证路由器可创建（配置正确）
#[test]
fn test_router_creation_succeeds() {
    let state = AppState::new();
    let _router = create_router(state);
    // 如果创建成功，说明配置无误
}

/// 验证健康检查端点存在
#[tokio::test]
async fn test_health_endpoint_exists() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt;

    let state = AppState::new();
    let app = create_router(state);

    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK, "健康检查端点应返回 200");
}

/// 验证 OpenAPI 文档端点存在
#[tokio::test]
async fn test_openapi_endpoint_exists() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt;

    let state = AppState::new();
    let app = create_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "OpenAPI 端点应返回 200"
    );
}

/// 验证 Swagger UI 端点存在
#[tokio::test]
async fn test_swagger_ui_endpoint_exists() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt;

    let state = AppState::new();
    let app = create_router(state);

    let response = app
        .oneshot(Request::builder().uri("/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Swagger UI 端点应返回 200"
    );
}

/// 验证响应包含 trace_id 头（可观测性配置）
#[tokio::test]
async fn test_trace_id_header_present() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    let state = AppState::new();
    let app = create_router(state);

    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert!(
        response.headers().contains_key("x-trace-id"),
        "响应应包含 X-Trace-Id 头（可观测性配置）"
    );
}

/// 验证 AuthManager 默认不注册任何用户
#[test]
fn test_auth_manager_starts_empty() {
    use eneros_api::auth::AuthManager;

    let manager = AuthManager::new("test-secret", 3600);
    assert!(
        !manager.has_users(),
        "新创建的 AuthManager 不应包含任何用户"
    );
}

/// 验证 AuthManager 默认不注册任何 API Key
#[test]
fn test_auth_manager_no_default_api_keys() {
    use eneros_api::auth::AuthManager;

    let manager = AuthManager::new("test-secret", 3600);
    let result = manager.authenticate_api_key("any-key");
    assert!(
        result.is_err(),
        "新创建的 AuthManager 不应有任何有效 API Key"
    );
}

/// HTTP 层测试：调试端点在生产中关闭（需要 API server 二进制）
#[tokio::test]
#[ignore = "需要启动 API server 二进制"]
async fn test_http_debug_endpoints_disabled_in_production() {
    let client = reqwest::Client::new();

    // 尝试访问可能的调试端点
    let debug_endpoints = [
        "/debug",
        "/api/debug",
        "/_debug",
        "/api/internal/debug",
        "/status/debug",
    ];

    for endpoint in &debug_endpoints {
        let resp = client
            .get(format!("http://127.0.0.1:8080{}", endpoint))
            .send()
            .await
            .expect("HTTP 请求应成功");

        assert_eq!(
            resp.status(),
            reqwest::StatusCode::NOT_FOUND,
            "调试端点 {} 应返回 404",
            endpoint
        );
    }
}

/// HTTP 层测试：CORS 配置（需要 API server 二进制）
#[tokio::test]
#[ignore = "需要启动 API server 二进制"]
async fn test_http_cors_configuration() {
    let client = reqwest::Client::new();

    let resp = client
        .get("http://127.0.0.1:8080/health")
        .header("Origin", "https://malicious-site.com")
        .send()
        .await
        .expect("HTTP 请求应成功");

    // 验证 CORS 头存在
    let cors_header = resp.headers().get("access-control-allow-origin");
    assert!(
        cors_header.is_some(),
        "响应应包含 CORS 头"
    );
}
