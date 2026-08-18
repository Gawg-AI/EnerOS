//! 能源调度 Agent — 编排双脑协调器执行储能调度（D9/D10/D11/D14）.
//!
//! 蓝图 40 WP-40.1/40.3/40.12 接线：
//!
//! - **输入**：`TrustedTelemetryProvider`（svc-telemetry 实现）提供受信任
//!   遥测（`TrustedTelemetry`）；无采集/过期/坏质量 → 结构化拒绝 →
//!   明确不动作（零推理、零新写，不制造"健康默认状态"）；
//! - **决策**：`DualBrainCoordinator::execute` 只产
//!   `DecisionOutcome`（Proposal/NoAction/Rejected）——任何路径不自行写设备；
//! - **执行**：`Proposal` 提交给唯一 `ControlDispatch`（WP-40.6）；
//!   `DispatchReceipt.final_state == Committed` 是唯一成功判据；
//! - **学习**：learning 只消费 `Committed`（WP-40.12）——Rejected/Failed/
//!   Indeterminate 进入失败数据但不更新成功奖励；被拒/失败不消耗成功预算。

use alloc::boxed::Box;
use alloc::format;

use eneros_agent::{AgentDescriptor, AgentState, AgentType};
use eneros_control_dispatch::{
    AuthenticatedActor, ControlDispatch, DispatchError, DispatchReceipt,
};
use eneros_control_gateway::envelope::ActorKind;
use eneros_dual_brain::decision::{DecisionOutcome, NoActionReason};
use eneros_dual_brain::DualBrainCoordinator;
use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::result::ScheduleResult;
use eneros_fast_path::state::RealtimeState;
use eneros_intent_contract::contract::FeedbackContract;
use eneros_llm_engine::engine::LlmEngine;
use eneros_safety_validator::{TrustRejection, TrustedTelemetry};
#[cfg(any(test, feature = "mock"))]
use eneros_solver_core::mock::MockSolver;
use eneros_solver_core::solver::Solver;

use crate::error::AgentRuntimeError;
use crate::market::MarketChannel;
use crate::runtime::{AgentRuntime, HeartbeatStatus};

/// 受信任遥测提供者（蓝图 40 WP-40.2 的消费者 seam——svc-telemetry 实现；
/// 输出只能是 `TrustedTelemetry` 或结构化拒绝）。
pub trait TrustedTelemetryProvider {
    /// 获取当前受信任遥测；拒绝 = 缺采集/过期/坏质量/重放等（零推理）。
    fn trusted_telemetry(&mut self, now_ms: u64) -> Result<TrustedTelemetry, TrustRejection>;
}

