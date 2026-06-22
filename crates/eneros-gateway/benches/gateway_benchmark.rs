//! 命令下发热点路径基准测试 (T029-12)
//!
//! 测量真实业务路径：决策结果 → 命令构建 → 安全检查 → 执行
//! 使用真实 SafetyGateway + 真实命令执行路径，mock 设备 I/O。
//!
//! T029-15：新增决策缓存基准测试，对比有缓存 vs 无缓存的决策延迟。
//!
//! 运行：cargo bench -p eneros-gateway

use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use eneros_core::Result as CoreResult;
use eneros_core::{
    AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState,
};
use eneros_constraint::ConstraintEngine;
use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_gateway::{
    Command, CommandPriority, CommandType, ConstrainedDecisionPipeline,
    DecisionCache, DecisionContext, DeviceValue,
    ExecutionResult, SafetyGateway,
};
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use async_trait::async_trait;
use eneros_gateway::executor::CommandExecutor;

/// 快速命令执行器（真实 CommandExecutor trait 实现）
///
/// 模拟设备 I/O 的最小延迟，使基准测试聚焦于网关逻辑而非设备 I/O。
/// LoggingExecutor 是生产代码中使用的默认执行器，但包含 tracing 日志开销。
/// 本执行器去除日志开销，测量纯网关逻辑性能。
struct FastExecutor;

#[async_trait]
impl CommandExecutor for FastExecutor {
    async fn execute(&self, _command: &Command) -> CoreResult<ExecutionResult> {
        Ok(ExecutionResult::ok(
            "fast-executor".to_string(),
            Duration::from_micros(1),
        ))
    }

    async fn read_back(&self, _command: &Command) -> Option<eneros_device::adapter::DataValue> {
        None
    }
}

/// 构建基准测试用的安全网关（真实业务逻辑）
fn build_gateway() -> Arc<SafetyGateway> {
    let executor = Arc::new(FastExecutor);
    Arc::new(SafetyGateway::with_executor(100, executor))
}

/// 构建测试命令
fn make_command(cmd_type: CommandType, target_id: u64, priority: CommandPriority) -> Command {
    let mut cmd = Command::new(cmd_type, target_id, priority, "benchmark");
    cmd.device_id = Some(format!("device-{}", target_id));
    cmd.device_address = Some(format!("point-{}", target_id));
    cmd.device_value = Some(DeviceValue::Float64(50.0));
    cmd
}

/// 命令下发完整路径基准测试 — 发电机设定值
///
/// 测量 SafetyGateway::execute_command() 的完整路径：
/// 1. 获取设备锁（per-device async mutex）
/// 2. 安全验证（validate_command）
/// 3. 命令执行（CommandExecutor::execute）
/// 4. 存储执行结果
/// 5. 记录命令历史
fn bench_gateway_command_generator(c: &mut Criterion) {
    let gateway = build_gateway();
    let cmd = make_command(CommandType::GeneratorSetpoint, 1, CommandPriority::Normal);
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("gateway_command");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("execute_generator_setpoint", |b| {
        b.to_async(&rt).iter(|| async {
            // 克隆命令以避免所有权转移
            gateway.execute_command(cmd.clone()).await.unwrap();
        });
    });

    group.finish();
}

/// 命令下发路径 — 开关操作（高优先级）
fn bench_gateway_command_switch(c: &mut Criterion) {
    let gateway = build_gateway();
    let cmd = make_command(CommandType::SwitchToggle, 42, CommandPriority::High);
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("gateway_command");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("execute_switch_toggle", |b| {
        b.to_async(&rt).iter(|| async {
            gateway.execute_command(cmd.clone()).await.unwrap();
        });
    });

    group.finish();
}

/// 命令下发路径 — 负荷削减（紧急优先级）
fn bench_gateway_command_load_shed(c: &mut Criterion) {
    let gateway = build_gateway();
    let cmd = make_command(CommandType::LoadShedding, 5, CommandPriority::Critical);
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("gateway_command");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("execute_load_shedding", |b| {
        b.to_async(&rt).iter(|| async {
            gateway.execute_command(cmd.clone()).await.unwrap();
        });
    });

    group.finish();
}

