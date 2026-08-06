//! v0.120.0 Metrics 指标采集 集成测试（host std 环境）.
//!
//! - INT1：多来源模拟并发记录——4 个 `std::thread` 模拟 4 个 Agent 来源，
//!   经 `Arc<Mutex<MetricsCollectorImpl>>` 共享采集器（实现非 `Sync`，
//!   Mutex 串行化为最简可靠方案），对 3 项已注册指标交错 record（标签
//!   agent_id/phase）→ maybe_flush → query，验证数据完整性（各指标条数
//!   正确、标签保留、按时间戳升序）；
//! - INT2：★ 出口判定复演（蓝图 §7）——register_defaults → 记录系统/
//!   Agent/业务三类指标（含 increment 跨刷入累加）→ maybe_flush → 范围
//!   查询结果正确（时间窗过滤、条数、值正确）；
//! - PERF3：release 模式打印 ① 单条 record 均值耗时（10240 次分批计时，
//!   批间刷入不计入，测纯 record 热路径）② 向 store 写入 1000 条后
//!   query_range 全量耗时；仅当 `ENEROS_PERF_GATE=1`（值恰为 "1"，规避
//!   终端残留空串误激活）时断言 record < 10µs、query 1000 点 < 5ms
//!   （蓝图 §7.2），默认仅打印不断言（与 v0.113.0~v0.119.0 PERF 先例
//!   一致）。
//!
//! 集成测试运行于 host std 环境（`std::thread`/`std::time::Instant`/
//! `std::env` 可用），通过 `eneros_metrics` crate 公共 API 调用被测的
//! no_std 代码。

// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use std::sync::{Arc, Mutex};

use eneros_metrics::{
    Label, MemoryMetricsStore, Metric, MetricType, MetricValue, MetricsCollector,
    MetricsCollectorImpl, MetricsError, MetricsStore,
};
use heapless::String as HString;

/// 构造一个标签（定容字符串转换，测试代码允许 unwrap）.
fn mk_label(key: &str, value: &str) -> Label {
    Label {
        key: HString::<32>::try_from(key).unwrap(),
        value: HString::<64>::try_from(value).unwrap(),
    }
}

/// 构造一条测试指标数据点（直写 store 场景使用）.
fn make_metric(name: &str, value: u64, ts: u64) -> Metric {
    Metric {
        name: HString::<64>::try_from(name).unwrap(),
        metric_type: MetricType::Counter,
        value: MetricValue::U64(value),
        labels: heapless::Vec::new(),
        timestamp_ms: ts,
    }
}

/// 断言查询结果按时间戳升序（允许相等）.
fn assert_ascending(ms: &[Metric]) {
    assert!(
        ms.windows(2)
            .all(|w| w[0].timestamp_ms <= w[1].timestamp_ms),
        "查询结果必须按时间戳升序"
    );
}

/// 取出指标值中的 U64（类型不符即失败）.
fn as_u64(m: &Metric) -> u64 {
    match m.value {
        MetricValue::U64(v) => v,
        _ => panic!("期望 U64 指标值"),
    }
}