/// 能源调度 Agent.
///
/// 持有 `DualBrainCoordinator<S>`（泛型求解器，构造器注入，P0-27）。
/// 通过 `market_channel` 接收 Market Agent 转发的市场数据。
pub struct EnergyAgent<S: Solver> {
    /// Agent 描述符.
    pub descriptor: AgentDescriptor,
    /// 双脑协调器（泛型求解器）.
    pub coordinator: DualBrainCoordinator<S>,
    /// 市场数据通道（接收 Market Agent 转发的数据）.
    pub market_channel: MarketChannel,
    /// 当前调度方案（双脑执行结果）.
    pub current_schedule: Option<ScheduleResult>,
    /// 当前电价缓存（从市场数据更新）.
    pub current_price: f64,
    /// 生命周期状态.
    pub state: AgentState,
    /// 32 AG-01：连续错误计数（Transient 错误 N 次连续才 Error——
    /// 旧实现任何 Err 即永久 Error，限流常态导致一次拒绝即离线）。
    pub consecutive_errors: u32,
    /// tick 计数.
    pub tick_count: u64,
    // --- 蓝图 40 WP-40.1/40.2：可信采集边界 ---
    /// 受信任遥测提供者（svc-telemetry 实现；None = 无采集源 → 不动作）。
    pub telemetry: Box<dyn TrustedTelemetryProvider>,
    /// 最近一次可信评估拒绝（观测缺失/坏质量等；诊断/审计）。
    pub last_trust_rejection: Option<TrustRejection>,
    /// 最近一次执行的反馈契约（仅 Committed 后更新；不再丢弃）。
    pub last_feedback: Option<FeedbackContract>,
    /// 观测缺失计数（NoTrustedInput 路径指标）。
    pub outcome_unknown_count: u64,
    /// 预测−观测偏差累计（指标 `decision.forecast_error` 的源数据；
    /// 仅 Committed 后累计——失败动作不进成功样本）。
    pub forecast_error_sum: f64,
    /// 偏差样本数。
    pub forecast_error_count: u64,
    /// 上一轮观测时刻（毫秒；时窗 = 本轮观测 − 上轮观测）。
    pub last_observed_at_ms: u64,
    // --- 蓝图 40 WP-40.6：唯一 ControlDispatch ---
    /// 唯一控制写出口（Proposal → validate/authorize/persist/deliver/
    /// ACK/readback/audit）。
    pub dispatch: Box<dyn ControlDispatch>,
    /// 最近一次投递回执（诊断/审计）。
    pub last_dispatch: Option<DispatchReceipt>,
    /// 最近一次投递错误。
    pub last_dispatch_error: Option<DispatchError>,
    // --- 22 §2.W2.4：设备拒绝/异常处理 ---
    /// 拒绝处理器（分类计数 + 重试预算，防无限重试）。
    pub rejections: eneros_dual_brain::rejection::RejectionHandler,
    /// 最近一次拒绝（分类 + 策略；None = 无拒绝）。
    pub last_rejection: Option<(
        eneros_dual_brain::rejection::RejectionKind,
        eneros_dual_brain::rejection::RejectionAction,
    )>,
    // --- 22 §2.W2.5：轻量在线学习（锁定 8，安全闸后生效；WP-40.12 只消费 Committed）---
    /// deadband 自适应学习器（学习输出过 safety-validator）。
    pub deadband_learner: eneros_dual_brain::learning::DeadbandAdaptive,
    /// 31-review（P1 AG-02②）：deadband 学习器输出接入生产消费。
    pub deadband_kw: f64,
    /// LP 预测修正器（偏差 EMA，钳位安全界）。
    pub forecast_corrector: eneros_dual_brain::learning::ForecastCorrector,
    // --- 22 §2.W2.6：twin 回灌决策（前瞻 feedforward 输入）---
    /// twin 预测的负荷序列（kW，按时段）；`None` = 无 twin 预测。
    pub twin_forecast: Option<alloc::vec::Vec<f64>>,
    /// 最近一次 twin 预测的置信度（诊断）。
    pub twin_confidence: f32,
}

impl<S: Solver> EnergyAgent<S> {
    /// 构造 Energy Agent.
    ///
    /// 蓝图 40：`dispatch` = 唯一 ControlDispatch（WP-40.6）；`telemetry` =
    /// 受信任遥测提供者（WP-40.2）。`coordinator = DualBrainCoordinator::new`
    /// 只产 Proposal（不再持有 sink）。
    pub fn new<S2: Solver>(
        name: &str,
        config: ScheduleConfig,
        now_ms: u64,
        llm_engine: Box<dyn LlmEngine>,
        solver: S2,
        dispatch: Box<dyn ControlDispatch>,
        telemetry: Box<dyn TrustedTelemetryProvider>,
    ) -> EnergyAgent<S2> {
        let descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms);
        let coordinator = DualBrainCoordinator::new(config, llm_engine, solver);
        EnergyAgent {
            descriptor,
            coordinator,
            market_channel: MarketChannel::new(16),
            current_schedule: None,
            current_price: 0.5,
            state: AgentState::Created,
            consecutive_errors: 0,
            tick_count: 0,
            telemetry,
            last_trust_rejection: None,
            last_feedback: None,
            outcome_unknown_count: 0,
            forecast_error_sum: 0.0,
            forecast_error_count: 0,
            last_observed_at_ms: now_ms,
            dispatch,
            last_dispatch: None,
            last_dispatch_error: None,
            rejections: eneros_dual_brain::rejection::RejectionHandler::new(3),
            last_rejection: None,
            deadband_learner: eneros_dual_brain::learning::DeadbandAdaptive::with_catalog(
                1.0,
                eneros_safety_validator::SafetyCatalog::default_eps(),
            ),
            // 31-review（P1 AG-02②）：学习器输出接入生产消费（每轮更新）。
            deadband_kw: 0.0,
            forecast_corrector: eneros_dual_brain::learning::ForecastCorrector::with_catalog(
                eneros_safety_validator::SafetyCatalog::default_eps(),
            ),
            twin_forecast: None,
            twin_confidence: 0.0,
        }
    }

    /// 注入 twin 预测（22 §2.W2.6：感知相前瞻输入——twin 预测不只发布，
    /// 回灌决策）。`points` = twin 预测值序列（kW/时段），`confidence` =
    /// 预测置信度；由上层（runner/设备服务）从 twin 预测结果提取注入
    /// （避免 agent↔twin 依赖环，见 ADR-032）。
    /// 32 AG-01：连续 Transient 错误上限（≥ 才转 Error）。
    pub const MAX_CONSECUTIVE_ERRORS: u32 = 3;

    pub fn set_twin_forecast(&mut self, points: alloc::vec::Vec<f64>, confidence: f32) {
        self.twin_forecast = Some(points);
        self.twin_confidence = confidence;
    }

    /// 获取市场通道可变引用（供测试注入数据）.
    pub fn market_channel_mut(&mut self) -> &mut MarketChannel {
        &mut self.market_channel
    }
}

