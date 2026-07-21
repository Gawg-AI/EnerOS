//! 远程证明生成与验证（v0.114.0，D5/D6/D10/D11）.
//!
//! 证明方（边缘节点）：[`RemoteAttestation::generate`] 组装 PCR Quote（nonce
//! 内嵌随签名绑定防重放，D10①）+ 64B SM2 签名（修复蓝图签名永不填充 bug，
//! D6）+ 事件日志克隆。
//!
//! 验证方：[`AttestVerifier::verify`] 四步流水线——① nonce  freshness
//! 校验 → ② SM2 验签 quote_digest → ③ 证明日志自一致性（重放
//! attest.event_log 比对 quote，D11）→ ④ 期望值重放比对（蓝图 §4.4「PCR
//! 重放不匹配 → 拒绝信任」，记录全部不匹配索引）。
//!
//! 远程传输（蓝图 `async verify_remote` + HttpClient）归 [`AttestTransport`]
//! 抽象与集成层（D5：no_std 无 async runtime/无 std::net）。
//!
//! [`AttestTransport`]: crate::AttestTransport

use alloc::vec::Vec;

use eneros_crypto::{sm2_verify, CsRng, Sm2PublicKey, Sm2Signature};

use crate::event_log::{TcgEvent, TcgEventLog};
use crate::tpm::{quote_digest, TpmBackend};
use crate::{AttestError, AttestStats};

/// PCR Quote（nonce 内嵌，D10①：随 quote_digest 签名绑定防重放）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcrQuote {
    /// 选中的 PCR 索引列表.
    pub pcr_select: Vec<u8>,
    /// 选中 PCR 的当前值（与 pcr_select 一一对应）.
    pub pcr_values: Vec<[u8; 32]>,
    /// 验证方下发的随机数（20 字节，freshness 防重放）.
    pub nonce: [u8; 20],
    /// Quote 生成时间戳（参数注入，D8：集成层由 v0.12.0 RTC 供给）.
    pub quote_time: u64,
}

/// 验证结论枚举（D11：蓝图 `reason: String` → Copy 枚举，机器可审计）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestReason {
    /// 验证通过（四步全过，可信）.
    Verified,
    /// Nonce 不符（重放攻击 / 会话错配）.
    NonceMismatch,
    /// SM2 验签失败（签名被篡改 / AK 公钥错误）.
    SignatureInvalid,
    /// 证明日志自一致性破坏（重放 attest.event_log ≠ quote 值，D11）.
    EventLogInconsistent,
    /// 期望值重放比对失败（启动链被篡改，蓝图 §4.4 拒绝信任）.
    PcrMismatch,
    /// 验证服务拒绝（远程通道语义，`AttestTransport` 侧结论）.
    ServerRejected,
}

/// 验证结果（D11：reason 枚举化；PcrMismatch 时记录全部不匹配索引）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestResult {
    /// 是否可信.
    pub trusted: bool,
    /// 不匹配的 PCR 索引列表（仅 PcrMismatch 时非空）.
    pub pcr_mismatches: Vec<u8>,
    /// 结论原因.
    pub reason: AttestReason,
}

/// 远程证明（D6：蓝图 `signature: Vec<u8>` 永不填充修复为定长 64B）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAttestation {
    /// PCR Quote（nonce 内嵌）.
    pub quote: PcrQuote,
    /// Quote 的 SM2 签名（r ‖ s 64 字节，对 quote_digest 签名）.
    pub signature: [u8; 64],
    /// 事件日志克隆（证明方提交的度量存证）.
    pub event_log: Vec<TcgEvent>,
}

