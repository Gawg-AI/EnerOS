//! 安全模块 — 密钥管理、Secure Boot 验证
//!
//! v0.24.0 引入，提供：
//! - `keystore`: 密钥存储抽象（SoftwareKeyStore / TpmKeyStore），支持版本管理与轮换
//! - `secure_boot`: Secure Boot 与内核锁定状态检测

pub mod keystore;
pub mod secure_boot;
