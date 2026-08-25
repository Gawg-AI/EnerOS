//! 市场数据 Agent — 从数据源接收并转发给 Energy Agent.

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_agent::{AgentDescriptor, AgentState, AgentType};

use crate::error::AgentRuntimeError;
#[cfg(any(test, feature = "mock"))]
use crate::market::MockMarketSource;
use crate::market::{MarketChannel, MarketDataSource};
use crate::runtime::{AgentRuntime, HeartbeatStatus};

/// 默认电价新鲜度窗口（ms）：超过该时长未收到报价即视为断供.
///
/// 蓝图 53 WP-53.3 必达②：断源必须显式降级（拒绝调度并告警），
/// 禁止静默沿用缓存电价继续计算收益。
pub const DEFAULT_PRICE_STALE_MS: u64 = 60_000;

/// 市场数据 Agent.
///
/// 从 `MarketDataSource` 接收市场数据，更新电价缓存，并通过 `market_channel`
/// 转发给 Energy Agent。
///
/// 蓝图 53 WP-53.3：`price_cache` 初始为**空**（不再有 0.5 常量兜底）；
/// 从未收到报价或报价超时（[`Self::price_stale_ms`]）时
/// [`Self::effective_price`] 返回 `None`，消费方必须显式降级。
pub struct MarketAgent {
    /// Agent 描述符.
    pub descriptor: AgentDescriptor,
    /// 市场数据源.
    pub source: Box<dyn MarketDataSource>,
    /// 市场数据通道（转发给 Energy Agent）.
    pub market_channel: MarketChannel,
    /// 电价缓存（96 时段预测；初始为空——无报价即无有效电价）.
    pub price_cache: Vec<f64>,
    /// 最近一次收到报价的时刻（ms）；`None` = 从未收到.
    pub last_quote_ms: Option<u64>,
    /// 报价是否已过期（进入过期态时 `stale_notices` 自增一次告警）.
    pub stale: bool,
    /// 报价过期告警计数（每次从新鲜→过期转换 +1）.
    pub stale_notices: u64,
    /// 电价新鲜度窗口（ms）.
    pub price_stale_ms: u64,
    /// 生命周期状态.
    pub state: AgentState,
    /// tick 计数.
    pub tick_count: u64,
    /// WP-53.6 复核修正：最近一次报价的 **current_price**（实时价）。
    ///
    /// 下游 EnergyAgent 消费的字段是 `MarketData.current_price`
    /// （energy_agent.rs:256），而非 forecast 首元素——effective_price
    /// 必须与之同源，否则语义错位。
    pub last_current_price: Option<f64>,
}

impl MarketAgent {
    /// 构造 Market Agent.
    ///
    /// `descriptor = AgentDescriptor::new(AgentType::Market, name, now_ms)`（D7）。
    /// `price_cache` 初始为空；电价有效性由 [`Self::effective_price`] 判定。
    pub fn new(name: &str, source: Box<dyn MarketDataSource>, now_ms: u64) -> Self {
        Self {
            descriptor: AgentDescriptor::new(AgentType::Market, name, now_ms),
            source,
            market_channel: MarketChannel::new(16),
            price_cache: Vec::new(),
            last_quote_ms: None,
            stale: false,
            stale_notices: 0,
            price_stale_ms: DEFAULT_PRICE_STALE_MS,
            last_current_price: None,
            state: AgentState::Created,
            tick_count: 0,
        }
    }

    /// 默认构造（使用 `MockMarketSource::new()`）——仅测试/仿真
    /// （蓝图/20 R2-1：生产必须注入真实数据源）。
    #[cfg(any(test, feature = "mock"))]
    pub fn new_default(now_ms: u64) -> Self {
        let source: Box<dyn MarketDataSource> = Box::new(MockMarketSource::new());
        Self::new("market-agent", source, now_ms)
    }

    /// 当前有效电价（元/kWh）：无报价、缓存为空或报价已过期 → `None`.
    ///
    /// WP-53.6 复核修正：返回最新报价的 **`current_price`**（与下游
    /// EnergyAgent 的消费字段同源），而非 forecast 首元素。
    /// 蓝图 53 WP-53.3 必达②：调用方拿到 `None` 必须显式降级，
    /// 禁止回退到任何常量价格继续调度/计收益。
    pub fn effective_price(&self, now_ms: u64) -> Option<f64> {
        if self.last_current_price.is_none() || self.price_cache.is_empty() {
            return None;
        }
        match self.last_quote_ms {
            Some(t) if now_ms.saturating_sub(t) <= self.price_stale_ms => self.last_current_price,
            _ => None,
        }
    }

    /// 获取市场通道可变引用（供测试读取 Energy Agent 接收的数据）.
    pub fn market_channel_mut(&mut self) -> &mut MarketChannel {
        &mut self.market_channel
    }
}

impl AgentRuntime for MarketAgent {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    fn on_start(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.state = AgentState::Running;
        Ok(())
    }

    fn on_tick(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError> {
        match self.source.recv_nonblocking()? {
            Some(data) => {
                self.price_cache = data.price_forecast.clone();
                self.last_current_price = Some(data.current_price);
                self.last_quote_ms = Some(now_ms);
                // 新报价无条件恢复新鲜态（过期→新鲜的转换不产生告警）。
                self.stale = false;
                self.market_channel.send(data)?;
            }
            None => {
                // 无新数据：若报价已超出新鲜度窗口，显式进入过期态并告警一次
                // （WP-53.3 必达②：禁止静默沿用缓存电价）。
                if !self.stale {
                    let expired = match self.last_quote_ms {
                        Some(t) => {
                            now_ms.saturating_sub(t) > self.price_stale_ms
                                && !self.price_cache.is_empty()
                        }
                        None => false,
                    };
                    if expired {
                        self.stale = true;
                        self.stale_notices += 1;
                    }
                }
            }
        }
        self.tick_count += 1;
        Ok(())
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
