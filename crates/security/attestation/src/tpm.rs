//! TPM 抽象与软件 TPM（v0.114.0，D4：蓝图 extern "C" FFI → sync trait +
//! SoftTpm 软件降级一等实现）.
//!
//! 蓝图 §2 阻塞项「无 TPM 则无法度量」由 [`SoftTpm`] 落地 §4.4/§5.1「软件
//! 度量降级」：同一 [`TpmBackend`] trait 下，真实 TPM2 FFI 适配器由集成层
//! 实现，本 crate 不提供 unsafe/extern "C"/NonNull 代码。
//!
//! PCR 语义对齐 TCG PC Client：24 个 PCR、SM3-only 单 bank（D9）、extend =
//! `sm3(current ‖ digest)`（[`pcr_extend_value`] 共享函数，SoftTpm /
//! `TcgEventLog::measure` / `TcgEventLog::replay` 三方共用防实现分叉，D7）、
//! PCR 仅随新实例复位（蓝图 §8.5 坑点：真实 TPM PCR 仅随复位清零）。

use alloc::vec::Vec;
use core::cell::Cell;

use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair, Sm2PublicKey, Sm3Hasher};

use crate::attest::PcrQuote;
use crate::TpmError;

/// PCR 寄存器数量（TCG PC Client 规范 PCR 0~23）.
pub const PCR_COUNT: usize = 24;

/// PCR bank（SM3-only 单 bank，D9：删除蓝图 HashAlgorithm 枚举与
/// selected_banks Vec——eneros-crypto 纯国密无 SHA-256，不支持即不建模）.
///
/// 初始全零（TCG 复位态）；PCR 仅随新实例复位（蓝图 §8.5 坑点文档化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcrBank {
    /// 24 个 PCR 寄存器当前值（每个 32 字节 SM3 摘要）.
    pub pcr_values: [[u8; 32]; PCR_COUNT],
}

impl Default for PcrBank {
    /// TCG 复位态：全部 PCR 清零.
    fn default() -> Self {
        Self {
            pcr_values: [[0u8; 32]; PCR_COUNT],
        }
    }
}

/// TCG extend 共享函数：`sm3(current ‖ digest)`（D7）.
///
/// SoftTpm 本地 bank 更新 / `TcgEventLog::measure` 后的重放 /
/// `TcgEventLog::replay` 三方共用同一函数，防实现分叉（v0.110.0 D11 CRC32
/// 共享先例）。
pub fn pcr_extend_value(current: &[u8; 32], digest: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sm3Hasher::new();
    hasher.update(current);
    hasher.update(digest);
    hasher.finalize()
}

/// Quote 规范编码摘要（D6/D10：签名绑定 nonce 防重放）.
///
/// SM3 over 规范编码：
/// `pcr_count u8 ‖ 每 pcr_idx u8 ‖ 每 pcr_value 32B ‖ nonce 20B ‖
/// quote_time u64 LE`。
pub fn quote_digest(quote: &PcrQuote) -> [u8; 32] {
    let mut hasher = Sm3Hasher::new();
    hasher.update(&[quote.pcr_select.len() as u8]);
    for &idx in &quote.pcr_select {
        hasher.update(&[idx]);
    }
    for value in &quote.pcr_values {
        hasher.update(value);
    }
    hasher.update(&quote.nonce);
    hasher.update(&quote.quote_time.to_le_bytes());
    hasher.finalize()
}

/// TPM 后端抽象（sync trait，no_std 单线程惯例，D4；无 Send+Sync 要求）.
///
/// # 实现细化（规格允许，文档注明）
///
/// 蓝图 `quote(&self, pcr_indices, nonce)` 不返回签名（D6 修复为返回
/// `(PcrQuote, [u8; 64])` 二元组）；SM2 签名必须随机数，故 `quote` 追加
/// `rng: &mut CsRng` 参数——真实 TPM2 适配器内部签名不需要外部随机数时可
/// 忽略该参数。
pub trait TpmBackend {
    /// 扩展 PCR：`PCR[idx] = sm3(PCR[idx] ‖ digest)`；idx ≥ 24 →
    /// `Err(InvalidPcrIndex)`（D10③）.
    fn pcr_extend(&mut self, pcr_idx: u8, digest: &[u8; 32]) -> Result<(), TpmError>;