impl<S: Solver> EnergyAgent<S> {
    /// Release 真后端构造器（21-B §2.A2）——与 [`EnergyAgent::new`] 相同
    /// 装配（真 LlmEngine + 真 Solver + 唯一 ControlDispatch + 受信任遥测）。
    pub fn new_real<S2: Solver>(
        name: &str,
        config: ScheduleConfig,
        now_ms: u64,
        llm_engine: Box<dyn LlmEngine>,
        solver: S2,
        dispatch: Box<dyn ControlDispatch>,
        telemetry: Box<dyn TrustedTelemetryProvider>,
    ) -> EnergyAgent<S2> {
        Self::new(
            name, config, now_ms, llm_engine, solver, dispatch, telemetry,
        )
    }
}

#[cfg(any(test, feature = "mock"))]
impl EnergyAgent<MockSolver> {
    /// 测试/演示默认构造（使用 mock 求解器）。
    pub fn new_default(now_ms: u64) -> EnergyAgent<MockSolver> {
        let descriptor = AgentDescriptor::new(AgentType::Energy, "energy-agent", now_ms);
        let coordinator = DualBrainCoordinator::default_with_mock();
        EnergyAgent {
            descriptor,
            coordinator,
            market_channel: MarketChannel::new(16),
            current_schedule: None,
            current_price: 0.5,
            state: AgentState::Created,
            consecutive_errors: 0,
            tick_count: 0,
            telemetry: Box::new(crate::agent_test_support::RejectingTelemetryProvider),
            last_trust_rejection: None,
            last_feedback: None,
            outcome_unknown_count: 0,
            forecast_error_sum: 0.0,
            forecast_error_count: 0,
            last_observed_at_ms: now_ms,
            dispatch: Box::new(crate::agent_test_support::RecordingDispatch::default()),
            last_dispatch: None,
            last_dispatch_error: None,
            rejections: eneros_dual_brain::rejection::RejectionHandler::new(3),
            last_rejection: None,
            deadband_learner: eneros_dual_brain::learning::DeadbandAdaptive::with_catalog(
                1.0,
                eneros_safety_validator::SafetyCatalog::default_eps(),
            ),
            deadband_kw: 0.0,
            forecast_corrector: eneros_dual_brain::learning::ForecastCorrector::with_catalog(
                eneros_safety_validator::SafetyCatalog::default_eps(),
            ),
            twin_forecast: None,
            twin_confidence: 0.0,
        }
    }
}

impl<S: Solver> AgentRuntime for EnergyAgent<S> {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    fn on_start(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.state = AgentState::Running;
        Ok(())
    }

