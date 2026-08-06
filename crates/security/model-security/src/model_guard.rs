//! 模型加载门禁：白名单 + 签名验证 + 审计日志 + 统计.
//!
//! 加载判定顺序固定：
//! 1. 计算模型数据 SM3 哈希（GB/T 32905）；
//! 2. 严格模式下：哈希未登记白名单 → 拒绝（[`GuardError::NotInWhitelist`]）；
//!    条目 `expiry < now` → 拒绝（[`GuardError::WhitelistEntryExpired`]）；
//! 3. 签名验证：[`ModelSignatureVerifier::verify`] 返回 `Err`（吊销/不可信/
//!    密码学错误）→ 拒绝；返回 `Ok(false)`（签名与数据不匹配）→ 拒绝
//!    （[`GuardError::SignatureInvalid`]）；
//! 4. 全部通过 → 允许加载。
//!
//! 每次判定（无论通过与否）写一条审计记录并更新统计。审计日志容量
//! [`AUDIT_CAPACITY`]（1000 条），写满后最旧记录被覆盖（环形语义，FIFO 淘汰）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm3_hash;

use crate::{
    GuardError, GuardStats, LoadResult, ModelSig, ModelSignatureVerifier, ModelWhitelist,
    WhitelistEntry,
};

/// 审计日志容量（写满后最旧记录被覆盖）.
pub const AUDIT_CAPACITY: usize = 1000;

/// 一次模型加载尝试的审计记录.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModelLoadAttempt {
    /// 判定时刻（Unix 时间戳，外部传入）.
    pub timestamp: u64,
    /// 模型数据的 SM3 哈希.
    pub model_hash: [u8; 32],
    /// 判定结果.
    pub result: LoadResult,
    /// 模型来源标识（取签名者 `signer_id`，审计可读）.
    pub source: String,
}

/// 模型加载门禁（白名单 + 签名验证 + 审计 + 统计）.
pub struct ModelGuard {
    /// 白名单（严格/宽松模式由 `whitelist.strict_mode` 决定）.
    whitelist: ModelWhitelist,
    /// 签名验证器（可信/吊销公钥列表）.
    verifier: ModelSignatureVerifier,
    /// 审计日志（容量 [`AUDIT_CAPACITY`]，FIFO 覆盖最旧）.
    audit_log: Vec<ModelLoadAttempt>,
    /// 可观测统计.
    stats: GuardStats,
}

impl ModelGuard {
    /// 创建门禁（注入白名单与签名验证器）.
    pub fn new(whitelist: ModelWhitelist, verifier: ModelSignatureVerifier) -> Self {
        Self {
            whitelist,
            verifier,
            audit_log: Vec::with_capacity(AUDIT_CAPACITY),
            stats: GuardStats::new(),
        }
    }

    /// 检查模型是否允许加载.
    ///
    /// `now`：当前 Unix 时间戳（no_std 无系统时钟，外部传入）。
    /// 判定顺序见模块文档；每次判定写审计 + 更新统计。
    pub fn check(&mut self, model_data: &[u8], sig: &ModelSig, now: u64) -> Result<(), GuardError> {
        // ① SM3 哈希（GB/T 32905）
        let hash = sm3_hash(model_data);

        // ② 严格模式白名单检查（宽松模式跳过：仅签名验证）
        if self.whitelist.strict_mode {
            if !self.whitelist.contains(&hash) {
                self.record(now, hash, LoadResult::DeniedWhitelist, &sig.signer_id);
                return Err(GuardError::NotInWhitelist);
            }
            // contains 命中后 get 必命中；取不到条目属内部不一致
            let entry = self.whitelist.get(&hash).ok_or(GuardError::InternalError)?;
            if entry.expiry < now {
                self.record(now, hash, LoadResult::DeniedExpired, &sig.signer_id);
                return Err(GuardError::WhitelistEntryExpired);
            }
        }

        // ③ 签名验证（吊销/不可信/密码学错误 → VerifyFailed；不匹配 → SignatureInvalid）
        match self.verifier.verify(model_data, sig) {
            Err(e) => {
                self.record(now, hash, LoadResult::DeniedSignature, &sig.signer_id);
                Err(GuardError::from(e))
            }
            Ok(false) => {
                self.record(now, hash, LoadResult::DeniedSignature, &sig.signer_id);
                Err(GuardError::SignatureInvalid)
            }
            Ok(true) => {
                // ④ 全部通过
                self.record(now, hash, LoadResult::Allowed, &sig.signer_id);
                Ok(())
            }
        }
    }