/// INT1：多来源模拟并发记录——4 线程交错 record → flush → query 完整性.
#[test]
fn int1_multi_source_concurrent_record() {
    // 共享采集器：store 每桶 1024，刷入间隔 1000ms
    let shared = Arc::new(Mutex::new(MetricsCollectorImpl::new(
        MemoryMetricsStore::new(1024),
        1000,
    )));
    shared.lock().unwrap().register_defaults();

    // 4 个 Agent 来源并发：每线程 10 条延迟 + 5 条错误 + 5 条控制 = 20 条，
    // 共 80 条 < 缓冲 256，无丢弃
    let handles: Vec<_> = (0..4u64)
        .map(|a| {
            let c = Arc::clone(&shared);
            std::thread::spawn(move || {
                let agent = alloc_agent_name(a);
                let lat_labels = [mk_label("agent_id", &agent), mk_label("phase", "control")];
                let id_labels = [mk_label("agent_id", &agent)];
                for i in 0..10u64 {
                    c.lock()
                        .unwrap()
                        .record(
                            "agent.request_latency",
                            MetricValue::U64(5 + i),
                            &lat_labels,
                            1000 + i * 10,
                        )
                        .unwrap();
                }
                for i in 0..5u64 {
                    c.lock()
                        .unwrap()
                        .record(
                            "agent.error_count",
                            MetricValue::U64(i),
                            &id_labels,
                            2000 + i * 10,
                        )
                        .unwrap();
                    c.lock()
                        .unwrap()
                        .record(
                            "business.control_commands",
                            MetricValue::U64(100 + i),
                            &id_labels,
                            3000 + i * 10,
                        )
                        .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    // 统一刷入：距初始 last_flush=0 已超 1000ms 间隔
    let mut c = shared.lock().unwrap();
    let flushed = c.maybe_flush(10_000).unwrap();
    assert_eq!(flushed, 80, "80 条记录应全部刷入 store");

    // ① 各指标条数正确
    let lat = c.query("agent.request_latency", 0, u64::MAX).unwrap();
    let err = c.query("agent.error_count", 0, u64::MAX).unwrap();
    let cmd = c.query("business.control_commands", 0, u64::MAX).unwrap();
    assert_eq!(lat.len(), 40, "4 Agent × 10 条延迟记录");
    assert_eq!(err.len(), 20, "4 Agent × 5 条错误记录");
    assert_eq!(cmd.len(), 20, "4 Agent × 5 条控制记录");

    // ② 按时间戳升序
    assert_ascending(&lat);
    assert_ascending(&err);
    assert_ascending(&cmd);

    // ③ 标签保留：延迟记录含 agent_id + phase，且每个 Agent 恰好 10 条
    assert!(
        lat.iter().all(|m| m.metric_type == MetricType::Histogram),
        "延迟指标类型应与注册表一致（Histogram）"
    );
    let mut per_agent = [0usize; 4];
    for m in &lat {
        let agent = m
            .labels
            .iter()
            .find(|l| l.key.as_str() == "agent_id")
            .unwrap_or_else(|| panic!("延迟记录缺 agent_id 标签"));
        let idx = match agent.value.as_str() {
            "agent-0" => 0,
            "agent-1" => 1,
            "agent-2" => 2,
            "agent-3" => 3,
            other => panic!("未知 agent_id: {other}"),
        };
        per_agent[idx] += 1;
        assert!(
            m.labels
                .iter()
                .any(|l| l.key.as_str() == "phase" && l.value.as_str() == "control"),
            "延迟记录缺 phase=control 标签"
        );
    }
    assert_eq!(per_agent, [10, 10, 10, 10], "每个 Agent 恰好 10 条延迟记录");

    // 错误/控制记录同样保留 agent_id 标签
    for m in err.iter().chain(cmd.iter()) {
        assert!(
            m.labels.iter().any(|l| l.key.as_str() == "agent_id"),
            "记录缺 agent_id 标签"
        );
    }
}

/// 生成 Agent 名（栈缓冲，避免引入格式化依赖）.
fn alloc_agent_name(a: u64) -> HString<64> {
    let mut s = HString::<64>::try_from("agent-").unwrap();
    let digit = char::from(b'0' + a as u8);
    let mut buf = [0u8; 1];
    s.push_str(digit.encode_utf8(&mut buf)).unwrap();
    s
}

/// INT2：★ 出口判定复演——注册默认指标 → 三类记录 → 刷入 → 范围查询正确.
#[test]
fn int2_end_to_end_exit_criteria() {
    let mut c = MetricsCollectorImpl::new(MemoryMetricsStore::new(1024), 100);
    c.register_defaults();

    // 系统类：3 个时间点 CPU + 内存 + 温度（Gauge/F64）
    c.record("system.cpu_usage", MetricValue::F64(0.42), &[], 10)
        .unwrap();
    c.record("system.cpu_usage", MetricValue::F64(0.55), &[], 300)
        .unwrap();
    c.record("system.cpu_usage", MetricValue::F64(0.61), &[], 700)
        .unwrap();
    c.record("system.mem_usage", MetricValue::F64(0.48), &[], 20)
        .unwrap();
    c.record("system.temperature", MetricValue::F64(46.5), &[], 30)
        .unwrap();

    // Agent 类：活跃数 + 请求延迟（带 phase 标签）
    c.record("agent.active_count", MetricValue::U64(7), &[], 40)
        .unwrap();
    c.record(
        "agent.request_latency",
        MetricValue::U64(12),
        &[mk_label("phase", "control")],
        50,
    )
    .unwrap();

    // 业务类：控制指令 increment（首次基于 store 空值累加）
    c.increment("business.control_commands", 3, &[], 60)
        .unwrap();

    // 第一次刷入（710 - 0 >= 100）：8 条全部落 store
    let n = c.maybe_flush(710).unwrap();
    assert_eq!(n, 8);

    // 跨刷入 increment：基于 store 最新值 3 累加 → 8
    c.increment("business.control_commands", 5, &[], 720)
        .unwrap();
    c.record("business.optimization_calls", MetricValue::U64(2), &[], 730)
        .unwrap();
    let n = c.maybe_flush(820).unwrap();
    assert_eq!(n, 2);

    // ① 时间窗过滤：CPU 3 点落在 [100,600] 仅 1 点（ts=300）
    let win = c.query("system.cpu_usage", 100, 600).unwrap();
    assert_eq!(win.len(), 1, "时间窗 [100,600] 应仅含 ts=300 一点");
    match win[0].value {
        MetricValue::F64(v) => assert!((v - 0.55).abs() < 1e-9),
        _ => panic!("期望 F64 指标值"),
    }
    assert_eq!(win[0].timestamp_ms, 300);

    // ② 全量查询：CPU 3 点升序、值正确
    let all = c.query("system.cpu_usage", 0, 1000).unwrap();
    assert_eq!(all.len(), 3);
    assert_ascending(&all);
    match all[2].value {
        MetricValue::F64(v) => assert!((v - 0.61).abs() < 1e-9),
        _ => panic!("期望 F64 指标值"),
    }

    // ③ increment 跨刷入累加：两次记录值为 3 与 8
    let cmd = c.query("business.control_commands", 0, 1000).unwrap();
    assert_eq!(cmd.len(), 2);
    assert_eq!(as_u64(&cmd[0]), 3);
    assert_eq!(as_u64(&cmd[1]), 8);

    // ④ 其余指标条数正确
    assert_eq!(c.query("system.mem_usage", 0, 1000).unwrap().len(), 1);
    assert_eq!(c.query("system.temperature", 0, 1000).unwrap().len(), 1);
    assert_eq!(c.query("agent.active_count", 0, 1000).unwrap().len(), 1);
    assert_eq!(c.query("agent.request_latency", 0, 1000).unwrap().len(), 1);
    assert_eq!(
        c.query("business.optimization_calls", 0, 1000)
            .unwrap()
            .len(),
        1
    );

    // ⑤ 查询范围非法拒绝
    assert!(matches!(
        c.query("system.cpu_usage", 600, 100),
        Err(MetricsError::QueryRangeInvalid)
    ));
}

/// PERF3：蓝图 §7.2 性能基准——
/// ① 单条 record 均值：缓冲保持非满（每 256 条刷入一次，刷入不计时），
///    测纯 record 热路径 10240 次取均值；
/// ② 向 store 写入 1000 条后 query_range 全量查询耗时。
///
/// 默认仅打印不断言（含 debug/release 口径说明）；仅当
/// `ENEROS_PERF_GATE=1`（`var(...).as_deref() == Ok("1")`，与
/// v0.113.0~v0.119.0 PERF 先例一致）时启用断言门禁（record < 10µs、
/// query 1000 点 < 5ms），面向性能 CI / 目标硬件场景。
#[test]
fn perf3_record_and_query_latency() {
    // flush_interval=0：maybe_flush 恒成功，批间刷入不计入 record 计时
    let mut c = MetricsCollectorImpl::new(MemoryMetricsStore::new(4096), 0);
    c.register_defaults();

    // 预热 1024 次（进入稳态，排除首次分配/缓存冷启动影响）
    for batch in 0..4u64 {
        for i in 0..256u64 {
            std::hint::black_box(c.record(
                "system.cpu_usage",
                MetricValue::F64(0.5),
                &[],
                batch * 256 + i,
            ))
            .unwrap();
        }
        c.maybe_flush(batch * 256 + 256).unwrap();
    }

    // ① 单条 record 均值：分批计时（每批 256 条填满缓冲即止，批间刷入不计时）
    let mut total = std::time::Duration::ZERO;
    let mut batches = 0u64;
    while batches * 256 < 10_000 {
        let start = std::time::Instant::now();
        for i in 0..256u64 {
            std::hint::black_box(c.record(
                "system.cpu_usage",
                MetricValue::F64(0.5),
                &[],
                10_000 + batches * 256 + i,
            ))
            .unwrap();
        }
        total += start.elapsed();
        batches += 1;
        // 刷入不计时：保持缓冲非满，测纯 record 热路径
        c.maybe_flush(10_000 + batches * 256).unwrap();
    }
    let n = batches * 256;
    let record_avg = total / n as u32;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "[PERF3-1] 单条 record 耗时: {n} 次总 {total:?}，均值 {record_avg:?}（{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <10µs 断言门禁）"
    );

    // ② 写入 1000 条后 query_range 全量查询耗时
    let mut store = MemoryMetricsStore::new(1000);
    for i in 0..1000u64 {
        store.append(&make_metric("perf.metric", i, i)).unwrap();
    }
    let start = std::time::Instant::now();
    let got = store.query_range("perf.metric", 0, u64::MAX).unwrap();
    let query_total = start.elapsed();
    println!(
        "[PERF3-2] query_range 1000 数据点耗时: {query_total:?}（{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <5ms 断言门禁）"
    );
    assert_eq!(got.len(), 1000, "查询结果应为全量 1000 点");
    assert_ascending(&got);

    // 性能门禁：仅 ENEROS_PERF_GATE=1 时启用断言（蓝图 §7.2）
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            record_avg.as_nanos() < 10_000,
            "PERF3 单条 record 均值 {record_avg:?} 超过 10µs 上限（蓝图 §7.2）"
        );
        assert!(
            query_total.as_millis() < 5,
            "PERF3 query 1000 点耗时 {query_total:?} 超过 5ms 上限（蓝图 §7.2）"
        );
    }
}
