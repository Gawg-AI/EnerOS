//! EnerOS 安全合规测试套件 (v0.30.0 — T030-02)
//!
//! 提供 OWASP Top 10 安全测试、依赖漏洞扫描 (cargo audit) 和自定义 SAST 规则。
//!
//! ## 模块
//!
//! - [`owasp`] — OWASP Top 10 (2021) 安全测试用例
//! - [`dependency_audit`] — 依赖漏洞扫描，调用 `cargo audit`
//! - [`sast_rules`] — 自定义静态应用安全测试规则
//!
//! ## 用法
//!
//! ```no_run
//! use eneros_security_tests::sast_rules;
//!
//! let findings = sast_rules::scan_hardcoded_secrets();
//! assert!(findings.is_empty(), "发现硬编码密钥: {:?}", findings);
//! ```

pub mod dependency_audit;
pub mod owasp;
pub mod sast_rules;
