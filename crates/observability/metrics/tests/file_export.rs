//! 33 OB-07 端到端导出证据链（host std 环境）。
//!
//! record → flush → JSONL 文件可读回：经 `--features std` 启用
//! `eneros-metrics` 的 FileMetricsStore 后端，采集器的存储后端直接
//! 挂文件 store，maybe_flush 即落盘 JSONL，可由新实例读回并经
//! query_range / get_latest 查询（商用验收的 host 可跑证据链）。
//!
//! 运行：`cargo test -p eneros-metrics --features std`
//! （默认 `cargo test --workspace` 下本文件整体 cfg 关闭，no_std 面不受影响）

#![cfg(feature = "std")]
// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use eneros_metrics::{
    FileMetricsStore, Label, MetricDef, MetricType, MetricValue, MetricsCollector,
    MetricsCollectorImpl, MetricsStore,
};
use heapless::String as HString;

fn mk_label(key: &str, value: &str) -> Label {
    Label {
        key: HString::<32>::try_from(key).unwrap(),
        value: HString::<64>::try_from(value).unwrap(),
    }
}

#[cfg(feature = "std")]
#[test]
fn e2e_record_flush_jsonl_readback() {
    let dir = std::env::temp_dir().join(format!("eneros-metric-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("export.jsonl");
    let path_str = path.to_str().unwrap();

    // 1) record：采集器存储后端直接挂文件 store（JSONL 落盘目标）
    let mut c = MetricsCollectorImpl::new(FileMetricsStore::open(path_str).unwrap(), 1_000_000);
    c.register(MetricDef {
        name: HString::<64>::try_from("agent.error_count").unwrap(),
        metric_type: MetricType::Counter,
        help: HString::<128>::try_from("错误计数").unwrap(),
        unit: HString::<16>::try_from("count").unwrap(),
    })
    .unwrap();
    c.increment("agent.error_count", 1, &[], 1000).unwrap();
    c.increment("agent.error_count", 1, &[], 2000).unwrap();

    // 2) flush：批量落盘为 JSONL（每条一行）
    let n = c.maybe_flush(2_000_000).unwrap();
    assert_eq!(n, 2, "两条数据点应 flush 到文件 store");

    // 3) 读回：磁盘 JSONL 行数与内容一致
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw.lines().count(), 2, "JSONL 应恰两行");
    assert!(raw.contains("\"name\":\"agent.error_count\""));

    // 4) 重启语义：新实例打开同一文件 → query_range / get_latest
    let reopened = FileMetricsStore::open(path_str).unwrap();
    let got = reopened
        .query_range("agent.error_count", 1000, 2000)
        .unwrap();
    assert_eq!(got.len(), 2);
    match got[1].value {
        MetricValue::U64(v) => assert_eq!(v, 2),
        _ => panic!("期望 U64"),
    }
    assert_eq!(reopened.get_latest("agent.error_count"), Some(2));

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "std")]
#[test]
fn e2e_labels_roundtrip() {
    let dir = std::env::temp_dir().join(format!("eneros-metric-e2e2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("labels.jsonl");
    let path_str = path.to_str().unwrap();

    let mut c = MetricsCollectorImpl::new(FileMetricsStore::open(path_str).unwrap(), 1_000_000);
    c.register(MetricDef {
        name: HString::<64>::try_from("agent.request_latency").unwrap(),
        metric_type: MetricType::Gauge,
        help: HString::<128>::try_from("请求延迟").unwrap(),
        unit: HString::<16>::try_from("ms").unwrap(),
    })
    .unwrap();
    c.record(
        "agent.request_latency",
        MetricValue::F64(1.5),
        &[mk_label("agent_id", "3")],
        500,
    )
    .unwrap();
    c.maybe_flush(2_000_000).unwrap();

    let reopened = FileMetricsStore::open(path_str).unwrap();
    let got = reopened
        .query_range("agent.request_latency", 0, 1000)
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].labels.len(), 1);
    assert_eq!(got[0].labels[0].key.as_str(), "agent_id");
    assert_eq!(got[0].labels[0].value.as_str(), "3");

    let _ = std::fs::remove_dir_all(&dir);
}
