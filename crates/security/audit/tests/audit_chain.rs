//! EnerOS v0.117.0 审计日志与哈希链 集成测试（INT18~INT20）+ 性能测试（PERF21）.
//!
//! 端到端验证「审计事件编码 + SHA-256 哈希链存证 + 链校验 + 重启恢复」全链路：
//! - INT18：千条链构建验证（1000 条事件连续入链 → read_range(1,1000) 全量
//!   读取 → verify_chain valid=true、total==1000、broken_at==None，且
//!   chain_state().entry_count==1000——蓝图 §6.2「1000 条事件链构建+验证」）；
//! - INT19：重启恢复续写（500 条后克隆存储 recover → 链头三字段恢复一致 →
//!   续写 500 条 → 全量 1000 条 verify_chain valid=true，且第 501 条
//!   prev_hash == 第 500 条 combined_hash——蓝图 §6.2「重启后链状态恢复」）；
//! - INT20：故障注入三场景（蓝图 §6.5）：①篡改第 10 条 event_hash 翻转
//!   一字节 → broken_at==Some(10)；②删除中间第 10 条 → 第 11 条 prev_hash
//!   衔接失败 → broken_at==Some(11)；③尾部删除断层 → 存储尾条 seq 与期望
//!   链尾 last_seq 差值检出（完整 20 条 vs 删除后 19 条）；
//! - PERF21：①单条 log 均值耗时（预热 100 条后测 1000 次取均值，µs 级）；
//!   ②10000 条链 verify_chain 总耗时（ms 级），均 println! 打印；仅当
//!   `ENEROS_PERF_GATE=1`（值恰为 "1"，规避终端残留空串误激活）时断言
//!   单条均值 < 1ms 且万条验证 < 500ms（蓝图 §6.3/§7.2 性能指标，门禁
//!   口径同 v0.116.0 PERF19 先例），默认仅打印不断言；测试本身断言链
//!   valid 保证计时用例有效。
//!
//! 集成测试运行于 host std 环境（`std::time::Instant`/`std::env`/`format!`
//! 可用），通过 `eneros_audit` crate 公共 API 调用被测的 no_std 代码。

use eneros_audit::{
    AuditEvent, AuditEventType, AuditLogger, AuditLoggerImpl, AuditStore, ChainVerifier,
    ChainVerifierImpl, MemStore,
};

/// 测试创世哈希（与 crate 内单测口径一致）.
const GENESIS: [u8; 32] = [0x42u8; 32];

/// 11 类审计事件轮转表（判别值 1~11 全覆盖）.
const EVENT_TYPES: [AuditEventType; 11] = [
    AuditEventType::AuthLogin,
    AuditEventType::AuthLogout,
    AuditEventType::ConfigChange,
    AuditEventType::ControlCommand,
    AuditEventType::ModelLoad,
    AuditEventType::KeyUse,
    AuditEventType::AgentStart,
    AuditEventType::AgentStop,
    AuditEventType::FederateJoin,
    AuditEventType::FederateLeave,
    AuditEventType::SecurityAlert,
];

/// 构造含序号区分的测试事件（`seq == 0` 自动赋序；`i` 为 1-based 逻辑序号，
/// event_type 轮转 11 种，actor/target/detail 含序号使每条事件哈希互异）.
fn make_event(i: u64) -> AuditEvent {
    let event_type = EVENT_TYPES[((i - 1) % 11) as usize];
    AuditEvent::new(
        event_type,
        &format!("agent-{i:04}"),
        &format!("target-{i:04}"),
        &format!("detail of event #{i}"),
    )
}

/// 建 logger 并连续追加 `n` 条含序号区分的事件（seq 1~n）.
fn build_chain(n: u64) -> AuditLoggerImpl<MemStore> {
    let mut logger = AuditLoggerImpl::new(MemStore::new(), GENESIS);
    for i in 1..=n {
        logger.log(make_event(i)).expect("追加应成功");
    }
    logger
}

// ============================================================
// INT18：千条链构建 + 全量验证
// ============================================================

/// INT18：genesis 固定 [0x42;32]，连续 log 1000 条（11 类事件轮转、字段含
/// 序号区分）→ read_range(1,1000) 全量 → verify_chain valid=true、
/// total==1000、broken_at==None；chain_state().entry_count==1000.
#[test]
fn int18_thousand_entry_chain_build_and_verify() {
    let logger = build_chain(1000);

    // 链头状态断言
    assert_eq!(logger.chain_state().entry_count, 1000);
    assert_eq!(logger.chain_state().last_seq, 1000);
    assert_eq!(logger.store().len(), 1000);

    // 全量读取：序号连续、首条锚定创世哈希
    let entries = logger.store().read_range(1, 1000).expect("全量读取应成功");
    assert_eq!(entries.len(), 1000);
    assert_eq!(entries[0].seq, 1);
    assert_eq!(entries[999].seq, 1000);
    assert_eq!(entries[0].prev_hash, GENESIS);

    // 全量链验证
    let verifier = ChainVerifierImpl;
    let report = verifier.verify_chain(&entries).expect("校验应成功");
    assert!(report.valid, "千条净链必须验证通过");
    assert_eq!(report.total, 1000);
    assert_eq!(report.broken_at, None);

    // 链头 last_hash 与末条 combined_hash 一致（链尾视图自洽）
    assert_eq!(logger.chain_state().last_hash, entries[999].combined_hash);
}

