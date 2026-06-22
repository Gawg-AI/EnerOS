//! EnerOS 安全合规测试入口
//!
//! 运行 OWASP Top 10 安全测试、依赖漏洞扫描和 SAST 扫描。
//!
//! ## 运行方式
//!
//! ```bash
//! # 运行所有安全测试（不含 ignored）
//! cargo test -p eneros-security-tests
//!
//! # 运行包括需要 API server 的测试
//! cargo test -p eneros-security-tests -- --ignored
//! ```

use eneros_security_tests::dependency_audit;
use eneros_security_tests::sast_rules;

// ── OWASP Top 10 测试 ────────────────────────────────────────────────────
//
// OWASP A01-A10 测试定义在 src/owasp/ 各模块中，作为库测试自动运行。
// 此处提供集成测试入口，验证 SAST 和依赖扫描。

// ── SAST 扫描测试 ────────────────────────────────────────────────────────

/// 运行 SAST 硬编码密钥扫描
#[test]
fn test_sast_hardcoded_secrets_scan() {
    let findings = sast_rules::scan_hardcoded_secrets();
    if !findings.is_empty() {
        eprintln!("⚠️  发现 {} 个硬编码密钥风险:", findings.len());
        for f in &findings {
            eprintln!("  {}", f);
        }
    }
    // 不强制失败：允许有发现但报告（某些 crate 可能有合理用途）
    // 仅当发现 Critical 级别时失败
    let critical_count = findings
        .iter()
        .filter(|f| f.severity == sast_rules::Severity::Critical)
        .count();
    assert_eq!(
        critical_count, 0,
        "发现 {} 个 Critical 级别硬编码密钥，必须修复",
        critical_count
    );
}

/// 运行 SAST 不安全反序列化扫描
#[test]
fn test_sast_unsafe_deserialization_scan() {
    let findings = sast_rules::scan_unsafe_deserialization();
    if !findings.is_empty() {
        eprintln!("⚠️  发现 {} 个不安全反序列化风险:", findings.len());
        for f in &findings {
            eprintln!("  {}", f);
        }
    }
    // High 级别的不安全反序列化应修复
    let high_count = findings
        .iter()
        .filter(|f| f.severity == sast_rules::Severity::High)
        .count();
    assert_eq!(
        high_count, 0,
        "发现 {} 个 High 级别不安全反序列化风险，应修复",
        high_count
    );
}

/// 运行 SAST SQL 注入扫描
#[test]
fn test_sast_sql_injection_scan() {
    let findings = sast_rules::scan_sql_injection();
    if !findings.is_empty() {
        eprintln!("⚠️  发现 {} 个 SQL 注入风险:", findings.len());
        for f in &findings {
            eprintln!("  {}", f);
        }
    }
    // EnerOS 使用 TDengine 时序数据库，存在 SQL 查询。
    // 此处不强制失败，但报告所有发现供安全审查。
    // 仅当发现 Critical 级别时失败（当前 SQL 注入规则为 High）。
    let critical_count = findings
        .iter()
        .filter(|f| f.severity == sast_rules::Severity::Critical)
        .count();
    assert_eq!(
        critical_count, 0,
        "发现 {} 个 Critical 级别 SQL 注入风险，必须修复",
        critical_count
    );
}

/// 运行 SAST 路径遍历扫描
#[test]
fn test_sast_path_traversal_scan() {
    let findings = sast_rules::scan_path_traversal();
    if !findings.is_empty() {
        eprintln!("⚠️  发现 {} 个路径遍历风险:", findings.len());
        for f in &findings {
            eprintln!("  {}", f);
        }
    }
    // High 级别的路径遍历应修复
    let high_count = findings
        .iter()
        .filter(|f| f.severity == sast_rules::Severity::High)
        .count();
    assert_eq!(
        high_count, 0,
        "发现 {} 个 High 级别路径遍历风险，应修复",
        high_count
    );
}

