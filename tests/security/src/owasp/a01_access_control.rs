//! A01:2021 — 访问控制失效 (Broken Access Control)
//!
//! 测试 EnerOS 的 RBAC 权限模型，验证：
//! - 未认证请求被拒绝
//! - 权限不足返回 403
//! - Observer 角色无法执行写操作
//! - Operator 角色无法执行控制动作

#[cfg(test)]
use eneros_api::auth::{AuthManager, Permission, Role};

/// 验证 Observer 角色只能读取，不能写入/控制/紧急操作
#[test]
fn test_observer_cannot_write() {
    assert!(Role::Observer.has_permission(Permission::Read));
    assert!(!Role::Observer.has_permission(Permission::Write));
    assert!(!Role::Observer.has_permission(Permission::Control));
    assert!(!Role::Observer.has_permission(Permission::Emergency));
}

/// 验证 Operator 角色可以读写，但不能执行控制动作和紧急操作
#[test]
fn test_operator_cannot_control() {
    assert!(Role::Operator.has_permission(Permission::Read));
    assert!(Role::Operator.has_permission(Permission::Write));
    assert!(!Role::Operator.has_permission(Permission::Control));
    assert!(!Role::Operator.has_permission(Permission::Emergency));
}

/// 验证 Supervisor 角色可以读/写/控制，但不能执行紧急操作
#[test]
fn test_supervisor_cannot_emergency() {
    assert!(Role::Supervisor.has_permission(Permission::Read));
    assert!(Role::Supervisor.has_permission(Permission::Write));
    assert!(Role::Supervisor.has_permission(Permission::Control));
    assert!(!Role::Supervisor.has_permission(Permission::Emergency));
}

/// 验证 Emergency 角色拥有所有权限
#[test]
fn test_emergency_has_all_permissions() {
    assert!(Role::Emergency.has_permission(Permission::Read));
    assert!(Role::Emergency.has_permission(Permission::Write));
    assert!(Role::Emergency.has_permission(Permission::Control));
    assert!(Role::Emergency.has_permission(Permission::Emergency));
}

/// 验证未认证用户无法获取有效凭证
#[test]
fn test_unauthenticated_request_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    let result = manager.authenticate(None, None);
    assert!(
        result.is_err(),
        "未提供凭证的请求应被拒绝"
    );
}

/// 验证无效 Bearer token 被拒绝
#[test]
fn test_invalid_bearer_token_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    let result = manager.authenticate(Some("Bearer invalid.token.here"), None);
    assert!(
        result.is_err(),
        "无效 Bearer token 应被拒绝"
    );
}

/// 验证无效 API Key 被拒绝
#[test]
fn test_invalid_api_key_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    let result = manager.authenticate(None, Some("invalid-api-key"));
    assert!(
        result.is_err(),
        "无效 API Key 应被拒绝"
    );
}

/// 验证权限矩阵：GET 请求需要 Read 权限
#[test]
fn test_required_permission_get_requests() {
    use eneros_api::auth::required_permission;
    assert_eq!(
        required_permission("GET", "/api/agents"),
        Permission::Read
    );
    assert_eq!(
        required_permission("HEAD", "/api/scada/latest"),
        Permission::Read
    );
}

/// 验证权限矩阵：POST 到 /api/actions/* 需要 Control 权限
#[test]
fn test_required_permission_control_actions() {
    use eneros_api::auth::required_permission;
    assert_eq!(
        required_permission("POST", "/api/actions/structured"),
        Permission::Control
    );
}

/// 验证权限矩阵：POST 到 /api/emergency/* 需要 Emergency 权限
#[test]
fn test_required_permission_emergency_actions() {
    use eneros_api::auth::required_permission;
    assert_eq!(
        required_permission("POST", "/api/emergency/shed"),
        Permission::Emergency
    );
}

/// 验证权限矩阵：普通 POST 请求需要 Write 权限
#[test]
fn test_required_permission_write_actions() {
    use eneros_api::auth::required_permission;
    assert_eq!(
        required_permission("POST", "/api/power-flow"),
        Permission::Write
    );
    assert_eq!(
        required_permission("PUT", "/api/devices/1"),
        Permission::Write
    );
    assert_eq!(
        required_permission("DELETE", "/api/agents/1"),
        Permission::Write
    );
}

/// 验证 Observer 用户的权限检查
#[test]
fn test_observer_user_permission_check() {
    let manager = AuthManager::new("test-secret", 3600);
    let user = eneros_api::auth::AuthenticatedUser {
        username: "observer-user".to_string(),
        role: Role::Observer,
        auth_method: eneros_api::auth::AuthMethod::Jwt,
    };
    assert!(manager.check_permission(&user, Permission::Read));
    assert!(!manager.check_permission(&user, Permission::Write));
    assert!(!manager.check_permission(&user, Permission::Control));
}

/// 验证 Operator 用户的权限检查
#[test]
fn test_operator_user_permission_check() {
    let manager = AuthManager::new("test-secret", 3600);
    let user = eneros_api::auth::AuthenticatedUser {
        username: "operator-user".to_string(),
        role: Role::Operator,
        auth_method: eneros_api::auth::AuthMethod::Jwt,
    };
    assert!(manager.check_permission(&user, Permission::Read));
    assert!(manager.check_permission(&user, Permission::Write));
    assert!(!manager.check_permission(&user, Permission::Control));
}

/// HTTP 层测试：未认证请求返回 401（需要 API server 二进制）
#[tokio::test]
#[ignore = "需要启动 API server 二进制"]
async fn test_http_unauthenticated_returns_401() {
    let client = reqwest::Client::new();
    let resp = client
        .get("http://127.0.0.1:8080/api/agents")
        .send()
        .await
        .expect("HTTP 请求应成功");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "未认证请求应返回 401"
    );
}

/// HTTP 层测试：权限不足返回 403（需要 API server 二进制）
#[tokio::test]
#[ignore = "需要启动 API server 二进制"]
async fn test_http_insufficient_permissions_returns_403() {
    let manager = AuthManager::new("test-secret", 3600);
    let token = manager
        .issue_token("observer", Role::Observer)
        .expect("签发 token 应成功");

    let client = reqwest::Client::new();
    let resp = client
        .post("http://127.0.0.1:8080/api/power-flow")
        .header("Authorization", format!("Bearer {}", token))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("HTTP 请求应成功");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::FORBIDDEN,
        "Observer 执行写操作应返回 403"
    );
}
