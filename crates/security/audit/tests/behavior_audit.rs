//! EnerOS v0.118.0 Agent 行为审计与异常检测 集成测试（INT16~INT18）
//! + 性能测试（PERF19）.
//!
//! 端到端验证「基线训练 → 加载 → 实时检测 → 分级告警」全链路：
//! - INT16：端到端——合成 200 条平稳正常事件（actor 为数字字符串，8:00 起
//!   跨 50 分钟，偶数分钟 3 条 / 奇数分钟 5 条 → RequestRate 基线
//!   mean=4.0、stddev=1.0）→ `BaselineTrainerImpl::train` →
//!   `StatAnomalyDetector::load_baseline` → 注入一分钟内高密度突发
//!   （1s 一条连续 12 条）→ 断言产生 RateSpike 告警：首条为 Low
//!   （速率 7 = mean+3σ），后升级为 High（速率 9 = mean+5σ），
//!   `evidence_seq` 精确指向触发事件；
//! - INT17：误报率（蓝图 §7.2 < 5%）——合成 500 条速率在基线 ±1σ
//!   （[3,5] 次/分）内波动的正常事件（交替 3 条/分与 5 条/分；时间构造在
//!   工作时段 8~20 点内，target 复用 3 个固定值，排除 OffHours /
//!   ScopeExpansion 干扰）→ 检测 → 告警率 < 5%；
//! - INT18：双告警——凌晨 3 点的 ControlCommand 事件 + 陌生 target 的
//!   KeyUse 事件 → OffHours + ScopeExpansion 两类告警均出现；
//! - PERF19：release 模式打印单事件 `check` 均值耗时与 1000 事件批量
//!   `check_batch` 总耗时；仅当 `ENEROS_PERF_GATE=1`（值恰为 "1"，规避
//!   终端残留空串误激活）时断言单事件 < 100µs、批量 < 50ms（蓝图
//!   §6.3/§7.2 性能指标），默认仅打印不断言（与 v0.113.0~v0.117.0
//!   PERF 先例一致）。
//!
//! 集成测试运行于 host std 环境（`std::time::Instant`/`std::env` 可用），
//! 通过 `eneros_audit` crate 公共 API 调用被测的 no_std 代码。

use eneros_audit::{
    AlertLevel, AnomalyDetector, AnomalyType, AuditEvent, AuditEventType, BaselineTrainer,
    BaselineTrainerImpl, BehaviorBaseline, BehaviorDim, StatAnomalyDetector, ThresholdConfig,
};

/// 一小时的毫秒数（非工作时段小时计算）.
const HOUR_MS: u64 = 3_600_000;

/// 一分钟的毫秒数（速率分桶粒度）.
const MINUTE_MS: u64 = 60_000;

/// 构造指定时间戳/序号的测试事件.
fn make_event(ty: AuditEventType, actor: &str, target: &str, ts: u64, seq: u64) -> AuditEvent {
    let mut ev = AuditEvent::new(ty, actor, target, "behavior audit test");
    ev.timestamp_ms = ts;
    ev.seq = seq;
    ev
}

/// 合成 200 条平稳正常事件并训练基线：actor 为数字字符串，8:00（UTC）起
/// 跨 50 分钟，偶数分钟 3 条（0/20/40s）、奇数分钟 5 条
/// （0/12/24/36/48s）→ 按分钟分桶速率序列为 25×3 + 25×5，RequestRate
/// 基线 mean=4.0、stddev=1.0、sample_count=50。
///
/// 返回（基线, 训练末事件时间戳, 训练末事件序号）。
fn train_steady_baseline(agent_id: u32) -> (BehaviorBaseline, u64, u64) {
    let actor = agent_id.to_string();
    let base = 8 * HOUR_MS; // 8:00 UTC，工作时段内
    let mut events = Vec::new();
    let mut seq = 1u64;
    for m in 0..50u64 {
        let offsets: &[u64] = if m % 2 == 0 {
            &[0, 20_000, 40_000] // 3 条/分
        } else {
            &[0, 12_000, 24_000, 36_000, 48_000] // 5 条/分
        };
        for &off in offsets {
            events.push(make_event(
                AuditEventType::AuthLogin,
                &actor,
                "meter-1",
                base + m * MINUTE_MS + off,
                seq,
            ));
            seq += 1;
        }
    }
    assert_eq!(events.len(), 200, "训练集应为 200 条");
    let last_ts = events.last().map_or(0, |ev| ev.timestamp_ms);
    let last_seq = seq - 1;
    let mut trainer = BaselineTrainerImpl::new(agent_id);
    let bl = trainer.train(&events).expect("200 条样本应训练成功");
    (bl, last_ts, last_seq)
}

