//! A04:2021 — 不安全设计 (Insecure Design)
//!
//! 测试 EnerOS 的安全设计模式，验证：
//! - 速率限制（如果实现）
//! - 账户锁定机制
//! - 默认拒绝策略

#[cfg(test)]
use eneros_api::auth::{AuthManager, AuthError, Role};

/// 验证默认拒绝策略：无凭证时拒绝访问
#[test]
fn test_default_deny_policy() {
    let manager = AuthManager::new("test-secret", 3600);

    // 无凭证应被拒绝
    let result = manager.authenticate(None, None);
    assert!(
        matches!(result, Err(AuthError::NoValidCredentials)),
        "默认应拒绝无凭证访问"
    );
}

/// 验证多次失败认证不会锁定账户（当前实现）
/// 注：EnerOS 当前版本未实现账户锁定，此测试验证行为一致性
#[test]
fn test_repeated_failed_auth_attempts() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "correct-password", Role::Operator);

    // 多次失败尝试
    for _ in 0..5 {
        let result = manager.validate_credentials("alice", "wrong-password");
        assert!(result.is_err(), "错误密码应被拒绝");
    }

    // 正确密码仍应有效（无锁定）
    let result = manager.validate_credentials("alice", "correct-password");
    assert!(
        result.is_ok(),
        "正确密码应验证成功（当前无账户锁定机制）"
    );
}

/// 验证 token TTL 有限（不会永久有效）
#[test]
fn test_token_ttl_is_bounded() {
    let manager = AuthManager::new("test-secret", 3600);
    let ttl = manager.token_ttl();
    assert!(
        ttl > 0 && ttl <= 86400,
        "token TTL 应为正数且不超过 24 小时，实际: {}",
        ttl
    );
}

/// 验证短 TTL token 会快速过期
#[test]
fn test_short_ttl_token_expires() {
    let manager = AuthManager::new("test-secret", 2); // 2 秒
    let token = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 立即验证应成功
    assert!(manager.verify_token(&token).is_ok());

    // 等待过期
    std::thread::sleep(std::time::Duration::from_secs(3));

    // 过期后应失败
    let result = manager.verify_token(&token);
    assert!(
        matches!(result, Err(AuthError::TokenExpired)),
        "短 TTL token 应过期"
    );
}

/// 验证无效凭证不会泄露用户是否存在（相同错误码）
#[test]
fn test_no_user_enumeration_via_error() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("existing-user", "password", Role::Operator);

    // 存在的用户 + 错误密码
    let result1 = manager.validate_credentials("existing-user", "wrong");
    // 不存在的用户 + 任意密码
    let result2 = manager.validate_credentials("nonexistent-user", "wrong");

    // 两者应返回相同的错误类型（InvalidCredentials）
    assert!(
        matches!(result1, Err(AuthError::InvalidCredentials)),
        "存在用户错误密码应返回 InvalidCredentials"
    );
    assert!(
        matches!(result2, Err(AuthError::InvalidCredentials)),
        "不存在用户应返回相同的 InvalidCredentials 错误（避免用户枚举）"
    );
}

/// HTTP 层测试：速率限制（需要 API server 二进制）
#[tokio::test]
#[ignore = "需要启动 API server 二进制"]
async fn test_http_rate_limiting() {
    let client = reqwest::Client::new();

    // 发送大量请求
    let mut responses = Vec::new();
    for _ in 0..100 {
        let resp = client
            .post("http://127.0.0.1:8080/api/auth/login")
            .json(&serde_json::json!({
                "username": "admin",
                "password": "wrong"
            }))
            .send()
            .await
            .expect("HTTP 请求应成功");
        responses.push(resp.status());
    }

    // 至少部分请求应被限流（429）
    let rate_limited = responses
        .iter()
        .filter(|s| **s == reqwest::StatusCode::TOO_MANY_REQUESTS)
        .count();
    assert!(
        rate_limited > 0,
        "大量请求应触发速率限制 (429)，实际限流数: {}",
        rate_limited
    );
}

/// HTTP 层测试：账户锁定机制（需要 API server 二进制）
#[tokio::test]
#[ignore = "需要启动 API server 二进制"]
async fn test_http_account_lockout() {
    let client = reqwest::Client::new();

    // 多次失败登录
    for _ in 0..10 {
        let _ = client
            .post("http://127.0.0.1:8080/api/auth/login")
            .json(&serde_json::json!({
                "username": "admin",
                "password": "wrong"
            }))
            .send()
            .await;
    }

    // 正确密码登录应被锁定
    let resp = client
        .post("http://127.0.0.1:8080/api/auth/login")
        .json(&serde_json::json!({
            "username": "admin",
            "password": "correct"
        }))
        .send()
        .await
        .expect("HTTP 请求应成功");

    assert!(
        resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
            || resp.status() == reqwest::StatusCode::FORBIDDEN,
        "多次失败后账户应被锁定"
    );
}
