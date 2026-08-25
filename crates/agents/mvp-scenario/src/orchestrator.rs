//! MVP 编排器 — 统一调度 Energy/Market/Device 三个 Agent 协同完成储能自治场景.

use alloc::string::String;

use eneros_device_agent::DeviceAgent;
use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_market_agent::{AgentRuntime, EnergyAgent, MarketAgent};
#[cfg(any(test, feature = "mock"))]
use eneros_solver_core::mock::MockSolver;
use eneros_solver_core::solver::Solver;

use crate::error::MvpError;
use crate::revenue::RevenueComparator;
use crate::traditional_ems::TraditionalEms;

/// 判据数据质量（WP-53.3 必达③：报告必须携带数据来源指纹，
/// 使「假数据闭环 PASS」在结构上不可能）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataQuality {
    /// 从未收到市场报价.
    Never,
    /// 收到过报价但已超出新鲜度窗口.
    Stale,
    /// 报价新鲜.
    Fresh,
}

/// 判据数据来源指纹.
#[derive(Debug, Clone, Copy)]
pub struct ReportProvenance {
    /// 电价缓存样本数（96 时段预测；0 = 无数据）.
    pub price_samples: usize,
    /// 最近一次报价时刻（ms）；`None` = 从未收到.
    pub price_last_quote_ms: Option<u64>,
    /// 传统 EMS 基线 SOC（本次判据实际使用的值）.
    pub baseline_soc: f64,
    /// 本 tick 的电价数据质量.
    pub quality: DataQuality,
}

/// 单 tick 报告.
#[derive(Debug, Clone, Copy)]
pub struct MvpTickReport {
    /// tick 序号（从 1 开始）.
    pub tick: u64,
    /// 双脑 EMS 累计收益（毫元定点，ARC-003）.
    pub dual_brain_revenue: crate::revenue::MilliYuan,
    /// 传统 EMS 累计收益（毫元定点，ARC-003）.
    pub traditional_revenue: crate::revenue::MilliYuan,
    /// 收益提升百分比.
    pub improvement_pct: f64,
    /// 数据来源指纹（WP-53.3 必达③）。
    ///
    /// `quality != Fresh` 时收益对比未记录新样本——消费方不得据
    /// `improvement_pct` 下结论。
    pub provenance: ReportProvenance,
}

/// MVP 编排器.
///
/// 直接持有 `EnergyAgent` / `MarketAgent` / `DeviceAgent` 三个 Agent（D4：非 clone），
/// 在 `tick` 中按顺序执行：market.on_tick → 转发市场数据 → energy.on_tick →
/// 记录收益对比 → device.on_tick。
pub struct MvpOrchestrator<S: Solver> {
    /// 能源调度 Agent.
    pub energy_agent: EnergyAgent<S>,
    /// 市场数据 Agent.
    pub market_agent: MarketAgent,
    /// 设备管理 Agent.
    pub device_agent: DeviceAgent,
    /// 收益对比器（D12：合并 RevenueTracker/Comparator）.
    pub revenue_comparator: RevenueComparator,
    /// 传统 EMS 基准策略.
    pub traditional_ems: TraditionalEms,
    /// tick 计数.
    pub tick_count: u64,
    /// 是否在运行（`start` 后置 true，`stop` 后置 false）.
    pub running: bool,
    /// 传统 EMS 基线 SOC（0.0..=1.0）。默认 0.5 为**历史遗留值**
    /// （WP-53.3 必达③：生产判据必须传入实测 SOC）。
    pub baseline_soc: f64,
}

/// 传统 EMS 基线 SOC 的历史遗留默认值.
///
/// 蓝图 53 WP-53.3：仅作为向后兼容的构造默认保留；报告的
/// `ReportProvenance::baseline_soc` 会如实携带本值，生产必须
/// 经 [`MvpOrchestrator::set_baseline_soc`] 传入实测值。
pub const LEGACY_BASELINE_SOC: f64 = 0.5;

