//! API 响应基准测试 (T030-06)
//!
//! 测量 API 热点端点的响应延迟（axum oneshot 模式，无需启动真实 HTTP 服务器）：
//! - GET /api/agents：Agent 列表查询
//! - GET /api/topology：拓扑数据查询（IEEE-14 默认数据）
//! - POST /api/actions/structured：结构化动作决策（含决策管线完整路径）
//!
//! 运行：cargo bench -p eneros-benches --bench api_bench

use std::hint::black_box;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use criterion::{Criterion, criterion_group, criterion_main};
use eneros_api::app::{create_router, AppState};
use eneros_constraint::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
use eneros_constraint::ConstraintEngine;
use eneros_core::StructuredAction;
use eneros_gateway::constraint_validator::ConstraintAwareValidator;
use eneros_gateway::{ConstrainedDecisionPipeline, SafetyGateway};
use tower::ServiceExt;

/// 基准测试用网络模拟器：返回无违例结果
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

/// 构建带决策管线的 AppState（用于 POST /api/actions/structured 基准测试）
fn build_app_state_with_pipeline() -> AppState {
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
        gateway,
    ));
    AppState::new().with_decision_pipeline(pipeline)
}

/// GET /api/agents 基准测试
///
/// 测量 Agent 列表查询的完整 API 路径：
/// 路由匹配 → 中间件 → handler → JSON 序列化 → 响应
fn bench_api_get_agents(c: &mut Criterion) {
    let app = create_router(AppState::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("api_get_agents");
    group.sample_size(300);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("get_agents", |b| {
        b.to_async(&rt).iter(|| async {
            let app = black_box(&app);
            let _ = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/agents")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
        });
    });

    group.finish();
}

/// GET /api/topology 基准测试
///
/// 测量拓扑数据查询的完整 API 路径（默认返回 IEEE-14 数据）：
/// 路由匹配 → 中间件 → handler → IEEE-14 数据构建 → JSON 序列化 → 响应
fn bench_api_get_topology(c: &mut Criterion) {
    let app = create_router(AppState::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("api_get_topology");
    group.sample_size(300);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("get_topology", |b| {
        b.to_async(&rt).iter(|| async {
            let app = black_box(&app);
            let _ = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/topology")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
        });
    });

    group.finish();
}

/// POST /api/actions/structured 基准测试
///
/// 测量结构化动作决策的完整 API 路径：
/// 路由匹配 → 中间件 → JSON 反序列化 → 决策管线 → JSON 序列化 → 响应
fn bench_api_post_structured_action(c: &mut Criterion) {
    let app = create_router(build_app_state_with_pipeline());
    let body = r#"{"action":{"StartGenerator":{"gen_id":1,"target_mw":100.0}},"authority":"Supervisor","system_state":"Normal"}"#;
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("api_post_structured_action");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("post_structured_action", |b| {
        b.to_async(&rt).iter(|| async {
            let app = black_box(&app);
            let _ = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/actions/structured")
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await;
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_api_get_agents,
    bench_api_get_topology,
    bench_api_post_structured_action,
);
criterion_main!(benches);
