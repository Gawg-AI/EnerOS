//! EnerOS v0.116.0 模型签名与白名单 集成测试（INT16~INT18）+ 性能测试（PERF19）.
//!
//! 端到端验证「SM3 哈希白名单 + SM2 签名验证」双重防线全链路：
//! - INT16：合法模型端到端放行（CsRng → 密钥对生成 → sm2_sign 签名 → 白名单
//!   登记 → 公钥加入可信 → check 返回 Ok，审计尾条 Allowed 且 source=signer_id）；
//! - INT17：伪造签名拦截（攻击者自生成密钥对签名模型，但 verifier 可信列表
//!   不含该公钥 → Err(VerifyFailed(KeyNotTrusted))，审计 DeniedSignature——
//!   蓝图 §7.3「后门模型 100% 拒绝」）；
//! - INT18：白名单移除后重放拒绝（先放行一次 → remove_from_whitelist(hash)
//!   → 严格模式再次 check → Err(NotInWhitelist)）；
//! - PERF19：单次完整 check（SM3 哈希 + SM2 验签全链路）耗时打印；仅当
//!   `ENEROS_PERF_GATE=1` 时断言 < 10ms（蓝图 §6.3/§7.2 性能指标，门禁面向
//!   目标硬件 SM2 加速 / 性能 CI 场景；主机纯软 SM2 实测超时属预期，同
//!   v0.113.0/v0.115.0 先例，见 docs/security/model-security-design.md D6）。
//!
//! 集成测试运行于 host std 环境（`std::time::Instant` 等可用），通过
//! `eneros_model_security` crate 公共 API 调用被测的 no_std 代码。

use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
use eneros_model_security::{
    GuardError, LoadResult, ModelGuard, ModelSig, ModelSignatureVerifier, ModelWhitelist,
    VerifyError, WhitelistEntry,
};

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