impl RemoteAttestation {
    /// 生成远程证明（蓝图 §4.3 证明时序第 2~4 步）.
    ///
    /// - `pcr_indices` 为空 → `Err(EmptyPcrSelection)`（调用 quote 前自查；
    ///   SoftTpm 层空选择归并 `TpmError::QuoteFailed`）
    /// - `tpm.quote(...)` 错误经 `From<TpmError>` 显式转换传播（D10②）
    /// - 事件日志克隆自 `log.events()`（D8：显式持有传递）
    pub fn generate<T: TpmBackend>(
        tpm: &mut T,
        pcr_indices: &[u8],
        nonce: &[u8; 20],
        now: u64,
        log: &TcgEventLog,
        rng: &mut CsRng,
    ) -> Result<Self, AttestError> {
        if pcr_indices.is_empty() {
            return Err(AttestError::EmptyPcrSelection);
        }
        let (quote, signature) = tpm.quote(pcr_indices, nonce, now, rng)?;
        Ok(Self {
            quote,
            signature,
            event_log: log.events().to_vec(),
        })
    }
}

/// 本地证明验证器（D5/D6：验签与重放比对本地承载，可独立测试）.
///
/// AK 公钥构造注入（验证方预置，集成层经安全信道分发）；[`AttestStats`]
/// 落地 §9 可观测（D11）。
pub struct AttestVerifier {
    /// 证明方 AK 公钥（构造注入）.
    ak_pubkey: Sm2PublicKey,
    /// 验证统计（quotes_verified / trusted / untrusted / last_reason）.
    stats: AttestStats,
}

impl AttestVerifier {
    /// 构造验证器（注入证明方 AK 公钥）.
    pub fn new(ak_pubkey: Sm2PublicKey) -> Self {
        Self {
            ak_pubkey,
            stats: AttestStats {
                quotes_verified: 0,
                trusted: 0,
                untrusted: 0,
                last_reason: None,
            },
        }
    }

    /// 验证远程证明（四步流水线，任一失败即拒绝并记统计）：
    ///
    /// 1. `attest.quote.nonce != *nonce` → `NonceMismatch`（防重放）
    /// 2. `sm2_verify(quote_digest, signature, ak_pubkey)` 非 true →
    ///    `SignatureInvalid`
    /// 3. 自一致性（D11）：重放 `attest.event_log`，选中索引重放值 ≠
    ///    `quote.pcr_values` 对应项 → `EventLogInconsistent`
    /// 4. 期望值比对：`expected_log.replay()` 选中索引值比对，收集全部
    ///    不匹配索引；非空 → `PcrMismatch`（蓝图 §4.4 拒绝信任）
    /// 5. 全过 → `trusted = true`，`reason = Verified`
    pub fn verify(
        &mut self,
        attest: &RemoteAttestation,
        expected_log: &TcgEventLog,
        nonce: &[u8; 20],
    ) -> AttestResult {
        // 1. nonce freshness 校验（防重放，D10①）
        if attest.quote.nonce != *nonce {
            return self.reject(AttestReason::NonceMismatch, Vec::new());
        }
        // 2. SM2 验签（签名消息为 quote_digest 32B，D6）
        let sig = Sm2Signature::from_bytes(&attest.signature);
        let digest = quote_digest(&attest.quote);
        match sm2_verify(&digest, &sig, &self.ak_pubkey) {
            Ok(true) => {}
            Ok(false) | Err(_) => return self.reject(AttestReason::SignatureInvalid, Vec::new()),
        }
        // 3. 证明日志自一致性（D11：防证明方提交与 quote 不符的日志）
        let attested_pcrs = TcgEventLog::from_events(attest.event_log.clone()).replay();
        for (i, &idx) in attest.quote.pcr_select.iter().enumerate() {
            let quoted = attest.quote.pcr_values.get(i);
            let replayed = attested_pcrs.get(idx as usize);
            match (quoted, replayed) {
                (Some(q), Some(r)) if q == r => {}
                _ => return self.reject(AttestReason::EventLogInconsistent, Vec::new()),
            }
        }
        // 4. 期望值重放比对（蓝图 §4.4：收集全部不匹配索引）
        let expected_pcrs = expected_log.replay();
        let mut mismatches = Vec::new();
        for (i, &idx) in attest.quote.pcr_select.iter().enumerate() {
            let quoted = attest.quote.pcr_values.get(i);
            let expected = expected_pcrs.get(idx as usize);
            match (quoted, expected) {
                (Some(q), Some(e)) if q == e => {}
                _ => mismatches.push(idx),
            }
        }
        if !mismatches.is_empty() {
            return self.reject(AttestReason::PcrMismatch, mismatches);
        }
        // 5. 全过：可信
        self.stats.quotes_verified += 1;
        self.stats.trusted += 1;
        self.stats.last_reason = Some(AttestReason::Verified);
        AttestResult {
            trusted: true,
            pcr_mismatches: Vec::new(),
            reason: AttestReason::Verified,
        }
    }

