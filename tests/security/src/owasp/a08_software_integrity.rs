//! A08:2021 — 软件和数据完整性失败 (Software and Data Integrity Failures)
//!
//! 测试 EnerOS 的完整性验证机制，验证：
//! - JWT 签名验证
//! - 审计日志完整性
//! - 数据完整性校验

#[cfg(test)]
use eneros_api::auth::{AuthManager, Role};
#[cfg(test)]
use eneros_api::audit::{AuditEntry, AuditLog};

/// 验证 JWT token 签名完整性
#[test]
fn test_jwt_signature_integrity() {
    let manager = AuthManager::new("integrity-secret", 3600);
    let token = manager
        .issue_token("alice", Role::Operator)
        .expect("签发 token 应成功");

    // 验证签名完整
    let claims = manager.verify_token(&token);
    assert!(claims.is_ok(), "完整 token 应验证成功");

    // 篡改签名部分
    let parts: Vec<&str> = token.split('.').collect();
    let tampered_sig = if parts[2].len() > 4 {
        let mut sig = parts[2].to_string();
        // 修改最后一个字符
        let last_char = sig.chars().last().unwrap();
        let new_char = if last_char == 'A' { 'B' } else { 'A' };
        sig.pop();
        sig.push(new_char);
        sig
    } else {
        "tampered".to_string()
    };
    let tampered_token = format!("{}.{}.{}", parts[0], parts[1], tampered_sig);

    let result = manager.verify_token(&tampered_token);
    assert!(
        result.is_err(),
        "篡改签名的 token 应被拒绝"
    );
}

/// 验证审计日志条目完整性
#[test]
fn test_audit_entry_integrity() {
    let entry = AuditEntry::new(
        "alice",
        "operator",
        "POST",
        "/api/power-flow",
        "127.0.0.1",
        "success",
    );

    // 验证所有字段完整
    assert!(!entry.id.is_empty(), "审计条目应有唯一 ID");
    assert!(!entry.actor.is_empty(), "审计条目应有 actor");
    assert!(!entry.role.is_empty(), "审计条目应有 role");
    assert!(!entry.method.is_empty(), "审计条目应有 method");
    assert!(!entry.path.is_empty(), "审计条目应有 path");
    assert!(!entry.client_ip.is_empty(), "审计条目应有 client_ip");
    assert!(!entry.result.is_empty(), "审计条目应有 result");
    assert!(entry.timestamp > 0, "审计条目应有有效时间戳");
}

/// 验证审计日志 ID 唯一性
#[test]
fn test_audit_entry_id_uniqueness() {
    let entry1 = AuditEntry::new(
        "alice", "operator", "POST", "/api/test", "127.0.0.1", "success",
    );
    let entry2 = AuditEntry::new(
        "alice", "operator", "POST", "/api/test", "127.0.0.1", "success",
    );

    assert_ne!(
        entry1.id, entry2.id,
        "两个审计条目应有不同的 ID（UUID）"
    );
}

/// 验证审计日志记录完整性
#[test]
fn test_audit_log_recording_integrity() {
    let audit_log = AuditLog::new(1000);

    // 记录多个条目
    for i in 0..10 {
        let entry = AuditEntry::new(
            format!("user-{}", i),
            "operator",
            "POST",
            "/api/test",
            "127.0.0.1",
            "success",
        );
        audit_log.record(entry);
    }

    // query 返回逆序（最新优先）
    let entries = audit_log.query(None, None, 100);
    assert_eq!(entries.len(), 10, "应记录 10 个审计条目");

    // 验证最新条目在前
    assert_eq!(entries[0].actor, "user-9");
    assert_eq!(entries[9].actor, "user-0");
}

/// 验证审计日志容量限制
#[test]
fn test_audit_log_capacity_limit() {
    let audit_log = AuditLog::new(5); // 最大 5 条

    // 记录 10 条
    for i in 0..10 {
        let entry = AuditEntry::new(
            format!("user-{}", i),
            "operator",
            "POST",
            "/api/test",
            "127.0.0.1",
            "success",
        );
        audit_log.record(entry);
    }

    // 应只保留最新的 5 条
    assert_eq!(audit_log.count(), 5, "审计日志应限制为最大容量 5 条");

    let entries = audit_log.query(None, None, 100);
    // query 返回逆序（最新优先），所以 user-9 在前，user-5 在后
    assert_eq!(entries[0].actor, "user-9");
    assert_eq!(entries[4].actor, "user-5");
}

/// 验证审计条目可序列化（用于持久化）
#[test]
fn test_audit_entry_serialization() {
    let entry = AuditEntry::new(
        "alice",
        "operator",
        "POST",
        "/api/power-flow",
        "127.0.0.1",
        "success",
    )
    .with_detail("Power flow calculation completed");

    let json = serde_json::to_string(&entry).expect("审计条目应可序列化为 JSON");
    let deserialized: AuditEntry =
        serde_json::from_str(&json).expect("JSON 应可反序列化为审计条目");

    assert_eq!(entry.id, deserialized.id);
    assert_eq!(entry.actor, deserialized.actor);
    assert_eq!(entry.detail, deserialized.detail);
}

/// 验证 token claims 不可篡改
#[test]
fn test_token_claims_tamper_resistance() {
    let manager = AuthManager::new("test-secret", 3600);
    let token = manager
        .issue_token("alice", Role::Observer)
        .expect("签发 token 应成功");

    // 验证原始 token
    let claims = manager.verify_token(&token).unwrap();
    assert_eq!(claims.role, "observer");

    // 尝试篡改 payload 中的 role 字段
    let parts: Vec<&str> = token.split('.').collect();
    // 解码 payload
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
    let mut payload: serde_json::Value =
        serde_json::from_slice(&payload_bytes).unwrap();

    // 篡改 role 为 supervisor
    payload["role"] = serde_json::Value::String("supervisor".to_string());
    let tampered_payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    let tampered_token = format!("{}.{}.{}", parts[0], tampered_payload, parts[2]);

    // 篡改后签名不匹配，应被拒绝
    let result = manager.verify_token(&tampered_token);
    assert!(
        result.is_err(),
        "篡改 claims 的 token 应被签名验证拒绝"
    );
}
