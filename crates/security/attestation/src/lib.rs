//! EnerOS v0.114.0 测量启动与远程证明（P2-I 安全体系第 2 版）.
//!
//! 远程可验证系统完整性，建立联邦信任基础（蓝图 §1）。TPM PCR 度量值 + Quote +
//! Nonce 使验证方可远程确认边缘节点启动链未被篡改；无 TPM 则无法度量（§2 阻塞
//! 项），故蓝图 §4.4/§5.1 要求软件度量降级。v0.113.0 已落地 Secure Boot 信任链
//! （启动时「验签」），本版落地测量启动（启动时「度量存证」）+ 远程证明（「远程
//! 可验证」），为 v0.115.0 mTLS 与联邦可信验证奠基。
//!
//! # 核心类型
//!
//! - [`PcrBank`] / [`TpmBackend`] / [`SoftTpm`] / [`pcr_extend_value`] /
//!   [`quote_digest`] — TPM 抽象：24 个 SM3 PCR 单 bank（D9）、sync trait（D4）、
//!   软件 TPM（落地 §4.4 降级方案 + 故障注入）、TCG extend 共享函数（D7）、
//!   Quote 规范编码摘要（签名绑定 nonce 防重放，D6/D10）
//! - [`TcgEvent`] / [`TcgEventLog`] — TCG 事件日志：`measure` = SM3 摘要 +
//!   PCR extend + 事件追挂（度量即存证，§5.2）；`replay` 从零值链式重放重算
//!   PCR（D7/D8）
//! - [`PcrQuote`] / [`RemoteAttestation`] / [`AttestVerifier`] /
//!   [`AttestResult`] / [`AttestReason`] — 远程证明：Quote（nonce 内嵌随签名
//!   绑定，D10）+ 64B SM2 签名（修复蓝图签名永不填充 bug，D6）+ 本地验证器
//!   四步流水线（nonce → 验签 → 日志自一致性 → 期望值重放比对，D6/D11）
//! - [`TpmError`] / [`AttestError`] / [`AttestStats`] — 错误模型与证明统计
//!   （§9 可观测，D11）
//! - [`AttestTransport`] / [`MockAttestTransport`] — 远程传输抽象（D5，
//!   v0.110.0 SyncTransport / v0.111.0 OtaTransport 同先例）
//!
//! # 偏差声明（D1~D12，相对蓝图 §3/§4/§5/§6）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/attestation/` → `crates/security/attestation/`（eneros-attestation） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；远程证明属安全体系，与 crypto/iec62351/secure-boot 同属 security 子系统 |
//! | **D2** | 蓝图 `docs/phase2/attestation.md` → `docs/security/attestation-design.md` | 记忆 §2.3.3 强制：文档按方向分类（docs/security/ 已有 secure-boot-design.md 等先例） |
//! | **D3** | 蓝图 `tests/attestation.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.113.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 蓝图 extern "C" TPM FFI（tpm2_initialize/pcr_extend/pcr_read/quote + NonNull + unsafe + Drop shutdown）→ `TpmBackend` sync trait + `SoftTpm` 软件 TPM | 主机无 TPM 硬件不可测；no_std 阶段无 C 库链接；蓝图 §4.4/§5.1 本就要求「软件度量降级」——SoftTpm 即该降级方案的一等实现；真实 TPM2 FFI 适配器在集成层实现同一 trait；无 unsafe/NonNull |
//! | **D5** | 蓝图 `async verify_remote(server_url)` + HttpClient + serde_json → `AttestTransport` sync trait（`verify_remote(&RemoteAttestation)`）+ `MockAttestTransport` | no_std 无 async runtime/无 std::net（v0.110.0 D4 / v0.111.0 D4 同先例）；线上格式/HTTP 归集成层；本地验证逻辑由 AttestVerifier 承载可独立测试 |
//! | **D6** | 蓝图 `quote()` 不返回签名、`RemoteAttestation.signature = Vec::new()`（注释「TPM 签名」）永不填充的 bug → `quote()` 返回 `(PcrQuote, [u8; 64])`，SoftTpm 用内置 AK（SM2 密钥对）对 `quote_digest` 签名；验签归 `AttestVerifier` | 无签名的 Quote 不可远程证明（核心功能缺失）；蓝图 `signature: Vec<u8>` 修复为定长 64B |
//! | **D7** | 蓝图 `sm3_hash_concat` 未定义 → `pcr_extend_value(current, digest) = sm3(current ‖ digest)` 共享函数 | TCG PC Client 标准 extend 语义；SoftTpm/measure/replay 三方共用同一函数防实现分叉（v0.110.0 D11 CRC32 共享先例） |
//! | **D8** | 蓝图 `current_time_ms()` / `load_event_log()` 未定义全局函数 → `now: u64` 参数注入 + `TcgEventLog` 显式持有传递 | no_std 无系统时间/无全局状态（v0.110.0 D7、v0.111.0 D8 同先例）；集成层由 v0.12.0 RTC 供给时间 |
//! | **D9** | 蓝图 `HashAlgorithm { Sha256, Sm3 }` + `selected_banks: Vec<HashAlgorithm>` → SM3-only 单 bank（删除枚举与 Vec） | eneros-crypto 纯国密无 SHA-256（信创 §5.6 全程国密）；v0.111.0 D6 RsaSha256 占位同先例——不支持即不建模 |
//! | **D10** | ① 蓝图 `nonce.try_into().unwrap_or([0u8; 20])` 静默回退 → nonce 固定 `[u8; 20]` 参数，嵌入 PcrQuote 随 quote_digest 签名绑定；② 蓝图 `pcr_read(idx).unwrap_or([0u8; 32])` 吞错 → 显式错误传播；③ 蓝图 quote mask 位移未校验 idx → pcr_idx ≥ 24 返回 InvalidPcrIndex | 安全关键路径禁止静默默认值与吞错（v0.111.0 D11 同原则）；nonce 嵌入 quote 使签名显式覆盖防重放 |
//! | **D11** | 蓝图 `AttestResult.reason: String` → `AttestReason` 6 变体枚举（Verified/NonceMismatch/SignatureInvalid/EventLogInconsistent/PcrMismatch/ServerRejected，Copy）；新增事件日志自一致性检查（重放 attest.event_log 比对 quote）；新增 `AttestStats`（quotes_verified/trusted/untrusted/last_reason）落地 §9 可观测 | no_std Copy 错误模型对齐 v0.111.0/v0.113.0 惯例；String 理由不利机器审计；自一致性检查防止证明方提交与 quote 不符的日志 |
//! | **D12** | 性能「Quote < 100ms」（§6.3/§7.2）落地为 release 模式打印 + `ENEROS_PERF_GATE=1` 环境变量断言门禁 | v0.113.0 D13 已确立先例：主机纯 Rust SM2 实测超目标硬件指标（验签 161~214ms），目标硬件 SM2 加速后方可达标；口径文档化于设计文档 §7 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `core::*` / `alloc::*`，唯一依赖 eneros-crypto（workspace 内 path
//! 依赖），零第三方依赖，零 unsafe，零 extern "C"，不调用 `panic!` /
//! `todo!` / `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod attest;
pub mod event_log;
pub mod tpm;

