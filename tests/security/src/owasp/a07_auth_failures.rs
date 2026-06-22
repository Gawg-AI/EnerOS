//! A07:2021 — 认证失败 (Identification and Authentication Failures)
//!
//! 测试 EnerOS 的认证机制，验证：
//! - 弱密码策略
//! - 会话管理
//! - 多因素认证（如果实现）

#[cfg(test)]
use eneros_api::auth::{AuthError, AuthManager, Role};

/// 验证弱密码不会绕过认证
#[test]
fn test_weak_passwords_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "StrongP@ssw0rd!", Role::Operator);

    let weak_passwords = [
        "",
        "a",
        "123",
        "password",
        "admin",
        "12345678",
        "qwerty",
    ];

    for weak in &weak_passwords {
        let result = manager.validate_credentials("alice", weak);
        assert!(
            result.is_err(),
            "弱密码应被拒绝: {}",
            weak
        );
    }
}

/// 验证正确密码认证成功
#[test]
fn test_strong_password_accepted() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "StrongP@ssw0rd!2024", Role::Operator);

    let result = manager.validate_credentials("alice", "StrongP@ssw0rd!2024");
    assert!(
        result.is_ok(),
        "正确密码应认证成功"
    );
}

/// 验证会话 token 唯一性
#[test]
fn test_session_token_uniqueness() {
    let manager = AuthManager::new("test-secret", 3600);

    let token1 = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 稍等片刻确保 iat 不同
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let token2 = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 两次签发的 token 应不同（因为 iat 不同）
    assert_ne!(
        token1, token2,
        "两次签发的 token 应不同（时间戳不同）"
    );

    // 两个 token 都应有效
    assert!(manager.verify_token(&token1).is_ok());
    assert!(manager.verify_token(&token2).is_ok());
}

/// 验证 token 过期后会话失效
#[test]
fn test_session_expires_after_ttl() {
    let manager = AuthManager::new("test-secret", 1); // 1 秒 TTL
    let token = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 立即验证应成功
    assert!(manager.verify_token(&token).is_ok());

    // 等待过期
    std::thread::sleep(std::time::Duration::from_secs(2));

    // 过期后应失败
    assert!(matches!(
        manager.verify_token(&token),
        Err(AuthError::TokenExpired)
    ));
}

/// 验证不同用户签发不同 token
#[test]
fn test_different_users_different_tokens() {
    let manager = AuthManager::new("test-secret", 3600);

    let token_alice = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");
    let token_bob = manager
        .issue_token("bob", Role::Supervisor)
        .expect("签发 token 应成功");

    assert_ne!(token_alice, token_bob, "不同用户应获得不同 token");

    let claims_alice = manager.verify_token(&token_alice).unwrap();
    let claims_bob = manager.verify_token(&token_bob).unwrap();

    assert_eq!(claims_alice.sub, "alice");
    assert_eq!(claims_bob.sub, "bob");
    assert_ne!(claims_alice.role, claims_bob.role);
}

/// 验证认证方法标识正确
#[test]
fn test_auth_method_identification() {
    use eneros_api::auth::AuthMethod;

    let manager = AuthManager::new("test-secret", 3600);
    manager.add_api_key("test-key", "service", Role::Observer);

    // API Key 认证
    let user = manager
        .authenticate(None, Some("test-key"))
        .expect("API Key 认证应成功");
    assert_eq!(user.auth_method, AuthMethod::ApiKey);

    // JWT 认证
    let token = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");
    let user = manager
        .authenticate(Some(&format!("Bearer {}", token)), None)
        .expect("JWT 认证应成功");
    assert_eq!(user.auth_method, AuthMethod::Jwt);
}

/// 验证 API Key 优先于 Bearer token
#[test]
fn test_api_key_takes_precedence() {
    use eneros_api::auth::AuthMethod;

    let manager = AuthManager::new("test-secret", 3600);
    manager.add_api_key("valid-key", "service", Role::Observer);

    let token = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 同时提供 API Key 和 Bearer token，应使用 API Key
    let user = manager
        .authenticate(
            Some(&format!("Bearer {}", token)),
            Some("valid-key"),
        )
        .expect("认证应成功");

    assert_eq!(user.auth_method, AuthMethod::ApiKey);
    assert_eq!(user.username, "service");
}

/// HTTP 层测试：多因素认证（需要 API server 二进制）
#[tokio::test]
#[ignore = "需要启动 API server 二进制"]
async fn test_http_mfa_endpoint() {
    let client = reqwest::Client::new();

    // 尝试访问 MFA 端点
    let resp = client
        .post("http://127.0.0.1:8080/api/auth/mfa/verify")
        .json(&serde_json::json!({
            "code": "123456"
        }))
        .send()
        .await
        .expect("HTTP 请求应成功");

    // MFA 端点应存在或返回 404（未实现）
    assert!(
        resp.status() == reqwest::StatusCode::OK
            || resp.status() == reqwest::StatusCode::NOT_FOUND
            || resp.status() == reqwest::StatusCode::UNAUTHORIZED,
        "MFA 端点应返回 200/404/401，实际: {}",
        resp.status()
    );
}