// ============================================================
// INT19：重启恢复续写
// ============================================================

/// INT19：log 500 条 → 克隆存储 recover（模拟断电重启）→ 断言恢复三字段 →
/// 续写 500 条 → 全量 1000 条 verify_chain valid=true，且第 501 条
/// prev_hash == 第 500 条 combined_hash（跨重启接缝衔接）.
#[test]
fn int19_restart_recovery_and_continue_chain() {
    // 阶段一：写 500 条，取链头快照与末条存证
    let logger = build_chain(500);
    let before = logger.chain_state().clone();
    let tail = logger.store().read_last_n(1).expect("读取末条应成功")[0].clone();

    // 阶段二：克隆存储交给 recover 消费（模拟重启后从持久化存储恢复链头）
    let store_clone = logger.store().clone();
    let mut recovered = AuditLoggerImpl::recover(store_clone, GENESIS).expect("恢复应成功");

    // 恢复三字段断言（last_hash / last_seq / entry_count）
    assert_eq!(recovered.chain_state().last_hash, before.last_hash);
    assert_eq!(recovered.chain_state().last_seq, before.last_seq);
    assert_eq!(recovered.chain_state().entry_count, before.entry_count);
    assert_eq!(recovered.chain_state().genesis_hash, GENESIS);

    // 阶段三：恢复后续写 500 条（seq 501~1000）
    for i in 501..=1000u64 {
        recovered.log(make_event(i)).expect("恢复后续写应成功");
    }
    assert_eq!(recovered.chain_state().entry_count, 1000);
    assert_eq!(recovered.chain_state().last_seq, 1000);

    // 全量 1000 条验证 valid
    let entries = recovered
        .store()
        .read_range(1, 1000)
        .expect("全量读取应成功");
    assert_eq!(entries.len(), 1000);
    let verifier = ChainVerifierImpl;
    let report = verifier.verify_chain(&entries).expect("校验应成功");
    assert!(report.valid, "跨重启续写的千条链必须验证通过");
    assert_eq!(report.total, 1000);
    assert_eq!(report.broken_at, None);

    // 跨重启接缝衔接：第 501 条 prev_hash == 第 500 条 combined_hash
    assert_eq!(entries[500].seq, 501);
    assert_eq!(entries[499].seq, 500);
    assert_eq!(entries[500].prev_hash, entries[499].combined_hash);
    assert_eq!(entries[500].prev_hash, tail.combined_hash);
}

// ============================================================
// INT20：故障注入三场景（篡改 / 中间删除 / 尾部删除断层）
// ============================================================

/// INT20：蓝图 §6.5 故障注入——
/// ①篡改：20 条净链篡改第 10 条 event_hash 翻转一字节 → broken_at==Some(10)；
/// ②删除中间：同一净链去掉第 10 条 → 第 11 条衔接失败 → broken_at==Some(11)；
/// ③尾部删除断层：模拟删除链尾末条后存储仅余 19 条（seq 连续到 19），
///   净前缀自身验证 valid，但存储尾条 seq(19) 与期望链尾 last_seq(20) 差值
///   检出尾部缺失.
#[test]
fn int20_fault_injection_tamper_delete_tail_gap() {
    let verifier = ChainVerifierImpl;
    let logger = build_chain(20);
    let clean = logger.store().read_range(1, 20).expect("读取应成功");
    assert_eq!(clean.len(), 20);

    // 场景①：篡改第 10 条 event_hash（翻转一字节）→ 组合哈希自洽检查失败
    let mut tampered = clean.clone();
    tampered[9].event_hash[0] ^= 0xFF;
    let report = verifier.verify_chain(&tampered).expect("校验应成功");
    assert!(!report.valid, "篡改必须被检出");
    assert_eq!(report.broken_at, Some(10));
    assert_eq!(report.total, 20);

    // 场景②：删除中间第 10 条 → 第 11 条 prev_hash 衔接不上第 9 条
    let mut middle_deleted = clean.clone();
    middle_deleted.remove(9); // 去掉 seq == 10
    assert_eq!(middle_deleted.len(), 19);
    let report = verifier.verify_chain(&middle_deleted).expect("校验应成功");
    assert!(!report.valid, "中间删除必须被检出");
    assert_eq!(report.broken_at, Some(11));

    // 场景③：尾部删除断层——模拟攻击者删除链尾末条（seq == 20）
    // 用第二个手工构造的 MemStore 只 append 前 19 条，模拟删除尾部后的存储
    let mut tail_deleted_store = MemStore::new();
    for entry in clean.iter().take(19) {
        tail_deleted_store.append(entry).expect("追加应成功");
    }
    assert_eq!(tail_deleted_store.count(), Ok(19));
    let stored = tail_deleted_store.read_range(1, 20).expect("读取应成功");
    assert_eq!(stored.len(), 19);
    assert_eq!(stored[stored.len() - 1].seq, 19, "删除尾部后 seq 连续到 19");
    // 净前缀 19 条自身链接关系完整（尾部截断不破坏前缀链）
    let report = verifier.verify_chain(&stored).expect("校验应成功");
    assert!(report.valid, "净前缀链自身链接关系完整");
    // 断层检出点：完整链 last_seq==20，删除后存储尾条 seq==19，
    // 与期望链尾不符即检出尾部缺失（count 差值关系）
    let expect_last_seq = logger.chain_state().last_seq;
    assert_eq!(expect_last_seq, 20);
    assert_eq!(
        expect_last_seq - stored[stored.len() - 1].seq,
        1,
        "尾条序号与期望链尾 last_seq 差值即检出尾部缺失 1 条"
    );
    // 对照：完整存储尾部 15 条切片 seq 6~20，其尾条与删除后存储尾条相差 1
    let tail15 = logger.store().read_last_n(15).expect("尾部读取应成功");
    assert_eq!(tail15.len(), 15);
    assert_eq!(tail15[0].seq, 6);
    assert_eq!(tail15[tail15.len() - 1].seq, 20);
    assert_eq!(
        tail15[tail15.len() - 1].seq - stored[stored.len() - 1].seq,
        1
    );
}

