//! HeartbeatWatcher — 单 Agent 心跳监控器（D2/D3）.
//!
//! [`HeartbeatWatcher`] 追踪最后心跳时间与连续超时次数，提供
//! [`HeartbeatStatus`] 状态查询。所有时间参数由调用方注入（D3：拒绝
//! `MonotonicTime::now()`）。
//!
//! # D2：本地轻量实现
//!
//! 不复用 v0.37.0 `HeartbeatMonitor`（多 Agent + 重依赖 `eneros-agent`），
//! 本版本在本地实现单 Agent 心跳监控，逻辑参考 v0.37.0 `check()` 算法。

/// 心跳状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatStatus {
    /// 心跳正常（在周期内收到心跳）。
    Alive,
    /// 心跳超时（连续超时次数，未达 Dead 阈值）。
    Timeout(u8),
    /// 心跳死亡（连续超时次数达阈值）。
    Dead,
}

/// 单 Agent 心跳监控器.
///
/// 追踪 `last_heartbeat_ns` 与 `timeout_count`，通过 `check(now_ns)` 返回
/// [`HeartbeatStatus`]。所有时间由调用方注入（D3）。
#[derive(Debug, Clone)]
pub struct HeartbeatWatcher {
    /// 心跳周期（纳秒）。
    pub heartbeat_period_ns: u64,
    /// 最后心跳时间（纳秒）。
    pub last_heartbeat_ns: u64,
    /// 连续超时次数。
    pub timeout_count: u8,
    /// 超时阈值（达到则判定 Dead）。
    pub max_timeout: u8,
    /// Agent 是否存活。
    pub agent_alive: bool,
}

impl HeartbeatWatcher {
    /// 创建心跳监控器.
    ///
    /// 初始 `agent_alive = true`，`last_heartbeat_ns = 0`（D3：调用方需在
    /// 首次 `check` 前调用 `on_heartbeat` 注入初始时间）。
    pub fn new(heartbeat_period_ns: u64, max_timeout: u8) -> Self {
        Self {
            heartbeat_period_ns,
            last_heartbeat_ns: 0,
            timeout_count: 0,
            max_timeout,
            agent_alive: true,
        }
    }

    /// 收到心跳（D3：`now_ns` 注入）.
    ///
    /// 更新 `last_heartbeat_ns = now_ns`，重置 `timeout_count = 0`，
    /// 设 `agent_alive = true`。
    pub fn on_heartbeat(&mut self, now_ns: u64) {
        self.last_heartbeat_ns = now_ns;
        self.timeout_count = 0;
        self.agent_alive = true;
    }

    /// 检查心跳状态（D3：`now_ns` 注入）.
    ///
    /// 若 `now_ns - last_heartbeat_ns > heartbeat_period_ns`：
    /// - `timeout_count += 1`
    /// - 若 `timeout_count >= max_timeout`，返回 [`HeartbeatStatus::Dead`]，设 `agent_alive = false`
    /// - 否则返回 [`HeartbeatStatus::Timeout(count)`]
    ///
    /// 若未超时：重置 `timeout_count = 0`，返回 [`HeartbeatStatus::Alive`]。
    pub fn check(&mut self, now_ns: u64) -> HeartbeatStatus {
        let elapsed = now_ns.saturating_sub(self.last_heartbeat_ns);
        if elapsed > self.heartbeat_period_ns {
            self.timeout_count = self.timeout_count.saturating_add(1);
            if self.timeout_count >= self.max_timeout {
                self.agent_alive = false;
                HeartbeatStatus::Dead
            } else {
                HeartbeatStatus::Timeout(self.timeout_count)
            }
        } else {
            self.timeout_count = 0;
            HeartbeatStatus::Alive
        }
    }

    /// Agent 是否存活。
    pub fn is_alive(&self) -> bool {
        self.agent_alive
    }
}
