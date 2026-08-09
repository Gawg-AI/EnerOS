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
        }
    }

    /// 注入设备观测源（W2.1；真实遥测读回由上层接 TSDB 后调用）。
    pub fn with_observer(mut self, observer: Box<dyn DeviceObserver>) -> Self {
        self.observer = Some(observer);
        self
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
            load_demand: None,
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
                                self.forecast_error_sum += (predicted - fb.actual_revenue).abs();
                                self.forecast_error_count += 1;
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
    use crate::runtime::AgentRuntime as _;

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
