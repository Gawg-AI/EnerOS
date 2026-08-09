//! 22 §2.G-R5 闸门：决策纪律——审计可追溯 + 闸门可控 + 学习不致灾。
//!
//! 把 W5.1（决策入审计链）/ W5.2（速率+能量闸）/ W5.3（学习输出
//! safety catalog）串进同一"决策纪律会话"：
//!
//! 1. 一条完整 AI 决策（五要素）入 SM3 审计链 → verify 全绿；
//! 2. 篡改该决策记录 → verify 检出（broken_at 定位）；
//! 3. 速率超限决策被拒（GateRejected + 计数）；
//! 4. 学习输出越界被 SafetyCatalog 拒绝（用旧值 + 计数）。
//!
//! 全部断言通过 = G-R5 判据「W5.1-W5.3 验收全 PASS」的端到端证据，
//! 可声称"AI 决策可审计可控"。

use eneros_audit::{ChainVerifier, ChainVerifierImpl};
use eneros_dual_brain::audit::{DecisionAudit, DecisionRecord, ExecOutcome, SafetyOutcome};
use eneros_dual_brain::gate::{DecisionGate, GateDecision};
use eneros_dual_brain::learning::{DeadbandAdaptive, ForecastCorrector};
use eneros_safety_validator::SafetyCatalog;

#[test]
fn gr5_decision_discipline_session() {
    let catalog = SafetyCatalog::default_eps();

    // ── 1. 决策入审计链（W5.1）：五要素可追溯，verify 全绿 ────────
    let mut audit = DecisionAudit::new([0x42; 32]);
    let rec = DecisionRecord {
        actor: "energy-agent",
        target: "pcs",
        intent: "discharge 5kW during peak",
        lp_solution: "p1=5.0;p2=0.0",
        safety: SafetyOutcome::Passed,
        exec: ExecOutcome::Succeeded,
        deviation: 0.12,
        now_ms: 1000,
    };
    audit.log_decision(&rec).expect("入链");
    let report = audit.verify().expect("校验");
    assert!(report.valid, "决策链必须全绿");
    assert_eq!(audit.chain_state().entry_count, 1);

    // ── 2. 篡改检出（W5.1）：改决策记录 → broken_at 定位 ──────────
    let mut entries = audit.read_range(1, 10).expect("全链");
    entries[0].event_hash[5] ^= 0x01; // 翻转一字节 = 篡改决策
    let report = ChainVerifierImpl.verify_chain(&entries).expect("校验");
    assert!(!report.valid, "篡改必须被检出");
    assert_eq!(report.broken_at, Some(1));

    // ── 3. 速率闸（W5.2）：超限决策被拒 + 计数 ────────────────────
    let mut gate = DecisionGate::new(2, 100.0, 3_600_000);
    assert_eq!(gate.check(0, 1.0), GateDecision::Allow);
    gate.record(0, 1.0);
    assert_eq!(gate.check(100, 1.0), GateDecision::Allow);
    gate.record(100, 1.0);
    assert_eq!(gate.check(200, 1.0), GateDecision::RateLimited, "超限拒绝");
    assert_eq!(gate.rejected_rate(), 1);

    // ── 4. 学习安全闸（W5.3）：越界输出被拒 + 用旧值 ──────────────
    let mut db = DeadbandAdaptive::with_catalog(1.0, catalog);
    for _ in 0..5 {
        db.learn(100.0); // 3σ 提议远超目录上界
    }
    assert_eq!(db.current, 1.0, "越界必须用旧值");
    assert_eq!(db.rejected_updates, 5);

    let mut fc = ForecastCorrector::with_catalog(catalog);
    for _ in 0..5 {
        fc.learn(5.0); // EMA 超 ±0.5
    }
    assert_eq!(fc.correction(), 0.0, "越界回退 0 修正");
    assert_eq!(fc.rejected_updates, 5);

    // 极端输入不致灾（W5.3）：无论学什么，输出都在 catalog 界内。
    for _ in 0..1000 {
        db.learn(1e9);
        fc.learn(1e9);
    }
    assert!(catalog.check_deadband(db.current));
    assert!(catalog.check_correction(fc.correction()));
}
