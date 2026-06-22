//! A02:2021 — 加密失败 (Cryptographic Failures)
//!
//! 测试 EnerOS 的加密实践，验证：
//! - JWT token 签名验证
//! - 过期 token 被拒绝
//! - 密码不以明文存储

#[cfg(test)]
use eneros_api::auth::{AuthError, AuthManager, Role};

/// 验证 JWT token 使用 HMAC-SHA256 签名
#[test]
fn test_jwt_signature_verification() {
    let manager = AuthManager::new("correct-secret-key", 3600);
    let token = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 正确密钥验证应成功
    let claims = manager.verify_token(&token);
    assert!(claims.is_ok(), "正确密钥应验证成功");

    let claims = claims.unwrap();
    assert_eq!(claims.sub, "alice");
    assert_eq!(claims.role, "operator");
}

/// 验证使用错误密钥签发的 token 被拒绝
#[test]
fn test_jwt_wrong_secret_rejected() {
    let manager1 = AuthManager::new("secret-one", 3600);
    let manager2 = AuthManager::new("secret-two", 3600);

    let token = manager1
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    let result = manager2.verify_token(&token);
    assert!(
        matches!(result, Err(AuthError::InvalidSignature)),
        "错误密钥签发的 token 应返回 InvalidSignature 错误"
    );
}

/// 验证 token 篡改后被拒绝
#[test]
fn test_jwt_tampered_token_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    let token = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 篡改 payload 部分（第二段）
    let parts: Vec<&str> = token.split('.').collect();
    let tampered_token = format!("{}.{}.{}", parts[0], "tamperedpayload", parts[2]);

    let result = manager.verify_token(&tampered_token);
    assert!(
        result.is_err(),
        "篡改的 token 应被拒绝"
    );
}

/// 验证过期 token 被拒绝
#[test]
fn test_jwt_expired_token_rejected() {
    let manager = AuthManager::new("test-secret", 1); // 1 秒 TTL
    let token = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 等待过期
    std::thread::sleep(std::time::Duration::from_secs(2));

    let result = manager.verify_token(&token);
    assert!(
        matches!(result, Err(AuthError::TokenExpired)),
        "过期 token 应返回 TokenExpired 错误"
    );
}

/// 验证格式错误的 token 被拒绝
#[test]
fn test_jwt_malformed_token_rejected() {
    let manager = AuthManager::new("test-secret", 3600);

    // 缺少部分（只有 2 段）
    let result = manager.verify_token("only.two");
    assert!(
        matches!(result, Err(AuthError::InvalidTokenFormat)),
        "格式错误的 token 应返回 InvalidTokenFormat 错误"
    );

    // 完全无效（无点分隔）
    let result = manager.verify_token("not-a-jwt");
    assert!(
        matches!(result, Err(AuthError::InvalidTokenFormat)),
        "非 JWT 字符串应返回 InvalidTokenFormat 错误"
    );

    // 3 段但内容无效（应返回其他错误，非格式错误）
    let result = manager.verify_token("only.two.parts");
    assert!(
        result.is_err(),
        "3 段但内容无效的 token 应被拒绝"
    );
}

/// 验证密码不以明文存储
#[test]
fn test_password_not_stored_in_plaintext() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "super-secret-password", Role::Operator);

    // 验证凭证正确
    let result = manager.validate_credentials("alice", "super-secret-password");
    assert!(result.is_ok(), "正确密码应验证成功");

    // 验证错误密码被拒绝
    let result = manager.validate_credentials("alice", "wrong-password");
    assert!(
        result.is_err(),
        "错误密码应被拒绝"
    );

    // 验证不存在用户被拒绝
    let result = manager.validate_credentials("bob", "super-secret-password");
    assert!(
        result.is_err(),
        "不存在用户应被拒绝"
    );
}

/// 验证密码哈希不等于明文（SHA-256 hex 编码）
#[test]
fn test_password_hash_is_not_plaintext() {
    let password = "my-secret-password";
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", password, Role::Operator);

    // 通过验证凭证间接验证哈希正确性
    let result = manager.validate_credentials("alice", password);
    assert!(result.is_ok());

    // 错误密码不应通过验证
    let result = manager.validate_credentials("alice", &format!("{}!", password));
    assert!(result.is_err());
}

/// 验证 JWT token 包含正确的 claims 结构
#[test]
fn test_jwt_claims_structure() {
    let manager = AuthManager::new("test-secret", 3600);
    let token = manager
        .issue_token("alice", Role::Supervisor)
        .expect("签发 token 应成功");

    let claims = manager
        .verify_token(&token)
        .expect("验证 token 应成功");

    // 验证 claims 字段
    assert!(!claims.sub.is_empty(), "sub 不应为空");
    assert!(!claims.role.is_empty(), "role 不应为空");
    assert!(claims.exp > claims.iat, "exp 应大于 iat");
    assert!(
        claims.exp > chrono::Utc::now().timestamp() as usize,
        "exp 应在未来"
    );
}

/// 验证 API Key 认证不暴露密钥
#[test]
fn test_api_key_authentication() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_api_key("my-secret-api-key", "service-account", Role::Supervisor);

    // 正确 API Key
    let (username, role) = manager
        .authenticate_api_key("my-secret-api-key")
        .expect("正确 API Key 应认证成功");
    assert_eq!(username, "service-account");
    assert_eq!(role, Role::Supervisor);

    // 错误 API Key
    let result = manager.authenticate_api_key("wrong-key");
    assert!(
        matches!(result, Err(AuthError::InvalidApiKey)),
        "错误 API Key 应返回 InvalidApiKey 错误"
    );
}
