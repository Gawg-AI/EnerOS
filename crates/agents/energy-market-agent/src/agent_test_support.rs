//! 测试/仿真支撑（蓝图 40：测试构造放 test-support；`#[cfg(any(test, feature = "mock"))]`
//! 门控——release resolved graph 不可达）。
//!
//! - [`RejectingTelemetryProvider`]：无可信遥测（评估器拒绝路径）。
//! - [`RecordingDispatch`]：记录提交的测试 ControlDispatch（可注入结果）。

#![cfg(any(test, feature = "mock"))]

use alloc::boxed::Box;
use alloc::string::String;

use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DispatchError, DispatchReceipt,
};
use eneros_safety_validator::{TrustRejection, TrustedTelemetry};

use crate::energy_agent::TrustedTelemetryProvider;

/// 无可信遥测（NoSamples——观测缺失路径）。
pub struct RejectingTelemetryProvider;

impl TrustedTelemetryProvider for RejectingTelemetryProvider {
    fn trusted_telemetry(&mut self, _now_ms: u64) -> Result<TrustedTelemetry, TrustRejection> {
        Err(TrustRejection::NoSamples)
    }
}

/// 固定受信任遥测提供者（经真实评估器构造——不手工捏造 TrustedTelemetry）。
///
/// 每次调用按递增序号/时间戳重建样本（评估器拒绝重放——连续 tick 必须
/// 新鲜样本；`None` = 评估器拒绝路径）。
pub struct StubTelemetryProvider {
    evaluator: eneros_safety_validator::TelemetryTrustEvaluator,
    /// None = 评估器拒绝路径（观测缺失）；Some = 基准值（point, value）。
    base: Option<alloc::vec::Vec<(u32, f64)>>,
    /// 下一序号。
    next_seq: u64,
}

impl StubTelemetryProvider {
    /// 构造（base = None → 评估器拒绝）。
    pub fn new(base: Option<alloc::vec::Vec<(u32, f64)>>) -> Self {
        use eneros_safety_validator::{
            EvaluatorConfig, RequiredPoint, RequiredPointCoverage, StateBindings,
        };
        Self {
            evaluator: eneros_safety_validator::TelemetryTrustEvaluator::new(EvaluatorConfig {
                coverage: RequiredPointCoverage {
                    points: alloc::vec![
                        RequiredPoint {
                            point: 1,
                            min: 0.0,
                            max: 1000.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Volt
                        },
                        RequiredPoint {
                            point: 2,
                            min: -1000.0,
                            max: 1000.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Ampere
                        },
                        RequiredPoint {
                            point: 3,
                            min: 45.0,
                            max: 55.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Hertz
                        },
                        RequiredPoint {
                            point: 4,
                            min: 0.0,
                            max: 1.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Ratio
                        },
                        RequiredPoint {
                            point: 5,
                            min: 0.0,
                            max: 1.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Other
                        },
                        RequiredPoint {
                            point: 6,
                            min: 0.0,
                            max: 1.0,
                            max_age_ms: 5000,
                            unit: eneros_safety_validator::PointUnit::Other
                        },
                    ],
                },
                bindings: StateBindings {
                    voltage: 1,
                    current: 2,
                    frequency: 3,
                    soc: 4,
                },
                mapping_version: 1,
                max_skew_ms: 1000,
                allowed_sources: alloc::vec![1],
                require_integrity: true,
                control_horizon_ms: 30_000,
            }),
            base,
            next_seq: 0,
        }
    }

    /// 全覆盖基准值（电压/电流/频率/SOC/过流/过温）。
    pub fn full_base() -> alloc::vec::Vec<(u32, f64)> {
        alloc::vec![
            (1, 380.0),
            (2, 10.0),
            (3, 50.0),
            (4, 0.6),
            (5, 0.0),
            (6, 0.0),
        ]
    }
}

