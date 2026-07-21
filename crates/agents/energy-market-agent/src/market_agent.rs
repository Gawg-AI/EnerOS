//! 市场数据 Agent — 从数据源接收并转发给 Energy Agent.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use eneros_agent::{AgentDescriptor, AgentState, AgentType};

use crate::error::AgentRuntimeError;
use crate::market::{MarketChannel, MarketDataSource, MockMarketSource};
use crate::runtime::{AgentRuntime, HeartbeatStatus};

/// 市场数据 Agent.
///
/// 从 `MarketDataSource` 接收市场数据，更新电价缓存，并通过 `market_channel`
/// 转发给 Energy Agent。
pub struct MarketAgent {
    /// Agent 描述符.
    pub descriptor: AgentDescriptor,
    /// 市场数据源.
    pub source: Box<dyn MarketDataSource>,
    /// 市场数据通道（转发给 Energy Agent）.
    pub market_channel: MarketChannel,
    /// 电价缓存（96 时段）.
    pub price_cache: Vec<f64>,
    /// 生命周期状态.
    pub state: AgentState,
    /// tick 计数.
    pub tick_count: u64,
}

impl MarketAgent {
    /// 构造 Market Agent.
    ///
    /// `descriptor = AgentDescriptor::new(AgentType::Market, name, now_ms)`（D7）。
    /// `price_cache = vec![0.5; 96]` 初始化。
    pub fn new(name: &str, source: Box<dyn MarketDataSource>, now_ms: u64) -> Self {
        Self {
            descriptor: AgentDescriptor::new(AgentType::Market, name, now_ms),
            source,
            market_channel: MarketChannel::new(16),
            price_cache: vec![0.5; 96],
            state: AgentState::Created,
            tick_count: 0,
        }
    }

    /// 默认构造（使用 `MockMarketSource::new()`）.
    pub fn new_default(now_ms: u64) -> Self {
        let source: Box<dyn MarketDataSource> = Box::new(MockMarketSource::new());
        Self::new("market-agent", source, now_ms)
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

    fn on_tick(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        match self.source.recv_nonblocking()? {
            Some(data) => {
                self.price_cache = data.price_forecast.clone();
                self.market_channel.send(data)?;
            }
            None => {
                // 无数据：使用缓存电价，正常返回
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