impl<S: Solver> MvpOrchestrator<S> {
    /// 默认构造（3 个 Agent 用 `new_default`）——仅测试/仿真（蓝图/20 R2-1：
    /// Mock 收编；生产必须经 [`Self::new`] 注入真实 solver/Agent）。
    ///
    /// 蓝图 40 WP-40.1/40.6：EnergyAgent 经 test-support 的受信任遥测 +
    /// 记录式唯一 dispatch 装配（Committed 回执——demo 决策链可走通）。
    #[allow(clippy::field_reassign_with_default)] // 44.2：测试夹具 default+覆写模式（语义等价）
    #[cfg(any(test, feature = "mock"))]
    pub fn new_default(now_ms: u64) -> MvpOrchestrator<MockSolver> {
        use eneros_energy_market_agent::agent_test_support::{
            RecordingDispatch, StubTelemetryProvider,
        };
        use eneros_energy_market_agent::EnergyAgent;
        let mut dispatch = RecordingDispatch::default();
        dispatch.result = Some(Ok(eneros_control_dispatch::DispatchReceipt {
            command_id: [1; 16],
            correlation_id: [2; 16],
            final_state: eneros_control_dispatch::DispatchState::Committed,
            state_path: alloc::vec![eneros_control_dispatch::DispatchState::Committed],
            committed_at_ms: Some(now_ms),
            note: None,
            latency_ms: 1,
        }));
        MvpOrchestrator::<MockSolver> {
            energy_agent: EnergyAgent::<MockSolver>::new(
                "energy-agent",
                ScheduleConfig::default(),
                now_ms,
                Box::new(eneros_dual_brain::DualBrainMockEngine::new()),
                MockSolver::new(),
                Box::new(dispatch),
                Box::new(StubTelemetryProvider::new(Some(
                    StubTelemetryProvider::full_base(),
                ))),
            ),
            market_agent: MarketAgent::new_default(now_ms),
            device_agent: DeviceAgent::new_default(now_ms),
            revenue_comparator: RevenueComparator::new(),
            traditional_ems: TraditionalEms::new(ScheduleConfig::default()),
            tick_count: 0,
            baseline_soc: LEGACY_BASELINE_SOC,
            running: false,
        }
    }

    /// 自定义 Agent 构造（生产入口：注入真实 solver 的 EnergyAgent 等）。
    ///
    /// 调用方提供 3 个已配置好的 Agent 与 `ScheduleConfig`（用于传统 EMS 基准）。
    pub fn new(
        energy: EnergyAgent<S>,
        market: MarketAgent,
        device: DeviceAgent,
        config: ScheduleConfig,
        _now_ms: u64,
    ) -> Self {
        Self {
            energy_agent: energy,
            market_agent: market,
            device_agent: device,
            revenue_comparator: RevenueComparator::new(),
            traditional_ems: TraditionalEms::new(config),
            tick_count: 0,
            baseline_soc: LEGACY_BASELINE_SOC,
            running: false,
        }
    }

    /// 启动编排器：依次调用 3 个 Agent 的 `on_start(now_ms)`，全部成功后 `running = true`。
    pub fn start(&mut self, now_ms: u64) -> Result<(), MvpError> {
        self.market_agent.on_start(now_ms).map_err(MvpError::from)?;
        self.energy_agent.on_start(now_ms).map_err(MvpError::from)?;
        self.device_agent.on_start(now_ms).map_err(MvpError::from)?;
        self.running = true;
        Ok(())
    }

    /// 设置传统 EMS 基线 SOC（生产判据必须传实测值；WP-53.3 必达③）。
    ///
    /// 越界值钳位到 `0.0..=1.0`。
    pub fn set_baseline_soc(&mut self, soc: f64) {
        self.baseline_soc = soc.clamp(0.0, 1.0);
    }

