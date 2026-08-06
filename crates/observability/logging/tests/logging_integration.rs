//! v0.121.0 结构化日志 集成测试（host std 环境）.
//!
//! - INT1：多 Agent 并发日志——4 个 `std::thread` 模拟 4 个 Agent 来源，
//!   经 `Arc<Mutex<StructuredLoggerImpl>>` 共享日志器（实现非 `Sync`，
//!   Mutex 串行化为最简可靠方案），不同 agent_id、交错级别（Info/Error
//!   保留、Debug 被 min_level 丢弃）→ flush_to(MemoryLogStorage) → 按
//!   agent_id 用 LogFilter 查询，验证条数/字段/trace_id 完整性；
//! - INT2：★ v0.120.0 指标关联回归——同一 agent_id 每产生一条 ERROR
//!   日志即对 eneros-metrics `increment("agent.error_count", 1, labels,
//!   now_ms)`（labels 的 agent_id 值与日志 agent_id 字符串形式一致），
//!   最终断言：按 agent_id 过滤的 ERROR 日志条数 == metrics query 到的
//!   agent.error_count 最新值（关联在集成层验证，库本体不依赖 metrics
//!   crate，偏差 D4）；
//! - INT3：★ 出口判定复演（蓝图 §7.5）——端到端：构建多级日志
//!   （Debug..Fatal，min_level=Debug 全收）→ to_json 每条可被 JSON 解析
//!   （以 '{' 开头 '}' 结尾、含 `"level":"`、含 `"trace_id":`）→
//!   flush_to → LogFilter 按 level_min + trace_id 组合检索结果正确；
//! - PERF4：release 模式打印 ① 单条 log+to_json 均值耗时（10112 次分批
//!   计时，批间刷入不计入，测纯热路径）② MemoryLogStorage 存 1000 条后
//!   LogFilter（level_min + agent_id）query 耗时；仅当
//!   `ENEROS_PERF_GATE=1`（值恰为 "1"，规避终端残留空串误激活）时断言
//!   均值 < 20µs、查询 < 10ms（蓝图 §6.3/§7.2），默认仅打印不断言
//!   （与 v0.113.0~v0.120.0 PERF 先例一致）。
//!
//! 集成测试运行于 host std 环境（`std::thread`/`std::time::Instant`/
//! `std::env` 可用），通过 `eneros_logging` crate 公共 API 调用被测的
//! no_std 代码；INT2 另经 dev-dependency `eneros_metrics` 验证跨 crate
//! 指标关联。

// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use std::sync::{Arc, Mutex};

use eneros_logging::{
    to_json, LogBuilder, LogEntry, LogFilter, LogLevel, LogStorage, MemoryLogStorage,
    StructuredLogger, StructuredLoggerImpl,
};
use eneros_metrics::{
    Label, MemoryMetricsStore, MetricValue, MetricsCollector, MetricsCollectorImpl,
};
use heapless::String as HString;

/// 构造一个指标标签（定容字符串转换，测试代码允许 unwrap）.
fn mk_label(key: &str, value: &str) -> Label {
    Label {
        key: HString::<32>::try_from(key).unwrap(),
        value: HString::<64>::try_from(value).unwrap(),
    }
}

/// 取出指标值中的 U64（类型不符即失败）.
fn metric_as_u64(m: &eneros_metrics::Metric) -> u64 {
    match m.value {
        MetricValue::U64(v) => v,
        _ => panic!("期望 U64 指标值"),
    }
}