    /// 读取 PCR 当前值；idx ≥ 24 → `Err(InvalidPcrIndex)`.
    fn pcr_read(&self, pcr_idx: u8) -> Result<[u8; 32], TpmError>;

    /// 生成 PCR Quote + 签名二元组（D6：修复蓝图签名永不填充 bug）.
    ///
    /// - `pcr_indices` 为空 → `Err(QuoteFailed)`（注：spec 中
    ///   `EmptyPcrSelection` 属 `AttestError`，`RemoteAttestation::generate`
    ///   在调用 quote 前自查空选择；SoftTpm 层归并 QuoteFailed）
    /// - 逐个索引收集 PCR 值，错误显式传播（禁蓝图
    ///   `unwrap_or([0u8; 32])` 吞错，D10②）
    /// - `now` 时间戳参数注入（D8：no_std 无系统时间，集成层由 v0.12.0
    ///   RTC 供给）
    fn quote(
        &mut self,
        pcr_indices: &[u8],
        nonce: &[u8; 20],
        now: u64,
        rng: &mut CsRng,
    ) -> Result<(PcrQuote, [u8; 64]), TpmError>;

    /// 返回证明密钥（AK）公钥（验证方据此验签 Quote）.
    fn attestation_pubkey(&self) -> Sm2PublicKey;
}

/// 软件 TPM（D4：蓝图 §4.4/§5.1 软件度量降级方案的一等实现 + 故障注入）.
///
/// - 内置 AK（SM2 密钥对，`new` 时经 CSRNG 生成）
/// - PCR 仅随新实例复位（蓝图 §8.5 坑点）
/// - `inject_failure` 注入后续 N 次 `Err(TpmUnavailable)`（蓝图 §6.5 故障
///   注入演练）；`fail_remaining` 用 `Cell` 使 `&self` 的 `pcr_read` 也能
///   消费故障次数
pub struct SoftTpm {
    /// 本地 PCR bank（SM3-only 单 bank）.
    bank: PcrBank,
    /// 证明密钥（Attestation Key，SM2 密钥对）.
    ak: Sm2KeyPair,
    /// 剩余故障注入次数（Cell 内部可变性：`pcr_read(&self)` 亦可消费）.
    fail_remaining: Cell<u32>,
}

impl SoftTpm {
    /// 构造软件 TPM：PCR 全零复位 + CSRNG 生成 AK.
    ///
    /// `Sm2KeyPair::generate` 内部已对标量越界无限重试，仅 CSRNG 内部错误
    /// 才会返回 Err（正常路径不发生），此处防御性重试直至成功（禁 panic）。
    pub fn new(rng: &mut CsRng) -> Self {
        let ak = loop {
            if let Ok(kp) = Sm2KeyPair::generate(rng) {
                break kp;
            }
        };
        Self {
            bank: PcrBank::default(),
            ak,
            fail_remaining: Cell::new(0),
        }
    }

    /// 注入后续 `count` 次 TPM 故障（所有操作方法逐次消费，蓝图 §6.5）.
    pub fn inject_failure(&mut self, count: u32) {
        self.fail_remaining.set(count);
    }

    /// 故障门禁：fail_remaining > 0 时递减并返回 `Err(TpmUnavailable)`.
    fn gate(&self) -> Result<(), TpmError> {
        let remaining = self.fail_remaining.get();
        if remaining > 0 {
            self.fail_remaining.set(remaining - 1);
            return Err(TpmError::TpmUnavailable);
        }
        Ok(())
    }
}

impl TpmBackend for SoftTpm {
    fn pcr_extend(&mut self, pcr_idx: u8, digest: &[u8; 32]) -> Result<(), TpmError> {
        self.gate()?;
        if pcr_idx as usize >= PCR_COUNT {
            return Err(TpmError::InvalidPcrIndex);
        }
        let current = &mut self.bank.pcr_values[pcr_idx as usize];
        *current = pcr_extend_value(current, digest);
        Ok(())
    }

    fn pcr_read(&self, pcr_idx: u8) -> Result<[u8; 32], TpmError> {
        self.gate()?;
        if pcr_idx as usize >= PCR_COUNT {
            return Err(TpmError::InvalidPcrIndex);
        }
        Ok(self.bank.pcr_values[pcr_idx as usize])
    }