    fn on_tick(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError> {
        // 1. 非阻塞接收市场数据，更新电价缓存
        if let Some(market_data) = self.market_channel.try_recv() {
            self.current_price = market_data.current_price;
        }

        // 2. 感知相（蓝图 40 WP-40.1）：只消费受信任遥测——评估器拒绝时
        //    明确不动作（零推理、零新写），绝不制造"健康默认状态"。
        let telemetry = match self.telemetry.trusted_telemetry(now_ms) {
            Ok(t) => t,
            Err(rej) => {
                self.outcome_unknown_count += 1;
                self.last_trust_rejection = Some(rej);
                self.last_feedback = None;
                return Ok(());
            }
        };
        let system = telemetry.state.clone();
        // 32 AG-02②：修正器接线消费——学习后的价格修正进 LP 输入。
        let corrected_price = self.forecast_corrector.correct(self.current_price);
        let state = RealtimeState {
            system,
            current_price: corrected_price,
            load_demand: self.twin_forecast.clone(),
        };
        let observed_at = now_ms;

        // 3. 调用双脑协调器（WP-40.3：只产 Proposal/NoAction/Rejected）。
        match self.coordinator.execute(&state, now_ms) {
            DecisionOutcome::Proposal(mut plan) => {
                // 绑定输入遥测摘要（provenance——dispatch 校验用）。
                plan.input_digest = Some(telemetry.provenance_digest);
                self.tick_count += 1;
                // 4. 提交给唯一 ControlDispatch（WP-40.6）。
                let actor = AuthenticatedActor {
                    kind: ActorKind::Ai,
                    service: [0u8; 16],
                    session_epoch: 0,
                };
                match self.dispatch.submit(&plan, &telemetry, &actor, 0) {
                    Ok(receipt) if receipt.final_state.is_committed() => {
                        // 5. 观测相（WP-40.12）：learning 只消费 Committed——
                        // 用受信任观测覆写实际值字段；预测值保留算偏差。
                        self.coordinator.mark_committed(&plan);
                        self.last_dispatch = Some(receipt.clone());
                        self.current_schedule = Some(plan.schedule.clone());
                        if let Some(mut fb) = plan.feedback {
                            let predicted = fb.actual_revenue;
                            let last = self.last_observed_at_ms;
                            let hours = observed_at.saturating_sub(last) as f64 / 3_600_000.0;
                            self.last_observed_at_ms = observed_at;
                            fb.actual_revenue =
                                (telemetry.state.current_a * telemetry.state.voltage_v / 1000.0)
                                    * self.current_price
                                    * hours;
                            fb.validation_passed = true;
                            if predicted.is_finite() {
                                let dev = (predicted - fb.actual_revenue).abs();
                                self.forecast_error_sum += dev;
                                self.forecast_error_count += 1;
                                let rel_bias = if predicted.abs() > 1e-9 {
                                    (predicted - fb.actual_revenue) / predicted.abs()
                                } else {
                                    0.0
                                };
                                self.deadband_learner.learn(dev);
                                self.deadband_kw = self.deadband_learner.current;
                                self.forecast_corrector.learn(rel_bias);
                            }
                            self.last_feedback = Some(fb);
                        }
                        self.consecutive_errors = 0;
                        if self.state == AgentState::Error {
                            self.state = AgentState::Running;
                        }
                        Ok(())
                    }
                    Ok(receipt) => {
                        // Failed/Indeterminate/Rejected：失败数据，不学习、
                        // 不记账（WP-40.12：零成功奖励更新）。
                        self.last_dispatch = Some(receipt.clone());
                        let note = receipt.note.clone().unwrap_or_default();
                        self.record_dispatch_failure(now_ms, &note);
                        Ok(())
                    }
                    Err(e) => {
                        // 前置失败（校验/授权/预算/持久化/不可达）：零投递。
                        self.last_dispatch_error = Some(e.clone());
                        self.record_dispatch_failure(now_ms, &format!("{e:?}"));
                        Ok(())
                    }
                }
            }
            DecisionOutcome::NoAction(reason) => {
                // 明确不动作（降级无基线/空调度）——审计但不下发。
                self.tick_count += 1;
                self.current_schedule = None;
                if reason == NoActionReason::NoTrustedInput {
                    self.outcome_unknown_count += 1;
                }
                self.consecutive_errors = 0;
                if self.state == AgentState::Error {
                    self.state = AgentState::Running;
                }
                Ok(())
            }
            DecisionOutcome::Rejected(rej) => {
                // 结构化拒绝：分类 + 预算（防无限重试，不静默丢弃）。
                self.record_dispatch_failure(now_ms, rej.reason.as_str());
                Ok(())
            }
        }
    }

    fn on_stop(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.state = AgentState::Dead;
        Ok(())
    }

    fn on_heartbeat(&self, _now_ms: u64) -> HeartbeatStatus {
        if self.state == AgentState::Running {
            HeartbeatStatus::Alive
        } else {
            HeartbeatStatus::Dead
        }
    }
}

impl<S: Solver> EnergyAgent<S> {
    /// 记录一次失败/拒绝（分类 + 预算；SafetyBlocked 立即 Error）。
    fn record_dispatch_failure(&mut self, _now_ms: u64, note: &str) {
        // 复用 RejectionHandler（分类 + 重试预算，防无限重试，不静默）。
        let err = eneros_dual_brain::error::DualBrainError::DispatchError(note.into());
        let kind = eneros_dual_brain::rejection::RejectionHandler::classify(&err);
        let action = self.rejections.handle(&err, "pcs");
        self.last_rejection = Some((kind, action));
        self.consecutive_errors += 1;
        if action == eneros_dual_brain::rejection::RejectionAction::EscalateToHuman
            || self.consecutive_errors >= Self::MAX_CONSECUTIVE_ERRORS
        {
            self.state = AgentState::Error;
        }
    }