    /// 执行一个 tick 周期.
    ///
    /// 步骤（D9：通过 `AgentRuntime` trait 调用，不直接访问 coordinator）：
    /// 1. `market_agent.on_tick(now_ms)` — 接收市场数据并转发到自身 channel
    /// 2. 转发 `market_agent.market_channel` → `energy_agent.market_channel`
    /// 3. `energy_agent.on_tick(now_ms)` — 双脑决策
    /// 4. 若 `current_schedule` 为 `Some`，记录双脑收益 + 传统 EMS 收益
    /// 5. `device_agent.on_tick(now_ms)` — 设备状态采集与命令执行
    /// 6. `tick_count += 1`，返回 `MvpTickReport`
    pub fn tick(&mut self, now_ms: u64) -> Result<MvpTickReport, MvpError> {
        if !self.running {
            return Err(MvpError::NotRunning);
        }

        // Step 1: market on_tick（接收并转发到自身 channel）.
        self.market_agent.on_tick(now_ms).map_err(MvpError::from)?;

        // Step 2: 转发市场数据 market_agent.market_channel -> energy_agent.market_channel.
        // MarketChannel::send 返回 Result<(), AgentRuntimeError>，永不为 Err（缓冲满丢弃最旧），
        // 用 `let _ =` 忽略以避免在转发阶段引入错误传播.
        while let Some(data) = self.market_agent.market_channel.try_recv() {
            let _ = self.energy_agent.market_channel_mut().send(data);
        }

        // Step 2.5: 判据数据质量（WP-53.6 复核修正：判定前移——
        // 电价非新鲜时必须**拒绝调度**，而不只是不记收益）。
        let quality = match (
            self.market_agent.last_quote_ms,
            self.market_agent.effective_price(now_ms),
        ) {
            (_, Some(_)) => DataQuality::Fresh,
            (Some(_), None) => DataQuality::Stale,
            (None, None) => DataQuality::Never,
        };

        // Step 3: energy on_tick（双脑决策）——仅当报价新鲜。
        //
        // WP-53.3 必达②（复核收紧）：Stale/Never 时跳过整个决策 tick——
        // EnergyAgent 内部 current_price 会停留在默认值，若照常执行
        // coordinator 则用假电价产出调度并下发 dispatch（假数据闭环的
        // 决策侧）。拒绝调度 = 不进 on_tick，设备采集（Step 5）照常。
        if quality == DataQuality::Fresh {
            self.energy_agent.on_tick(now_ms).map_err(MvpError::from)?;

            // Step 4: 记录收益对比（双侧对称累计；必达③指纹随报告输出）。
            if let Some(schedule) = &self.energy_agent.current_schedule {
                self.revenue_comparator
                    .record_dual_brain(schedule.total_revenue_yuan);
                // D10：Energy Agent 内部自建状态，Orchestrator 不重复构建 SystemState.
                // D13：传统 EMS 接收基本类型，不依赖 SystemState.
                let trad = self
                    .traditional_ems
                    .schedule(self.energy_agent.current_price, self.baseline_soc);
                self.revenue_comparator
                    .record_traditional(trad.total_revenue_yuan);
            }
        }

        // Step 5: device on_tick（设备采集 + 命令执行）.
        self.device_agent.on_tick(now_ms).map_err(MvpError::from)?;

        // Step 6: tick_count += 1.
        self.tick_count += 1;

        Ok(MvpTickReport {
            tick: self.tick_count,
            dual_brain_revenue: self.revenue_comparator.dual_brain_total(),
            traditional_revenue: self.revenue_comparator.traditional_total(),
            improvement_pct: self.revenue_comparator.improvement_pct(),
            provenance: ReportProvenance {
                price_samples: self.market_agent.price_cache.len(),
                price_last_quote_ms: self.market_agent.last_quote_ms,
                baseline_soc: self.baseline_soc,
                quality,
            },
        })
    }

    /// 停止编排器：依次调用 3 个 Agent 的 `on_stop(now_ms)`，`running = false`。
    pub fn stop(&mut self, now_ms: u64) -> Result<(), MvpError> {
        self.running = false;
        self.market_agent.on_stop(now_ms).map_err(MvpError::from)?;
        self.energy_agent.on_stop(now_ms).map_err(MvpError::from)?;
        self.device_agent.on_stop(now_ms).map_err(MvpError::from)?;
        Ok(())
    }

    /// 生成收益对比报告（委托 `RevenueComparator`）.
    pub fn report(&self) -> String {
        self.revenue_comparator.report()
    }
}