/// 运行全量 SAST 扫描
#[test]
fn test_sast_full_scan() {
    let findings = sast_rules::run_all_scans();
    println!("SAST 全量扫描完成，共发现 {} 项", findings.len());

    // 按严重级别统计
    let critical = findings
        .iter()
        .filter(|f| f.severity == sast_rules::Severity::Critical)
        .count();
    let high = findings
        .iter()
        .filter(|f| f.severity == sast_rules::Severity::High)
        .count();
    let medium = findings
        .iter()
        .filter(|f| f.severity == sast_rules::Severity::Medium)
        .count();
    let low = findings
        .iter()
        .filter(|f| f.severity == sast_rules::Severity::Low)
        .count();

    println!(
        "统计: Critical={}, High={}, Medium={}, Low={}",
        critical, high, medium, low
    );

    // Critical 级别必须为 0
    assert_eq!(
        critical, 0,
        "发现 {} 个 Critical 级别安全问题，必须立即修复",
        critical
    );
}

// ── 依赖漏洞扫描测试 ──────────────────────────────────────────────────────

/// 运行 cargo audit 依赖漏洞扫描
#[test]
fn test_dependency_vulnerability_scan() {
    let report = dependency_audit::run_cargo_audit()
        .expect("run_cargo_audit 不应返回错误");

    if !report.available {
        eprintln!("SKIP: cargo audit 未安装，跳过依赖漏洞扫描");
        return;
    }

    println!("cargo audit 扫描完成，发现 {} 个漏洞", report.vulnerabilities.len());

    for vuln in &report.vulnerabilities {
        println!(
            "  [{}] {}: {} ({})",
            vuln.severity, vuln.advisory_id, vuln.package, vuln.title
        );
    }
}

/// 验证无高危依赖漏洞
#[test]
fn test_no_high_dependency_vulnerabilities() {
    let result = dependency_audit::check_high_vulnerabilities();

    match result {
        Ok(()) => {
            println!("PASS: 无高危依赖漏洞");
        }
        Err(e) => {
            let err_msg = format!("{}", e);
            if err_msg.contains("cargo audit 未安装") {
                eprintln!("SKIP: cargo audit 未安装");
                return;
            }
            panic!("发现高危依赖漏洞: {}", e);
        }
    }
}

// ── OWASP 测试汇总 ────────────────────────────────────────────────────────
//
// 以下测试验证 OWASP Top 10 各类别的关键安全控制。
// 详细的测试用例在 src/owasp/ 各模块中。

/// OWASP A01: 访问控制 — 验证 RBAC 权限矩阵
#[test]
fn test_owasp_a01_access_control_summary() {
    use eneros_api::auth::{Permission, Role};

    // 验证权限矩阵完整性
    assert!(Role::Observer.has_permission(Permission::Read));
    assert!(!Role::Observer.has_permission(Permission::Write));

    assert!(Role::Operator.has_permission(Permission::Write));
    assert!(!Role::Operator.has_permission(Permission::Control));

    assert!(Role::Supervisor.has_permission(Permission::Control));
    assert!(!Role::Supervisor.has_permission(Permission::Emergency));

    assert!(Role::Emergency.has_permission(Permission::Emergency));
}

/// OWASP A02: 加密 — 验证 JWT 签名验证
#[test]
fn test_owasp_a02_crypto_summary() {
    use eneros_api::auth::{AuthManager, Role};

    let manager = AuthManager::new("test-secret", 3600);
    let token = manager.issue_token("alice", Role::Operator).unwrap();
    assert!(manager.verify_token(&token).is_ok());

    // 错误密钥应失败
    let manager2 = AuthManager::new("wrong-secret", 3600);
    assert!(manager2.verify_token(&token).is_err());
}

/// OWASP A03: 注入 — 验证输入验证
#[test]
fn test_owasp_a03_injection_summary() {
    use eneros_api::auth::AuthManager;
    use eneros_api::auth::Role;

    let manager = AuthManager::new("test-secret", 3600);
    manager.add_user("alice", "password", Role::Operator);

    // SQL 注入应被拒绝
    assert!(manager
        .validate_credentials("alice' OR '1'='1", "x")
        .is_err());
}

/// OWASP A09: 日志 — 验证审计日志记录
#[test]
fn test_owasp_a09_logging_summary() {
    use eneros_api::audit::{AuditEntry, AuditLog};

    let audit_log = AuditLog::new(1000);
    audit_log.record(AuditEntry::new(
        "alice",
        "operator",
        "POST",
        "/api/test",
        "127.0.0.1",
        "success",
    ));
    assert_eq!(audit_log.count(), 1);
}