    /// 失败/拒绝分类（安全 → SafetyBlocked；离线 → Offline；越限 →
    /// OutOfRange；其余 → CommFailed 兜底，fail-closed）。
    #[allow(dead_code)] // 44.2：失败分类工具，当前调用面未接线（44.4 冻结登记）
    fn classify_failure(&self, note: &str) -> eneros_dual_brain::rejection::RejectionKind {
        use eneros_dual_brain::rejection::RejectionKind;
        let n = note.to_ascii_lowercase();
        if n.contains("safety") || n.contains("fatal") || n.contains("unsafe") {
            RejectionKind::SafetyBlocked
        } else if n.contains("offline") || n.contains("unreachable") || n.contains("timeout") {
            RejectionKind::Offline
        } else if n.contains("limit")
            || n.contains("range")
            || n.contains("越限")
            || n.contains("proposalinvalid")
        {
            RejectionKind::OutOfRange
        } else {
            RejectionKind::CommFailed
        }
    }
}

// ============================================================================
// 蓝图 40 WP-40.1/40.6/40.12 验收：可信采集 + 唯一 dispatch + Committed 门控
// ============================================================================
#[cfg(test)]
mod w40_trusted_dispatch_tests {
    use eneros_control_dispatch::{DispatchReceipt, DispatchState};

    use super::*;
    use crate::agent_test_support::{RecordingDispatch, StubTelemetryProvider};

    fn agent_with(
        now_ms: u64,
        telemetry: Box<dyn TrustedTelemetryProvider>,
        dispatch: Box<dyn ControlDispatch>,
    ) -> EnergyAgent<MockSolver> {
        let mut agent = EnergyAgent::<MockSolver>::new(
            "energy-agent",
            ScheduleConfig::default(),
            now_ms,
            Box::new(eneros_dual_brain::DualBrainMockEngine::new()),
            MockSolver::new(),
            dispatch,
            telemetry,
        );
        agent.on_start(now_ms).expect("start");
        agent
    }

    /// 验收 1：可信遥测 → Proposal → 唯一 dispatch 提交（Committed 后学习）。
    #[allow(clippy::field_reassign_with_default)] // 44.2：测试夹具 default+覆写模式（语义等价）
    #[test]
    fn test_trusted_telemetry_dispatch_committed_learns() {
        let mut dispatch = RecordingDispatch::default();
        dispatch.result = Some(Ok(DispatchReceipt {
            command_id: [1; 16],
            correlation_id: [2; 16],
            final_state: DispatchState::Committed,
            state_path: alloc::vec![DispatchState::Committed],
            committed_at_ms: Some(10_000),
            note: None,
            latency_ms: 1,
        }));
        let mut agent = agent_with(
            10_000,
            Box::new(StubTelemetryProvider::new(Some(
                StubTelemetryProvider::full_base(),
            ))),
            Box::new(dispatch),
        );
        agent.on_tick(10_000).expect("tick");
        // dispatch 收到提交（唯一出口被调用）。
        assert_eq!(
            agent.last_dispatch.as_ref().unwrap().final_state,
            DispatchState::Committed
        );
        // 学习器被喂入（Committed 后）——预测反馈存在（慢路径）。
        assert!(
            agent.forecast_corrector.samples >= 1 || agent.deadband_learner.samples >= 1,
            "Committed 后必须进入学习（WP-40.12）"
        );
        // 降级基线登记（mark_committed）。
        assert!(agent.coordinator.last_committed_schedule.is_some());
        assert_eq!(agent.consecutive_errors, 0);
    }

    /// 验收 2：无可信遥测（评估器拒绝）→ 零推理、零新写（不制造默认状态）。
    #[test]
    fn test_no_trusted_telemetry_no_action() {
        let mut agent = agent_with(
            10_000,
            Box::new(StubTelemetryProvider::new(None)),
            Box::new(RecordingDispatch::default()),
        );
        agent.on_tick(10_000).expect("tick");
        assert_eq!(agent.outcome_unknown_count, 1, "观测缺失必须计数");
        assert_eq!(agent.tick_count, 0, "无观测不得推进决策（零推理）");
        assert!(
            agent.last_dispatch.is_none(),
            "无观测不得产生投递（零新写）"
        );
        assert!(agent.last_feedback.is_none());
        assert_eq!(agent.forecast_corrector.samples, 0, "失败路径不得学习");
    }

