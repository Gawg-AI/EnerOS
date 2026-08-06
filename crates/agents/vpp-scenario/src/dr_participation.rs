//! DR（需求响应）参与：DrEvent / DrState 数据模型 + DrHandler 状态机.
//!
//! 状态机迁移：Idle → Received → Dispatched → Acked → Completed；
//! Received / Dispatched 状态超 deadline_ms（D5：响应时限预算）→ Expired。

/// DR 事件描述.
#[derive(Debug, Clone, PartialEq)]
pub struct DrEvent {
    /// 事件唯一标识
    pub event_id: u32,
    /// 事件时间戳（毫秒）
    pub timestamp_ms: u64,
    /// 目标调节功率（kW），负值表示降载
    pub target_kw: f64,
    /// 持续时长（分钟）
    pub duration_min: u32,
    /// 响应时限预算（毫秒，D5：以代码语义为准，非绝对截止时间）
    pub deadline_ms: u64,
    /// 响应奖励（元）
    pub reward: f64,
}

/// DR 事件状态机状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrState {
    /// 空闲：无进行中事件
    Idle,
    /// 已接收：事件已到达，尚未调度
    Received,
    /// 已调度：调度计划（node_id→share_kw）已生成并下发
    Dispatched,
    /// 已确认：全部节点确认执行
    Acked,
    /// 已完成：事件执行完毕并结算奖励
    Completed,
    /// 已过期：超过 deadline_ms 未确认完成
    Expired,
}

/// DR 事件处理器：接收 → 调度 → 确认 → 完成 状态机.
///
/// `plan` 记录调度计划（node_id → share_kw，D6 偏差，容量 16）；
/// `acked` 跟踪已确认节点，计划内全部节点确认完毕 → Acked。
#[derive(Debug, Clone)]
pub struct DrHandler {
    /// 当前状态
    state: DrState,
    /// 当前进行中的 DR 事件
    current_event: Option<DrEvent>,
    /// 调度计划（node_id → share_kw，容量 16）
    plan: heapless::Vec<(u32, f64), 16>,
    /// 已确认节点集合（容量与 plan 一致，用于跟踪 ack 进度）
    acked: heapless::Vec<u32, 16>,
}

impl DrHandler {
    /// 创建处理器，初始状态 Idle.
    pub fn new() -> Self {
        Self {
            state: DrState::Idle,
            current_event: None,
            plan: heapless::Vec::new(),
            acked: heapless::Vec::new(),
        }
    }

    /// 接收 DR 事件 → Received.
    ///
    /// 非 Idle 状态重复接收新事件时：覆盖 current_event、清空 plan 与 acked、回 Received。
    pub fn receive(&mut self, event: DrEvent) {
        self.current_event = Some(event);
        self.plan.clear();
        self.acked.clear();
        self.state = DrState::Received;
    }

    /// 下发调度计划：仅 Received 状态接受.
    ///
    /// 接受时记录 plan、清空 acked → Dispatched，返回 `true`；
    /// 其他状态调用为无操作（保持现状），返回 `false`。
    pub fn dispatch(&mut self, plan: heapless::Vec<(u32, f64), 16>) -> bool {
        if self.state != DrState::Received {
            return false;
        }
        self.plan = plan;
        self.acked.clear();
        self.state = DrState::Dispatched;
        true
    }

    /// 节点确认：仅 Dispatched 状态有效.
    ///
    /// node_id 在 plan 内且未确认过 → 记入 acked；plan 内全部节点确认完毕 → Acked。
    /// 非计划内 node_id、重复确认、其他状态下的调用均被忽略。
    pub fn ack(&mut self, node_id: u32) {
        if self.state != DrState::Dispatched {
            return;
        }
        let in_plan = self.plan.iter().any(|&(id, _)| id == node_id);
        if !in_plan || self.acked.contains(&node_id) {
            return;
        }
        // plan 与 acked 容量均为 16 且 node_id ∈ plan 未确认过，push 不会失败
        let _ = self.acked.push(node_id);
        let all_acked = self.plan.iter().all(|&(id, _)| self.acked.contains(&id));
        if all_acked {
            self.state = DrState::Acked;
        }
    }

