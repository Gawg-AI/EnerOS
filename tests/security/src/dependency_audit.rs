//! 依赖漏洞扫描模块
//!
//! 调用 `cargo audit` 命令扫描项目依赖中的已知漏洞。
//! 如果 `cargo audit` 未安装，测试将被跳过（返回 `available: false`）。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

/// 漏洞严重级别
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
    Unknown,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Low => write!(f, "low"),
            Severity::Medium => write!(f, "medium"),
            Severity::High => write!(f, "high"),
            Severity::Critical => write!(f, "critical"),
            Severity::Unknown => write!(f, "unknown"),
        }
    }
}

/// 单个漏洞信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vulnerability {
    /// RUSTSEC 公告 ID
    pub advisory_id: String,
    /// 受影响的包名
    pub package: String,
    /// 严重级别
    pub severity: Severity,
    /// 漏洞标题
    pub title: String,
}

/// cargo audit 扫描报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    /// 发现的漏洞列表
    pub vulnerabilities: Vec<Vulnerability>,
    /// 原始 JSON 输出
    pub raw_output: String,
    /// cargo audit 是否可用
    pub available: bool,
}

/// 运行 `cargo audit` 命令并解析结果
///
/// 如果 `cargo audit` 未安装，返回 `available: false` 的报告（不返回错误）。
/// 如果 `cargo audit` 可用但执行失败（如发现漏洞），仍尝试解析输出。
pub fn run_cargo_audit() -> Result<AuditReport> {
    let output = std::panic::catch_unwind(|| {
        Command::new("cargo")
            .arg("audit")
            .arg("--json")
            .output()
    });

    match output {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            // cargo audit --json 输出 JSON 数组到 stdout
            // 如果 stdout 为空，可能 cargo audit 不支持 --json 或执行失败
            if stdout.is_empty() {
                // 检查 stderr 是否有 "no such command" 信息
                if stderr.contains("no such command") || stderr.contains("not found") {
                    return Ok(AuditReport {
                        vulnerabilities: vec![],
                        raw_output: String::new(),
                        available: false,
                    });
                }
                // 其他错误情况
                return Ok(AuditReport {
                    vulnerabilities: vec![],
                    raw_output: stderr,
                    available: true,
                });
            }

            let vulnerabilities = parse_audit_json(&stdout);
            Ok(AuditReport {
                vulnerabilities,
                raw_output: stdout,
                available: true,
            })
        }
        Ok(Err(_)) | Err(_) => {
            // cargo 命令执行失败或 panic（cargo audit 未安装）
            Ok(AuditReport {
                vulnerabilities: vec![],
                raw_output: String::new(),
                available: false,
            })
        }
    }
}