    /// 验收 3：过期/坏质量遥测 → 结构化拒绝 → 零投递。
    #[test]
    fn test_stale_telemetry_rejected_no_dispatch() {
        // 构造"测量时刻极旧"的样本（评估器 Stale 拒绝）。
        struct StaleTelemetryProvider;
        impl TrustedTelemetryProvider for StaleTelemetryProvider {
            fn trusted_telemetry(
                &mut self,
                now_ms: u64,
            ) -> Result<TrustedTelemetry, TrustRejection> {
                use eneros_safety_validator::{IntegrityStatus, SampleQuality};
                let samples: alloc::vec::Vec<eneros_safety_validator::TelemetrySample> =
                    StubTelemetryProvider::full_base()
                        .iter()
                        .map(|(point, value)| eneros_safety_validator::TelemetrySample {
                            point: *point,
                            value: *value,
                            source: 1,
                            source_epoch: 0,
                            measured_at_ms: now_ms.saturating_sub(10_000), // age 10s > 5s
                            received_at_ms: now_ms,
                            quality: SampleQuality::Good,
                            sequence: 1,
                            integrity: IntegrityStatus::Verified,
                            mapping_version: 1,
                        })
                        .collect();
                let mut ev = eneros_safety_validator::TelemetryTrustEvaluator::new(
                    eneros_safety_validator::EvaluatorConfig {
                        coverage: eneros_safety_validator::RequiredPointCoverage {
                            points: alloc::vec![
                                eneros_safety_validator::RequiredPoint {
                                    point: 1,
                                    min: 0.0,
                                    max: 1000.0,
                                    max_age_ms: 5000,
                                    unit: eneros_safety_validator::PointUnit::Volt
                                },
                                eneros_safety_validator::RequiredPoint {
                                    point: 2,
                                    min: -1000.0,
                                    max: 1000.0,
                                    max_age_ms: 5000,
                                    unit: eneros_safety_validator::PointUnit::Ampere
                                },
                                eneros_safety_validator::RequiredPoint {
                                    point: 3,
                                    min: 45.0,
                                    max: 55.0,
                                    max_age_ms: 5000,
                                    unit: eneros_safety_validator::PointUnit::Hertz
                                },
                                eneros_safety_validator::RequiredPoint {
                                    point: 4,
                                    min: 0.0,
                                    max: 1.0,
                                    max_age_ms: 5000,
                                    unit: eneros_safety_validator::PointUnit::Ratio
                                },
                                eneros_safety_validator::RequiredPoint {
                                    point: 5,
                                    min: 0.0,
                                    max: 1.0,
                                    max_age_ms: 5000,
                                    unit: eneros_safety_validator::PointUnit::Other
                                },
                                eneros_safety_validator::RequiredPoint {
                                    point: 6,
                                    min: 0.0,
                                    max: 1.0,
                                    max_age_ms: 5000,
                                    unit: eneros_safety_validator::PointUnit::Other
                                },
                            ],
                        },
                        bindings: eneros_safety_validator::StateBindings {
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
                    },
                );
                ev.evaluate(&samples, now_ms)
            }
        }
        let mut agent = agent_with(
            10_000,
            Box::new(StaleTelemetryProvider),
            Box::new(RecordingDispatch::default()),
        );
        agent.on_tick(10_000).expect("tick");
        assert!(
            matches!(
                agent.last_trust_rejection,
                Some(TrustRejection::Stale { .. })
            ),
            "过期遥测必须结构化拒绝：{:?}",
            agent.last_trust_rejection
        );
        assert_eq!(agent.outcome_unknown_count, 1);
        assert!(agent.last_dispatch.is_none(), "坏遥测不得投递");
    }

    /// 验收 4：dispatch Failed/Indeterminate → 失败数据，不学习（WP-40.12）。
    #[allow(clippy::field_reassign_with_default)] // 44.2：测试夹具 default+覆写模式（语义等价）
    #[test]
    fn test_failed_dispatch_no_learning() {
        let mut dispatch = RecordingDispatch::default();
        dispatch.result = Some(Ok(DispatchReceipt {
            command_id: [1; 16],
            correlation_id: [2; 16],
            final_state: DispatchState::Failed,
            state_path: alloc::vec![DispatchState::Failed],
            committed_at_ms: None,
            note: Some(String::from("readback-mismatch")),
            latency_ms: 1,
        }));
        let mut agent = agent_with(
            10_000,
            Box::new(StubTelemetryProvider::new(Some(
                StubTelemetryProvider::full_base(),
            ))),
            Box::new(dispatch),
        );
        agent.on_tick(10_000).expect("tick");
        assert_eq!(
            agent.last_dispatch.as_ref().unwrap().final_state,
            DispatchState::Failed
        );
        assert_eq!(
            agent.forecast_corrector.samples, 0,
            "失败动作不得更新成功奖励"
        );
        assert_eq!(agent.deadband_learner.samples, 0);
        assert!(
            agent.coordinator.last_committed_schedule.is_none(),
            "失败不得登记降级基线"
        );
        assert_eq!(agent.consecutive_errors, 1, "失败必须计数（不静默）");
    }

    /// 验收 5：dispatch 前置错误（零投递）→ 拒绝分类 + 不学习。
    #[allow(clippy::field_reassign_with_default)] // 44.2：测试夹具 default+覆写模式（语义等价）
    #[test]
    fn test_dispatch_error_classified() {
        let mut dispatch = RecordingDispatch::default();
        dispatch.result = Some(Err(DispatchError::SafetyRejected(String::from(
            "unsafe: soc below floor",
        ))));
        let mut agent = agent_with(
            10_000,
            Box::new(StubTelemetryProvider::new(Some(
                StubTelemetryProvider::full_base(),
            ))),
            Box::new(dispatch),
        );
        agent.on_tick(10_000).expect("tick");
        assert!(agent.last_dispatch_error.is_some());
        let (kind, _action) = agent.last_rejection.expect("rejection");
        assert_eq!(
            kind,
            eneros_dual_brain::rejection::RejectionKind::SafetyBlocked,
            "安全拒绝必须分类为 SafetyBlocked"
        );
        assert_eq!(agent.state, AgentState::Error, "安全阻断立即 Error");
        assert_eq!(agent.forecast_corrector.samples, 0);
    }

    /// 验收 6：重复失败 N 次连续 → Error；成功归零（心跳语义保留）。
    #[allow(clippy::field_reassign_with_default)] // 44.2：测试夹具 default+覆写模式（语义等价）
    #[test]
    fn test_consecutive_failures_escalate_and_success_recovers() {
        let mut dispatch = RecordingDispatch::default();
        dispatch.result = Some(Err(DispatchError::BudgetExceeded(String::from("rate"))));
        let mut agent = agent_with(
            10_000,
            Box::new(StubTelemetryProvider::new(Some(
                StubTelemetryProvider::full_base(),
            ))),
            Box::new(dispatch),
        );
        for _ in 0..2 {
            agent.on_tick(10_000).expect("tick");
        }
        assert_eq!(agent.state, AgentState::Running, "瞬态 2 次不 Error");
        agent.on_tick(10_000).expect("tick");
        assert_eq!(agent.state, AgentState::Error, "连续 3 次 → Error");
        // 成功恢复：换一个返回 Committed 的 dispatch。
        let mut ok_dispatch = RecordingDispatch::default();
        ok_dispatch.result = Some(Ok(DispatchReceipt {
            command_id: [1; 16],
            correlation_id: [2; 16],
            final_state: DispatchState::Committed,
            state_path: alloc::vec![DispatchState::Committed],
            committed_at_ms: Some(10_000),
            note: None,
            latency_ms: 1,
        }));
        agent.dispatch = Box::new(ok_dispatch);
        agent.on_tick(10_000).expect("tick");
        assert_eq!(agent.state, AgentState::Running, "成功必须恢复");
    }
}

// ============================================================================
// 22 §2.W2.4 验收：设备拒绝分类处理（接入 agent 决策路径）
// ============================================================================
#[cfg(test)]
mod w24_rejection_tests {
    use eneros_dual_brain::rejection::{RejectionAction, RejectionHandler, RejectionKind};