    /// 登记白名单条目（运维入口）.
    pub fn add_to_whitelist(&mut self, entry: WhitelistEntry) {
        self.whitelist.add(entry);
    }

    /// 按哈希移除白名单条目（运维入口）.
    pub fn remove_from_whitelist(&mut self, hash: &[u8; 32]) {
        self.whitelist.remove(hash);
    }

    /// 读审计日志（只读切片，按写入顺序，最旧在前）.
    pub fn get_audit_log(&self) -> &[ModelLoadAttempt] {
        &self.audit_log
    }

    /// 读统计（只读引用）.
    pub fn stats(&self) -> &GuardStats {
        &self.stats
    }

    /// 写审计记录 + 更新统计（内部辅助）.
    ///
    /// 容量控制：push 后若超过 [`AUDIT_CAPACITY`] 则移除最旧一条（FIFO 覆盖）。
    fn record(&mut self, timestamp: u64, model_hash: [u8; 32], result: LoadResult, source: &str) {
        self.audit_log.push(ModelLoadAttempt {
            timestamp,
            model_hash,
            result: result.clone(),
            source: String::from(source),
        });
        if self.audit_log.len() > AUDIT_CAPACITY {
            self.audit_log.remove(0);
        }
        self.stats.checks += 1;
        match result {
            LoadResult::Allowed => self.stats.allowed += 1,
            _ => self.stats.denied += 1,
        }
        self.stats.last_result = Some(result);
    }
}

