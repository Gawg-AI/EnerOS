//! 市场数据结构 + Agent 间通信通道 + 数据源抽象（D4/D5/D13）.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::error::AgentRuntimeError;

/// 市场信号类型.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketSignal {
    /// 实时电价.
    RealtimePrice,
    /// 日前预测.
    DayAheadForecast,
    /// 需求响应.
    DemandResponse,
    /// 紧急调度.
    EmergencyDispatch,
}

/// 市场数据（D13：派生 serde 用于 JSON 解析）.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketData {
    /// 时间戳（ms）.
    pub timestamp: u64,
    /// 电价预测（96 时段，15min/段，未来 24h）.
    pub price_forecast: Vec<f64>,
    /// 当前电价（元/kWh）.
    pub current_price: f64,
    /// 负荷预测（kW，可选）.
    pub load_forecast: Option<Vec<f64>>,
    /// 信号类型.
    pub signal_type: MarketSignal,
}

/// Agent 间通信通道（D4：Vec-backed 非阻塞发送/接收）.
///
/// 蓝图 `ChannelReceiver`/`ChannelSender` 不存在，本地定义简单 Vec-backed 实现。
/// 缓冲满时丢弃最旧数据（蓝图 §8.3）。
pub struct MarketChannel {
    buffer: Vec<MarketData>,
    capacity: usize,
}

impl MarketChannel {
    /// 创建通道（指定容量）.
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::new(),
            capacity,
        }
    }

    /// 非阻塞发送；缓冲满时丢弃最旧数据.
    ///
    /// `capacity == 0` 时静默丢弃数据并返回 `Ok`。
    pub fn send(&mut self, data: MarketData) -> Result<(), AgentRuntimeError> {
        if self.capacity == 0 {
            return Ok(());
        }
        if self.buffer.len() >= self.capacity {
            self.buffer.remove(0);
        }
        self.buffer.push(data);
        Ok(())
    }

    /// 非阻塞接收；无数据返回 `None`.
    pub fn try_recv(&mut self) -> Option<MarketData> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(self.buffer.remove(0))
        }
    }

    /// 缓冲是否为空.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// 缓冲数据量.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }
}

/// 市场数据源抽象（D5：`TcpConnection` 不存在，本地定义 trait）.
pub trait MarketDataSource {
    /// 非阻塞接收；无数据返回 `Ok(None)`.
    fn recv_nonblocking(&mut self) -> Result<Option<MarketData>, AgentRuntimeError>;
}

/// Mock 市场数据源（预加载数据队列）.
pub struct MockMarketSource {
    data: VecDeque<MarketData>,
}

impl MockMarketSource {
    /// 创建空 source.
    pub fn new() -> Self {
        Self {
            data: VecDeque::new(),
        }
    }

    /// 预加载数据.
    pub fn with_data(data: Vec<MarketData>) -> Self {
        Self {
            data: data.into_iter().collect(),
        }
    }

    /// 追加数据到队尾.
    pub fn push(&mut self, data: MarketData) {
        self.data.push_back(data);
    }
}

impl Default for MockMarketSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketDataSource for MockMarketSource {
    fn recv_nonblocking(&mut self) -> Result<Option<MarketData>, AgentRuntimeError> {
        Ok(self.data.pop_front())
    }
}