/// 命令下发路径 — 使用默认 LoggingExecutor（生产默认配置）
///
/// 对比 FastExecutor，测量 tracing 日志开销对性能的影响
fn bench_gateway_command_logging_executor(c: &mut Criterion) {
    let gateway = Arc::new(SafetyGateway::new(100));
    let cmd = make_command(CommandType::GeneratorSetpoint, 1, CommandPriority::Normal);
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("gateway_command");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("execute_with_logging_executor", |b| {
        b.to_async(&rt).iter(|| async {
            gateway.execute_command(cmd.clone()).await.unwrap();
        });
    });

    group.finish();
}

// ── 决策缓存基准测试（T029-15）──

/// 基准测试用网络模拟器：返回无违例结果，聚焦管线逻辑性能
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
        vec![(1, 0.0, 200.0)]
    }
    fn current_voltages(&self) -> Vec<(u64, f64)> {
        vec![(1, 1.02)]
    }
}

/// 构建决策管线（无缓存）
fn build_pipeline_uncached() -> ConstrainedDecisionPipeline {
    let projector = Arc::new(FeasibilityProjector::new(Arc::new(BenchSimulator)));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::with_executor(100, Arc::new(FastExecutor)));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine, gateway.clone(),
    ));
    ConstrainedDecisionPipeline::new(projector, validator, gateway)
}

/// 构建决策管线（有缓存）
fn build_pipeline_cached() -> (ConstrainedDecisionPipeline, Arc<DecisionCache>) {
    let cache = Arc::new(DecisionCache::new(256, Duration::from_secs(60)));
    let projector = Arc::new(FeasibilityProjector::new(Arc::new(BenchSimulator)));
    let constraint_engine = Arc::new(ConstraintEngine::new());
    let gateway = Arc::new(SafetyGateway::with_executor(100, Arc::new(FastExecutor)));
    let validator = Arc::new(ConstraintAwareValidator::with_default_interlocking(
        constraint_engine, gateway.clone(),
    ));
    let pipeline = ConstrainedDecisionPipeline::new(projector, validator, gateway)
        .with_cache(cache.clone());
    (pipeline, cache)
}

/// 无缓存决策管线基准测试 — 每次调用执行完整管线
///
/// 测量 ConstrainedDecisionPipeline::decide_enhanced() 的完整路径：
/// precondition → projection → validation → decomposition → execution → postcondition
fn bench_decision_pipeline_uncached(c: &mut Criterion) {
    let pipeline = build_pipeline_uncached();
    let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Normal,
    );
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("decision_pipeline");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("decide_enhanced_uncached", |b| {
        b.to_async(&rt).iter(|| async {
            pipeline.decide_enhanced(&action, &ctx).await
        });
    });

    group.finish();
}

/// 有缓存决策管线基准测试 — 首次调用未命中，后续调用命中缓存
///
/// 对比无缓存基准，验证缓存命中显著降低决策延迟（目标 > 30%）
fn bench_decision_pipeline_cached_hit(c: &mut Criterion) {
    let (pipeline, _cache) = build_pipeline_cached();
    let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
    let ctx = DecisionContext::new(
        AuthorityLevel::Supervisor,
        Jurisdiction::unrestricted(),
        SystemOperatingState::Normal,
    );
    let rt = tokio::runtime::Runtime::new().unwrap();

    // 预热缓存：iter 闭包外执行一次 decide_enhanced，确保后续 iter 调用均为缓存命中
    // 否则首次 iter 调用将为 miss，导致统计偏差
    let _ = rt.block_on(pipeline.decide_enhanced(&action, &ctx));

    let mut group = c.benchmark_group("decision_pipeline");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("decide_enhanced_cached_hit", |b| {
        b.to_async(&rt).iter(|| async {
            pipeline.decide_enhanced(&action, &ctx).await
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_gateway_command_generator,
    bench_gateway_command_switch,
    bench_gateway_command_load_shed,
    bench_gateway_command_logging_executor,
    bench_decision_pipeline_uncached,
    bench_decision_pipeline_cached_hit,
);
criterion_main!(benches);