    fn quote(
        &mut self,
        pcr_indices: &[u8],
        nonce: &[u8; 20],
        now: u64,
        rng: &mut CsRng,
    ) -> Result<(PcrQuote, [u8; 64]), TpmError> {
        self.gate()?;
        // 空 PCR 选择（SoftTpm 层归并 QuoteFailed；AttestError::
        // EmptyPcrSelection 由 RemoteAttestation::generate 调用前自查）
        if pcr_indices.is_empty() {
            return Err(TpmError::QuoteFailed);
        }
        // 逐个索引收集 PCR 值（越界显式 Err，禁蓝图 unwrap_or 吞错，D10②）
        let mut pcr_values = Vec::with_capacity(pcr_indices.len());
        for &idx in pcr_indices {
            if idx as usize >= PCR_COUNT {
                return Err(TpmError::InvalidPcrIndex);
            }
            pcr_values.push(self.bank.pcr_values[idx as usize]);
        }
        let quote = PcrQuote {
            pcr_select: pcr_indices.to_vec(),
            pcr_values,
            nonce: *nonce,
            quote_time: now,
        };
        // SM2 签名 quote_digest（签名绑定 nonce 防重放，D6/D10）
        let digest = quote_digest(&quote);
        let signature = match sm2_sign(&digest, &self.ak.private_key, &self.ak.public_key, rng) {
            Ok(sig) => sig,
            Err(_) => return Err(TpmError::QuoteFailed),
        };
        Ok((quote, signature.to_bytes()))
    }

    fn attestation_pubkey(&self) -> Sm2PublicKey {
        self.ak.public_key
    }
}

#[cfg(test)]
mod tests {
    use eneros_crypto::{sm2_verify, Sm2Signature};

    use super::*;

    /// 构造 SoftTpm + CsRng 测试对.
    fn make_tpm() -> (SoftTpm, CsRng) {
        let mut rng = CsRng::new();
        let tpm = SoftTpm::new(&mut rng);
        (tpm, rng)
    }

    // ============================================================
    // TPM1：SoftTpm 初始 PCR 全零
    // ============================================================

    /// TPM1 新实例 24 个 PCR 全部为零值（TCG 复位态）.
    #[test]
    fn tpm1_initial_pcrs_all_zero() {
        let (tpm, _rng) = make_tpm();
        for idx in 0..PCR_COUNT as u8 {
            assert_eq!(tpm.pcr_read(idx), Ok([0u8; 32]), "PCR{} 应为零值", idx);
        }
    }

    // ============================================================
    // TPM2：extend 确定性（蓝图 §6.1）
    // ============================================================

    /// TPM2 对 PCR0 extend digest D 后 == sm3([0u8;32] ‖ D).
    #[test]
    fn tpm2_extend_deterministic() {
        let (mut tpm, _rng) = make_tpm();
        let digest = [0x11u8; 32];
        assert_eq!(tpm.pcr_extend(0, &digest), Ok(()));
        let expected = pcr_extend_value(&[0u8; 32], &digest);
        assert_eq!(tpm.pcr_read(0), Ok(expected));
        // 其余 PCR 不受影响
        assert_eq!(tpm.pcr_read(1), Ok([0u8; 32]));
    }

    // ============================================================
    // TPM3：extend 链式非幂等（蓝图 §6.1）
    // ============================================================

    /// TPM3 同一 digest 连续 extend 两次 == sm3(sm3(0‖D) ‖ D)（链式非幂等）.
    #[test]
    fn tpm3_extend_chained_not_idempotent() {
        let (mut tpm, _rng) = make_tpm();
        let digest = [0x22u8; 32];
        assert_eq!(tpm.pcr_extend(0, &digest), Ok(()));
        assert_eq!(tpm.pcr_extend(0, &digest), Ok(()));
        let once = pcr_extend_value(&[0u8; 32], &digest);
        let twice = pcr_extend_value(&once, &digest);
        assert_eq!(tpm.pcr_read(0), Ok(twice));
        assert_ne!(once, twice, "extend 应非幂等");
    }

    // ============================================================
    // TPM4：pcr_idx = 24 越界（蓝图 §6.5，D10③）
    // ============================================================