// ============================================================
// INT16：端到端（训练 → 加载 → 突发注入 → RateSpike 分级告警）
// ============================================================

/// INT16：200 条平稳事件训练得 mean=4.0/stddev=1.0 基线 → 加载检测器 →
/// 注入 1s 一条连续 12 条的分钟级突发 → 第 k 条（0-based）窗口速率 = k+1：
/// 速率 7（+3σ）越 Low 阈、速率 9（+5σ）越 High 阈；断言首条告警为 Low 级
/// RateSpike 且 `evidence_seq` 指向第 7 条突发，随后升级出 High 级且
/// `evidence_seq` 指向第 9 条突发，全部告警均为 RateSpike。
#[test]
fn int16_end_to_end_train_then_detect_rate_spike() {
    let (bl, last_ts, last_seq) = train_steady_baseline(7);
    // 基线统计量断言（训练集 25×3 + 25×5：mean=4.0、stddev=1.0）
    assert_eq!(bl.agent_id, 7);
    assert_eq!(bl.window_hours, 24);
    assert_eq!(bl.trained_at, last_ts);
    let (dim, st) = bl
        .baselines
        .iter()
        .find(|(d, _)| *d == BehaviorDim::RequestRate)
        .expect("应含 RequestRate 基线");
    assert_eq!(*dim, BehaviorDim::RequestRate);
    assert!((st.mean - 4.0).abs() < 1e-9, "mean 应为 4.0");
    assert!((st.stddev - 1.0).abs() < 1e-9, "stddev 应为 1.0");
    assert_eq!(st.sample_count, 50);

    let mut det = StatAnomalyDetector::new(ThresholdConfig::default());
    det.load_baseline(bl);

    // 注入突发：紧跟训练末事件，1s 一条连续 12 条（一分钟内高密度）
    let mut alerts = Vec::new();
    for k in 0..12u64 {
        let ev = make_event(
            AuditEventType::AuthLogin,
            "7",
            "meter-1",
            last_ts + (k + 1) * 1_000,
            last_seq + 1 + k,
        );
        let batch = det.check(&ev).expect("已加载基线应检测成功");
        alerts.extend(batch);
    }

    // 第 k 条突发（0-based）时窗口速率 = k+1：
    //   k=0..5 → 速率 1~6，均未越 2σ 低阈（速率 1/2 的负偏离因冷启动抑制不报）；
    //   k=6（速率 7，+3σ）→ Low；k=7（速率 8，+4σ，未严格大于 4σ）→ Low；
    //   k=8..11（速率 9~12，+5σ 以上）→ High。
    assert!(!alerts.is_empty(), "突发必须产生告警");
    assert!(
        alerts.iter().all(|a| a.anomaly == AnomalyType::RateSpike),
        "非特权事件突发应只产生 RateSpike 告警"
    );
    // 首条告警：Low 级，evidence_seq 指向第 7 条突发（速率 7 = mean+3σ）
    let first = alerts.first().expect("应有首条告警");
    assert_eq!(first.level, AlertLevel::Low);
    assert_eq!(first.evidence_seq, last_seq + 7);
    assert_eq!(first.timestamp_ms, last_ts + 7_000);
    assert_eq!(first.agent_id, 7);
    assert_eq!(first.id, 1);
    // 升级出 High：evidence_seq 指向第 9 条突发（速率 9 = mean+5σ）
    let high = alerts
        .iter()
        .find(|a| a.level == AlertLevel::High)
        .expect("应升级出 High 级 RateSpike");
    assert_eq!(high.evidence_seq, last_seq + 9);
    assert_eq!(high.timestamp_ms, last_ts + 9_000);
    // 告警 id 严格自增
    for (i, a) in alerts.iter().enumerate() {
        assert_eq!(a.id, i as u64 + 1);
    }
}

// ============================================================
// INT17：误报率 < 5%（蓝图 §7.2）
// ============================================================

