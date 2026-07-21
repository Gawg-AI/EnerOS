//! WatchdogDegradeFlow — 端到端降级流程核心编排器（D6/D11）.
//!
//! [`WatchdogDegradeFlow`] 整合心跳监控（[`crate::heartbeat::HeartbeatWatcher`]）、
//! 降级引擎（[`eneros_rtos_degrade::engine::DegradeEngine`]）、命令执行器
//! （[`eneros_rtos_cmd_exec::executor::CommandExecutor`]）与分层看门狗
//! （[`eneros_watchdog::Watchdog`]），通过 5 态状态机
//! （[`crate::state::DegradeState`]）编排端到端降级流程。
//!
//! # D6：泛型设计
//!
//! `WatchdogDegradeFlow<P: PointAccess, S: DeviceStateProvider>` 持有
//! `DegradeEngine<P>` + `CommandExecutor<P, S>`，不使用 `Box<dyn PointAccess>`。
//!
//! # D11：单步驱动
//!
//! `tick(&mut self, ctx: &DegradeContext) -> FlowReport`，从 `ctx.now_ns` 取时间戳。
//!
//! # 状态机
//!
//! ```text
//! Normal --Dead--> Degrading --(same tick)--> Degraded
//! Degraded --Alive--> Recovering --complete--> Normal
//! Recovering --Dead--> Degraded (风险 8.4)
//! Any --watchdog HardReset--> Emergency (不自动恢复, D12)
//! ```

use eneros_protocol_abstract::PointAccess;
use eneros_rtos_cmd_exec::executor::CommandExecutor;
use eneros_rtos_cmd_exec::state_provider::DeviceStateProvider;
use eneros_rtos_cmd_exec::stats::ExecutorReport;
use eneros_rtos_degrade::context::DegradeContext;
use eneros_rtos_degrade::engine::DegradeEngine;
use eneros_rtos_degrade::stats::DegradeReport;
use eneros_upa_model::PointValue;
use eneros_watchdog::{LayerId, Watchdog, WatchdogStatus};

use crate::config::DegradeConfig;
use crate::heartbeat::{HeartbeatStatus, HeartbeatWatcher};
use crate::recovery::RecoveryManager;
use crate::state::DegradeState;
use crate::stats::{FlowReport, FlowStats};

/// 端到端降级流程管理器.
///
/// 泛型参数：
/// - `P`: 协议访问层（实现 `PointAccess`）
/// - `S`: 设备状态来源（实现 `DeviceStateProvider`）
pub struct WatchdogDegradeFlow<P: PointAccess, S: DeviceStateProvider> {
    /// 当前状态。
    pub state: DegradeState,
    /// 心跳监控器。
    pub heartbeat: HeartbeatWatcher,
    /// 降级引擎。
    pub degrade_engine: DegradeEngine<P>,
    /// 命令执行器。
    pub cmd_executor: CommandExecutor<P, S>,
    /// 恢复过渡管理器（纯状态，D10）。
    pub recovery: RecoveryManager,
    /// 分层看门狗。
    pub watchdog: Watchdog,
    /// 降级配置。
    pub config: DegradeConfig,
    /// 累计统计。
    pub stats: FlowStats,
    /// 内核层 ID（100ms 周期）。
    kernel_layer: LayerId,
    /// 运行时层 ID（1000ms 周期）。
    runtime_layer: LayerId,
    /// Agent 层 ID（1000ms 周期）。
    agent_layer: LayerId,
}

impl<P: PointAccess, S: DeviceStateProvider> WatchdogDegradeFlow<P, S> {
    /// 创建端到端降级流程管理器.
    ///
    /// 注册 3 个看门狗层（kernel=100ms, runtime=1000ms, agent=1000ms），
    /// 初始状态为 [`DegradeState::Normal`]。
    pub fn new(
        degrade_engine: DegradeEngine<P>,
        cmd_executor: CommandExecutor<P, S>,
        mut watchdog: Watchdog,
        config: DegradeConfig,
    ) -> Self {
        let kernel_layer = watchdog.register_layer("kernel", 100).unwrap_or(LayerId(0));
        let runtime_layer = watchdog
            .register_layer("runtime", 1000)
            .unwrap_or(LayerId(0));
        let agent_layer = watchdog.register_layer("agent", 1000).unwrap_or(LayerId(0));

        let heartbeat = HeartbeatWatcher::new(
            config.heartbeat_period_ms.saturating_mul(1_000_000),
            config.heartbeat_timeout_count,
        );
        let recovery =
            RecoveryManager::new(config.recovery_transition_ms.saturating_mul(1_000_000));

        Self {
            state: DegradeState::Normal,
            heartbeat,
            degrade_engine,
            cmd_executor,
            recovery,
            watchdog,
            config,
            stats: FlowStats::default(),
            kernel_layer,
            runtime_layer,
            agent_layer,
        }
    }