    /// 完成结算：仅 Acked 状态有效.
    ///
    /// → Completed 并返回 `Some(reward)`；其他状态返回 `None`。
    pub fn complete(&mut self) -> Option<f64> {
        if self.state != DrState::Acked {
            return None;
        }
        self.state = DrState::Completed;
        self.current_event.as_ref().map(|e| e.reward)
    }

    /// 超时检查：state 为 Received 或 Dispatched 且 elapsed_ms 超过事件 deadline_ms → Expired.
    pub fn check_timeout(&mut self, elapsed_ms: u64) {
        if !matches!(self.state, DrState::Received | DrState::Dispatched) {
            return;
        }
        if let Some(event) = &self.current_event {
            if elapsed_ms > event.deadline_ms {
                self.state = DrState::Expired;
            }
        }
    }

    /// 当前状态.
    pub fn state(&self) -> DrState {
        self.state
    }

    /// 当前事件（无事件时为 None）.
    pub fn current_event(&self) -> Option<&DrEvent> {
        self.current_event.as_ref()
    }

    /// 当前调度计划切片（node_id → share_kw）.
    pub fn plan(&self) -> &[(u32, f64)] {
        self.plan.as_slice()
    }

    /// 是否终态（Completed 或 Expired）.
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, DrState::Completed | DrState::Expired)
    }

    /// 复位：回 Idle，清空 current_event / plan / acked.
    pub fn reset(&mut self) {
        self.state = DrState::Idle;
        self.current_event = None;
        self.plan.clear();
        self.acked.clear();
    }
}

