//! 自定义 SAST (Static Application Security Testing) 规则
//!
//! 扫描 `crates/` 目录下的 Rust 源文件，检测常见安全漏洞：
//! - 硬编码密钥
//! - 不安全反序列化
//! - SQL 注入
//! - 路径遍历

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// 发现的严重级别
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Low => write!(f, "low"),
            Severity::Medium => write!(f, "medium"),
            Severity::High => write!(f, "high"),
            Severity::Critical => write!(f, "critical"),
        }
    }
}

/// SAST 扫描发现
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// 文件路径（相对于 workspace 根目录）
    pub file: String,
    /// 行号
    pub line: usize,
    /// 严重级别
    pub severity: Severity,
    /// 描述
    pub description: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {}:{} — {}",
            self.severity, self.file, self.line, self.description
        )
    }
}

/// 获取 workspace 根目录下的 `crates/` 目录路径
fn crates_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest_dir)
        .parent() // tests/
        .and_then(|p| p.parent()) // workspace root
        .map(|p| p.join("crates"))
        .unwrap_or_else(|| PathBuf::from("../../crates"))
}

/// 递归收集目录下所有 `.rs` 文件
fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // 跳过 tests 子目录（测试代码，非生产代码）
                if path.file_name().and_then(|n| n.to_str()) == Some("tests") {
                    continue;
                }
                collect_rust_files(&path, files);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
}

/// 检查一行是否为注释
fn is_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with("*")
        || trimmed.starts_with("//!")
        || trimmed.starts_with("///")
}