/// 构造夹具：生成密钥对并对模型数据签名（签名者标识可定制）.
fn make_fixture(data: &[u8], signer_id: &str) -> Fixture {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成");
    let signature = sm2_sign(data, &kp.private_key, &kp.public_key, &mut rng).expect("签名");
    let sig = ModelSig {
        signature,
        signer_pubkey: kp.public_key,
        signer_id: String::from(signer_id),
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

/// 构造严格模式门禁：白名单登记夹具模型 + 签名者公钥加入可信（合法全链路）.
fn make_guard(fx: &Fixture) -> ModelGuard {
    let mut wl = ModelWhitelist::new(true);
    wl.add(make_entry(fx));
    let mut verifier = ModelSignatureVerifier::new();
    verifier.add_trusted_key(fx.kp.public_key);
    ModelGuard::new(wl, verifier)
}

// ============================================================
// INT16：端到端放行
// ============================================================

/// INT16：CsRng → Sm2KeyPair::generate → 构造模型字节 → sm2_sign 签名 →
/// 白名单登记（hash=SM3(model)，expiry 未来值）→ 公钥加入可信 →
/// ModelGuard::check 返回 Ok；审计尾条 result=Allowed 且 source=signer_id.
#[test]
fn int16_legal_model_end_to_end_allowed() {
    let fx = make_fixture(b"model-weights-v1-ota", "vendor-eneros-001");
    let mut guard = make_guard(&fx);

    let result = guard.check(&fx.data, &fx.sig, NOW);
    assert_eq!(result, Ok(()), "合法模型端到端必须放行");

    let log = guard.get_audit_log();
    assert_eq!(log.len(), 1, "应有一条审计记录");
    let tail = &log[log.len() - 1];
    assert_eq!(tail.result, LoadResult::Allowed, "审计尾条必须为 Allowed");
    assert_eq!(tail.source, "vendor-eneros-001", "审计来源必须为签名者标识");
    assert_eq!(tail.model_hash, fx.hash, "审计哈希必须为模型 SM3");
    assert_eq!(tail.timestamp, NOW);

    let stats = guard.stats();
    assert_eq!(stats.checks, 1);
    assert_eq!(stats.allowed, 1);
    assert_eq!(stats.denied, 0);
    assert_eq!(stats.last_result, Some(LoadResult::Allowed));
}

// ============================================================
// INT17：伪造签名拦截（后门模型 100% 拒绝）
// ============================================================

/// INT17：攻击者自生成密钥对签名模型（模型哈希已登记白名单，模拟攻击者
/// 仿冒合法分发包投递后门模型），但 verifier 可信列表不含该公钥 →
/// check 返回 Err(VerifyFailed(KeyNotTrusted))，审计记录 DeniedSignature.
#[test]
fn int17_forged_signer_denied() {
    let attacker = make_fixture(b"backdoor-model-weights", "attacker-001");

    // 模型哈希登记白名单（严格模式可通过），可信列表为空（攻击者公钥不可信）
    let mut wl = ModelWhitelist::new(true);
    wl.add(make_entry(&attacker));
    let verifier = ModelSignatureVerifier::new();
    let mut guard = ModelGuard::new(wl, verifier);

    let result = guard.check(&attacker.data, &attacker.sig, NOW);
    assert_eq!(
        result,
        Err(GuardError::VerifyFailed(VerifyError::KeyNotTrusted)),
        "伪造签名（不可信签名者）必须拒绝"
    );

    let log = guard.get_audit_log();
    assert_eq!(log.len(), 1);
    let tail = &log[log.len() - 1];
    assert_eq!(
        tail.result,
        LoadResult::DeniedSignature,
        "审计尾条必须为 DeniedSignature"
    );
    assert_eq!(tail.source, "attacker-001");

    let stats = guard.stats();
    assert_eq!(stats.checks, 1);
    assert_eq!(stats.allowed, 0);
    assert_eq!(stats.denied, 1);
    assert_eq!(stats.last_result, Some(LoadResult::DeniedSignature));
}

// ============================================================
// INT18：白名单移除后重放拒绝
// ============================================================

/// INT18：先放行一次 → remove_from_whitelist(hash)（模型下线/吊销）→
/// 严格模式再次 check 同一模型 → Err(NotInWhitelist)，审计尾条 DeniedWhitelist.
#[test]
fn int18_replay_denied_after_whitelist_removal() {
    let fx = make_fixture(b"model-weights-v1-ota", "vendor-eneros-001");
    let mut guard = make_guard(&fx);

    // 首次放行
    let first = guard.check(&fx.data, &fx.sig, NOW);
    assert_eq!(first, Ok(()), "登记期间合法模型必须放行");

    // 运维移除白名单条目后重放同一模型（严格模式）
    guard.remove_from_whitelist(&fx.hash);
    let replay = guard.check(&fx.data, &fx.sig, NOW + 1);
    assert_eq!(
        replay,
        Err(GuardError::NotInWhitelist),
        "白名单移除后重放必须拒绝"
    );

    let log = guard.get_audit_log();
    assert_eq!(log.len(), 2, "应有放行 + 拒绝两条审计记录");
    assert_eq!(log[0].result, LoadResult::Allowed);
    assert_eq!(
        log[log.len() - 1].result,
        LoadResult::DeniedWhitelist,
        "审计尾条必须为 DeniedWhitelist"
    );

    let stats = guard.stats();
    assert_eq!(stats.checks, 2);
    assert_eq!(stats.allowed, 1);
    assert_eq!(stats.denied, 1);
    assert_eq!(stats.last_result, Some(LoadResult::DeniedWhitelist));
}

// ============================================================
// PERF19：单次完整 check 耗时（SM3 + SM2 验签全链路）
// ============================================================

/// PERF19：`std::time::Instant` 测量单次完整 check（合法模型，SM3 哈希 +
/// SM2 验签全链路）耗时并打印.
///
/// 主机纯软 SM2 实测超 10ms 属预期（v0.113.0/v0.115.0 同先例：蓝图
/// §6.3/§7.2「验证 < 10ms」为目标硬件 SM2 加速指标），故默认仅打印不断言；
/// 仅当 `ENEROS_PERF_GATE=1`（值恰为 "1"，规避终端残留空串误激活）时启用
/// < 10ms 断言门禁，面向性能 CI / 目标硬件场景。测试本身断言 check 成功。
#[test]
fn perf19_single_check_under_10ms() {
    let fx = make_fixture(b"model-weights-v1-ota", "vendor-eneros-001");
    let mut guard = make_guard(&fx);

    let start = std::time::Instant::now();
    let result = guard.check(&fx.data, &fx.sig, NOW);
    let elapsed = start.elapsed();
    assert_eq!(result, Ok(()), "计时用例本身必须为合法放行");

    println!(
        "[PERF19] 单次完整 check（SM3 + SM2 验签全链路）耗时: {:?}（设 ENEROS_PERF_GATE=1 启用 <10ms 断言门禁）",
        elapsed
    );
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            elapsed.as_millis() < 10,
            "PERF19 单次 check 耗时 {:?} 超过 10ms 上限（蓝图 §6.3/§7.2）",
            elapsed
        );
    }
}
