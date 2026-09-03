//! EnerOS v0.116.0 模型签名与白名单（P2-I 安全体系第 4 版）.
//!
//! 联邦场景下模型文件（LLM 权重 / Solver 参数 / Agent 策略）在分发与 OTA
//! 加载过程中存在被篡改、被替换、来源不可信的风险。本 crate 实现「模型
//! 签名 + 白名单」双重防线：模型数据先经 SM3 哈希登记入白名单（含有效
//! 期），加载前再以 SM2 签名验证来源可信且未被吊销，全部通过方可加载，
//! 并落审计日志。
//!
//! # 国标引用
//!
//! - GB/T 32905-2016 信息安全技术 SM3 密码杂凑算法（模型哈希登记）
//! - GB/T 32918.2-2017 信息安全技术 SM2 椭圆曲线公钥密码算法 第 2 部分：
//!   数字签名算法（模型签名验证，对原始消息验签，内部含 Z 值 + SM3）
//!
//! # 核心类型
//!
//! - [`WhitelistEntry`] / [`ModelWhitelist`] — 白名单：以 SM3 哈希为键登记
//!   模型条目（含 `added_at` / `expiry` 有效期），支持严格/宽松两种模式
//! - [`ModelSig`] / [`ModelSignatureVerifier`] — 签名验证：可信公钥列表 +
//!   吊销列表，经 keyring 类型化原语对原消息验签（63.3 WP-63.3.2 收口）
//! - [`ModelGuard`] / [`ModelLoadAttempt`] / [`GuardStats`] — 加载门禁：
//!   白名单检查 → 有效期检查 → 签名验证，全链路审计（容量 1000 环形覆盖）
//!   与统计
//! - [`GuardError`] / [`VerifyError`] / [`LoadResult`] — 错误模型与加载结果
//!
//! # 偏差声明（相对蓝图）
//!
//! 1. **BTreeMap 替代 HashMap**：no_std 下 `std::collections::HashMap` 不可
//!    用，白名单存储改用 `alloc::collections::BTreeMap`（`[u8; 32]` 有序键）。
//! 2. **外部 now 参数替代系统时钟**：no_std 环境无系统时钟，有效期判定由
//!    调用方传入 `now: u64`（Unix 时间戳），同 v0.113.0/v0.115.0 先例。
//! 3. **对原消息验签（GB/T 32918.2）**：`verify` 对模型原始数据经 keyring
//!    `verify_signature` 验签（底层 sm2_verify 内部完成 Z 值拼接 + SM3
//!    摘要），而非先手工 SM3 再验签，避免双重哈希与国标语义偏差；
//!    不匹配 = `Err(SigMismatch)`（63.3 收口，布尔陷阱消除）。
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `core::*` / `alloc::*`，唯一依赖 eneros-crypto（workspace 内 path
//! 依赖），零第三方依赖，零 unsafe，不调用 `panic!` / `todo!` /
//! `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;

use eneros_crypto::{CryptoError, Sm2PublicKey, Sm2Signature};

pub mod model_guard;
pub mod signature;
pub mod whitelist;

pub use model_guard::{ModelGuard, ModelLoadAttempt, AUDIT_CAPACITY};
pub use signature::ModelSignatureVerifier;
pub use whitelist::{ModelWhitelist, WhitelistEntry};

/// 签名验证错误（模型签名验证专用；`Crypto` 保留底层密码学错误原因）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// 签名者公钥已被吊销（命中吊销列表）.
    KeyRevoked,
    /// 签名者公钥不在可信列表（来源不可信）.
    KeyNotTrusted,
    /// 签名与数据不匹配（篡改/伪造）——63.3 WP-63.3.2：布尔陷阱收口，
    /// 原 `Ok(false)` 面改为显式 Err。
    SigMismatch,
    /// 底层密码学运算失败（验签过程内部错误，非"签名不匹配"）.
    Crypto(CryptoError),
}

impl From<CryptoError> for VerifyError {
    /// 密码学错误 → 验证错误映射：统一包装为 [`VerifyError::Crypto`].
    fn from(e: CryptoError) -> Self {
        VerifyError::Crypto(e)
    }
}

/// 模型加载门禁错误（≥4 变体；`VerifyFailed` 携带 [`VerifyError`] 保留拒绝原因）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    /// 模型哈希不在白名单（严格模式下拒绝）.
    NotInWhitelist,
    /// 白名单条目已过期（`entry.expiry < now`）.
    WhitelistEntryExpired,
    /// 签名与模型数据不匹配（验签不通过——数据被篡改或签名伪造）.
    SignatureInvalid,
    /// 签名验证失败（携带具体 [`VerifyError`] 原因：吊销/不可信/密码学错误）.
    VerifyFailed(VerifyError),
    /// 内部错误（不应发生的情形）.
    InternalError,
}

impl From<VerifyError> for GuardError {
    /// 验证错误 → 门禁错误映射：统一包装为 [`GuardError::VerifyFailed`].
    fn from(e: VerifyError) -> Self {
        GuardError::VerifyFailed(e)
    }
}

/// 模型加载结果（审计与统计口径）.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LoadResult {
    /// 允许加载（白名单 + 有效期 + 签名全部通过）.
    Allowed,
    /// 拒绝：哈希不在白名单（严格模式）.
    DeniedHash,
    /// 拒绝：签名验证失败（不可信 / 已吊销 / 签名不匹配）.
    DeniedSignature,
    /// 拒绝：白名单条目不存在（同 DeniedHash，语义细分预留）.
    DeniedWhitelist,
    /// 拒绝：白名单条目已过期.
    DeniedExpired,
}

/// 模型签名（SM2 签名 + 签名者公钥 + 签名者标识）.
#[derive(Clone)]
pub struct ModelSig {
    /// SM2 签名值（对模型原始数据，GB/T 32918.2）.
    pub signature: Sm2Signature,
    /// 签名者公钥（用于验签 + 可信/吊销列表比对）.
    pub signer_pubkey: Sm2PublicKey,
    /// 签名者标识（审计可读用途，如 "vendor-eneros-001"）.
    pub signer_id: String,
}

/// 门禁可观测统计（蓝图落地；全零起步）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardStats {
    /// 累计检查次数.
    pub checks: u64,
    /// 累计允许加载次数.
    pub allowed: u64,
    /// 累计拒绝次数.
    pub denied: u64,
    /// 最近一次检查结果（无检查为 `None`）.
    pub last_result: Option<LoadResult>,
}

impl GuardStats {
    /// 创建全零统计.
    pub fn new() -> Self {
        Self {
            checks: 0,
            allowed: 0,
            denied: 0,
            last_result: None,
        }
    }
}

impl Default for GuardStats {
    fn default() -> Self {
        Self::new()
    }
}
