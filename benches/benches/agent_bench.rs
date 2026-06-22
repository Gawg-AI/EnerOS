//! Agent 决策基准测试 (T030-06)
//!
//! 测量 Agent 决策热点路径的分阶段延迟：
//! - perception：事件接收（EventBus::publish 广播延迟）
//! - decision：决策生成（ConstrainedDecisionPipeline::decide_enhanced）
//! - execution：命令执行（ActionDispatcher::dispatch_structured）
//!
//! 运行：cargo bench -p eneros-benches --bench agent_bench

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use eneros_agent::ActionDispatcher;
use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_constraint::ConstraintEngine;
use eneros_core::event::{Event, EventPayload, EventType};
use eneros_core::{AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState};
use eneros_eventbus::EventBus;
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::{ConstrainedDecisionPipeline, DecisionContext, SafetyGateway};

/// 基准测试用网络模拟器：返回无违例结果，聚焦决策管线逻辑性能
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

/// 构建决策管线（真实业务逻辑，mock 网络模拟器）
fn build_decision_pipeline() -> Arc<ConstrainedDecisionPipeline> {
    let projector = Arc::new(FeasibilityProjector::new(Arc::new(BenchSimulator)));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::new(100));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine,
        gateway.clone(),
    ));
    Arc::new(ConstrainedDecisionPipeline::new(
        projector,
        validator,
        gateway,
    ))
}

/// 构建 ActionDispatcher（真实业务逻辑）
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

/// perception 基准测试：事件接收延迟
///
/// 测量 EventBus::publish() 的延迟——事件从发布到广播通道的路径。
/// 这是 Agent 感知路径的第一步：上游事件 → EventBus → Agent 上下文更新。
fn bench_agent_perception(c: &mut Criterion) {
    let event_bus = Arc::new(EventBus::new(64));
    // 订阅以模拟真实接收方（broadcast channel 在有订阅者时才有意义）
    let _receiver = event_bus.subscribe();
    let event = Event::new(
        EventType::DataReceived,
        "scada-collector",
        EventPayload::PowerFlowResult {
            converged: true,
            iterations: 4,
            total_losses: 1.23,
        },
    );

    let mut group = c.benchmark_group("agent_perception");
    group.sample_size(500);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("event_publish", |b| {
        b.iter(|| {
            // publish 可能返回 Err（无订阅者），此处有订阅者故应成功
            let _ = black_box(event_bus.publish(black_box(event.clone())));
        });
    });

    group.finish();
}

/// decision 基准测试：决策生成延迟
///
/// 测量 ConstrainedDecisionPipeline::decide_enhanced() 的延迟——
/// 从上下文（authority + jurisdiction + system_state）到决策结果生成的完整路径：
/// precondition → projection → validation → decomposition → postcondition
fn bench_agent_decision(c: &mut Criterion) {
    let pipeline = build_decision_pipeline();
    let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Normal,
    );
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("agent_decision");
    // 决策管线涉及多次 await 与安全网关检查，单次延迟较高，故降低 sample_size
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("decide_enhanced", |b| {
        b.to_async(&rt).iter(|| async {
            black_box(pipeline.decide_enhanced(black_box(&action), black_box(&ctx)).await);
        });
    });

    group.finish();
}

/// execution 基准测试：命令执行延迟
///
/// 测量 ActionDispatcher::dispatch_structured() 的延迟——
/// 从 StructuredAction 到 DispatchResult 的完整执行路径：
/// 决策管线 → 命令构建 → 安全网关 → 设备执行（mock）→ 结果返回
fn bench_agent_execution(c: &mut Criterion) {
    let dispatcher = build_dispatcher();
    let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("agent_execution");
    // 执行路径跨越决策→构建→网关→设备 mock 多次 await，单次延迟较高，故降低 sample_size
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("dispatch_structured", |b| {
        b.to_async(&rt).iter(|| async {
            black_box(
                dispatcher
                    .dispatch_structured(
                        black_box(&action),
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

criterion_group!(benches, bench_agent_perception, bench_agent_decision, bench_agent_execution);
criterion_main!(benches);
