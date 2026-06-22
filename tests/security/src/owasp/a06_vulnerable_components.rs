//! A06:2021 — 易受攻击组件 (Vulnerable and Outdated Components)
//!
//! 测试 EnerOS 的依赖漏洞扫描能力，验证：
//! - cargo audit 集成
//! - 高危漏洞为零

#[cfg(test)]
use crate::dependency_audit;

/// 验证 cargo audit 可调用（或正确跳过）
#[test]
fn test_cargo_audit_integration() {
    let report = dependency_audit::run_cargo_audit()
        .expect("run_cargo_audit 不应返回错误（即使 cargo audit 未安装也应返回 available=false）");

    if !report.available {
        eprintln!("SKIP: cargo audit 未安装，跳过依赖漏洞扫描");
        return;
    }

    // 如果 cargo audit 可用，验证报告结构
    println!(
        "cargo audit 报告: {} 个漏洞",
        report.vulnerabilities.len()
    );
}

/// 验证无高危漏洞（如果 cargo audit 可用）
#[test]
fn test_no_high_vulnerabilities() {
    let result = dependency_audit::check_high_vulnerabilities();

    match result {
        Ok(()) => {
            println!("PASS: 无高危依赖漏洞");
        }
        Err(e) => {
            // 如果 cargo audit 未安装，跳过
            let err_msg = format!("{}", e);
            if err_msg.contains("cargo audit 未安装") {
                eprintln!("SKIP: cargo audit 未安装");
                return;
            }
            panic!("发现高危依赖漏洞: {}", e);
        }
    }
}

/// 验证依赖漏洞报告可序列化
#[test]
fn test_audit_report_serialization() {
    let report = dependency_audit::AuditReport {
        vulnerabilities: vec![],
        raw_output: String::new(),
        available: false,
    };

    let json = serde_json::to_string(&report).expect("报告应可序列化为 JSON");
    assert!(json.contains("vulnerabilities"), "JSON 应包含 vulnerabilities 字段");
}