    /// TPM4 pcr_extend(24) / pcr_read(24) / quote 含索引 24 均
    /// Err(InvalidPcrIndex)；quote 空选择 Err(QuoteFailed).
    #[test]
    fn tpm4_pcr_index_out_of_range() {
        let (mut tpm, mut rng) = make_tpm();
        let digest = [0x33u8; 32];
        assert_eq!(
            tpm.pcr_extend(PCR_COUNT as u8, &digest),
            Err(TpmError::InvalidPcrIndex)
        );
        assert_eq!(
            tpm.pcr_read(PCR_COUNT as u8),
            Err(TpmError::InvalidPcrIndex)
        );
        assert_eq!(
            tpm.quote(&[0, PCR_COUNT as u8], &[0xAB; 20], 1000, &mut rng),
            Err(TpmError::InvalidPcrIndex)
        );
        // 空 PCR 选择
        assert_eq!(
            tpm.quote(&[], &[0xAB; 20], 1000, &mut rng),
            Err(TpmError::QuoteFailed)
        );
    }

    // ============================================================
    // TPM5：故障注入（蓝图 §6.5）
    // ============================================================

    /// TPM5 inject_failure(1) 后任意操作 Err(TpmUnavailable) 且后续恢复；
    /// pcr_read(&self) 同样消费故障次数.
    #[test]
    fn tpm5_failure_injection() {
        let (mut tpm, _rng) = make_tpm();
        let digest = [0x44u8; 32];
        // extend 路径：注入 1 次 → 首次失败，第二次恢复
        tpm.inject_failure(1);
        assert_eq!(tpm.pcr_extend(0, &digest), Err(TpmError::TpmUnavailable));
        assert_eq!(tpm.pcr_extend(0, &digest), Ok(()));
        // read 路径：注入 1 次 → 首次失败，第二次恢复
        tpm.inject_failure(1);
        assert_eq!(tpm.pcr_read(0), Err(TpmError::TpmUnavailable));
        assert_ne!(tpm.pcr_read(0), Err(TpmError::TpmUnavailable));
        // 注入 2 次 → 连续两次失败
        tpm.inject_failure(2);
        assert_eq!(tpm.pcr_read(0), Err(TpmError::TpmUnavailable));
        assert_eq!(tpm.pcr_read(0), Err(TpmError::TpmUnavailable));
        assert!(tpm.pcr_read(0).is_ok());
    }

    // ============================================================
    // TPM6：quote 返回签名可验（蓝图 §6.1 Quote 签名）
    // ============================================================

    /// TPM6 quote 返回 (PcrQuote, 64B 签名)：AK 公钥 sm2_verify == true；
    /// nonce / quote_time / pcr_select / pcr_values 与度量一致.
    #[test]
    fn tpm6_quote_signature_verifiable() {
        let (mut tpm, mut rng) = make_tpm();
        // 度量两笔
        let d0 = [0x55u8; 32];
        let d1 = [0x66u8; 32];
        assert_eq!(tpm.pcr_extend(0, &d0), Ok(()));
        assert_eq!(tpm.pcr_extend(1, &d1), Ok(()));
        let nonce = [0xAB; 20];
        let now = 1_700_000_000u64;
        let (quote, sig_bytes) = tpm
            .quote(&[0, 1, 2], &nonce, now, &mut rng)
            .expect("quote 应成功");
        // 字段断言
        assert_eq!(quote.pcr_select, alloc::vec![0, 1, 2]);
        assert_eq!(quote.nonce, nonce);
        assert_eq!(quote.quote_time, now);
        assert_eq!(quote.pcr_values.len(), 3);
        assert_eq!(
            quote.pcr_values[0],
            pcr_extend_value(&[0u8; 32], &d0),
            "PCR0 值应与本地 extend 一致"
        );
        assert_eq!(quote.pcr_values[1], pcr_extend_value(&[0u8; 32], &d1));
        assert_eq!(quote.pcr_values[2], [0u8; 32], "PCR2 未度量应为零值");
        // 签名可验（quote_digest + AK 公钥）
        let sig = Sm2Signature::from_bytes(&sig_bytes);
        let verified = sm2_verify(&quote_digest(&quote), &sig, &tpm.attestation_pubkey());
        assert_eq!(verified, Ok(true), "quote 签名应可被 AK 公钥验证");
    }
}
