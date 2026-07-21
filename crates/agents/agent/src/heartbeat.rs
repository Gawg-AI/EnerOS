//! Agent 心跳监控 — HeartbeatMonitor / HeartbeatState
//!
//! # 设计
//! - `HeartbeatMonitor` 维护 `BTreeMap<AgentId, HeartbeatState>`（D1：非 HashMap，零依赖）
//! - 心跳协议：1s 周期、3 次超时 = 故障（`DEFAULT_INTERVAL_MS` / `DEFAULT_MAX_MISSED`）
//! - `check()` 算法：`elapsed > interval` 时计算 `missed_count`，达阈值设 `Unhealthy`（D7：不设 Dead）
//! - `register()` 接受 `now: u64` 参数（D2：no_std 无系统时钟）
//!
//! # 偏差声明
//! - D1: 使用 `BTreeMap`（非蓝图的 `HashMap`），零外部依赖
//! - D2: `register()` 追加 `now: u64`（no_std 时间约定，无 `crate::time::now_ms()`）
//! - D3: `HealthStatus` derive Clone/Copy/Debug/PartialEq/Eq
//! - D4: `HeartbeatState` derive Clone/Debug，`HeartbeatMonitor` derive Debug
//! - D5: 新增 2 个 `AgentError` 变体（HeartbeatTimeout / AgentUnhealthy）
//! - D6: 独立监控器（不引用 registry/lifecycle，v0.38.0 集成）
//! - D7: `check()` 设 `Unhealthy` 而非 `Dead`（Dead 由 v0.38.0 设置）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::health::HealthStatus;
use crate::id::AgentId;

/// 默认心跳间隔（1 秒）.
const DEFAULT_INTERVAL_MS: u64 = 1000;

/// 默认最大缺失次数（3 次超时 = 故障）.
const DEFAULT_MAX_MISSED: u32 = 3;

/// Agent 心跳状态.
///
/// 每个 Agent 对应一个 `HeartbeatState`，记录最后心跳时间、缺失次数、健康状态与 per-Agent 间隔。
#[derive(Clone, Debug)]
pub struct HeartbeatState {
    /// 最后心跳时间戳（由外部提供，no_std 无系统时钟）
    pub last_heartbeat: u64,
    /// 缺失心跳数
    pub missed_count: u32,
    /// 当前健康状态
    pub status: HealthStatus,
    /// 该 Agent 的心跳间隔（毫秒），默认为 `default_interval_ms`，可通过 `set_interval` 覆盖
    pub interval_ms: u64,
}

/// Agent 心跳监控器.
///
/// 维护所有已注册 Agent 的心跳状态，提供注册、心跳记录、健康检查与查询能力。
/// 独立于 `AgentRegistry` 与 `LifecycleManager`（D6 偏差），v0.38.0 将集成。
#[derive(Debug)]
pub struct HeartbeatMonitor {
    agents: BTreeMap<AgentId, HeartbeatState>,
    default_interval_ms: u64,
    max_missed: u32,
}

impl HeartbeatMonitor {
    /// 创建心跳监控器.
    ///
    /// # 参数
    /// * `interval_ms` - 默认心跳间隔（毫秒）
    /// * `max_missed` - 最大缺失次数（达到即判定 Unhealthy）
    pub fn new(interval_ms: u64, max_missed: u32) -> Self {
        HeartbeatMonitor {
            agents: BTreeMap::new(),
            default_interval_ms: interval_ms,
            max_missed,
        }
    }

