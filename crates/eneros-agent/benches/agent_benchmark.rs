//! Agent 决策热点路径基准测试 (T029-12)
//!
//! 测量真实业务路径：事件接收 → 上下文构建 → 决策执行 → 结果返回
//! 使用真实 ConstrainedDecisionPipeline（eneros-gateway）+
//! 真实 ActionDispatcher（eneros-agent），mock 输入数据。
//!
//! 运行：cargo bench -p eneros-agent

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use eneros_agent::ActionDispatcher;
use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_constraint::ConstraintEngine;
use eneros_core::{
    AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState,
};
use eneros_eventbus::EventBus;
use eneros_gateway::{
    ConstrainedDecisionPipeline, SafetyGateway,
    constraint_validator::ConstraintAwareValidator,
};

/// 基准测试用的网络模拟器（真实 NetworkSimulator trait 实现）
///
/// 返回可行的 What-If 结果，使决策管线完整执行所有阶段
struct BenchSimulator;

impl NetworkSimulator for BenchSimulator {
    fn simulate_action(&self, _action: &StructuredAction) -> WhatIfResult {
        WhatIfResult {
            applicable: true,
            converged: true,
            voltage_violations: vec![],
            thermal_violations: vec![],
            all_constraints_satisfied: true,
            summary: "OK".to_string(),
        }
    }

    fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
        vec![(1, 0.0, 200.0), (2, 0.0, 150.0), (3, 0.0, 100.0)]
    }

    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02), (2, 1.01), (3, 0.99)]
    }
}

/// 构建基准测试用的决策管线（真实业务逻辑）
fn build_dispatcher() -> ActionDispatcher {
    let projector = Arc::new(FeasibilityProjector::new(Arc::new(BenchSimulator)));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        gateway.clone(),
    ));

    let pipeline = Arc::new(ConstrainedDecisionPipeline::new(
        projector,
        validator,
        gateway.clone(),
    ));

    let event_bus = Arc::new(EventBus::new(64));
    ActionDispatcher::new_local(event_bus, gateway).with_pipeline(pipeline)
}

/// Agent 决策完整路径基准测试
///
/// 测量 ActionDispatcher::dispatch_structured() 的完整路径：
/// 1. 事件接收（StructuredAction 作为输入）
/// 2. 上下文构建（AuthorityLevel, Jurisdiction, SystemOperatingState）
/// 3. 决策执行（ConstrainedDecisionPipeline::decide()）
///    - 前置条件检查
///    - 可行性投影
///    - 约束验证（6 步管线）
///    - 动作分解
///    - 命令执行
///    - 后置条件验证
/// 4. 结果返回（DispatchResult）
fn bench_agent_decision_generator(c: &mut Criterion) {
    let dispatcher = build_dispatcher();
    let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("agent_decision");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("dispatch_structured_start_generator", |b| {
        b.to_async(&rt).iter(|| async {
            // 使用 black_box 防止编译器优化掉计算，避免 assert! 导致基准测试崩溃
            std::hint::black_box(
                dispatcher
                    .dispatch_structured(
                        &action,
                        AuthorityLevel::Supervisor,
                        &Jurisdiction::unrestricted(),
                        SystemOperatingState::Normal,
                    )
                    .await
                    .unwrap(),
            );
        });
    });

    group.finish();
}

/// Agent 决策路径 — 负荷削减（高风险动作）
fn bench_agent_decision_load_shed(c: &mut Criterion) {
    let dispatcher = build_dispatcher();
    let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 30.0 };
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("agent_decision");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("dispatch_structured_shed_load", |b| {
        b.to_async(&rt).iter(|| async {
            // 使用 black_box 防止编译器优化掉计算
            std::hint::black_box(
                dispatcher
                    .dispatch_structured(
                        &action,
                        AuthorityLevel::Supervisor,
                        &Jurisdiction::unrestricted(),
                        SystemOperatingState::Normal,
                    )
                    .await
                    .unwrap(),
            );
        });
    });

    group.finish();
}

/// Agent 决策路径 — 故障隔离（多步分解动作）
fn bench_agent_decision_isolate_fault(c: &mut Criterion) {
    let dispatcher = build_dispatcher();
    let action = StructuredAction::IsolateFault {
        upstream_switch: 10,
        downstream_switch: 20,
    };
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("agent_decision");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("dispatch_structured_isolate_fault", |b| {
        b.to_async(&rt).iter(|| async {
            // 使用 black_box 防止编译器优化掉计算
            std::hint::black_box(
                dispatcher
                    .dispatch_structured(
                        &action,
                        AuthorityLevel::Emergency,
                        &Jurisdiction::unrestricted(),
                        SystemOperatingState::Emergency,
                    )
                    .await
                    .unwrap(),
            );
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_agent_decision_generator,
    bench_agent_decision_load_shed,
    bench_agent_decision_isolate_fault,
);
criterion_main!(benches);
