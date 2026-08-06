//! EnerOS 加密支持包与 redaction 验证 (OBS-008).
//!
//! MASTER §21.13 OBS-008: "encrypted support bundle — redaction 验证"。
//! G8 Exit: "支持包可诊断且不泄露 secret/隐私数据"。
//!
//! 本 crate 提供：
//! - [`BundleItem`] — 单个诊断条目（名称 + 类型 + 数据）。
//! - [`Redactor`] — 基于模式的密钥/凭证脱敏引擎。
//! - [`SupportBundle`] — 收集条目 → 脱敏 → SM4-GCM 加密 → 导出。
//! - [`verify_no_secrets`] — 加密后扫描，确保无残留密钥模式。
//!
//! # 设计
//!
//! 1. 设备端收集诊断条目（日志、配置、crash dump、metrics 快照）。
//! 2. 对每个条目执行模式脱敏（PEM 私钥、password=、Bearer token、
//!    API key、SM2 私钥 hex 等）。
//! 3. 序列化为紧凑二进制 manifest + payload。
//! 4. 使用 SM4-GCM（AEAD）加密整个 payload，nonce 由 CSRNG 生成。
//! 5. 导出为 `.ensb` 文件，主机端用支持密钥解密。
//! 6. `verify_no_secrets` 在加密前对明文做最终扫描，任何残留密钥模式
//!    导致导出失败（fail-closed）。
//!
//! # no_std 合规
//!
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod format;
pub mod redact;

pub use format::{decrypt_bundle, encrypt_bundle, BundleError, BundleItem, SupportBundle};
pub use redact::{verify_no_secrets, Redactor, SecretPattern};