// ============================================================
// 单元测试 GRD11~GRD15（测试模块内允许 std）
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

    use super::*;
    use crate::VerifyError;

    /// 固定测试时间戳（2023-11-14 22:13:20 UTC）.
    const NOW: u64 = 1_700_000_000;
    /// 白名单条目有效期（远未来，不过期）.
    const EXPIRY: u64 = 2_000_000_000;

    /// 测试夹具：密钥对 + 模型数据 + 签名 + 其 SM3 哈希.
    struct Fixture {
        kp: Sm2KeyPair,
        data: Vec<u8>,
        sig: ModelSig,
        hash: [u8; 32],
    }

    /// 构造夹具：生成密钥对并对模型数据签名.
    fn make_fixture(data: &[u8]) -> Fixture {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成");
        let signature = sm2_sign(data, &kp.private_key, &kp.public_key, &mut rng).expect("签名");
        let sig = ModelSig {
            signature,
            signer_pubkey: kp.public_key,
            signer_id: String::from("vendor-eneros-001"),
        };
        Fixture {
            kp,
            data: data.to_vec(),
            sig,
            hash: sm3_hash(data),
        }
    }

    /// 构造白名单条目（哈希取夹具模型哈希，有效期 EXPIRY）.
    fn make_entry(fx: &Fixture) -> WhitelistEntry {
        WhitelistEntry {
            model_id: String::from("llm-7b-int4"),
            version: String::from("1.0.0"),
            hash: fx.hash,
            added_at: NOW,
            added_by: String::from("ops-admin"),
            expiry: EXPIRY,
        }
    }

    /// 构造严格模式门禁：白名单登记夹具模型 + 签名者公钥加入可信.
    fn make_guard(fx: &Fixture) -> ModelGuard {
        let mut wl = ModelWhitelist::new(true);
        wl.add(make_entry(fx));
        let mut verifier = ModelSignatureVerifier::new();
        verifier.add_trusted_key(fx.kp.public_key);
        ModelGuard::new(wl, verifier)
    }

    /// GRD11：合法模型（白名单登记未过期 + 可信签名）check 返回 Ok；
    /// 审计尾条 result=Allowed，stats.allowed=1.
    #[test]
    fn grd11_legal_model_allowed() {
        let fx = make_fixture(b"model-weights-v1");
        let mut guard = make_guard(&fx);

        let result = guard.check(&fx.data, &fx.sig, NOW);
        assert_eq!(result, Ok(()), "合法模型必须允许加载");

        let log = guard.get_audit_log();
        assert_eq!(log.len(), 1, "应有一条审计记录");
        let tail = &log[log.len() - 1];
        assert_eq!(tail.result, LoadResult::Allowed, "审计尾条必须为 Allowed");
        assert_eq!(tail.model_hash, fx.hash, "审计哈希必须为模型 SM3");
        assert_eq!(tail.timestamp, NOW);
        assert_eq!(tail.source, "vendor-eneros-001");

        let stats = guard.stats();
        assert_eq!(stats.checks, 1);
        assert_eq!(stats.allowed, 1, "stats.allowed 必须为 1");
        assert_eq!(stats.denied, 0);
        assert_eq!(stats.last_result, Some(LoadResult::Allowed));
    }

    /// GRD12：签名者不可信（未加入 trusted）→ Err(VerifyFailed(KeyNotTrusted))；
    /// 审计 DeniedSignature，stats.denied=1.
    #[test]
    fn grd12_untrusted_signer_denied() {
        let fx = make_fixture(b"model-weights-v1");
        let mut wl = ModelWhitelist::new(true);
        wl.add(make_entry(&fx));
        // 签名者公钥不加入可信列表
        let verifier = ModelSignatureVerifier::new();
        let mut guard = ModelGuard::new(wl, verifier);

        let result = guard.check(&fx.data, &fx.sig, NOW);
        assert_eq!(
            result,
            Err(GuardError::VerifyFailed(VerifyError::KeyNotTrusted)),
            "不可信签名者必须拒绝"
        );

        let log = guard.get_audit_log();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log[log.len() - 1].result,
            LoadResult::DeniedSignature,
            "审计尾条必须为 DeniedSignature"
        );

        let stats = guard.stats();
        assert_eq!(stats.checks, 1);
        assert_eq!(stats.allowed, 0);
        assert_eq!(stats.denied, 1, "stats.denied 必须为 1");
        assert_eq!(stats.last_result, Some(LoadResult::DeniedSignature));
    }

    /// GRD13：严格模式 + 未登记白名单 → Err(NotInWhitelist)；审计 DeniedWhitelist.
    #[test]
    fn grd13_not_in_whitelist_denied() {
        let fx = make_fixture(b"model-weights-v1");
        // 严格模式空白名单（不登记夹具模型）
        let wl = ModelWhitelist::new(true);
        let mut verifier = ModelSignatureVerifier::new();
        verifier.add_trusted_key(fx.kp.public_key);
        let mut guard = ModelGuard::new(wl, verifier);

        let result = guard.check(&fx.data, &fx.sig, NOW);
        assert_eq!(
            result,
            Err(GuardError::NotInWhitelist),
            "严格模式未登记模型必须拒绝"
        );

        let log = guard.get_audit_log();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log[log.len() - 1].result,
            LoadResult::DeniedWhitelist,
            "审计尾条必须为 DeniedWhitelist"
        );

        let stats = guard.stats();
        assert_eq!(stats.checks, 1);
        assert_eq!(stats.denied, 1);
        assert_eq!(stats.last_result, Some(LoadResult::DeniedWhitelist));
    }

    /// GRD14：白名单条目过期（expiry < now）→ Err(WhitelistEntryExpired)；
    /// 审计 DeniedExpired.
    #[test]
    fn grd14_expired_entry_denied() {
        let fx = make_fixture(b"model-weights-v1");
        let mut guard = make_guard(&fx);
        // 条目有效期 EXPIRY = 2_000_000_000，取 now 超过有效期
        let now_after_expiry: u64 = EXPIRY + 1;

        let result = guard.check(&fx.data, &fx.sig, now_after_expiry);
        assert_eq!(
            result,
            Err(GuardError::WhitelistEntryExpired),
            "过期条目必须拒绝"
        );

        let log = guard.get_audit_log();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log[log.len() - 1].result,
            LoadResult::DeniedExpired,
            "审计尾条必须为 DeniedExpired"
        );

        let stats = guard.stats();
        assert_eq!(stats.checks, 1);
        assert_eq!(stats.denied, 1);
        assert_eq!(stats.last_result, Some(LoadResult::DeniedExpired));
    }

    /// GRD15：审计容量 FIFO 覆盖——连续 1001 次 check，audit_log.len()==1000，
    /// 首条 timestamp 为第 2 次写入的值（最旧一条被覆盖）.
    ///
    /// 提速说明：1001 次循环采用**不可信签名者**（白名单正常登记严格模式可
    /// 通过，但签名者公钥不加入可信列表）。`verify` 在可信列表检查处快速返回
    /// `Err(KeyNotTrusted)`，不再执行纯 Rust SM2 验签（debug 模式单次约 1s，
    /// 1001 次合法签名约 17 分钟，会破坏 CI < 10 分钟约束）；拒绝路径审计仍
    /// 逐条记录 DeniedSignature，FIFO 覆盖语义与断言口径（len==1000、最旧
    /// 条目被覆盖、stats.denied==1001）保持不变。
    #[test]
    fn grd15_audit_capacity_fifo_eviction() {
        let fx = make_fixture(b"model-weights-v1");
        let mut wl = ModelWhitelist::new(true);
        wl.add(make_entry(&fx));
        // 签名者公钥不加入可信列表：verify 在可信检查处快速失败（提速，见上文）
        let verifier = ModelSignatureVerifier::new();
        let mut guard = ModelGuard::new(wl, verifier);

        // 写入 1001 次：timestamp 依次取 1..=1001
        for i in 1..=(AUDIT_CAPACITY as u64 + 1) {
            let result = guard.check(&fx.data, &fx.sig, i);
            assert_eq!(
                result,
                Err(GuardError::VerifyFailed(VerifyError::KeyNotTrusted)),
                "不可信签名者每次必须拒绝"
            );
        }

        let log = guard.get_audit_log();
        assert_eq!(
            log.len(),
            AUDIT_CAPACITY,
            "审计日志长度必须封顶为 AUDIT_CAPACITY"
        );
        assert_eq!(
            log[0].timestamp, 2,
            "首条 timestamp 必须为第 2 次写入的值（最旧的 timestamp=1 已被覆盖）"
        );
        assert_eq!(
            log[log.len() - 1].timestamp,
            AUDIT_CAPACITY as u64 + 1,
            "尾条必须为最近一次写入"
        );
        // 首尾记录均为拒绝路径审计（DeniedSignature）
        assert_eq!(log[0].result, LoadResult::DeniedSignature);
        assert_eq!(log[log.len() - 1].result, LoadResult::DeniedSignature);

        let stats = guard.stats();
        assert_eq!(stats.checks, AUDIT_CAPACITY as u64 + 1);
        assert_eq!(stats.allowed, 0);
        assert_eq!(
            stats.denied,
            AUDIT_CAPACITY as u64 + 1,
            "stats.denied 必须累计 1001 次拒绝"
        );
        assert_eq!(stats.last_result, Some(LoadResult::DeniedSignature));
    }
}