// ============================================================
// PERF21：单条 log 均值 + 万条链验证总耗时
// ============================================================

/// PERF21：蓝图 §6.3/§7.2 性能基准——
/// ①单条 log 耗时：预热 100 条后 `std::time::Instant` 测 1000 次 log 总耗时
///   取均值并打印（µs 级）；
/// ②10000 条链验证：构建 10000 条后 read_range 全量，Instant 测
///   verify_chain 总耗时并打印（ms 级）。
///
/// 默认仅打印不断言；仅当 `ENEROS_PERF_GATE=1` 时启用断言门禁（单条均值
/// < 1ms 且万条验证 < 500ms，同 v0.116.0 PERF19 口径），面向性能 CI /
/// 目标硬件场景。测试本身断言链 valid 保证计时用例有效。
#[test]
fn perf21_log_latency_and_10k_chain_verify() {
    // ① 单条 log 耗时：预热 100 条后测 1000 次
    let mut logger = AuditLoggerImpl::new(MemStore::new(), GENESIS);
    for i in 1..=100u64 {
        logger.log(make_event(i)).expect("预热追加应成功");
    }
    let start = std::time::Instant::now();
    for i in 101..=1100u64 {
        logger.log(make_event(i)).expect("计时追加应成功");
    }
    let log_total = start.elapsed();
    let log_avg = log_total / 1000;
    println!(
        "[PERF21-1] 单条 log 耗时: 1000 次总 {:?}，均值 {:?}（设 ENEROS_PERF_GATE=1 启用 <1ms 断言门禁）",
        log_total, log_avg
    );
    // 计时链自身完整（测试有效性保证）
    assert_eq!(logger.chain_state().entry_count, 1100);
    assert_eq!(logger.store().len(), 1100);

    // ② 10000 条链验证：构建 10000 条后全量读取，计时 verify_chain
    let logger10k = build_chain(10_000);
    assert_eq!(logger10k.chain_state().entry_count, 10_000);
    let entries = logger10k
        .store()
        .read_range(1, 10_000)
        .expect("全量读取应成功");
    assert_eq!(entries.len(), 10_000);
    let verifier = ChainVerifierImpl;
    let start = std::time::Instant::now();
    let report = verifier.verify_chain(&entries).expect("校验应成功");
    let verify_total = start.elapsed();
    println!(
        "[PERF21-2] 10000 条链验证总耗时: {:?}（设 ENEROS_PERF_GATE=1 启用 <500ms 断言门禁）",
        verify_total
    );
    assert!(report.valid, "万条净链必须验证通过");
    assert_eq!(report.total, 10_000);
    assert_eq!(report.broken_at, None);

    // 性能门禁：仅 ENEROS_PERF_GATE=1 时启用断言（蓝图 §6.3/§7.2）
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            log_avg.as_micros() < 1000,
            "PERF21 单条 log 均值 {:?} 超过 1ms 上限（蓝图 §6.3/§7.2）",
            log_avg
        );
        assert!(
            verify_total.as_millis() < 500,
            "PERF21 万条链验证 {:?} 超过 500ms 上限（蓝图 §6.3/§7.2）",
            verify_total
        );
    }
}