/// INT17：500 条速率在基线 ±1σ（[3,5] 次/分）内波动的正常事件——交替
/// 5 条/分（偶数分钟）与 3 条/分（奇数分钟），滑动 60s 窗口计数稳定落在
/// [3,5] 内；时间构造在工作时段（8:50 起约 125 分钟，止于 11 时前），
/// target 复用 3 个固定值，排除 OffHours / ScopeExpansion 干扰。
/// 断言告警率 < 5%。
#[test]
fn int17_false_positive_rate_under_5_percent() {
    let (bl, last_ts, last_seq) = train_steady_baseline(7);
    let mut det = StatAnomalyDetector::new(ThresholdConfig::default());
    det.load_baseline(bl);

    // 合成 500 条 ±1σ 波动正常事件（8:50 起，交替 5 条/分与 3 条/分）
    let targets = ["meter-1", "meter-2", "meter-3"];
    let mut events: Vec<AuditEvent> = Vec::new();
    let mut seq = last_seq + 1;
    let mut minute = 0u64;
    while events.len() < 500 {
        let base = last_ts + MINUTE_MS + minute * MINUTE_MS;
        let offsets: &[u64] = if minute % 2 == 0 {
            &[0, 12_000, 24_000, 36_000, 48_000] // 5 条/分（+1σ 边界）
        } else {
            &[0, 20_000, 40_000] // 3 条/分（-1σ 边界）
        };
        for &off in offsets {
            if events.len() >= 500 {
                break;
            }
            events.push(make_event(
                AuditEventType::AuthLogin,
                "7",
                targets[events.len() % 3],
                base + off,
                seq,
            ));
            seq += 1;
        }
        minute += 1;
    }
    assert_eq!(events.len(), 500);
    // 时间窗校验：全部落在 8~20 点工作时段内
    for ev in &events {
        let hour = (ev.timestamp_ms / HOUR_MS) % 24;
        assert!((8..=20).contains(&hour), "事件应构造在工作时段内");
    }

    // 检测并统计告警率
    let alerts = det.check_batch(&events).expect("检测应成功");
    let fp_rate = alerts.len() as f64 / events.len() as f64;
    println!(
        "[INT17] 正常流误报统计: {} 条告警 / {} 事件 = {:.2}%（蓝图 §7.2 门禁 < 5%）",
        alerts.len(),
        events.len(),
        fp_rate * 100.0
    );
    assert!(
        fp_rate < 0.05,
        "误报率 {:.2}% 超过蓝图 §7.2 的 5% 上限（{} 条告警：{:?}）",
        fp_rate * 100.0,
        alerts.len(),
        alerts
            .iter()
            .map(|a| (a.anomaly, a.level, a.evidence_seq))
            .collect::<Vec<_>>()
    );
}

// ============================================================
// INT18：双告警（OffHours + ScopeExpansion）
// ============================================================

/// INT18：凌晨 3:01 的 ControlCommand → Low 级 OffHours（连带陌生 target
/// 的 ScopeExpansion）；凌晨 3:01:10 的 KeyUse 作用于陌生 target → Low 级
/// ScopeExpansion（KeyUse 非控制命令，不触发 OffHours）。断言两类告警均
/// 出现且 `evidence_seq` 各指向触发事件。
#[test]
fn int18_off_hours_plus_scope_expansion_dual_alerts() {
    let (bl, ..) = train_steady_baseline(7);
    let mut det = StatAnomalyDetector::new(ThresholdConfig::default());
    det.load_baseline(bl);

    // 凌晨 3:01 控制命令（窗口仅自身，速率负偏离因冷启动抑制不报）
    let night = make_event(
        AuditEventType::ControlCommand,
        "7",
        "breaker-9",
        3 * HOUR_MS + 60_000,
        9001,
    );
    let a1 = det.check(&night).expect("检测应成功");
    let off = a1
        .iter()
        .find(|a| a.anomaly == AnomalyType::OffHours)
        .expect("凌晨 3 点控制命令应触发 OffHours");
    assert_eq!(off.level, AlertLevel::Low);
    assert_eq!(off.evidence_seq, 9001);
    assert_eq!(off.agent_id, 7);

    // 凌晨 3:01:10 陌生 target 的密钥使用（KeyUse 非控制命令，无 OffHours）
    let key = make_event(
        AuditEventType::KeyUse,
        "7",
        "key-slot-x",
        3 * HOUR_MS + 70_000,
        9002,
    );
    let a2 = det.check(&key).expect("检测应成功");
    let scope = a2
        .iter()
        .find(|a| a.anomaly == AnomalyType::ScopeExpansion)
        .expect("陌生 target 特权操作应触发 ScopeExpansion");
    assert_eq!(scope.level, AlertLevel::Low);
    assert_eq!(scope.evidence_seq, 9002);
    assert!(
        !a2.iter().any(|a| a.anomaly == AnomalyType::OffHours),
        "KeyUse 不是控制命令，不应触发 OffHours"
    );

    // 双告警汇总：OffHours + ScopeExpansion 两类均出现
    let mut all = a1.clone();
    all.extend(a2.iter().cloned());
    assert!(all.iter().any(|a| a.anomaly == AnomalyType::OffHours));
    assert!(all.iter().any(|a| a.anomaly == AnomalyType::ScopeExpansion));
}

