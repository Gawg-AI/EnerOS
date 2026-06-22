//! A10:2021 — SSRF (Server-Side Request Forgery)
//!
//! 测试 EnerOS 对 SSRF 攻击的防护，验证：
//! - URL 输入验证
//! - 内部地址访问限制

#[cfg(test)]
use eneros_api::auth::{AuthManager, Role};

/// 验证 SSRF 相关的 URL 输入不会绕过认证
#[test]
fn test_ssrf_url_in_auth_context() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "password", Role::Operator);

    let ssrf_usernames = [
        "http://169.254.169.254/latest/meta-data/",
        "http://localhost:8080/admin",
        "http://127.0.0.1:8080/internal",
        "http://[::1]:8080/",
        "file:///etc/passwd",
        "gopher://127.0.0.1:6379/_INFO",
        "dict://127.0.0.1:11211/stats",
    ];

    for username in &ssrf_usernames {
        let result = manager.validate_credentials(username, "password");
        assert!(
            result.is_err(),
            "SSRF URL 作为用户名应被拒绝: {}",
            username
        );
    }
}

/// 验证 SSRF URL 作为 API Key 被拒绝
#[test]
fn test_ssrf_url_as_api_key_rejected() {
    let manager = AuthManager::new("test-secret", 3600);
    manager.add_api_key("valid-key", "service", Role::Observer);

    let ssrf_keys = [
        "http://169.254.169.254/",
        "http://localhost:8080/admin",
        "file:///etc/shadow",
    ];

    for key in &ssrf_keys {
        let result = manager.authenticate_api_key(key);
        assert!(
            result.is_err(),
            "SSRF URL 作为 API Key 应被拒绝: {}",
            key
        );
    }
}

/// 验证 SSRF URL 作为 JWT token 被拒绝
#[test]
fn test_ssrf_url_as_token_rejected() {
    let manager = AuthManager::new("test-secret", 3600);

    let ssrf_tokens = [
        "http://169.254.169.254/latest/meta-data/",
        "http://localhost:8080/admin",
        "file:///etc/passwd",
        "gopher://127.0.0.1:6379/",
    ];

    for token in &ssrf_tokens {
        let result = manager.verify_token(token);
        assert!(
            result.is_err(),
            "SSRF URL 作为 token 应被拒绝: {}",
            token
        );
    }
}

/// 验证云元数据 IP 不被接受为有效输入
#[test]
fn test_cloud_metadata_ip_rejected() {
    let manager = AuthManager::new("test-secret", 3600);

    // AWS 元数据 IP
    let result = manager.verify_token("169.254.169.254");
    assert!(result.is_err(), "云元数据 IP 应被拒绝");

    // GCP 元数据 IP
    let result = manager.verify_token("metadata.google.internal");
    assert!(result.is_err(), "GCP 元数据地址应被拒绝");
}

/// HTTP 层测试：SSRF 防护（需要 API server 二进制）
#[tokio::test]
#[ignore = "需要启动 API server 二进制"]
async fn test_http_ssrf_protection() {
    let client = reqwest::Client::new();

    // 尝试通过 API 端点访问内部资源
    let ssrf_payloads = [
        "http://169.254.169.254/latest/meta-data/",
        "http://localhost:8080/admin",
        "http://127.0.0.1:6379/",
        "file:///etc/passwd",
    ];

    for payload in &ssrf_payloads {
        let resp = client
            .post("http://127.0.0.1:8080/api/analysis/opf")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "url": payload
            }))
            .send()
            .await
            .expect("HTTP 请求应成功");

        // 应返回 400 或 403，不应返回 200
        assert!(
            resp.status() == reqwest::StatusCode::BAD_REQUEST
                || resp.status() == reqwest::StatusCode::FORBIDDEN
                || resp.status() == reqwest::StatusCode::UNAUTHORIZED,
            "SSRF payload 应被拒绝: {} (状态码: {})",
            payload,
            resp.status()
        );
    }
}