pub use attest::{AttestReason, AttestResult, AttestVerifier, PcrQuote, RemoteAttestation};
pub use event_log::{TcgEvent, TcgEventLog};
pub use tpm::{pcr_extend_value, quote_digest, PcrBank, SoftTpm, TpmBackend};

/// TPM 错误（D4：删除蓝图 C 返回码 payload——FFI 已移除；Copy 对齐
/// v0.111.0/v0.113.0 错误模型惯例）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpmError {
    /// TPM 不可用（故障注入生效中 / 真实硬件不在位，蓝图 §4.4 软件降级触发面）.
    TpmUnavailable,
    /// PCR 索引越界（pcr_idx ≥ 24，D10③：蓝图 mask 位移未校验 idx）.
    InvalidPcrIndex,
    /// PCR extend 失败（保留变体：真实 TPM2 适配器返回扩展失败时使用）.
    ExtendFailed,
    /// PCR 读取失败（保留变体：真实 TPM2 适配器返回读取失败时使用）.
    ReadFailed,
    /// Quote 生成失败（含 SoftTpm 空 PCR 选择 / SM2 签名内部错误）.
    QuoteFailed,
}

/// 远程证明错误（D5/D6：6 变体，Copy 对齐 v0.111.0 OtaError 惯例）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestError {
    /// TPM 不可用（由 [`TpmError::TpmUnavailable`] 转换）.
    TpmUnavailable,
    /// PCR 索引越界（由 [`TpmError::InvalidPcrIndex`] 转换）.
    InvalidPcrIndex,
    /// Quote 生成失败（由 [`TpmError::QuoteFailed`] / `ExtendFailed` /
    /// `ReadFailed` 转换）.
    QuoteFailed,
    /// 空 PCR 选择（`RemoteAttestation::generate` 调用 quote 前自查，
    /// 蓝图 quote 空选择未定义行为显式化）.
    EmptyPcrSelection,
    /// 远程传输故障（[`AttestTransport`] 通道错误，蓝图 §4.4）.
    TransportError,
    /// 验证服务拒绝（蓝图 `ServerRejected(status)` 去 payload，HTTP 语义归
    /// 集成层，D5）.
    ServerRejected,
}

impl From<TpmError> for AttestError {
    /// TPM 错误 → 证明错误映射：TpmUnavailable/InvalidPcrIndex/QuoteFailed
    /// 同名转换；ExtendFailed/ReadFailed 归并 QuoteFailed.
    fn from(e: TpmError) -> Self {
        match e {
            TpmError::TpmUnavailable => AttestError::TpmUnavailable,
            TpmError::InvalidPcrIndex => AttestError::InvalidPcrIndex,
            TpmError::QuoteFailed | TpmError::ExtendFailed | TpmError::ReadFailed => {
                AttestError::QuoteFailed
            }
        }
    }
}