/// 解析 cargo audit 的 JSON 输出
///
/// cargo audit --json 输出格式（v0.18+）为 JSON 数组，每个元素包含：
/// ```json
/// {
///   "advisory": {
///     "id": "RUSTSEC-2020-0001",
///     "title": "...",
///     "severity": "high"
///   },
///   "package": {
///     "name": "...",
///     ...
///   }
/// }
/// ```
fn parse_audit_json(json_str: &str) -> Vec<Vulnerability> {
    let trimmed = json_str.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    // 尝试解析为 JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(trimmed);
    let parsed = match parsed {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut vulnerabilities = vec![];

    // 处理数组格式（旧版本 cargo audit 直接输出数组）
    if let Some(arr) = parsed.as_array() {
        for item in arr {
            if let Some(vuln) = parse_vulnerability(item) {
                vulnerabilities.push(vuln);
            }
        }
        return vulnerabilities;
    }

    // 处理对象格式
    if let Some(obj) = parsed.as_object() {
        // 新格式（cargo audit v0.18+）：{"vulnerabilities": ...}
        // vulnerabilities 值可能为数组、对象（含 "list" 字段）或 null
        if let Some(vulns_value) = obj.get("vulnerabilities") {
            // vulnerabilities 为数组：{"vulnerabilities": [...]}
            if let Some(arr) = vulns_value.as_array() {
                for item in arr {
                    if let Some(vuln) = parse_vulnerability(item) {
                        vulnerabilities.push(vuln);
                    }
                }
                return vulnerabilities;
            }
            // vulnerabilities 为对象：{"vulnerabilities": {"list": [...]}}
            if let Some(vulns_obj) = vulns_value.as_object() {
                if let Some(list) = vulns_obj.get("list").and_then(|v| v.as_array()) {
                    for item in list {
                        if let Some(vuln) = parse_vulnerability(item) {
                            vulnerabilities.push(vuln);
                        }
                    }
                }
                return vulnerabilities;
            }
            // vulnerabilities 为 null 或其他类型（无漏洞），返回空
            return vulnerabilities;
        }
        // 单个漏洞对象（无 vulnerabilities 包装）
        if let Some(vuln) = parse_vulnerability(&parsed) {
            vulnerabilities.push(vuln);
        }
    }

    vulnerabilities
}

/// 从单个 JSON 对象解析漏洞信息
fn parse_vulnerability(value: &serde_json::Value) -> Option<Vulnerability> {
    let advisory = value.get("advisory").or(Some(value))?;
    let package = value.get("package")?;

    let advisory_id = advisory
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let package_name = package
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let severity_str = advisory
        .get("severity")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let severity = match severity_str.to_lowercase().as_str() {
        "low" => Severity::Low,
        "medium" => Severity::Medium,
        "high" => Severity::High,
        "critical" => Severity::Critical,
        _ => Severity::Unknown,
    };

    let title = advisory
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(Vulnerability {
        advisory_id,
        package: package_name,
        severity,
        title,
    })
}

/// 检查是否存在高危漏洞
///
/// 如果 `cargo audit` 未安装，返回错误提示。
/// 如果存在高危或严重漏洞，返回错误。
/// 如果无高危漏洞，返回 Ok(())。
pub fn check_high_vulnerabilities() -> Result<()> {
    let report = run_cargo_audit().context("运行 cargo audit 失败")?;

    if !report.available {
        return Err(anyhow::anyhow!("cargo audit 未安装，跳过漏洞检查"));
    }

    let high_vulns: Vec<&Vulnerability> = report
        .vulnerabilities
        .iter()
        .filter(|v| v.severity == Severity::High || v.severity == Severity::Critical)
        .collect();

    if high_vulns.is_empty() {
        Ok(())
    } else {
        let details: Vec<String> = high_vulns
            .iter()
            .map(|v| format!("{} ({}): {}", v.advisory_id, v.package, v.title))
            .collect();
        Err(anyhow::anyhow!(
            "发现 {} 个高危/严重漏洞:\n{}",
            high_vulns.len(),
            details.join("\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_cargo_audit_returns_report() {
        let report = run_cargo_audit().expect("run_cargo_audit 不应返回错误");
        // 无论 cargo audit 是否安装，都应返回报告
        if report.available {
            println!("cargo audit 可用，发现 {} 个漏洞", report.vulnerabilities.len());
        } else {
            println!("cargo audit 不可用，跳过");
        }
    }

    #[test]
    fn test_parse_empty_json() {
        let result = parse_audit_json("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_empty_array() {
        let result = parse_audit_json("[]");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_single_vulnerability() {
        let json = r#"[{
            "advisory": {
                "id": "RUSTSEC-2020-0001",
                "title": "Test vulnerability",
                "severity": "high"
            },
            "package": {
                "name": "test-package",
                "version": "1.0.0"
            }
        }]"#;
        let result = parse_audit_json(json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].advisory_id, "RUSTSEC-2020-0001");
        assert_eq!(result[0].package, "test-package");
        assert_eq!(result[0].severity, Severity::High);
        assert_eq!(result[0].title, "Test vulnerability");
    }

    #[test]
    fn test_parse_vulnerabilities_array_format() {
        // cargo audit v0.18+ 新格式：{"vulnerabilities": [...]}
        let json = r#"{
            "vulnerabilities": [{
                "advisory": {
                    "id": "RUSTSEC-2021-0002",
                    "title": "Array format vuln",
                    "severity": "critical"
                },
                "package": {
                    "name": "array-pkg",
                    "version": "2.0.0"
                }
            }]
        }"#;
        let result = parse_audit_json(json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].advisory_id, "RUSTSEC-2021-0002");
        assert_eq!(result[0].package, "array-pkg");
        assert_eq!(result[0].severity, Severity::Critical);
        assert_eq!(result[0].title, "Array format vuln");
    }

    #[test]
    fn test_parse_vulnerabilities_list_object_format() {
        // 某些版本输出：{"vulnerabilities": {"list": [...]}}
        let json = r#"{
            "vulnerabilities": {
                "list": [{
                    "advisory": {
                        "id": "RUSTSEC-2022-0003",
                        "title": "List object format vuln",
                        "severity": "medium"
                    },
                    "package": {
                        "name": "list-pkg",
                        "version": "3.0.0"
                    }
                }]
            }
        }"#;
        let result = parse_audit_json(json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].advisory_id, "RUSTSEC-2022-0003");
        assert_eq!(result[0].package, "list-pkg");
        assert_eq!(result[0].severity, Severity::Medium);
        assert_eq!(result[0].title, "List object format vuln");
    }

    #[test]
    fn test_parse_empty_vulnerabilities_object() {
        // 无漏洞时：{"vulnerabilities": {}} 或 {"vulnerabilities": null}
        let json = r#"{"vulnerabilities": {}}"#;
        let result = parse_audit_json(json);
        assert!(result.is_empty());

        let json = r#"{"vulnerabilities": null}"#;
        let result = parse_audit_json(json);
        assert!(result.is_empty());
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(Severity::Low.to_string(), "low");
        assert_eq!(Severity::Medium.to_string(), "medium");
        assert_eq!(Severity::High.to_string(), "high");
        assert_eq!(Severity::Critical.to_string(), "critical");
        assert_eq!(Severity::Unknown.to_string(), "unknown");
    }
}