impl TrustedTelemetryProvider for StubTelemetryProvider {
    fn trusted_telemetry(&mut self, now_ms: u64) -> Result<TrustedTelemetry, TrustRejection> {
        let Some(base) = &self.base else {
            return Err(TrustRejection::NoSamples);
        };
        self.next_seq += 1;
        let seq = self.next_seq;
        let samples: alloc::vec::Vec<eneros_safety_validator::TelemetrySample> = base
            .iter()
            .map(|(point, value)| {
                use eneros_safety_validator::{IntegrityStatus, SampleQuality};
                eneros_safety_validator::TelemetrySample {
                    point: *point,
                    value: *value,
                    source: 1,
                    source_epoch: 0,
                    measured_at_ms: now_ms.saturating_sub(50),
                    received_at_ms: now_ms,
                    quality: SampleQuality::Good,
                    sequence: seq,
                    integrity: IntegrityStatus::Verified,
                    mapping_version: 1,
                }
            })
            .collect();
        self.evaluator.evaluate(&samples, now_ms)
    }
}

/// 记录提交的测试 dispatch（`result`/`result_fn` 都未配置 → `Internal` 兜底）。
#[derive(Default)]
pub struct RecordingDispatch {
    /// 注入的固定返回结果（None = 未配置 → Internal 错误）。
    pub result: Option<Result<DispatchReceipt, DispatchError>>,
    /// 注入的动态返回函数（优先于 `result`；可用于 flaky 序列）。
    pub result_fn: Option<Box<dyn FnMut() -> Result<DispatchReceipt, DispatchError>>>,
    /// 收到的提交（proposal/telemetry/actor/deadline 轨迹）。
    pub submitted: alloc::vec::Vec<(
        eneros_dual_brain::ProposedControlPlan,
        TrustedTelemetry,
        AuthenticatedActor,
        u64,
    )>,
}

impl RecordingDispatch {
    /// 提交计数。
    pub fn submit_count(&self) -> usize {
        self.submitted.len()
    }
}

impl ControlDispatch for RecordingDispatch {
    fn submit(
        &mut self,
        proposal: &eneros_dual_brain::ProposedControlPlan,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        self.submitted
            .push((proposal.clone(), telemetry.clone(), *actor, deadline_ms));
        if let Some(f) = self.result_fn.as_mut() {
            return f();
        }
        match &self.result {
            Some(r) => r.clone(),
            None => Err(DispatchError::Internal(String::from(
                "unconfigured test dispatch",
            ))),
        }
    }

    /// 蓝图 61 v0.314.29：typed 提交（测试 double 记录轨迹——市场链路
    /// 不消费 typed 路径）。
    fn submit_typed(
        &mut self,
        plan: &eneros_decision_contract::CandidatePlan,
        target_device: &str,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        let _ = (plan, target_device, telemetry, actor, deadline_ms);
        if let Some(f) = self.result_fn.as_mut() {
            return f();
        }
        match &self.result {
            Some(r) => r.clone(),
            None => Err(DispatchError::Internal(String::from(
                "unconfigured test dispatch",
            ))),
        }
    }

    /// 47（WP-47.1）：协议写意图提交。测试 double 记录轨迹并返回
    /// 配置结果（与 [`submit`](Self::submit) 同语义——市场链路不消费
    /// 意图路径）。
    fn submit_intent(
        &mut self,
        intent: &eneros_protocol_abstract::write::PointWriteIntent,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
        deadline_ms: u64,
    ) -> Result<DispatchReceipt, DispatchError> {
        let _ = (intent, telemetry, actor, deadline_ms);
        self.submitted.push((
            // 意图路径无 proposal——以最小占位记录（测试 double 不消费）。
            eneros_dual_brain::ProposedControlPlan {
                plan_id: alloc::string::String::from("intent"),
                path: eneros_dual_brain::DecisionPath::Fast,
                schedule: eneros_energy_lp_model::result::ScheduleResult {
                    schedule: alloc::vec![],
                    total_revenue_yuan: 0.0,
                    objective_value: 0.0,
                    solve_status: eneros_solver_core::result::SolveStatus::Optimal,
                },
                power_kw: 0.0,
                target_device: alloc::string::String::from("intent"),
                ttl_secs: 60,
                issued_at_ms: 0,
                input_digest: None,
                model_digest: None,
                latency: Default::default(),
                safety_violations: 0,
                safety_clamped: false,
                feedback: None,
            },
            telemetry.clone(),
            *actor,
            deadline_ms,
        ));
        if let Some(f) = self.result_fn.as_mut() {
            return f();
        }
        match &self.result {
            Some(r) => r.clone(),
            None => Err(DispatchError::Internal(String::from(
                "unconfigured test dispatch",
            ))),
        }
    }
}
