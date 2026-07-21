//! MVP 编排器 — 统一调度 Energy/Market/Device 三个 Agent 协同完成储能自治场景.

use alloc::string::String;

use eneros_device_agent::DeviceAgent;
use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_market_agent::{AgentRuntime, EnergyAgent, MarketAgent};

use crate::error::MvpError;
use crate::revenue::RevenueComparator;
use crate::traditional_ems::TraditionalEms;

/// 单 tick 报告.
#[derive(Debug, Clone, Copy)]
pub struct MvpTickReport {
    /// tick 序号（从 1 开始）.
    pub tick: u64,
    /// 双脑 EMS 累计收益（元）.
    pub dual_brain_revenue: f64,
    /// 传统 EMS 累计收益（元）.
    pub traditional_revenue: f64,
    /// 收益提升百分比.
    pub improvement_pct: f64,
}

/// MVP 编排器.
///
/// 直接持有 `EnergyAgent` / `MarketAgent` / `DeviceAgent` 三个 Agent（D4：非 clone），
/// 在 `tick` 中按顺序执行：market.on_tick → 转发市场数据 → energy.on_tick →
/// 记录收益对比 → device.on_tick。
pub struct MvpOrchestrator {
    /// 能源调度 Agent.
    pub energy_agent: EnergyAgent,
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
}

impl MvpOrchestrator {
    /// 默认构造：3 个 Agent 用 `new_default`，`TraditionalEms` 用 `ScheduleConfig::default()`，
    /// `RevenueComparator::new()`，`running = false`，`tick_count = 0`。
    pub fn new_default(now_ms: u64) -> Self {
        Self {
            energy_agent: EnergyAgent::new_default(now_ms),
            market_agent: MarketAgent::new_default(now_ms),
            device_agent: DeviceAgent::new_default(now_ms),
            revenue_comparator: RevenueComparator::new(),
            traditional_ems: TraditionalEms::new(ScheduleConfig::default()),
            tick_count: 0,
            running: false,
        }
    }

    /// 自定义 Agent 构造.
    ///
    /// 调用方提供 3 个已配置好的 Agent 与 `ScheduleConfig`（用于传统 EMS 基准）。
    pub fn new(
        energy: EnergyAgent,
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

        // Step 3: energy on_tick（双脑决策）.
        self.energy_agent.on_tick(now_ms).map_err(MvpError::from)?;

        // Step 4: 记录收益对比.
        if let Some(schedule) = &self.energy_agent.current_schedule {
            self.revenue_comparator
                .record_dual_brain(schedule.total_revenue_yuan);
            // D10：Energy Agent 内部自建状态，Orchestrator 不重复构建 SystemState.
            // D13：传统 EMS 接收基本类型，不依赖 SystemState.
            let trad = self
                .traditional_ems
                .schedule(self.energy_agent.current_price, 0.5);
            self.revenue_comparator
                .record_traditional(trad.total_revenue_yuan);
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