impl Default for DrHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试事件：target -300kW，deadline 30s，reward 128.5 元
    fn make_event() -> DrEvent {
        DrEvent {
            event_id: 1,
            timestamp_ms: 1_000,
            target_kw: -300.0,
            duration_min: 30,
            deadline_ms: 30_000,
            reward: 128.5,
        }
    }

    /// 构造 3 节点调度计划（按容量比例分配，总份额 ≈ -300kW）
    fn make_plan() -> heapless::Vec<(u32, f64), 16> {
        let mut plan = heapless::Vec::new();
        plan.push((1, -85.71)).unwrap();
        plan.push((2, -128.57)).unwrap();
        plan.push((3, -85.72)).unwrap();
        plan
    }

    /// 推进到 Dispatched 状态的辅助函数
    fn dispatched_handler() -> DrHandler {
        let mut h = DrHandler::new();
        h.receive(make_event());
        assert!(h.dispatch(make_plan()));
        h
    }

    // DRH1：完整状态机迁移 Idle→Received→Dispatched→Acked→Completed（3 节点全部 ack）
    #[test]
    fn drh1_full_state_machine_transition() {
        let mut h = DrHandler::new();
        assert_eq!(h.state(), DrState::Idle);
        assert!(!h.is_terminal());
        assert!(h.current_event().is_none());
        // 非 Received 状态 dispatch 拒绝
        assert!(!h.dispatch(make_plan()));
        assert_eq!(h.state(), DrState::Idle);

        h.receive(make_event());
        assert_eq!(h.state(), DrState::Received);
        assert_eq!(h.current_event(), Some(&make_event()));

        assert!(h.dispatch(make_plan()));
        assert_eq!(h.state(), DrState::Dispatched);
        assert_eq!(h.plan().len(), 3);

        h.ack(1);
        h.ack(2);
        assert_eq!(h.state(), DrState::Dispatched);
        h.ack(3);
        assert_eq!(h.state(), DrState::Acked);

        let reward = h.complete();
        assert_eq!(reward, Some(128.5));
        assert_eq!(h.state(), DrState::Completed);
        assert!(h.is_terminal());
    }

    // DRH2：complete 返回正确 reward 结算
    #[test]
    fn drh2_complete_returns_reward() {
        let mut event = make_event();
        event.reward = 256.75;
        let mut h = DrHandler::new();
        h.receive(event);
        assert!(h.dispatch(make_plan()));
        for &(id, _) in make_plan().iter() {
            h.ack(id);
        }
        assert_eq!(h.state(), DrState::Acked);
        assert_eq!(h.complete(), Some(256.75));
        // 非 Acked 状态 complete 返回 None
        assert_eq!(h.complete(), None);
    }

    // DRH3：部分 ack 不前进（仍 Dispatched）；非计划内 node_id 的 ack 被忽略
    #[test]
    fn drh3_partial_and_invalid_ack_ignored() {
        let mut h = dispatched_handler();
        h.ack(1);
        assert_eq!(h.state(), DrState::Dispatched);
        // 非计划内节点 → 忽略
        h.ack(99);
        assert_eq!(h.state(), DrState::Dispatched);
        // 重复 ack 同一节点 → 忽略
        h.ack(1);
        assert_eq!(h.state(), DrState::Dispatched);
        // 补齐其余计划内节点后仍可前进（验证非法 ack 未污染确认集合）
        h.ack(2);
        h.ack(3);
        assert_eq!(h.state(), DrState::Acked);
        // 非 Dispatched 状态下的 ack 被忽略
        h.ack(1);
        assert_eq!(h.state(), DrState::Acked);
    }

    // DRH4：全部 ack → Acked
    #[test]
    fn drh4_all_acked_transitions_to_acked() {
        let mut h = dispatched_handler();
        assert_eq!(h.state(), DrState::Dispatched);
        h.ack(1);
        h.ack(2);
        assert_eq!(h.state(), DrState::Dispatched);
        h.ack(3);
        assert_eq!(h.state(), DrState::Acked);
    }

    // DRH5：check_timeout 超 deadline → Expired（Received 与 Dispatched 两态）；未超时不迁移
    #[test]
    fn drh5_timeout_expires_received_and_dispatched() {
        // Received 状态：未超时不迁移，超时 → Expired
        let mut h = DrHandler::new();
        h.receive(make_event());
        h.check_timeout(30_000); // 等于 deadline 不视为超时
        assert_eq!(h.state(), DrState::Received);
        h.check_timeout(30_001); // 超过 deadline
        assert_eq!(h.state(), DrState::Expired);
        assert!(h.is_terminal());

        // Dispatched 状态：未超时不迁移，超时 → Expired
        let mut h = dispatched_handler();
        h.check_timeout(29_999);
        assert_eq!(h.state(), DrState::Dispatched);
        h.check_timeout(30_001);
        assert_eq!(h.state(), DrState::Expired);

        // 终态不再受超时检查影响
        h.check_timeout(u64::MAX);
        assert_eq!(h.state(), DrState::Expired);
    }

    // DRH6：reset 语义（回 Idle 且事件/计划/ack 清空）；重复 receive 覆盖语义
    #[test]
    fn drh6_reset_and_receive_overwrite() {
        // reset 清空一切回 Idle
        let mut h = dispatched_handler();
        h.ack(1);
        h.reset();
        assert_eq!(h.state(), DrState::Idle);
        assert!(h.current_event().is_none());
        assert!(h.plan().is_empty());
        assert!(!h.is_terminal());

        // 重复 receive 覆盖：旧事件被覆盖、旧计划与 ack 清空
        let mut h = dispatched_handler();
        h.ack(1);
        h.ack(2);
        let mut event2 = make_event();
        event2.event_id = 2;
        event2.reward = 64.0;
        h.receive(event2.clone());
        assert_eq!(h.state(), DrState::Received);
        assert_eq!(h.current_event(), Some(&event2));
        assert!(h.plan().is_empty());
        // 旧 ack 已清空：新计划需重新全部确认
        let mut plan2: heapless::Vec<(u32, f64), 16> = heapless::Vec::new();
        plan2.push((7, -50.0)).unwrap();
        assert!(h.dispatch(plan2));
        h.ack(1); // 旧计划节点，已不在新计划内 → 忽略
        assert_eq!(h.state(), DrState::Dispatched);
        h.ack(7);
        assert_eq!(h.state(), DrState::Acked);
        assert_eq!(h.complete(), Some(64.0));
    }
}