    /// 使用默认参数创建心跳监控器（`DEFAULT_INTERVAL_MS` / `DEFAULT_MAX_MISSED`）.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED)
    }

    /// 注册 Agent（D2 偏差：追加 `now` 参数）.
    ///
    /// 用 `now` 初始化 `last_heartbeat`，状态设为 `Healthy`，间隔设为 `default_interval_ms`。
    pub fn register(&mut self, id: AgentId, now: u64) {
        self.agents.insert(
            id,
            HeartbeatState {
                last_heartbeat: now,
                missed_count: 0,
                status: HealthStatus::Healthy,
                interval_ms: self.default_interval_ms,
            },
        );
    }

    /// 记录心跳.
    ///
    /// 更新 `last_heartbeat` 为 `timestamp`，重置 `missed_count` 为 0，状态设为 `Healthy`。
    /// 若 Agent 未注册，静默忽略。
    pub fn heartbeat(&mut self, id: AgentId, timestamp: u64) {
        if let Some(state) = self.agents.get_mut(&id) {
            state.last_heartbeat = timestamp;
            state.missed_count = 0;
            state.status = HealthStatus::Healthy;
        }
    }

    /// 检查所有 Agent 健康状态.
    ///
    /// 对每个已注册 Agent：
    /// 1. `elapsed = now.saturating_sub(state.last_heartbeat)` — 防溢出（时钟回拨）
    /// 2. 若 `elapsed > state.interval_ms`：
    ///    - `missed_count = (elapsed / state.interval_ms) as u32`
    ///    - 若 `missed_count >= max_missed` → `Unhealthy`（D7：不设 Dead）
    ///    - 否则若 `missed_count > 0` → `Degraded`
    /// 3. 返回 `(id, status)` 列表
    pub fn check(&mut self, now: u64) -> Vec<(AgentId, HealthStatus)> {
        let mut results = Vec::new();
        for (&id, state) in self.agents.iter_mut() {
            let elapsed = now.saturating_sub(state.last_heartbeat);
            if elapsed > state.interval_ms {
                state.missed_count = (elapsed / state.interval_ms) as u32;
                if state.missed_count >= self.max_missed {
                    state.status = HealthStatus::Unhealthy;
                } else if state.missed_count > 0 {
                    state.status = HealthStatus::Degraded;
                }
            }
            results.push((id, state.status));
        }
        results
    }

    /// 查询指定 Agent 是否健康.
    ///
    /// 返回 `true` 当且仅当 Agent 已注册且状态为 `Healthy`。
    pub fn is_healthy(&self, id: AgentId) -> bool {
        self.agents
            .get(&id)
            .map(|s| matches!(s.status, HealthStatus::Healthy))
            .unwrap_or(false)
    }

    /// 设置 per-Agent 心跳间隔（蓝图 §4.2 / §9.5 可维护）.
    ///
    /// 若 Agent 未注册，静默忽略。
    pub fn set_interval(&mut self, id: AgentId, interval_ms: u64) {
        if let Some(state) = self.agents.get_mut(&id) {
            state.interval_ms = interval_ms;
        }
    }

    /// 注销 Agent（蓝图 §4.2）.
    ///
    /// 移除该 Agent 的心跳状态，后续 `check` 不再返回该 Agent。
    pub fn unregister(&mut self, id: AgentId) {
        self.agents.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentId;

    /// 辅助：从 check() 结果中提取指定 Agent 的状态.
    fn status_of(results: &[(AgentId, HealthStatus)], id: AgentId) -> Option<HealthStatus> {
        results.iter().find(|(i, _)| *i == id).map(|(_, s)| *s)
    }

    #[test]
    fn test_new_defaults() {
        let m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        // 空监控器：任何 id 都不健康，且不 panic.
        assert!(!m.is_healthy(AgentId::ZERO));
    }

    #[test]
    fn test_register_agent() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        assert!(m.is_healthy(id));
    }

    #[test]
    fn test_heartbeat_updates_state() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        // elapsed=1500 > 1000 → missed=1 → Degraded
        let results = m.check(2500);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Degraded));
        // heartbeat 重置状态为 Healthy
        m.heartbeat(id, 2600);
        assert!(m.is_healthy(id));
    }

    #[test]
    fn test_check_healthy_no_missed() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        // elapsed=500 <= 1000，状态保持 Healthy
        let results = m.check(1500);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Healthy));
    }

    #[test]
    fn test_check_degraded_one_missed() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        // elapsed=1500 > 1000 → missed=1 → Degraded
        let results = m.check(2500);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Degraded));
    }

    #[test]
    fn test_check_unhealthy_max_missed() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        // elapsed=3500 → missed=3 >= 3 → Unhealthy
        let results = m.check(4500);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Unhealthy));
    }

    #[test]
    fn test_check_unhealthy_exceeds_max() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        // elapsed=9000 → missed=9 >= 3 → Unhealthy
        let results = m.check(10000);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Unhealthy));
    }

    #[test]
    fn test_is_healthy_unregistered() {
        let m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        assert!(!m.is_healthy(AgentId::generate()));
    }

    #[test]
    fn test_is_healthy_degraded() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        m.check(2500); // → Degraded
        assert!(!m.is_healthy(id));
    }

    #[test]
    fn test_set_interval_override() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        m.set_interval(id, 500);
        // elapsed=750 > 500 → missed=1 → Degraded（per-Agent 间隔生效）
        let results = m.check(1750);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Degraded));
    }

    #[test]
    fn test_unregister_removes_agent() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        m.unregister(id);
        let results = m.check(2000);
        assert!(results.iter().all(|(i, _)| *i != id));
    }

    #[test]
    fn test_check_empty_monitor() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let results = m.check(1000);
        assert!(results.is_empty());
    }

    #[test]
    fn test_check_multiple_agents() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id1 = AgentId::generate();
        let id2 = AgentId::generate();
        let id3 = AgentId::generate();
        m.register(id1, 1000);
        m.register(id2, 1100);
        m.register(id3, 1200);
        let results = m.check(1500);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_check_multiple_agents_independent() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id_a = AgentId::generate();
        let id_b = AgentId::generate();
        m.register(id_a, 1000);
        m.register(id_b, 1000);
        // id_a 发送心跳，id_b 不发送
        m.heartbeat(id_a, 2500);
        let results = m.check(2500);
        // id_a: elapsed=0 → Healthy
        assert_eq!(status_of(&results, id_a), Some(HealthStatus::Healthy));
        // id_b: elapsed=1500 > 1000 → Degraded 或 Unhealthy（非 Healthy）
        assert_ne!(status_of(&results, id_b), Some(HealthStatus::Healthy));
    }

    #[test]
    fn test_clock_rollback_saturating() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 2000);
        // 时钟回拨：elapsed = 1000.saturating_sub(2000) = 0，0 <= 1000，保持 Healthy
        let results = m.check(1000);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Healthy));
    }

    #[test]
    fn test_heartbeat_resets_missed_count() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        // elapsed=2500 → missed=2 → Degraded
        let results = m.check(3500);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Degraded));
        // heartbeat 重置 missed_count=0，status=Healthy
        m.heartbeat(id, 3600);
        let results = m.check(3600);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Healthy));
    }

    #[test]
    fn test_check_boundary_exact_interval() {
        let mut m = HeartbeatMonitor::new(DEFAULT_INTERVAL_MS, DEFAULT_MAX_MISSED);
        let id = AgentId::generate();
        m.register(id, 1000);
        // 边界：elapsed=1000，NOT > 1000（严格大于），保持 Healthy
        let results = m.check(2000);
        assert_eq!(status_of(&results, id), Some(HealthStatus::Healthy));
    }
}