    /// 单步驱动（D11：`ctx.now_ns` 注入时间）。
    ///
    /// 流程：
    /// 1. 心跳检查 → [`HeartbeatStatus`]
    /// 2. 看门狗检查 → [`WatchdogStatus`]；若 `HardReset` 则转 [`DegradeState::Emergency`]
    /// 3. 状态转换评估 → [`Self::evaluate_state_transition`]
    /// 4. 状态转换动作 → [`Self::on_state_transition`]
    /// 5. 按状态执行动作（命令执行/降级评估/恢复插值/不喂狗）
    /// 6. 喂狗（Emergency 外都喂，Normal 喂 3 层，其他喂 2 层）
    pub fn tick(&mut self, ctx: &DegradeContext) -> FlowReport {
        let now_ns = ctx.now_ns;

        // 1. 心跳检查
        let heartbeat_status = self.heartbeat.check(now_ns);
        if !matches!(heartbeat_status, HeartbeatStatus::Alive) {
            self.stats.heartbeat_timeouts += 1;
        }

        // 2. 看门狗检查
        let watchdog_status = self.watchdog.check(now_ns);

        // 3. 状态转换评估
        let mut state_changed = false;
        let new_state = if watchdog_status == WatchdogStatus::HardReset {
            DegradeState::Emergency
        } else {
            self.evaluate_state_transition(heartbeat_status)
        };

        // 4. 状态转换动作
        if new_state != self.state {
            self.on_state_transition(self.state, new_state, now_ns);
            self.state = new_state;
            state_changed = true;
        }

        // 5. 按状态执行动作
        let mut cmd_report = ExecutorReport::default();
        let mut degrade_report = DegradeReport::default();

        match self.state {
            DegradeState::Normal => {
                self.stats.cmds_executed += 1;
                cmd_report = self.cmd_executor.tick(now_ns);
                self.feed_layers(now_ns, true);
            }
            DegradeState::Degrading => {
                // Degrading 是瞬时态：立即转为 Degraded（同一 tick）
                self.on_state_transition(DegradeState::Degrading, DegradeState::Degraded, now_ns);
                self.state = DegradeState::Degraded;
                state_changed = true;
                self.stats.degrade_evaluations += 1;
                degrade_report = self.degrade_engine.evaluate(ctx, now_ns);
                self.feed_layers(now_ns, false);
            }
            DegradeState::Degraded => {
                self.stats.degrade_evaluations += 1;
                degrade_report = self.degrade_engine.evaluate(ctx, now_ns);
                self.feed_layers(now_ns, false);
            }
            DegradeState::Recovering => {
                // 线性插值过渡
                if let Some(value) = self.recovery.transition_step(now_ns) {
                    let _ = self
                        .degrade_engine
                        .protocol_mut()
                        .write_point(self.config.power_cmd_point, PointValue::Float(value));
                }
                // 过渡完成 → 转 Normal
                if self.recovery.is_complete() {
                    self.on_state_transition(
                        DegradeState::Recovering,
                        DegradeState::Normal,
                        now_ns,
                    );
                    self.state = DegradeState::Normal;
                    state_changed = true;
                }
                self.feed_layers(now_ns, false);
            }
            DegradeState::Emergency => {
                // D12: 不喂狗，等待硬件复位
            }
        }

        // 6. 构建报告
        FlowReport {
            state: self.state,
            state_changed,
            heartbeat: heartbeat_status,
            cmd_report,
            degrade_report,
            watchdog: watchdog_status,
        }
    }

