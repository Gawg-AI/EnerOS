//! SCADA 数据采集热点路径基准测试 (T029-12)
//!
//! 测量真实业务路径：数据读取 → 解析 → 质量检查 → 入库
//! 使用 MockDataSource（真实 DataSource trait 实现）+ 真实 ScadaCollector +
//! 真实 DataPipeline + 真实 TimeSeriesEngine，模拟 IEEE-14 节点系统规模。
//!
//! 运行：cargo bench -p eneros-scada

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use eneros_scada::{
    DataPipeline, MockDataSource, ScadaCollector, ScadaConfig, ScadaPoint,
};
use eneros_timeseries::TimeSeriesEngine;

/// 构建 IEEE-14 节点规模的 SCADA 配置（14 母线 × 4 参数 = 56 测点）
fn build_ieee14_config() -> ScadaConfig {
    let mut points = Vec::with_capacity(56);
    let parameters = [
        ("voltage_pu", 0.8, 1.2),
        ("active_power_mw", -100.0, 500.0),
        ("reactive_power_mvar", -200.0, 200.0),
        ("frequency_hz", 49.5, 50.5),
    ];

    for bus_id in 1..=14u64 {
        for (param, min, max) in &parameters {
            points.push(ScadaPoint {
                element_id: bus_id,
                parameter: param.to_string(),
                scan_rate_ms: 1000,
                deadband: 0.01,
                min_value: Some(*min),
                max_value: Some(*max),
            });
        }
    }

    ScadaConfig {
        points,
        default_scan_rate_ms: 1000,
        timeout_ms: 5000,
        enable_quality_check: true,
        pool: Default::default(),
    }
}

/// 构建基准测试用的数据管线（真实业务逻辑，mock 输入数据）
fn build_pipeline() -> DataPipeline {
    let mock = Arc::new(MockDataSource::new());

    // 为每个测点注入合理的电力系统典型值
    for bus_id in 1..=14u64 {
        mock.insert(bus_id, "voltage_pu", 1.02);
        mock.insert(bus_id, "active_power_mw", 50.0 + bus_id as f64 * 1.5);
        mock.insert(bus_id, "reactive_power_mvar", 10.0 + bus_id as f64 * 0.5);
        mock.insert(bus_id, "frequency_hz", 50.0);
    }

    let config = build_ieee14_config();
    let collector = Arc::new(ScadaCollector::new(config, mock));
    let ts_engine = Arc::new(TimeSeriesEngine::new(10000));

    DataPipeline::new(collector, ts_engine)
}

/// SCADA 数据采集完整路径基准测试
///
/// 测量 DataPipeline::run_once() 的完整周期：
/// 1. refresh_data_source() — 刷新上游数据源
/// 2. collect_once() — 读取缓存值、构建 ScadaReading、质量检查
/// 3. detect_soe_events() — 断路器/开关状态变化检测
/// 4. ts_engine.record() — 持久化到时间序列引擎
fn bench_scada_collection(c: &mut Criterion) {
    let pipeline = build_pipeline();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("scada_collection");
    // 1000 次足够，避免过长测试时间
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("run_once_ieee14_56points", |b| {
        b.to_async(&rt).iter(|| async {
            // 使用 black_box 防止编译器优化掉计算
            std::hint::black_box(pipeline.run_once().await.unwrap());
        });
    });

    group.finish();
}

/// SCADA 单次采集（不含入库）基准测试
///
/// 仅测量 ScadaCollector::collect_once()：读取 → 质量检查 → 更新缓存
/// 用于隔离采集逻辑与入库逻辑的性能
fn bench_scada_collect_only(c: &mut Criterion) {
    let mock = Arc::new(MockDataSource::new());
    for bus_id in 1..=14u64 {
        mock.insert(bus_id, "voltage_pu", 1.02);
        mock.insert(bus_id, "active_power_mw", 50.0);
        mock.insert(bus_id, "reactive_power_mvar", 10.0);
        mock.insert(bus_id, "frequency_hz", 50.0);
    }
    let config = build_ieee14_config();
    let collector = ScadaCollector::new(config, mock);

    let mut group = c.benchmark_group("scada_collect_only");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("collect_once_56points", |b| {
        b.iter(|| {
            // 使用 black_box 防止编译器优化掉计算
            std::hint::black_box(collector.collect_once())
        });
    });

    group.finish();
}

criterion_group!(benches, bench_scada_collection, bench_scada_collect_only);
criterion_main!(benches);
