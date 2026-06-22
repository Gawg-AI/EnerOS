//! 潮流计算基准测试 (T030-06)
//!
//! 测量潮流求解器的端到端延迟：
//! - IEEE-14：标准 14 节点测试系统（Newton-Raphson 法）
//! - IEEE-118：合成 118 节点系统（星形拓扑，模拟大规模电网求解延迟）
//!
//! 运行：cargo bench -p eneros-benches --bench powerflow_bench

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use eneros_core::{BusTypeNR, ElementId, YBusMatrix};
use eneros_powerflow::{PowerFlowSolver, ieee14};

/// IEEE-14 潮流求解基准测试
///
/// 测量 PowerFlowSolver::solve() 在标准 IEEE-14 节点系统上的延迟：
/// YBus 构建 → Jacobian 计算 → Newton-Raphson 迭代 → 收敛判定 → 结果返回
fn bench_powerflow_ieee14(c: &mut Criterion) {
    let data = ieee14();
    let (ybus, p_spec, q_spec, bus_types) = data.to_solver_input();
    let solver = PowerFlowSolver::default_solver();

    let mut group = c.benchmark_group("powerflow_ieee14");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("solve_newton_raphson", |b| {
        b.iter(|| {
            black_box(
                solver
                    .solve(
                        black_box(&ybus),
                        black_box(&p_spec),
                        black_box(&q_spec),
                        black_box(&bus_types),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

/// 构建合成 IEEE-118 节点系统（星形拓扑，bus 1 为中心）
///
/// - Bus 1: Slack (V=1.05)
/// - Bus 2-118: PQ (小负荷 1MW + 0.5MVar)
/// - 拓扑：Bus 1 连接到所有其他 bus（星形），低阻抗 (0.01 + j0.1)
/// - base_mva = 100
fn build_ieee118_system() -> (YBusMatrix, Vec<f64>, Vec<f64>, Vec<BusTypeNR>) {
    let n: usize = 118;
    let base_mva: f64 = 100.0;

    // 构建 bus_map: bus_id (1-based) → index (0-based)
    let bus_map: HashMap<ElementId, usize> = (1..=n)
        .map(|bus_id| (bus_id as ElementId, bus_id - 1))
        .collect();

    // 构建分支：星形拓扑，bus 1 连接到所有其他 bus
    let branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = (2..=n)
        .map(|bus_id| {
            (
                1 as ElementId,
                bus_id as ElementId,
                0.01,  // r (p.u.)
                0.1,   // x (p.u.)
                0.0,   // b (p.u.)
                1.0,   // tap ratio
            )
        })
        .collect();

    let mut ybus = YBusMatrix::from_branches(&branches, &bus_map);
    ybus.set_base_mva(base_mva);

    // P/Q spec: bus 1 = slack (0), bus 2-118 = PQ (-1MW, -0.5MVar → p.u.)
    let p_spec: Vec<f64> = (0..n)
        .map(|i| if i == 0 { 0.0 } else { -1.0 / base_mva })
        .collect();
    let q_spec: Vec<f64> = (0..n)
        .map(|i| if i == 0 { 0.0 } else { -0.5 / base_mva })
        .collect();

    // Bus types: bus 1 = Slack, rest = PQ
    let bus_types: Vec<BusTypeNR> = (0..n)
        .map(|i| if i == 0 { BusTypeNR::Slack } else { BusTypeNR::PQ })
        .collect();

    (ybus, p_spec, q_spec, bus_types)
}

/// 合成 118 节点潮流求解基准测试
///
/// 测量 PowerFlowSolver::solve() 在合成 118 节点系统上的延迟。
/// 118 节点系统模拟中等规模电网，用于评估求解器的可扩展性。
fn bench_powerflow_synthetic_118(c: &mut Criterion) {
    let (ybus, p_spec, q_spec, bus_types) = build_ieee118_system();
    let solver = PowerFlowSolver::default_solver();

    let mut group = c.benchmark_group("powerflow_synthetic_118");
    group.sample_size(50);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("solve_newton_raphson", |b| {
        b.iter(|| {
            black_box(
                solver
                    .solve(
                        black_box(&ybus),
                        black_box(&p_spec),
                        black_box(&q_spec),
                        black_box(&bus_types),
                    )
                    .unwrap(),
            );
        });
    });

    group.finish();
}

criterion_group!(benches, bench_powerflow_ieee14, bench_powerflow_synthetic_118);
criterion_main!(benches);