    /// 验证统计访问器（§9 可观测）.
    pub fn stats(&self) -> AttestStats {
        self.stats
    }

    /// 拒绝路径统一处理：记统计（untrusted + last_reason）并返回结果.
    fn reject(&mut self, reason: AttestReason, mismatches: Vec<u8>) -> AttestResult {
        self.stats.quotes_verified += 1;
        self.stats.untrusted += 1;
        self.stats.last_reason = Some(reason);
        AttestResult {
            trusted: false,
            pcr_mismatches: mismatches,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tpm::SoftTpm;

    /// 固定测试 nonce.
    const NONCE: [u8; 20] = [0xAB; 20];
    /// 固定测试时间戳.
    const NOW: u64 = 1_700_000_000;
    /// 事件类型：Bootloader 镜像.
    const EV_BL: u32 = 0x8001;
    /// 事件类型：内核镜像.
    const EV_KERNEL: u32 = 0x8002;
    /// 事件类型：Runtime 镜像.
    const EV_RUNTIME: u32 = 0x8003;

    /// 测试环境：SoftTpm + 事件日志 + CsRng（已完成三级度量）.
    fn make_measured() -> (SoftTpm, TcgEventLog, CsRng) {
        let mut rng = CsRng::new();
        let mut tpm = SoftTpm::new(&mut rng);
        let mut log = TcgEventLog::new();
        assert_eq!(log.measure(&mut tpm, 0, EV_BL, b"bl-image"), Ok(()));
        assert_eq!(log.measure(&mut tpm, 1, EV_KERNEL, b"kernel-image"), Ok(()));
        assert_eq!(
            log.measure(&mut tpm, 2, EV_RUNTIME, b"runtime-image"),
            Ok(())
        );
        (tpm, log, rng)
    }

    /// 验证方期望日志（用另一 SoftTpm 度量相同数据构造，仅取其 replay 值）.
    fn make_expected_log(runtime_image: &[u8]) -> TcgEventLog {
        let mut rng = CsRng::new();
        let mut tpm = SoftTpm::new(&mut rng);
        let mut log = TcgEventLog::new();
        assert_eq!(log.measure(&mut tpm, 0, EV_BL, b"bl-image"), Ok(()));
        assert_eq!(log.measure(&mut tpm, 1, EV_KERNEL, b"kernel-image"), Ok(()));
        assert_eq!(log.measure(&mut tpm, 2, EV_RUNTIME, runtime_image), Ok(()));
        log
    }

    /// 生成一份标准证明（PCR[0..7]）.
    fn make_attestation(
        tpm: &mut SoftTpm,
        log: &TcgEventLog,
        rng: &mut CsRng,
    ) -> RemoteAttestation {
        RemoteAttestation::generate(tpm, &[0, 1, 2, 3, 4, 5, 6, 7], &NONCE, NOW, log, rng)
            .expect("证明生成应成功")
    }

    // ============================================================
    // ATT10：generate 组装
    // ============================================================

    /// ATT10 generate 组装：quote 选中 [0..7] + nonce/时间戳正确 +
    /// 64B 签名非全零 + 日志克隆一致.
    #[test]
    fn att10_generate_assembles_attestation() {
        let (mut tpm, log, mut rng) = make_measured();
        let attest = make_attestation(&mut tpm, &log, &mut rng);
        assert_eq!(attest.quote.pcr_select, alloc::vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(attest.quote.pcr_values.len(), 8);
        assert_eq!(attest.quote.nonce, NONCE);
        assert_eq!(attest.quote.quote_time, NOW);
        assert_ne!(attest.signature, [0u8; 64], "签名应非全零（D6 修复）");
        assert_eq!(attest.event_log, log.events().to_vec(), "日志应克隆一致");
        // PCR3~7 未度量应为零值
        for value in &attest.quote.pcr_values[3..8] {
            assert_eq!(*value, [0u8; 32]);
        }
    }

    // ============================================================
    // ATT11：verify 快乐路径（蓝图 §6.2/§7.1）
    // ============================================================

    /// ATT11 端到端可信：trusted + Verified + mismatches 空 +
    /// stats.trusted == 1.
    #[test]
    fn att11_verify_happy_path() {
        let (mut tpm, log, mut rng) = make_measured();
        let ak_pubkey = tpm.attestation_pubkey();
        let attest = make_attestation(&mut tpm, &log, &mut rng);
        let expected_log = make_expected_log(b"runtime-image");
        let mut verifier = AttestVerifier::new(ak_pubkey);
        let result = verifier.verify(&attest, &expected_log, &NONCE);
        assert!(result.trusted);
        assert_eq!(result.reason, AttestReason::Verified);
        assert!(result.pcr_mismatches.is_empty());
        let stats = verifier.stats();
        assert_eq!(stats.quotes_verified, 1);
        assert_eq!(stats.trusted, 1);
        assert_eq!(stats.untrusted, 0);
        assert_eq!(stats.last_reason, Some(AttestReason::Verified));
    }

    // ============================================================
    // ATT12：nonce 不符（蓝图 §7.3）
    // ============================================================

    /// ATT12 验证方 nonce 与 quote 内嵌 nonce 不符 → untrusted +
    /// NonceMismatch + stats 更新.
    #[test]
    fn att12_nonce_mismatch_rejected() {
        let (mut tpm, log, mut rng) = make_measured();
        let ak_pubkey = tpm.attestation_pubkey();
        let attest = make_attestation(&mut tpm, &log, &mut rng);
        let expected_log = make_expected_log(b"runtime-image");
        let mut verifier = AttestVerifier::new(ak_pubkey);
        let wrong_nonce = [0xCD; 20];
        let result = verifier.verify(&attest, &expected_log, &wrong_nonce);
        assert!(!result.trusted);
        assert_eq!(result.reason, AttestReason::NonceMismatch);
        let stats = verifier.stats();
        assert_eq!(stats.quotes_verified, 1);
        assert_eq!(stats.untrusted, 1);
        assert_eq!(stats.last_reason, Some(AttestReason::NonceMismatch));
    }

    // ============================================================
    // ATT13：签名篡改 1 字节（蓝图 §7.3）
    // ============================================================

    /// ATT13 签名翻转 1 字节 → untrusted + SignatureInvalid.
    #[test]
    fn att13_tampered_signature_rejected() {
        let (mut tpm, log, mut rng) = make_measured();
        let ak_pubkey = tpm.attestation_pubkey();
        let mut attest = make_attestation(&mut tpm, &log, &mut rng);
        attest.signature[0] ^= 0x01;
        let expected_log = make_expected_log(b"runtime-image");
        let mut verifier = AttestVerifier::new(ak_pubkey);
        let result = verifier.verify(&attest, &expected_log, &NONCE);
        assert!(!result.trusted);
        assert_eq!(result.reason, AttestReason::SignatureInvalid);
    }

    // ============================================================
    // ATT14：期望日志多一事件（蓝图 §7.3 PcrMismatch）
    // ============================================================

    /// ATT14 期望日志较证明侧多一笔 PCR1 度量 → untrusted + PcrMismatch
    /// + mismatches 恰为 [1].
    #[test]
    fn att14_expected_log_extra_event_rejected() {
        let (mut tpm, log, mut rng) = make_measured();
        let ak_pubkey = tpm.attestation_pubkey();
        let attest = make_attestation(&mut tpm, &log, &mut rng);
        // 期望侧在 PCR1 上多度量一笔（期望值偏离证明侧）
        let mut expected_log = make_expected_log(b"runtime-image");
        let mut rng2 = CsRng::new();
        let mut tpm2 = SoftTpm::new(&mut rng2);
        // 用临时 TPM 重放到期望状态后再补一笔
        for ev in expected_log.events().to_vec() {
            assert_eq!(tpm2.pcr_extend(ev.pcr_index, &ev.digest), Ok(()));
        }
        assert_eq!(
            expected_log.measure(&mut tpm2, 1, EV_KERNEL, b"kernel-image-v2"),
            Ok(())
        );
        let mut verifier = AttestVerifier::new(ak_pubkey);
        let result = verifier.verify(&attest, &expected_log, &NONCE);
        assert!(!result.trusted);
        assert_eq!(result.reason, AttestReason::PcrMismatch);
        assert_eq!(result.pcr_mismatches, alloc::vec![1u8]);
    }

    // ============================================================
    // ATT15：证明日志自一致性破坏（D11）
    // ============================================================

    /// ATT15 篡改证明方提交的事件日志（改一笔 digest）→ 重放值与 quote
    /// 不符 → untrusted + EventLogInconsistent.
    #[test]
    fn att15_event_log_inconsistent_rejected() {
        let (mut tpm, log, mut rng) = make_measured();
        let ak_pubkey = tpm.attestation_pubkey();
        let mut attest = make_attestation(&mut tpm, &log, &mut rng);
        // 篡改证明日志第一笔事件的 digest（签名不覆盖日志，验签仍过，
        // 但自一致性重放值 ≠ quote 值）
        attest.event_log[0].digest[0] ^= 0x01;
        let expected_log = make_expected_log(b"runtime-image");
        let mut verifier = AttestVerifier::new(ak_pubkey);
        let result = verifier.verify(&attest, &expected_log, &NONCE);
        assert!(!result.trusted);
        assert_eq!(result.reason, AttestReason::EventLogInconsistent);
    }

    // ============================================================
    // ATT16：错误 AK 公钥
    // ============================================================

    /// ATT16 验证器持另一密钥对的公钥 → 验签失败 → untrusted +
    /// SignatureInvalid.
    #[test]
    fn att16_wrong_ak_pubkey_rejected() {
        let (mut tpm, log, mut rng) = make_measured();
        let attest = make_attestation(&mut tpm, &log, &mut rng);
        let expected_log = make_expected_log(b"runtime-image");
        // 另一密钥对的公钥（错误 AK）：CsRng 固定种子（eneros-crypto 偏差
        // 声明 ①），须复用状态已推进的 rng 才能生成不同密钥
        let other_tpm = SoftTpm::new(&mut rng);
        let mut verifier = AttestVerifier::new(other_tpm.attestation_pubkey());
        let result = verifier.verify(&attest, &expected_log, &NONCE);
        assert!(!result.trusted);
        assert_eq!(result.reason, AttestReason::SignatureInvalid);
    }

    // ============================================================
    // ATT17：空 pcr_indices
    // ============================================================

    /// ATT17 generate 空 PCR 选择 → Err(EmptyPcrSelection)（调用 quote
    /// 前自查）.
    #[test]
    fn att17_empty_pcr_selection_rejected() {
        let (mut tpm, log, mut rng) = make_measured();
        let result = RemoteAttestation::generate(&mut tpm, &[], &NONCE, NOW, &log, &mut rng);
        assert_eq!(result, Err(AttestError::EmptyPcrSelection));
    }

    // ============================================================
    // INT20：端到端三级度量证明（蓝图 §6.2/§7.1）
    // ============================================================

    /// INT20 三级镜像 b"bl-image"/b"kernel-image"/b"runtime-image" 分别
    /// measure 到 PCR0/1/2 → generate(PCR[0..7]) → 验证方相同数据重放
    /// → trusted + Verified + mismatches 空 + stats.trusted == 1.
    #[test]
    fn int20_end_to_end_three_stage_attestation() {
        // 证明侧：三级度量
        let mut rng = CsRng::new();
        let mut tpm = SoftTpm::new(&mut rng);
        let mut log = TcgEventLog::new();
        assert_eq!(log.measure(&mut tpm, 0, EV_BL, b"bl-image"), Ok(()));
        assert_eq!(log.measure(&mut tpm, 1, EV_KERNEL, b"kernel-image"), Ok(()));
        assert_eq!(
            log.measure(&mut tpm, 2, EV_RUNTIME, b"runtime-image"),
            Ok(())
        );
        let ak_pubkey = tpm.attestation_pubkey();
        // 生成证明（PCR[0..7]，含未度量 PCR3~7）
        let attest = RemoteAttestation::generate(
            &mut tpm,
            &[0, 1, 2, 3, 4, 5, 6, 7],
            &NONCE,
            NOW,
            &log,
            &mut rng,
        )
        .expect("证明生成应成功");
        // 验证侧：相同数据重放构造期望日志
        let expected_log = make_expected_log(b"runtime-image");
        let mut verifier = AttestVerifier::new(ak_pubkey);
        let result = verifier.verify(&attest, &expected_log, &NONCE);
        assert!(result.trusted, "端到端证明应可信");
        assert_eq!(result.reason, AttestReason::Verified);
        assert!(result.pcr_mismatches.is_empty());
        assert_eq!(verifier.stats().trusted, 1);
    }

    // ============================================================
    // INT21：端到端攻击：期望侧 Runtime 镜像被换（蓝图 §4.4/§7.3）
    // ============================================================

    /// INT21 期望日志的 runtime-image 换成 b"runtime-image-evil" →
    /// untrusted + PcrMismatch + mismatches 含 2.
    #[test]
    fn int21_tampered_runtime_image_rejected() {
        let (mut tpm, log, mut rng) = make_measured();
        let ak_pubkey = tpm.attestation_pubkey();
        let attest = make_attestation(&mut tpm, &log, &mut rng);
        // 攻击场景：期望侧（白名单）Runtime 镜像被替换
        let expected_log = make_expected_log(b"runtime-image-evil");
        let mut verifier = AttestVerifier::new(ak_pubkey);
        let result = verifier.verify(&attest, &expected_log, &NONCE);
        assert!(!result.trusted, "Runtime 镜像被换应拒绝信任（§4.4）");
        assert_eq!(result.reason, AttestReason::PcrMismatch);
        assert!(
            result.pcr_mismatches.contains(&2u8),
            "mismatches 应含 PCR2，实际 {:?}",
            result.pcr_mismatches
        );
        assert_eq!(verifier.stats().untrusted, 1);
        assert_eq!(
            verifier.stats().last_reason,
            Some(AttestReason::PcrMismatch)
        );
    }

    // ============================================================
    // PERF22：quote 生成性能（蓝图 §6.3/§7.2，cfg(test) Instant 口径，
    // 同 v0.113.0 PERF20 先例，D12）
    // ============================================================

    /// PERF22 单次 generate（含 SM2 签名）计时：debug 仅打印；release
    /// 默认打印，设 `ENEROS_PERF_GATE=1` 时断言 < 100ms（D12：主机纯 Rust
    /// SM2 实测超蓝图指标，100ms 门禁面向目标硬件 SM2 加速 / 性能 CI 场景）.
    #[test]
    fn perf22_quote_generate_under_100ms() {
        let (mut tpm, log, mut rng) = make_measured();
        let start = std::time::Instant::now();
        let result = RemoteAttestation::generate(
            &mut tpm,
            &[0, 1, 2, 3, 4, 5, 6, 7],
            &NONCE,
            NOW,
            &log,
            &mut rng,
        );
        let elapsed = start.elapsed();
        assert!(result.is_ok());
        #[cfg(debug_assertions)]
        eprintln!("[PERF22] 单次 generate（含 SM2 签名）耗时: {:?}", elapsed);
        #[cfg(not(debug_assertions))]
        {
            // 仅当变量值恰为 "1" 时启用门禁：规避终端残留空串变量误激活（D12）
            if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
                assert!(
                    elapsed.as_millis() < 100,
                    "PERF22 quote 生成耗时 {:?} 超过 100ms 上限",
                    elapsed
                );
            } else {
                eprintln!(
                    "[PERF22] 单次 generate（含 SM2 签名）耗时: {:?}（设 ENEROS_PERF_GATE=1 启用 <100ms 断言，D12）",
                    elapsed
                );
            }
        }
    }
}