// ============================================================
// PERF19：单事件 check 均值 + 1000 事件批量检测耗时
// ============================================================

/// PERF19：蓝图 §6.3/§7.2 性能基准——
/// ① 单事件 `check` 均值耗时：预热 100 条（窗口进入淘汰稳态）后
///    `std::time::Instant` 测 1000 次 `check` 总耗时取均值（µs 级）；
/// ② 1000 事件批量 `check_batch` 总耗时（ms 级）。
///
/// 事件流为 15s 一条的稳态正常流（4 次/分，恰为基线均值，无告警热路径，
/// 对齐线上典型负载）。默认仅打印不断言；仅当 `ENEROS_PERF_GATE=1` 时
/// 启用断言门禁（单事件 < 100µs、批量 < 50ms，与 v0.113.0~v0.117.0 PERF
/// 先例一致），面向性能 CI / 目标硬件场景。
#[test]
fn perf19_check_latency_and_batch_throughput() {
    let (bl, last_ts, last_seq) = train_steady_baseline(7);
    let mut det = StatAnomalyDetector::new(ThresholdConfig::default());
    det.load_baseline(bl);

    // 稳态事件流：8:50 起 15s 一条（4 次/分，基线均值，时间戳单调递增）
    let stream: Vec<AuditEvent> = (0..2100u64)
        .map(|k| {
            make_event(
                AuditEventType::AuthLogin,
                "7",
                "meter-1",
                last_ts + MINUTE_MS + (k + 1) * 15_000,
                last_seq + 1 + k,
            )
        })
        .collect();

    // 预热 100 条（64 条滑动窗口进入淘汰稳态，测量段含 remove(0) 开销）
    for ev in &stream[..100] {
        let _ = det.check(ev).expect("预热检测应成功");
    }

    // ① 单事件 check 均值：测 1000 次
    let start = std::time::Instant::now();
    for ev in &stream[100..1100] {
        let _ = det.check(ev).expect("计时检测应成功");
    }
    let single_total = start.elapsed();
    let single_avg = single_total / 1000;
    println!(
        "[PERF19-1] 单事件 check 耗时: 1000 次总 {:?}，均值 {:?}（设 ENEROS_PERF_GATE=1 启用 <100µs 断言门禁）",
        single_total, single_avg
    );

    // ② 1000 事件批量 check_batch 总耗时
    let start = std::time::Instant::now();
    let alerts = det
        .check_batch(&stream[1100..2100])
        .expect("批量检测应成功");
    let batch_total = start.elapsed();
    println!(
        "[PERF19-2] 1000 事件批量检测总耗时: {:?}（均值 {:?}/事件；设 ENEROS_PERF_GATE=1 启用 <50ms 断言门禁）",
        batch_total,
        batch_total / 1000
    );
    // 稳态正常流不应产生告警（计时用例有效性保证）
    assert!(alerts.is_empty(), "稳态正常流不应产生告警");

    // 性能门禁：仅 ENEROS_PERF_GATE=1 时启用断言（蓝图 §6.3/§7.2）
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            single_avg.as_micros() < 100,
            "PERF19 单事件 check 均值 {:?} 超过 100µs 上限（蓝图 §6.3/§7.2）",
            single_avg
        );
        assert!(
            batch_total.as_millis() < 50,
            "PERF19 千事件批量检测 {:?} 超过 50ms 上限（蓝图 §6.3/§7.2）",
            batch_total
        );
    }
}