/// 远程证明统计（§9 可观测落地，D11）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttestStats {
    /// 累计验证的证明数（`AttestVerifier::verify` 每次调用 +1）.
    pub quotes_verified: u32,
    /// 累计判定可信次数.
    pub trusted: u32,
    /// 累计判定不可信次数.
    pub untrusted: u32,
    /// 最近一次验证结论（未验证过时为 None）.
    pub last_reason: Option<AttestReason>,
}

/// 远程传输抽象（D5：sync trait，no_std 无 async runtime/无 std::net；
/// 线上序列化/HTTP 语义归实现侧与集成层，v0.110.0 SyncTransport /
/// v0.111.0 OtaTransport 同先例）.
pub trait AttestTransport {
    /// 将证明发送至远端验证服务并返回判定结果.
    fn verify_remote(&mut self, attest: &RemoteAttestation) -> Result<AttestResult, AttestError>;
}

/// Mock 远程传输（测试用：预设结果 + 故障注入 + 调用计数）.
///
/// `calls` 统计 `verify_remote` 全部调用次数（含故障注入拦截的调用）。
pub struct MockAttestTransport {
    /// 预设返回结果（None 时返回 `Err(ServerRejected)`）.
    preset: Option<AttestResult>,
    /// 剩余故障注入次数（> 0 时逐次递减并返回 `Err(TransportError)`）.
    fail_remaining: u32,
    /// `verify_remote` 累计调用次数（含被故障注入拦截的调用）.
    pub calls: u32,
}

impl MockAttestTransport {
    /// 构造 Mock 传输（preset 为预设返回结果）.
    pub fn new(preset: Option<AttestResult>) -> Self {
        Self {
            preset,
            fail_remaining: 0,
            calls: 0,
        }
    }

    /// 注入后续 `count` 次传输故障（蓝图 §4.4 远程通道故障演练）.
    pub fn inject_failure(&mut self, count: u32) {
        self.fail_remaining = count;
    }
}

impl AttestTransport for MockAttestTransport {
    fn verify_remote(&mut self, _attest: &RemoteAttestation) -> Result<AttestResult, AttestError> {
        self.calls += 1;
        if self.fail_remaining > 0 {
            self.fail_remaining -= 1;
            return Err(AttestError::TransportError);
        }
        match &self.preset {
            Some(result) => Ok(result.clone()),
            None => Err(AttestError::ServerRejected),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::AttestReason;

    /// 构造一个预设 AttestResult.
    fn preset_result(trusted: bool, reason: AttestReason) -> AttestResult {
        AttestResult {
            trusted,
            pcr_mismatches: alloc::vec::Vec::new(),
            reason,
        }
    }

    // ============================================================
    // MOCK18：Mock 返回预设结果
    // ============================================================

    /// MOCK18 MockAttestTransport 返回预设结果且内容一致，calls == 1.
    #[test]
    fn mock18_mock_returns_preset_result() {
        let preset = preset_result(true, AttestReason::Verified);
        let mut mock = MockAttestTransport::new(Some(preset.clone()));
        // 构造一个最小 RemoteAttestation 作为入参（内容不被 Mock 消费）
        let attest = RemoteAttestation {
            quote: PcrQuote {
                pcr_select: alloc::vec![0, 1],
                pcr_values: alloc::vec![[0u8; 32], [0u8; 32]],
                nonce: [0xAB; 20],
                quote_time: 1_700_000_000,
            },
            signature: [0u8; 64],
            event_log: alloc::vec::Vec::new(),
        };
        let result = mock.verify_remote(&attest);
        assert_eq!(result, Ok(preset));
        assert_eq!(mock.calls, 1);
    }

    // ============================================================
    // MOCK19：传输故障注入（蓝图 §4.4 远程通道故障）
    // ============================================================

    /// MOCK19 注入 1 次失败：首次 Err(TransportError)，第二次返回 preset，
    /// calls == 2（含被拦截调用）；preset 为 None 时恢复后 Err(ServerRejected).
    #[test]
    fn mock19_transport_failure_injection() {
        let preset = preset_result(false, AttestReason::ServerRejected);
        let mut mock = MockAttestTransport::new(Some(preset.clone()));
        mock.inject_failure(1);
        let attest = RemoteAttestation {
            quote: PcrQuote {
                pcr_select: alloc::vec![0],
                pcr_values: alloc::vec![[0u8; 32]],
                nonce: [0xAB; 20],
                quote_time: 42,
            },
            signature: [0u8; 64],
            event_log: alloc::vec::Vec::new(),
        };
        // 首次：故障注入拦截
        assert_eq!(
            mock.verify_remote(&attest),
            Err(AttestError::TransportError)
        );
        // 第二次：恢复，返回预设结果
        assert_eq!(mock.verify_remote(&attest), Ok(preset));
        assert_eq!(mock.calls, 2);
        // preset 为 None 时返回 ServerRejected
        let mut mock_none = MockAttestTransport::new(None);
        assert_eq!(
            mock_none.verify_remote(&attest),
            Err(AttestError::ServerRejected)
        );
        assert_eq!(mock_none.calls, 1);
    }
}
