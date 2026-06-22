//! A09:2021 — 日志监控失败 (Security Logging and Monitoring Failures)
//!
//! 测试 EnerOS 的审计日志机制，验证：
//! - 审计日志记录
//! - 日志完整性

#[cfg(test)]
use eneros_api::audit::{AuditEntry, AuditLog};

/// 验证审计日志可记录安全事件
#[test]
fn test_audit_log_records_security_events() {
    let audit_log = AuditLog::new(1000);

    let entry = AuditEntry::new(
        "alice",
        "operator",
        "POST",
        "/api/power-flow",
        "127.0.0.1",
        "success",
    );
    audit_log.record(entry);

    assert_eq!(audit_log.count(), 1, "应记录 1 个审计条目");
}

/// 验证审计日志可记录失败事件
#[test]
fn test_audit_log_records_failed_events() {
    let audit_log = AuditLog::new(1000);

    let entry = AuditEntry::new(
        "unknown",
        "anonymous",
        "POST",
        "/api/auth/login",
        "192.168.1.100",
        "failed",
    )
    .with_detail("Invalid credentials");
    audit_log.record(entry);

    let entries = audit_log.query(None, Some("failed"), 10);
    assert_eq!(entries.len(), 1, "应能查询失败事件");
    assert_eq!(entries[0].result, "failed");
    assert_eq!(entries[0].detail.as_deref(), Some("Invalid credentials"));
}

/// 验证审计日志可按 actor 过滤
#[test]
fn test_audit_log_filter_by_actor() {
    let audit_log = AuditLog::new(1000);

    audit_log.record(AuditEntry::new(
        "alice", "operator", "GET", "/api/agents", "127.0.0.1", "success",
    ));
    audit_log.record(AuditEntry::new(
        "bob", "observer", "GET", "/api/agents", "127.0.0.1", "success",
    ));
    audit_log.record(AuditEntry::new(
        "alice", "operator", "POST", "/api/power-flow", "127.0.0.1", "success",
    ));

    let alice_entries = audit_log.query(Some("alice"), None, 10);
    assert_eq!(alice_entries.len(), 2, "应能按 actor 过滤");

    let bob_entries = audit_log.query(Some("bob"), None, 10);
    assert_eq!(bob_entries.len(), 1, "bob 应只有 1 条记录");
}

/// 验证审计日志可按结果过滤
#[test]
fn test_audit_log_filter_by_result() {
    let audit_log = AuditLog::new(1000);

    audit_log.record(AuditEntry::new(
        "alice", "operator", "POST", "/api/test", "127.0.0.1", "success",
    ));
    audit_log.record(AuditEntry::new(
        "alice", "operator", "POST", "/api/test", "127.0.0.1", "failed",
    ));
    audit_log.record(AuditEntry::new(
        "bob", "observer", "POST", "/api/test", "127.0.0.1", "denied",
    ));

    let success_entries = audit_log.query(None, Some("success"), 10);
    assert_eq!(success_entries.len(), 1, "应有 1 条 success 记录");

    let failed_entries = audit_log.query(None, Some("failed"), 10);
    assert_eq!(failed_entries.len(), 1, "应有 1 条 failed 记录");

    let denied_entries = audit_log.query(None, Some("denied"), 10);
    assert_eq!(denied_entries.len(), 1, "应有 1 条 denied 记录");
}

/// 验证审计日志条目包含完整信息
#[test]
fn test_audit_entry_contains_complete_info() {
    let entry = AuditEntry::new(
        "alice",
        "operator",
        "POST",
        "/api/actions/structured",
        "192.168.1.50",
        "success",
    )
    .with_detail("Executed control action: open breaker BRK-001");

    assert_eq!(entry.actor, "alice");
    assert_eq!(entry.role, "operator");
    assert_eq!(entry.method, "POST");
    assert_eq!(entry.path, "/api/actions/structured");
    assert_eq!(entry.client_ip, "192.168.1.50");
    assert_eq!(entry.result, "success");
    assert!(entry.detail.is_some());
    assert!(!entry.id.is_empty());
    assert!(entry.timestamp > 0);
}

/// 验证审计日志可清空
#[test]
fn test_audit_log_clear() {
    let audit_log = AuditLog::new(1000);

    audit_log.record(AuditEntry::new(
        "alice", "operator", "POST", "/api/test", "127.0.0.1", "success",
    ));
    assert_eq!(audit_log.count(), 1);

    audit_log.clear();
    assert_eq!(audit_log.count(), 0, "清空后应无记录");
}

/// 验证审计日志查询限制
#[test]
fn test_audit_log_query_limit() {
    let audit_log = AuditLog::new(1000);

    for i in 0..20 {
        audit_log.record(AuditEntry::new(
            format!("user-{}", i),
            "operator",
            "POST",
            "/api/test",
            "127.0.0.1",
            "success",
        ));
    }

    let limited = audit_log.query(None, None, 5);
    assert_eq!(limited.len(), 5, "查询限制应生效");
}

/// 验证审计日志默认容量
#[test]
fn test_audit_log_default_capacity() {
    let audit_log = AuditLog::default();
    // 默认容量应为 10000
    for i in 0..15 {
        audit_log.record(AuditEntry::new(
            format!("user-{}", i),
            "operator",
            "POST",
            "/api/test",
            "127.0.0.1",
            "success",
        ));
    }
    assert_eq!(audit_log.count(), 15, "默认容量应能容纳 15 条");
}