    use super::*;

    /// 验收：设备拒绝 → 分类 + 计数（不静默）。
    #[test]
    fn test_device_rejection_classified_not_silent() {
        let mut agent = EnergyAgent::new_default(1_000);
        agent.on_start(1_000).expect("start");
        let err = eneros_dual_brain::error::DualBrainError::DispatchError(String::from(
            "device-exec: Offline",
        ));
        let kind = RejectionHandler::classify(&err);
        assert_eq!(kind, RejectionKind::Offline);
        let action = agent.rejections.handle(&err, "pcs");
        assert_eq!(action, RejectionAction::Retry { attempt: 1 });
        assert_eq!(agent.rejections.total_rejections(), 1, "拒绝必须计数");
        // 预算耗尽 → 上报人工（绝不无限重试）。
        for _ in 0..4 {
            let _ = agent.rejections.handle(&err, "pcs");
        }
        assert_eq!(
            agent.rejections.handle(&err, "pcs"),
            RejectionAction::EscalateToHuman,
            "超预算必须上报人工"
        );
    }

    /// 验收：安全阻断分类独立（不重试，立即上报）。
    #[test]
    fn test_safety_blocked_escalates() {
        let agent = EnergyAgent::new_default(1_000);
        let note = "unsafe: soc below floor";
        assert_eq!(agent.classify_failure(note), RejectionKind::SafetyBlocked);
        let mut h = RejectionHandler::new(3);
        let err = eneros_dual_brain::error::DualBrainError::SafetyFatal(String::from(
            "soc below minimum",
        ));
        assert_eq!(h.handle(&err, "pcs"), RejectionAction::EscalateToHuman);
    }
}

// ============================================================================
// 22 §2.W2.5 验收：轻量在线学习接入（deadband 自适应 + 偏差修正）
// ============================================================================
#[cfg(test)]
mod w25_learning_tests {
    use eneros_dual_brain::learning::{DeadbandAdaptive, ForecastCorrector};

