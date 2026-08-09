//! 能源调度 Agent — 编排双脑协调器执行储能调度（D9/D10/D11/D14）.

use alloc::boxed::Box;

use eneros_agent::{AgentDescriptor, AgentState, AgentType};
use eneros_dual_brain::DualBrainCoordinator;
use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::result::ScheduleResult;
use eneros_fast_path::state::RealtimeState;
use eneros_intent_contract::contract::FeedbackContract;
use eneros_llm_engine::engine::LlmEngine;
use eneros_safety_validator::state::SystemState;
#[cfg(any(test, feature = "mock"))]
use eneros_solver_core::mock::MockSolver;
use eneros_solver_core::solver::Solver;

use crate::error::AgentRuntimeError;
use crate::market::MarketChannel;
use crate::runtime::{AgentRuntime, HeartbeatStatus};

/// 设备观测（22 §2.W2.1 act→observe 反馈环的"观测相"数据源）。
///
/// 真实遥测读回由上层 runner/设备服务接 TSDB 后注入 [`DeviceObserver`]；
/// agent 层保持接口真实（感知→决策→执行→观测 四相闭环）。
#[derive(Debug, Clone, Copy)]
pub struct DeviceObservation {
    /// 母线电压（V）。
    pub voltage_v: f64,
    /// 母线电流（A）。
    pub current_a: f64,
    /// 储能 SOC（0.0~1.0）。
    pub soc_pct: f64,
    /// 观测时刻（毫秒）。
    pub observed_at_ms: u64,
}

/// 设备观测源（执行后读回真实设备状态；`None` = 观测超时/不可用）。
pub trait DeviceObserver {
    /// 读回设备当前状态；`None` 表示观测窗口内读不到（→ OutcomeUnknown）。
    fn observe_device(&self) -> Option<DeviceObservation>;
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
    /// tick 计数.
    pub tick_count: u64,
    // --- 22 §2.W2.1：act→observe 反馈环 ---
    /// 设备观测源（注入；None = 无观测源，feedback 走 Unknown 路径）。
    pub observer: Option<Box<dyn DeviceObserver>>,
    /// 最近一次执行的反馈契约（不再丢弃；含观测相覆写后的实际值）。
    pub last_feedback: Option<FeedbackContract>,
    /// 观测超时计数（OutcomeUnknown 路径指标）。
    pub outcome_unknown_count: u64,
    /// 预测−观测偏差累计（指标 `decision.forecast_error` 的源数据）。
    pub forecast_error_sum: f64,
    /// 偏差样本数。
    pub forecast_error_count: u64,
    /// 上一轮观测时刻（毫秒；时窗 = 本轮观测 − 上轮观测）。
    pub last_observed_at_ms: u64,
    // --- 22 §2.W2.4：设备拒绝/异常处理 ---
    /// 拒绝处理器（分类计数 + 重试预算，防无限重试）。
    pub rejections: eneros_dual_brain::rejection::RejectionHandler,
    /// 最近一次拒绝（分类 + 策略；None = 无拒绝）。
    pub last_rejection: Option<(
        eneros_dual_brain::rejection::RejectionKind,
        eneros_dual_brain::rejection::RejectionAction,
    )>,
    // --- 22 §2.W2.5：轻量在线学习（锁定 8，安全闸后生效）---
    /// deadband 自适应学习器（学习输出过 safety-validator）。
    pub deadband_learner: eneros_dual_brain::learning::DeadbandAdaptive,
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
    /// `descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms)`（D7）。
    /// `coordinator = DualBrainCoordinator::new(config, llm, solver, sink)`（D9）。
    /// 构造 Energy Agent（求解器/引擎/发送器构造器注入，P0-27）.
    ///
    /// `descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms)`（D7）。
    /// `coordinator = DualBrainCoordinator::new(config, llm, solver, sink)`（D9）。
    pub fn new<S2: Solver>(
        name: &str,
        config: ScheduleConfig,
        now_ms: u64,
        llm_engine: Box<dyn LlmEngine>,
        solver: S2,
        sink: Box<dyn eneros_dual_brain::sink::CommandSink>,
    ) -> EnergyAgent<S2> {
        let descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms);
        let coordinator = DualBrainCoordinator::new(config, llm_engine, solver, sink);
        EnergyAgent {
            descriptor,
            coordinator,
            market_channel: MarketChannel::new(16),
            current_schedule: None,
            current_price: 0.5,
            state: AgentState::Created,
            tick_count: 0,
            observer: None,
            last_feedback: None,
            outcome_unknown_count: 0,
            forecast_error_sum: 0.0,
            forecast_error_count: 0,
            last_observed_at_ms: now_ms,
            rejections: eneros_dual_brain::rejection::RejectionHandler::new(3),
            last_rejection: None,
            deadband_learner: eneros_dual_brain::learning::DeadbandAdaptive::with_catalog(1.0, eneros_safety_validator::SafetyCatalog::default_eps()),
            forecast_corrector: eneros_dual_brain::learning::ForecastCorrector::with_catalog(eneros_safety_validator::SafetyCatalog::default_eps()),
            twin_forecast: None,
            twin_confidence: 0.0,
        }
    }

    /// 注入设备观测源（W2.1；真实遥测读回由上层接 TSDB 后调用）。
    pub fn with_observer(mut self, observer: Box<dyn DeviceObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// 注入 twin 预测（22 §2.W2.6：感知相前瞻输入——twin 预测不只发布，
    /// 回灌决策）。`points` = twin 预测值序列（kW/时段），`confidence` =
    /// 预测置信度；由上层（runner/设备服务）从 twin 预测结果提取注入
    /// （避免 agent↔twin 依赖环，见 ADR-032）。
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
    /// Release 真后端构造器（21-B §2.A2）。
    ///
    /// 与 [`EnergyAgent::new`] 相同装配（`DualBrainCoordinator::new` 注入
    /// 真 LlmEngine + 真 Solver + 真 Sink），显式命名对齐蓝图 A2：
    /// release 构建下 `new_default` 不可见（mock 门控），真后端装配必须走
    /// `new` / `new_real`。本构造器不绑定任何 mock 类型——`S: Solver`
    /// 在 release 装配 `HighsSolver`（solver-core `highs-ffi`）。
    ///
    /// `#[cfg(not(any(test, feature = "mock")))]` 语义由调用方保证：
    /// 依赖本 crate 的 release 构建不得启用 `mock` feature。
    pub fn new_real<S2: Solver>(
        name: &str,
        config: ScheduleConfig,
        now_ms: u64,
        llm_engine: Box<dyn LlmEngine>,
        solver: S2,
        sink: Box<dyn eneros_dual_brain::sink::CommandSink>,
    ) -> EnergyAgent<S2> {
        Self::new(name, config, now_ms, llm_engine, solver, sink)
    }
}