/// 检查值是否为明显的占位符/测试值
fn is_placeholder_value(value: &str) -> bool {
    let lower = value.to_lowercase();
    // 常见占位符前缀模式
    const PLACEHOLDER_PREFIXES: &[&str] = &[
        "test-",
        "test_",
        "example-",
        "example_",
        "placeholder-",
        "placeholder_",
        "changeme",
        "your-",
        "your_",
        "dummy-",
        "dummy_",
        "fake-",
        "fake_",
        "sample-",
        "sample_",
        "todo",
        "fixme",
        "xxx",
        "default-",
        "default_",
    ];

    // 完全匹配的占位符
    const PLACEHOLDER_EXACT: &[&str] = &[
        "test",
        "example",
        "placeholder",
        "password",
        "secret",
        "none",
        "null",
        "empty",
        "secret-key",
        "my-secret",
        "test-secret",
        "changeme",
    ];

    if value.is_empty() || value.len() < 8 {
        return true;
    }

    // 完全匹配
    if PLACEHOLDER_EXACT.iter().any(|p| lower == *p) {
        return true;
    }

    // 前缀匹配
    PLACEHOLDER_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// 扫描硬编码密钥
///
/// 检测模式：
/// - `let password = "..."` / `let secret = "..."` / `let api_key = "..."` / `let token = "..."`
/// - `password: "..."` / `secret: "..."` 等结构体字段赋值
/// - 私钥块 (`-----BEGIN ... PRIVATE KEY-----`)
///
/// 排除：
/// - 注释行
/// - 占位符/测试值
/// - `env!()` / `option_env!()` 调用
pub fn scan_hardcoded_secrets() -> Vec<Finding> {
    let crates_path = crates_dir();
    let mut files = Vec::new();
    collect_rust_files(&crates_path, &mut files);

    // 匹配密钥赋值模式
    let secret_pattern = Regex::new(
        r#"(?i)(password|passwd|secret|api_?key|access_?key|private_?key|auth_?token|access_?token|bearer_?token)\s*[:=]\s*"([^"]*)""#,
    )
    .expect("正则表达式应编译成功");

    // 匹配私钥块
    let private_key_pattern = Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")
        .expect("正则表达式应编译成功");

    let mut findings = Vec::new();

    for file_path in &files {
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let relative_path = file_path
            .strip_prefix(crates_path.parent().unwrap_or(Path::new("")))
            .unwrap_or(file_path)
            .to_string_lossy()
            .replace('\\', "/");

        for (line_num, line) in content.lines().enumerate() {
            if is_comment_line(line) {
                continue;
            }

            // 检查私钥块
            if private_key_pattern.is_match(line) {
                findings.push(Finding {
                    file: relative_path.clone(),
                    line: line_num + 1,
                    severity: Severity::Critical,
                    description: "发现硬编码私钥块".to_string(),
                });
                continue;
            }

            // 检查密钥赋值
            if let Some(caps) = secret_pattern.captures(line) {
                let value = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                // 跳过 env! / option_env! 调用
                if line.contains("env!(") || line.contains("option_env!(") {
                    continue;
                }

                // 跳过占位符值
                if is_placeholder_value(value) {
                    continue;
                }

                let var_name = caps.get(1).map(|m| m.as_str()).unwrap_or("unknown");
                findings.push(Finding {
                    file: relative_path.clone(),
                    line: line_num + 1,
                    severity: Severity::High,
                    description: format!(
                        "可能的硬编码密钥: {} = \"{}\"",
                        var_name,
                        if value.len() > 20 {
                            format!("{}...", &value[..20])
                        } else {
                            value.to_string()
                        }
                    ),
                });
            }
        }
    }

    findings
}

/// 扫描不安全反序列化
///
/// 检测模式：
/// - `serde_json::from_str::<serde_json::Value>` (无类型校验的反序列化)
/// - `serde_json::from_slice::<serde_json::Value>`
/// - `serde_json::from_reader::<serde_json::Value>`
pub fn scan_unsafe_deserialization() -> Vec<Finding> {
    let crates_path = crates_dir();
    let mut files = Vec::new();
    collect_rust_files(&crates_path, &mut files);

    // 匹配反序列化到 serde_json::Value
    let pattern = Regex::new(r"serde_json::from_(str|slice|reader)\s*::\s*<\s*serde_json::Value\s*>")
        .expect("正则表达式应编译成功");

    // 匹配 from_str 后用 .as_object() / .as_array() 等无类型访问
    let untyped_pattern = Regex::new(r"serde_json::from_(str|slice|reader)\s*\([^)]*\)\s*\.unwrap\(\)\s*\.as_(object|array|str|u64|i64|f64|bool)")
        .expect("正则表达式应编译成功");

    let mut findings = Vec::new();

    for file_path in &files {
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let relative_path = file_path
            .strip_prefix(crates_path.parent().unwrap_or(Path::new("")))
            .unwrap_or(file_path)
            .to_string_lossy()
            .replace('\\', "/");

        for (line_num, line) in content.lines().enumerate() {
            if is_comment_line(line) {
                continue;
            }

            if pattern.is_match(line) {
                findings.push(Finding {
                    file: relative_path.clone(),
                    line: line_num + 1,
                    severity: Severity::Medium,
                    description: "不安全反序列化: 反序列化到 serde_json::Value 缺少类型校验".to_string(),
                });
            }

            if untyped_pattern.is_match(line) {
                findings.push(Finding {
                    file: relative_path.clone(),
                    line: line_num + 1,
                    severity: Severity::Low,
                    description: "潜在不安全反序列化: from_str 后直接访问无类型字段".to_string(),
                });
            }
        }
    }

    findings
}

/// 扫描 SQL 注入风险
///
/// 检测模式：
/// - `format!` 宏拼接 SQL 语句（包含 SELECT/INSERT/UPDATE/DELETE/WHERE 等关键字）
pub fn scan_sql_injection() -> Vec<Finding> {
    let crates_path = crates_dir();
    let mut files = Vec::new();
    collect_rust_files(&crates_path, &mut files);

    // 匹配 format! 宏中包含 SQL 关键字
    let pattern = Regex::new(r#"(?i)format!\s*\(\s*"[^"]*(?:SELECT|INSERT\s+INTO|UPDATE\s+|DELETE\s+FROM|WHERE\s+|JOIN\s+|UNION\s+SELECT)[^"]*"#)
        .expect("正则表达式应编译成功");

    let mut findings = Vec::new();

    for file_path in &files {
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let relative_path = file_path
            .strip_prefix(crates_path.parent().unwrap_or(Path::new("")))
            .unwrap_or(file_path)
            .to_string_lossy()
            .replace('\\', "/");

        for (line_num, line) in content.lines().enumerate() {
            if is_comment_line(line) {
                continue;
            }

            if pattern.is_match(line) {
                findings.push(Finding {
                    file: relative_path.clone(),
                    line: line_num + 1,
                    severity: Severity::High,
                    description: "潜在 SQL 注入: format! 宏拼接 SQL 语句".to_string(),
                });
            }
        }
    }

    findings
}

/// 扫描路径遍历风险
///
/// 检测模式：
/// - `Path::new(format!(...))` 或 `PathBuf::from(format!(...))` 使用 format! 构建路径
/// - `std::fs::read_to_string(format!(...))` 等文件操作使用 format! 拼接路径
pub fn scan_path_traversal() -> Vec<Finding> {
    let crates_path = crates_dir();
    let mut files = Vec::new();
    collect_rust_files(&crates_path, &mut files);

    // 匹配 Path::new(format!(...)) 等模式
    let path_format_pattern = Regex::new(r"(?:Path|PathBuf)::(?:new|from)\s*\(\s*format!")
        .expect("正则表达式应编译成功");

    // 匹配 fs::xxx(format!(...)) 模式
    let fs_format_pattern = Regex::new(r"fs::(?:read_to_string|read|write|create|remove_file|open)\s*\(\s*format!")
        .expect("正则表达式应编译成功");

    let mut findings = Vec::new();

    for file_path in &files {
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let relative_path = file_path
            .strip_prefix(crates_path.parent().unwrap_or(Path::new("")))
            .unwrap_or(file_path)
            .to_string_lossy()
            .replace('\\', "/");

        for (line_num, line) in content.lines().enumerate() {
            if is_comment_line(line) {
                continue;
            }

            if path_format_pattern.is_match(line) || fs_format_pattern.is_match(line) {
                findings.push(Finding {
                    file: relative_path.clone(),
                    line: line_num + 1,
                    severity: Severity::Medium,
                    description: "潜在路径遍历: 使用 format! 构建文件路径".to_string(),
                });
            }
        }
    }

    findings
}

/// 运行所有 SAST 规则
pub fn run_all_scans() -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(scan_hardcoded_secrets());
    findings.extend(scan_unsafe_deserialization());
    findings.extend(scan_sql_injection());
    findings.extend(scan_path_traversal());
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_hardcoded_secrets_returns_findings() {
        let findings = scan_hardcoded_secrets();
        // 应返回 Vec（可能为空）
        println!("硬编码密钥扫描发现 {} 项", findings.len());
        for f in &findings {
            println!("  {}", f);
        }
    }

    #[test]
    fn test_scan_unsafe_deserialization_returns_findings() {
        let findings = scan_unsafe_deserialization();
        println!("不安全反序列化扫描发现 {} 项", findings.len());
        for f in &findings {
            println!("  {}", f);
        }
    }

    #[test]
    fn test_scan_sql_injection_returns_findings() {
        let findings = scan_sql_injection();
        println!("SQL 注入扫描发现 {} 项", findings.len());
        for f in &findings {
            println!("  {}", f);
        }
    }

    #[test]
    fn test_scan_path_traversal_returns_findings() {
        let findings = scan_path_traversal();
        println!("路径遍历扫描发现 {} 项", findings.len());
        for f in &findings {
            println!("  {}", f);
        }
    }

    #[test]
    fn test_run_all_scans_returns_findings() {
        let findings = run_all_scans();
        println!("SAST 全量扫描发现 {} 项", findings.len());
        for f in &findings {
            println!("  {}", f);
        }
    }

    #[test]
    fn test_is_comment_line() {
        assert!(is_comment_line("// comment"));
        assert!(is_comment_line("/// doc comment"));
        assert!(is_comment_line("//! module doc"));
        assert!(is_comment_line("/* block comment"));
        assert!(is_comment_line(" * continuation"));
        assert!(!is_comment_line("let x = 1;"));
        assert!(!is_comment_line("code // inline comment"));
    }

    #[test]
    fn test_is_placeholder_value() {
        assert!(is_placeholder_value(""));
        assert!(is_placeholder_value("short"));
        assert!(is_placeholder_value("test-secret"));
        assert!(is_placeholder_value("example-key"));
        assert!(is_placeholder_value("placeholder-value"));
        assert!(is_placeholder_value("your-api-key"));
        assert!(!is_placeholder_value("AKIAIOSFODNN7EXAMPLE2"));
        assert!(!is_placeholder_value("a1b2c3d4e5f6g7h8i9j0"));
    }

    #[test]
    fn test_finding_display() {
        let finding = Finding {
            file: "src/main.rs".to_string(),
            line: 42,
            severity: Severity::High,
            description: "Test finding".to_string(),
        };
        let display = format!("{}", finding);
        assert!(display.contains("high"));
        assert!(display.contains("src/main.rs"));
        assert!(display.contains("42"));
        assert!(display.contains("Test finding"));
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(Severity::Low.to_string(), "low");
        assert_eq!(Severity::Medium.to_string(), "medium");
        assert_eq!(Severity::High.to_string(), "high");
        assert_eq!(Severity::Critical.to_string(), "critical");
    }
}
