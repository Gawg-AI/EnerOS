//! SCADA 数据采集基准测试 (T030-06)
//!
//! 测量 SCADA 热点路径的单点延迟：
//! - refresh：数据源刷新（MockDataSource::insert 模拟上游数据更新）
//! - collect：单点采集（ScadaCollector::collect_once 读取 + 质量检查）
//! - store：单点入库（TimeSeriesEngine::record 持久化）
//!
//! 运行：cargo bench -p eneros-benches --bench scada_bench

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use eneros_scada::{MockDataSource, ScadaCollector, ScadaConfig, ScadaPoint};
use eneros_timeseries::TimeSeriesEngine;

/// 构建单点 SCADA 配置（最小化配置，聚焦单点延迟）
fn build_single_point_config() -> ScadaConfig {
    ScadaConfig {
        points: vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 1000,
            deadband: 0.01,
            min_value: Some(0.8),
            max_value: Some(1.2),
        }],
        default_scan_rate_ms: 1000,
        timeout_ms: 5000,
        enable_quality_check: true,
        pool: Default::default(),
    }
}

/// refresh 基准测试：数据源单点刷新延迟
///
/// 测量 MockDataSource::insert 的延迟——模拟上游数据源（MQTT/Modbus/IEC104）
/// 将最新值推入缓存的路径。这是 SCADA 采集管线的第一步。
fn bench_scada_refresh(c: &mut Criterion) {
    let mock = std::sync::Arc::new(MockDataSource::new());

    let mut group = c.benchmark_group("scada_refresh");
    group.sample_size(500);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("single_point", |b| {
        b.iter(|| {
            mock.insert(black_box(1), black_box("voltage_pu"), black_box(1.02));
            black_box(());
        });
    });

    group.finish();
}

/// collect 基准测试：单点采集延迟
///
/// 测量 ScadaCollector::collect_once() 的延迟——读取数据源缓存值、
/// 构建 ScadaReading、执行质量检查（min/max 范围校验）。
fn bench_scada_collect(c: &mut Criterion) {
    let mock = std::sync::Arc::new(MockDataSource::new());
    mock.insert(1, "voltage_pu", 1.02);

    let config = build_single_point_config();
    let collector = ScadaCollector::new(config, mock);

    let mut group = c.benchmark_group("scada_collect");
    group.sample_size(500);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("single_point", |b| {
        b.iter(|| {
            black_box(collector.collect_once());
        });
    });

    group.finish();
}

/// store 基准测试：单点入库延迟
///
/// 测量 TimeSeriesEngine::record() 的延迟——将 ScadaReading 写入
/// 内存时序存储（含 max_retention 淘汰逻辑）。
fn bench_scada_store(c: &mut Criterion) {
    let ts_engine = TimeSeriesEngine::new(10000);
    let timestamp = chrono::Utc::now();

    let mut group = c.benchmark_group("scada_store");
    group.sample_size(500);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("single_point", |b| {
        b.iter(|| {
            ts_engine
                .record(
                    black_box(1),
                    black_box("voltage_pu"),
                    black_box(1.02),
                    black_box(timestamp),
                )
                .unwrap();
            black_box(());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_scada_refresh, bench_scada_collect, bench_scada_store);
criterion_main!(benches);