/// INT1：多 Agent 并发日志——4 线程交错 log → flush → 按 agent_id 查询完整性.
#[test]
fn int1_multi_agent_concurrent_logging() {
    // 共享日志器：min_level=Info（Debug 级别丢弃，验证级别过滤在并发下成立）
    let shared = Arc::new(Mutex::new(StructuredLoggerImpl::new(LogLevel::Info)));

    // 4 个 Agent 并发：每线程 8 条 Info + 4 条 Debug（丢弃）+ 4 条 Error，
    // 每 Agent 保留 12 条，共 48 条 < 缓冲 128，无缓冲丢弃
    let handles: Vec<_> = (0..4u32)
        .map(|a| {
            let c = Arc::clone(&shared);
            std::thread::spawn(move || {
                for i in 0..8u64 {
                    let seq = i.to_string();
                    let e = LogBuilder::new(
                        LogLevel::Info,
                        "agent.control",
                        "控制周期完成",
                        1000 + u64::from(a) * 100 + i,
                    )
                    .trace(u64::from(a) * 1000 + i)
                    .span(i)
                    .agent(a)
                    .field("seq", &seq)
                    .field("kind", "info")
                    .build();
                    c.lock().unwrap().log(e).unwrap();
                }
                // Debug 级别低于 min_level=Info，应被静默丢弃
                for i in 0..4u64 {
                    let e = LogBuilder::new(
                        LogLevel::Debug,
                        "agent.control",
                        "调试细节",
                        2000 + u64::from(a) * 100 + i,
                    )
                    .agent(a)
                    .build();
                    c.lock().unwrap().log(e).unwrap();
                }
                for i in 0..4u64 {
                    let seq = (100 + i).to_string();
                    let e = LogBuilder::new(
                        LogLevel::Error,
                        "agent.protection",
                        "保护动作告警",
                        3000 + u64::from(a) * 100 + i,
                    )
                    .trace(u64::from(a) * 1000 + 100 + i)
                    .span(100 + i)
                    .agent(a)
                    .field("seq", &seq)
                    .field("kind", "error")
                    .build();
                    c.lock().unwrap().log(e).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    // 统一刷入存储
    let mut store = MemoryLogStorage::new(64);
    let flushed = {
        let mut logger = shared.lock().unwrap();
        assert_eq!(logger.buffered(), 48, "48 条保留日志应全部在缓冲中");
        logger.flush_to(&mut store).unwrap()
    };
    assert_eq!(flushed, 48, "48 条日志应全部刷入 store");
    assert_eq!(store.len(), 48);

    // 按 agent_id 用 LogFilter 查询：条数/字段/trace_id 完整性
    for a in 0..4u32 {
        let got = store
            .query(&LogFilter {
                agent_id: Some(a),
                ..LogFilter::default()
            })
            .unwrap();
        assert_eq!(
            got.len(),
            12,
            "agent {a} 应恰好 12 条保留日志（Debug 被丢弃）"
        );
        assert!(got.iter().all(|e| e.agent_id == a));

        // 级别构成：8 条 Info + 4 条 Error，无 Debug
        let info_cnt = got.iter().filter(|e| e.level == LogLevel::Info).count();
        let err_cnt = got.iter().filter(|e| e.level == LogLevel::Error).count();
        assert_eq!((info_cnt, err_cnt), (8, 4), "agent {a} 级别构成不符");

        // trace_id 完整性：Info 段 a*1000+0..8 与 Error 段 a*1000+100..104 全覆盖
        let mut traces: Vec<u64> = got.iter().map(|e| e.trace_id).collect();
        traces.sort_unstable();
        let expect: Vec<u64> = (0..8u64)
            .chain(100..104u64)
            .map(|i| u64::from(a) * 1000 + i)
            .collect();
        assert_eq!(traces, expect, "agent {a} trace_id 集合不完整");

        // 字段完整性：每条含 kind 字段且与级别一致，seq 字段非空
        for e in &got {
            let kind = e
                .fields
                .iter()
                .find(|(k, _)| k.as_str() == "kind")
                .unwrap_or_else(|| panic!("agent {a} 日志缺 kind 字段"));
            let expect_kind = if e.level == LogLevel::Info {
                "info"
            } else {
                "error"
            };
            assert_eq!(kind.1.as_str(), expect_kind);
            assert!(
                e.fields
                    .iter()
                    .any(|(k, v)| k.as_str() == "seq" && !v.is_empty()),
                "agent {a} 日志缺 seq 字段"
            );
        }
    }

    // 存储中的 JSON 文本同样可检索到 agent_id 字段
    let first = store.json_of(0).unwrap();
    assert!(first.starts_with('{') && first.ends_with('}'));
    assert!(first.contains("\"agent_id\":"));
}

/// INT2：★ v0.120.0 指标关联回归——同一 agent_id 的 ERROR 日志计数 ==
/// 经 eneros-metrics increment 记录的 agent.error_count 最新值.
#[test]
fn int2_metrics_error_count_correlation() {
    const AGENT: u32 = 7;
    const ERROR_COUNT: u64 = 5;

    let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
    let mut store = MemoryLogStorage::new(16);
    // flush_interval=1ms：每次 increment 后立即刷入，保证 increment 基于
    // store 最新值连续累加（v0.120.0 已知口径：未刷入的缓冲增量不参与累加）
    let mut metrics = MetricsCollectorImpl::new(MemoryMetricsStore::new(64), 1);
    metrics.register_defaults();

    // 标签 agent_id 值与日志 agent_id 字符串形式一致（u32 7 → "7"）
    let labels = [mk_label("agent_id", "7")];

    // 同一 agent 产生 5 条 ERROR 日志；每条同步 increment 一次错误计数
    for i in 0..ERROR_COUNT {
        let e = LogBuilder::new(
            LogLevel::Error,
            "agent.protection",
            "过流保护动作",
            1000 + i * 10,
        )
        .trace(900 + i)
        .agent(AGENT)
        .field("code", "E101")
        .build();
        logger.log(e).unwrap();
        metrics
            .increment("agent.error_count", 1, &labels, 1000 + i * 10)
            .unwrap();
        metrics.maybe_flush(1000 + i * 10 + 1).unwrap();
    }
    // 同 agent 再产生 3 条 Info 日志（不计入错误计数，验证关联口径仅为 ERROR）
    for i in 0..3u64 {
        logger
            .log(
                LogBuilder::new(LogLevel::Info, "agent.control", "控制周期完成", 2000 + i)
                    .agent(AGENT)
                    .build(),
            )
            .unwrap();
    }
    let flushed = logger.flush_to(&mut store).unwrap();
    assert_eq!(flushed, 8, "5 ERROR + 3 Info 共 8 条应全部刷入");

    // 日志侧：按 agent_id + level_min=Error 检索的条数
    let err_logs = store
        .query(&LogFilter {
            level_min: Some(LogLevel::Error),
            agent_id: Some(AGENT),
            ..LogFilter::default()
        })
        .unwrap();
    assert_eq!(err_logs.len(), ERROR_COUNT as usize);

    // 指标侧：query agent.error_count，取最新值
    let points = metrics.query("agent.error_count", 0, u64::MAX).unwrap();
    assert_eq!(
        points.len(),
        ERROR_COUNT as usize,
        "每次 increment 应产生一个数据点"
    );
    let latest = metric_as_u64(points.last().unwrap());
    assert_eq!(
        latest, ERROR_COUNT,
        "error_count 应连续累加到 {ERROR_COUNT}"
    );

    // ★ 关联断言：按 agent_id 过滤的 ERROR 日志条数 == 指标最新值
    assert_eq!(
        err_logs.len() as u64,
        latest,
        "v0.120.0 指标关联回归失败：日志计数与指标计数不一致"
    );
    // 标签保留：全部数据点 agent_id 标签值为 "7"（与日志 agent_id 字符串形式一致）
    assert!(
        points.iter().all(|m| m
            .labels
            .iter()
            .any(|l| l.key.as_str() == "agent_id" && l.value.as_str() == "7")),
        "指标数据点缺 agent_id=7 标签"
    );
}

/// INT3：★ 出口判定复演——分级记录 → JSON 序列化 → flush → 组合检索端到端.
#[test]
fn int3_end_to_end_exit_criteria() {
    // min_level=Debug：五级全收
    let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
    let levels = [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
        LogLevel::Fatal,
    ];

    // 3 个 trace × 5 级 = 15 条；每条先验证 to_json 可解析性再入库
    for tid in 1..=3u64 {
        for (li, &lv) in levels.iter().enumerate() {
            let e = LogBuilder::new(
                lv,
                "agent.e2e",
                "端到端验证消息",
                5000 + tid * 100 + li as u64,
            )
            .trace(tid)
            .span(tid * 10 + li as u64)
            .agent(tid as u32)
            .field("round", "1")
            .build();
            // JSON 简单可解析断言：以 '{' 开头 '}' 结尾、含关键字段
            let json = to_json(&e);
            assert!(json.starts_with('{') && json.ends_with('}'));
            assert!(json.contains("\"level\":\""));
            assert!(json.contains("\"trace_id\":"));
            logger.log(e).unwrap();
        }
    }

    let mut store = MemoryLogStorage::new(32);
    let flushed = logger.flush_to(&mut store).unwrap();
    assert_eq!(flushed, 15, "15 条应全部刷入");
    assert_eq!(logger.buffered(), 0, "刷入后缓冲应清空");

    // ① 组合检索：level_min=Warn + trace_id=2 → 恰好 Warn/Error/Fatal 3 条
    let got = store
        .query(&LogFilter {
            level_min: Some(LogLevel::Warn),
            trace_id: Some(2),
            ..LogFilter::default()
        })
        .unwrap();
    assert_eq!(got.len(), 3);
    assert!(got
        .iter()
        .all(|e| e.trace_id == 2 && e.level >= LogLevel::Warn));
    let got_levels: Vec<LogLevel> = got.iter().map(|e| e.level).collect();
    assert_eq!(
        got_levels,
        [LogLevel::Warn, LogLevel::Error, LogLevel::Fatal]
    );

    // ② 组合检索：level_min=Error + trace_id=1 → 恰好 Error/Fatal 2 条
    let e1 = store
        .query(&LogFilter {
            level_min: Some(LogLevel::Error),
            trace_id: Some(1),
            ..LogFilter::default()
        })
        .unwrap();
    assert_eq!(e1.len(), 2);

    // ③ 全量检索：15 条全在，存储 JSON 文本同样含五级字符串
    let all = store.query(&LogFilter::default()).unwrap();
    assert_eq!(all.len(), 15);
    for i in 0..15usize {
        let json = store.json_of(i).unwrap();
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"trace_id\":"));
    }
    // ④ 空结果组合：不存在的 trace_id
    let none = store
        .query(&LogFilter {
            trace_id: Some(999),
            ..LogFilter::default()
        })
        .unwrap();
    assert!(none.is_empty());
}

/// PERF4：蓝图 §6.3/§7.2 性能基准——
/// ① 单条 log+to_json 均值：缓冲保持非满（每批 128 条刷入一次，刷入不计时），
///    测纯热路径 10112 次（79 批 × 128 ≥ 10000）取均值；
/// ② MemoryLogStorage 存 1000 条后 LogFilter（level_min + agent_id）query 耗时。
///
/// 默认仅打印不断言（含 debug/release 口径说明）；仅当
/// `ENEROS_PERF_GATE=1`（`var(...).as_deref() == Ok("1")`，与
/// v0.113.0~v0.120.0 PERF 先例一致）时启用断言门禁（均值 < 20µs、
/// 查询 1000 条 < 10ms），面向性能 CI / 目标硬件场景。
#[test]
fn perf4_log_to_json_and_query_latency() {
    let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
    let mut store = MemoryLogStorage::new(4096);

    // 预热 2 批（进入稳态，排除首次分配/缓存冷启动影响）
    for _ in 0..2 {
        for i in 0..128u64 {
            let e = LogBuilder::new(LogLevel::Info, "perf.warm", "预热", i).build();
            std::hint::black_box(logger.log(e.clone())).unwrap();
            std::hint::black_box(to_json(&e));
        }
        logger.flush_to(&mut store).unwrap();
    }

    // ① 单条 log+to_json 均值：分批计时（每批 128 条填满缓冲即止，批间刷入不计时）
    let mut total = std::time::Duration::ZERO;
    let mut done = 0u64;
    while done < 10_000 {
        // 条目构建不计入：预构建本批 128 条
        let batch: Vec<LogEntry> = (0..128u64)
            .map(|i| {
                LogBuilder::new(LogLevel::Info, "perf.agent", "性能基准日志消息", done + i)
                    .field("key", "value")
                    .trace(done + i)
                    .agent(1)
                    .build()
            })
            .collect();
        let start = std::time::Instant::now();
        for e in &batch {
            std::hint::black_box(logger.log(e.clone())).unwrap();
            std::hint::black_box(to_json(std::hint::black_box(e)));
        }
        total += start.elapsed();
        done += 128;
        // 刷入不计时：保持缓冲非满，测纯 log+to_json 热路径
        logger.flush_to(&mut store).unwrap();
    }
    let log_avg = total / done as u32;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "[PERF4-1] 单条 log+to_json 耗时: {done} 次总 {total:?}，均值 {log_avg:?}（{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <20µs 断言门禁）"
    );

    // ② 存 1000 条后 LogFilter（level_min + agent_id）query 耗时
    let mut store2 = MemoryLogStorage::new(1000);
    let levels = [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
        LogLevel::Fatal,
    ];
    for i in 0..1000u64 {
        let e = LogBuilder::new(levels[(i % 5) as usize], "perf.store", "存储查询基准", i)
            .agent((i % 10) as u32)
            .trace(i)
            .build();
        let json = to_json(&e);
        store2.append(&e, json.as_bytes()).unwrap();
    }
    let filter = LogFilter {
        level_min: Some(LogLevel::Warn),
        agent_id: Some(7),
        ..LogFilter::default()
    };
    let start = std::time::Instant::now();
    let got = store2.query(std::hint::black_box(&filter)).unwrap();
    let query_total = start.elapsed();
    println!(
        "[PERF4-2] LogFilter 查询 1000 条耗时: {query_total:?}（{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <10ms 断言门禁）"
    );
    // i%10==7 ⇒ i%5==2 ⇒ level==Warn：匹配 {7,17,...,997} 共 100 条
    assert_eq!(got.len(), 100, "组合过滤应恰好命中 100 条");
    assert!(got
        .iter()
        .all(|e| e.level == LogLevel::Warn && e.agent_id == 7));

    // 性能门禁：仅 ENEROS_PERF_GATE=1 时启用断言（蓝图 §6.3/§7.2）
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            log_avg.as_nanos() < 20_000,
            "PERF4 单条 log+to_json 均值 {log_avg:?} 超过 20µs 上限（蓝图 §7.2）"
        );
        assert!(
            query_total.as_millis() < 10,
            "PERF4 查询 1000 条耗时 {query_total:?} 超过 10ms 上限（蓝图 §6.3）"
        );
    }
}