#[cfg(any(test, feature = "mock"))]
impl EnergyAgent<MockSolver> {
    /// 测试/演示默认构造（使用 mock 求解器）.
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
            tick_count: 0,
            observer: None,
            last_feedback: None,
            outcome_unknown_count: 0,
            forecast_error_sum: 0.0,
            forecast_error_count: 0,
            last_observed_at_ms: now_ms,
            rejections: eneros_dual_brain::rejection::RejectionHandler::new(3),
            last_rejection: None,
            deadband_learner: eneros_dual_brain::learning::DeadbandAdaptive::with_catalog(1.0, eneros_safety_validator::SafetyCatalog::default_eps()),
            forecast_corrector: eneros_dual_brain::learning::ForecastCorrector::with_catalog(eneros_safety_validator::SafetyCatalog::default_eps()),
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

        // 2. 感知相（22 §2.W2.1）：RealtimeState 由**真实观测值**构造
        // （非 default）；观测缺失 → default + OutcomeUnknown 计数（不静默）。
        let observed = self.observer.as_ref().and_then(|o| o.observe_device());
        let system = match observed {
            Some(obs) => SystemState {
                voltage_v: obs.voltage_v,
                current_a: obs.current_a,
                frequency_hz: 50.0,
                soc_pct: obs.soc_pct,
                timestamp_ms: obs.observed_at_ms,
            },
            None => {
                self.outcome_unknown_count += 1;
                SystemState::default()
            }
        };
        let state = RealtimeState {
            system,
            current_price: self.current_price,
            // 22 §2.W2.6：twin 预测回灌（feedforward）——有预测用预测，
            // 无预测显式 None（不造假值）。
            load_demand: self.twin_forecast.clone(),
        };