    use super::*;

    /// 验收 1：学习器接入 agent——观测偏差喂入后状态推进（安全闸内置）。
    #[test]
    fn test_learner_wired_into_agent() {
        let mut agent = EnergyAgent::new_default(1_000);
        let before = agent.deadband_learner.samples;
        agent.deadband_learner.learn(2.0);
        assert_eq!(agent.deadband_learner.samples, before + 1);
    }

    /// 验收 2：学习输出过 safety-validator——越界拒绝用旧值。
    #[test]
    fn test_learning_outputs_pass_safety_validator() {
        let mut db = DeadbandAdaptive::new(1.0, 0.1, 10.0);
        for _ in 0..100 {
            db.learn(100.0); // 极端噪声
        }
        assert!(db.current <= db.max, "死区必须钳位在安全 catalog 内");
        let mut fc = ForecastCorrector::new(0.5);
        for _ in 0..200 {
            fc.learn(50.0); // 极端偏差
        }
        assert!(fc.correction().abs() <= 0.5, "修正系数必须钳位安全界");
    }

    /// 32 AG-02 判据：归一化后 20% 相对偏差喂 100 轮 → correction ≈ +0.2。
    #[test]
    fn test_ag02_relative_bias_converges_and_no_rejection() {
        let mut fc = ForecastCorrector::new(0.5);
        for _ in 0..100 {
            fc.learn(0.2);
        }
        let c = fc.correction();
        assert!(c > 0.19 && c < 0.21, "收敛到 +0.2：{c}");
        assert_ne!(c, 0.0, "修正必须生效（非旧实现的恒 0 回退）");
        assert_eq!(fc.rejected_updates, 0, "界内相对偏差不得被拒");
        let corrected = fc.correct(100.0);
        assert!(
            (corrected - 120.0).abs() < 1.0,
            "correct 消费生效：{corrected}"
        );
    }
}

// ============================================================================
// 22 §2.W2.6 验收：twin 回灌决策（感知相前瞻输入）
// ============================================================================
#[cfg(test)]
mod w26_twin_feedforward_tests {
    use super::*;

    /// 验收 1：决策感知读 twin 预测（feedforward 注入生效）。
    #[test]
    fn test_twin_forecast_feedforward() {
        let mut agent = EnergyAgent::new_default(1_000);
        assert!(agent.twin_forecast.is_none(), "默认无 twin 预测");
        agent.set_twin_forecast(alloc::vec![10.0, 20.0, 30.0], 0.9);
        assert_eq!(agent.twin_forecast.as_ref().unwrap().len(), 3);
        assert_eq!(agent.twin_confidence, 0.9);
        agent.on_start(1_000).expect("start");
        agent.on_tick(2_000).expect("tick");
    }
}