    /// 状态转换评估（基于心跳状态）.
    ///
    /// - Normal + Dead → Degrading
    /// - Degrading → Degraded（防御性，实际由 tick 动作处理）
    /// - Degraded + Alive → Recovering
    /// - Recovering + Dead → Degraded（风险 8.4）
    /// - Emergency → Emergency（D12 不自动恢复）
    /// - 其他 → 保持当前状态
    pub fn evaluate_state_transition(&self, heartbeat: HeartbeatStatus) -> DegradeState {
        match (self.state, heartbeat) {
            (DegradeState::Normal, HeartbeatStatus::Dead) => DegradeState::Degrading,
            (DegradeState::Degrading, _) => DegradeState::Degraded,
            (DegradeState::Degraded, HeartbeatStatus::Alive) => DegradeState::Recovering,
            (DegradeState::Recovering, HeartbeatStatus::Dead) => DegradeState::Degraded,
            (DegradeState::Emergency, _) => DegradeState::Emergency,
            _ => self.state,
        }
    }

    /// 状态转换动作.
    ///
    /// - Normal → Degrading：读取当前设定值并保存
    /// - Degraded → Recovering：读取降级值，启动过渡
    /// - Recovering → Normal：完成过渡，累加恢复计数
    /// - 任意 → Emergency：累加紧急计数
    /// - 其他：仅累加状态转换计数
    pub fn on_state_transition(&mut self, from: DegradeState, to: DegradeState, now_ns: u64) {
        self.stats.state_transitions += 1;
        match (from, to) {
            (DegradeState::Normal, DegradeState::Degrading) => {
                // 读取当前设定值并保存
                let setpoint = self
                    .degrade_engine
                    .protocol_mut()
                    .read_point(self.config.power_setpoint_point)
                    .ok()
                    .and_then(|dp| {
                        if let PointValue::Float(v) = dp.value {
                            Some(v)
                        } else {
                            None
                        }
                    });
                if let Some(v) = setpoint {
                    self.recovery.save_setpoint(v);
                }
            }
            (DegradeState::Degraded, DegradeState::Recovering) => {
                // 读取降级值，读取 Agent 设定值，启动过渡
                let degraded = self
                    .degrade_engine
                    .protocol_mut()
                    .read_point(self.config.power_cmd_point)
                    .ok()
                    .and_then(|dp| {
                        if let PointValue::Float(v) = dp.value {
                            Some(v)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0.0);
                let agent = self.recovery.saved_setpoint.unwrap_or(0.0);
                self.recovery.start_transition(degraded, agent, now_ns);
            }
            (DegradeState::Recovering, DegradeState::Normal) => {
                self.recovery.complete();
                self.stats.recovery_count += 1;
            }
            (_, DegradeState::Emergency) => {
                self.stats.emergency_count += 1;
            }
            _ => {
                // 其他转换（Degrading→Degraded, Recovering→Degraded）— 无特定动作
            }
        }
    }

    /// 喂狗辅助：喂 kernel + runtime 层，可选喂 agent 层.
    fn feed_layers(&mut self, now_ns: u64, include_agent: bool) {
        self.watchdog.feed_layer(self.kernel_layer, now_ns);
        self.watchdog.feed_layer(self.runtime_layer, now_ns);
        if include_agent {
            self.watchdog.feed_layer(self.agent_layer, now_ns);
        }
    }

    /// 当前状态。
    pub fn state(&self) -> DegradeState {
        self.state
    }

    /// 累计统计引用。
    pub fn stats(&self) -> &FlowStats {
        &self.stats
    }

    /// 心跳监控器引用。
    pub fn heartbeat(&self) -> &HeartbeatWatcher {
        &self.heartbeat
    }

    /// 心跳监控器可变引用（供调用方注册心跳）。
    pub fn heartbeat_mut(&mut self) -> &mut HeartbeatWatcher {
        &mut self.heartbeat
    }

    /// 看门狗引用（供测试检查层级喂狗状态）。
    pub fn watchdog(&self) -> &Watchdog {
        &self.watchdog
    }

    /// 恢复管理器引用。
    pub fn recovery(&self) -> &RecoveryManager {
        &self.recovery
    }
}