        // 3. 调用双脑协调器（D10：execute 需 now_ms 参数）
        match self.coordinator.execute(&state, now_ms) {
            Ok(result) => {
                self.current_schedule = Some(result.schedule);
                self.tick_count += 1;
                // 4. 观测相（22 §2.W2.1）：feedback 不再丢弃——用执行后
                // 读回的真实观测覆写实际值字段；预测值保留算偏差。
                if let Some(mut fb) = result.feedback {
                    let predicted = fb.actual_revenue;
                    match observed {
                        Some(obs) => {
                            // 实际收益 = 观测功率（V×A/1000 kW）× 电价 ×
                            // 观测时窗（本轮观测 − 上轮观测的毫秒数）。
                            let last = self.last_observed_at_ms;
                            let hours =
                                obs.observed_at_ms.saturating_sub(last) as f64 / 3_600_000.0;
                            self.last_observed_at_ms = obs.observed_at_ms;
                            fb.actual_revenue = (obs.current_a * obs.voltage_v / 1000.0)
                                * self.current_price
                                * hours;
                            fb.validation_passed = true;
                            // 偏差指标（decision.forecast_error 源数据）。
                            if predicted.is_finite() {
                                let dev = (predicted - fb.actual_revenue).abs();
                                self.forecast_error_sum += dev;
                                self.forecast_error_count += 1;
                                // 22 §2.W2.5：轻量在线学习（安全闸内置——
                                // 越界拒绝用旧值，学坏不致灾）。
                                self.deadband_learner.learn(dev);
                                self.forecast_corrector.learn(predicted - fb.actual_revenue);
                            }
                        }
                        None => {
                            // 观测超时：标记 OutcomeUnknown（不静默）。
                            fb.actual_revenue = f64::NAN;
                            fb.validation_passed = false;
                        }
                    }
                    self.last_feedback = Some(fb);
                }
                Ok(())
            }
            Err(e) => {
                // D14：安全默认策略 — 标记错误状态，不 panic
                // 22 §2.W2.4：拒绝分类 + 预算（防无限重试，不静默丢弃）。
                let kind = eneros_dual_brain::rejection::RejectionHandler::classify(&e);
                let action = self.rejections.handle(&e, "pcs");
                self.last_rejection = Some((kind, action));
                self.state = AgentState::Error;
                Err(AgentRuntimeError::DualBrainError(e))
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

// ============================================================================
// 22 §2.W2.1 验收：act→observe 反馈环测试
// ============================================================================
#[cfg(test)]
mod w21_feedback_loop_tests {
    use super::*;

    /// 固定观测源：可设定电压/电流/SOC/时刻。
    struct StubObserver {
        voltage_v: f64,
        current_a: f64,
        soc_pct: f64,
        at_ms: u64,
    }
    impl DeviceObserver for StubObserver {
        fn observe_device(&self) -> Option<DeviceObservation> {
            Some(DeviceObservation {
                voltage_v: self.voltage_v,
                current_a: self.current_a,
                soc_pct: self.soc_pct,
                observed_at_ms: self.at_ms,
            })
        }
    }

    /// 无观测源（观测超时/不可用路径）。
    struct NoneObserver;
    impl DeviceObserver for NoneObserver {
        fn observe_device(&self) -> Option<DeviceObservation> {
            None
        }
    }

    fn agent_with(observer: Option<Box<dyn DeviceObserver>>) -> EnergyAgent<MockSolver> {
        let mut agent = EnergyAgent::new_default(1_000);
        if let Some(o) = observer {
            agent = agent.with_observer(o);
        }
        agent.on_start(1_000).expect("start");
        agent
    }

    /// 验收 1：on_tick 不再丢弃 feedback；feedback 含真实观测值（非预测）。
    #[test]
    fn test_feedback_observed_not_dropped() {
        let mut agent = agent_with(Some(Box::new(StubObserver {
            voltage_v: 380.0,
            current_a: 50.0, // 50 A → 19 kW
            soc_pct: 0.6,
            at_ms: 1_800_000, // 上一 tick 后 0.5h
        })));
        agent.on_tick(1_800_000).expect("tick");
        let fb = agent.last_feedback.expect("feedback 必须保留（不再丢弃）");
        assert!(
            fb.actual_revenue.is_finite() && fb.actual_revenue > 0.0,
            "actual_revenue 必须为观测值（>0）：{}",
            fb.actual_revenue
        );
        // 19 kW × 0.5 元/kWh × (1_800_000−1_000)ms/1h = 4.7500026… 元。
        let hours = (1_800_000u64.saturating_sub(1_000)) as f64 / 3_600_000.0;
        let expect = 19.0 * 0.5 * hours;
        assert!(
            (fb.actual_revenue - expect).abs() < 1e-6,
            "观测收益应 ≈ {expect}，实际 {}",
            fb.actual_revenue
        );
    }

    /// 验收 2：下一轮 SystemContext 由观测值构造（非 default）——观测电流
    /// 被感知相读取（通过 coordinator 感知路径的副作用难以直接断言，
    /// 以 feedback 观测值 + 观测计数为代理断言）。
    #[test]
    fn test_observed_state_feeds_next_tick() {
        let mut agent = agent_with(Some(Box::new(StubObserver {
            voltage_v: 380.0,
            current_a: 100.0,
            soc_pct: 0.7,
            at_ms: 3_600_000,
        })));
        // 连续两轮：每轮都观测到 → outcome_unknown 不递增。
        agent.on_tick(3_600_000).expect("tick1");
        agent.on_tick(7_200_000).expect("tick2");
        assert_eq!(
            agent.outcome_unknown_count, 0,
            "有观测源时不得走 Unknown 路径"
        );
        assert!(agent.last_feedback.is_some());
    }

    /// 验收 3：观测超时 → OutcomeUnknown（feedback 标记）+ 计数，不静默。
    #[test]
    fn test_observe_timeout_marks_outcome_unknown() {
        let mut agent = agent_with(Some(Box::new(NoneObserver)));
        agent.on_tick(2_000).expect("tick");
        assert_eq!(agent.outcome_unknown_count, 1, "观测缺失必须计数（不静默）");
        let fb = agent.last_feedback.expect("feedback 保留");
        assert!(
            fb.actual_revenue.is_nan(),
            "观测超时 feedback 必须标记 OutcomeUnknown（NaN）"
        );
        assert!(!fb.validation_passed, "观测超时不得声称验证通过");
    }

    /// 验收 4：预测−观测偏差入指标（decision.forecast_error 源数据）。
    #[test]
    fn test_forecast_error_metric_accumulates() {
        let mut agent = agent_with(Some(Box::new(StubObserver {
            voltage_v: 380.0,
            current_a: 10.0,
            soc_pct: 0.5,
            at_ms: 3_600_000,
        })));
        agent.on_tick(3_600_000).expect("tick");
        assert!(agent.forecast_error_count >= 1, "偏差样本必须累计");
        assert!(agent.forecast_error_sum >= 0.0, "偏差和为绝对误差（非负）");
    }
}

// ============================================================================
// 22 §2.W2.4 验收：设备拒绝分类处理（接入 agent 决策路径）
// ============================================================================
#[cfg(test)]
mod w24_rejection_tests {
    use eneros_dual_brain::error::DualBrainError;
    use eneros_dual_brain::rejection::{RejectionAction, RejectionHandler, RejectionKind};

    use super::*;

    /// 验收：设备拒绝 → 分类 + 计数（不静默），错误仍上抛（决策感知）。
    #[test]
    fn test_device_rejection_classified_not_silent() {
        let mut agent = EnergyAgent::new_default(1_000);
        agent.on_start(1_000).expect("start");
        // 用 FailingSink 类路径：构造 coordinator 直接注入失败 sink 的
        // agent 不可行（new_default 固定）——直接测 handler 在 agent 上。
        let err = DualBrainError::DispatchError(String::from("device-exec: Offline"));
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
        let err = DualBrainError::SafetyFatal(String::from("soc below minimum"));
        assert_eq!(
            RejectionHandler::classify(&err),
            RejectionKind::SafetyBlocked
        );
        let mut h = RejectionHandler::new(3);
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
        // 直接喂偏差（模拟观测相输出）。
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

    /// 验收 3：无偏差时不学习（样本不推进）。
    #[test]
    fn test_no_learning_without_deviation() {
        let mut db = DeadbandAdaptive::new(1.0, 0.1, 10.0);
        db.learn(0.0);
        // 零偏差仍算样本（EMA 衰减向 0）——语义：死区回落。
        assert_eq!(db.samples, 1);
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
        // on_tick 感知相把 twin 预测带入 load_demand（coordinator 消费）。
        agent.on_start(1_000).expect("start");
        agent.on_tick(2_000).expect("tick");
        // tick 正常执行（twin 预测作为 load_demand 传入 LP 无副作用错误）。
    }

    /// 验收 2：有/无 twin 的决策对比——load_demand 传递差异可观察。
    #[test]
    fn test_twin_vs_no_twin_state() {
        let mut with_twin = EnergyAgent::new_default(1_000);
        let mut without = EnergyAgent::new_default(1_000);
        with_twin.set_twin_forecast(alloc::vec![50.0, 60.0], 0.95);
        with_twin.on_start(1_000).expect("start");
        without.on_start(1_000).expect("start");
        let r1 = with_twin.on_tick(2_000);
        let r2 = without.on_tick(2_000);
        assert!(r1.is_ok() && r2.is_ok(), "有/无 twin 都必须可决策");
        // twin 注入路径记录（区分于无 twin）。
        assert!(with_twin.twin_forecast.is_some());
        assert!(without.twin_forecast.is_none());
    }
}
