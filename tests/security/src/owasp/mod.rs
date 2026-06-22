//! OWASP Top 10 (2021) 安全测试用例
//!
//! 覆盖 OWASP Top 10 的 10 个类别，验证 EnerOS 的安全控制措施：
//!
//! - [A01] 访问控制失效
//! - [A02] 加密失败
//! - [A03] 注入
//! - [A04] 不安全设计
//! - [A05] 安全配置错误
//! - [A06] 易受攻击组件
//! - [A07] 认证失败
//! - [A08] 软件和数据完整性
//! - [A09] 日志监控失败
//! - [A10] SSRF

pub mod a01_access_control;
pub mod a02_cryptographic_failures;
pub mod a03_injection;
pub mod a04_insecure_design;
pub mod a05_security_misconfiguration;
pub mod a06_vulnerable_components;
pub mod a07_auth_failures;
pub mod a08_software_integrity;
pub mod a09_logging_failures;
pub mod a10_ssrf;
