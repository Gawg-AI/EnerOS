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

/// 提交轨迹最小事实（WP-75.2.4 记录面真化：原 typed 臂整体丢弃、
/// intent 臂伪造占位 ProposedControlPlan——「记录轨迹」宣称失实；现记
/// 最小摘要，避免整计划克隆）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionTrace {
    /// typed 提交（submit_typed/submit_trusted 同记录）。
    Typed {
        /// 候选计划 id。
        plan_id: eneros_decision_contract::PlanId,
        /// 提交方身份。
        actor: AuthenticatedActor,
        /// 遥测来源 epoch。
        telemetry_epoch: u64,
        /// 遥测评估时刻（同 epoch 内的新鲜度序列事实）。
        telemetry_generated_at_ms: u64,
    },
    /// 意图提交（submit_intent）：协议 + 单调序号级最小事实。
    Intent {
        /// 来源协议 id。
        protocol: String,
        /// 意图单调序号。
        sequence: u64,
        /// 提交方身份。
        actor: AuthenticatedActor,
    },
}

/// 记录提交的测试 dispatch（`result`/`result_fn` 都未配置 → `Internal` 兜底）。
#[derive(Default)]
pub struct RecordingDispatch {
    /// 注入的固定返回结果（None = 未配置 → Internal 错误）。
    pub result: Option<Result<DispatchReceipt, DispatchError>>,
    /// 注入的动态返回函数（优先于 `result`；可用于 flaky 序列）。
    pub result_fn: Option<Box<dyn FnMut() -> Result<DispatchReceipt, DispatchError>>>,
    /// 收到的提交（最小事实轨迹；WP-75.2.4 起真记录——消费断言
    /// `recording_face_tests::submissions_recorded_as_minimal_facts`）。
    pub submitted: alloc::vec::Vec<SubmissionTrace>,
}

impl RecordingDispatch {
    /// 提交计数。
    pub fn submit_count(&self) -> usize {
        self.submitted.len()
    }
}

impl ControlDispatch for RecordingDispatch {
    // （2026-09-24 C2 收口退役）原 `fn submit(&ProposedControlPlan, …)`
    // 臂随 trait facade 入口删除；市场链路经 `submit_typed` 记账。

    /// 蓝图 61 v0.314.29：typed 提交（WP-75.2.4：测试 double 真记录
    /// 最小事实轨迹——原实现整体丢弃）。
    fn submit_typed(
        &mut self,
        plan: &eneros_decision_contract::CandidatePlan,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
    ) -> Result<DispatchReceipt, DispatchError> {
        self.submitted.push(SubmissionTrace::Typed {
            plan_id: *plan.id(),
            actor: *actor,
            telemetry_epoch: telemetry.source_epoch,
            telemetry_generated_at_ms: telemetry.generated_at_ms,
        });
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

    /// 蓝图 70.8 WP-70.8.4：带可信输入身份的 typed 提交。测试 double
    /// 与 [`submit_typed`](Self::submit_typed) 同语义（市场链路不消费
    /// 该路径——可信输入准入由引擎实现执法）。
    fn submit_trusted(
        &mut self,
        plan: &eneros_decision_contract::CandidatePlan,
        trusted: &eneros_agent_sdk::snap_input_wire::TrustedInput,
        telemetry: &TrustedTelemetry,
        actor: &AuthenticatedActor,
    ) -> Result<DispatchReceipt, DispatchError> {
        let _ = (trusted,);
        self.submit_typed(plan, telemetry, actor)
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
        let _ = (telemetry, deadline_ms);
        // WP-75.2.4：协议 + 单调序号级最小事实（伪造占位 ProposedControlPlan
        // 已删——「记录轨迹」宣称变真）。
        self.submitted.push(SubmissionTrace::Intent {
            protocol: intent.protocol_id.clone(),
            sequence: intent.sequence,
            actor: *actor,
        });
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

#[cfg(test)]
mod recording_face_tests {
    use super::*;
    use eneros_decision_contract::{
        Assignment, CandidatePlan, CandidatePlanParts, PlanId, SnapshotId, SolveStatusKind,
    };

    /// WP-75.2.4 记录面真化：typed/intent 提交逐次留痕（原 typed 臂整体
    /// 丢弃、intent 臂伪造占位 ProposedControlPlan——submitted 为陈旧
    /// 形态死字段，doc「记录轨迹」宣称失实）。轨迹 = 最小事实摘要
    /// （plan_id/actor/遥测身份摘要；协议+序号），submit_count 随之有牙。
    #[test]
    fn submissions_recorded_as_minimal_facts() {
        let mut d = RecordingDispatch::default();
        // 结果配置无关记录面（记录发生在提交时，成败均留痕）。
        d.result = Some(Err(DispatchError::Internal(String::from("unused"))));
        let mut stub = StubTelemetryProvider::new(Some(StubTelemetryProvider::full_base()));
        let tel = stub.trusted_telemetry(2000).expect("受信任遥测");
        let actor = AuthenticatedActor {
            kind: eneros_control_gateway::envelope::ActorKind::Ai,
            service: [0u8; 16],
            session_epoch: 0,
        };
        let plan = CandidatePlan::from_parts(CandidatePlanParts {
            id: PlanId([0x62; 16]),
            snapshot_id: SnapshotId([0x53; 16]),
            objective_digest: [0x63; 32],
            resource_catalog_version: 1,
            horizon_start_ms: 2000,
            step_ns: 900_000_000,
            step_count: 1,
            solver_status: SolveStatusKind::Optimal,
            objective_value: 1.0,
            relative_gap: 0.0,
            max_primal_residual: 0.0,
            elapsed_ms: 1,
            model_digest: [0x64; 32],
            input_digest: [0x65; 32],
            assignments: alloc::vec![Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: 5.0,
                unit_code: 6,
                step_index: 1,
            }],
            issued_at_ms: 2000,
            ttl_ms: 60_000,
        })
        .expect("合法候选计划");
        let _ = d.submit_typed(&plan, &tel, &actor);
        assert_eq!(d.submit_count(), 1, "typed 提交必须留痕（原实现丢弃）");
        match &d.submitted[0] {
            SubmissionTrace::Typed {
                plan_id,
                actor: a,
                telemetry_epoch,
                telemetry_generated_at_ms,
            } => {
                assert_eq!(*plan_id, *plan.id());
                assert_eq!(*a, actor);
                assert_eq!(*telemetry_epoch, tel.source_epoch);
                assert_eq!(*telemetry_generated_at_ms, tel.generated_at_ms);
            }
            other => panic!("typed 轨迹形态不符: {other:?}"),
        }
        // intent 路径：(protocol, sequence) 级最小事实（伪造占位已删）。
        let intent = eneros_protocol_abstract::write::PointWriteIntent::new(
            "mqtt",
            1u16,
            10u32,
            eneros_upa_model::PointValue::Float(5.0),
            7,
            1000,
        );
        let _ = d.submit_intent(&intent, &tel, &actor, 60_000);
        assert_eq!(d.submit_count(), 2, "intent 提交必须留痕");
        match &d.submitted[1] {
            SubmissionTrace::Intent {
                protocol,
                sequence,
                actor: a,
            } => {
                assert_eq!(protocol.as_str(), "mqtt");
                assert_eq!(*sequence, 7);
                assert_eq!(*a, actor);
            }
            other => panic!("intent 轨迹形态不符: {other:?}"),
        }
    }
}
