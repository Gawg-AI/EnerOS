//! 三级仲裁器 — [`DomainArbiter`] / [`ArbiterPolicy`] / [`ArbitrationRequest`]
//! / [`ArbitrationResult`] / [`ArbitrationReason`]（v0.92.0）.
//!
//! "竞价为主 + 安全底线"三级仲裁（蓝图 §7.3），优先级不可逾越：
//!
//! ```text
//! 安全(safety_critical, 按 Priority 最高) > deadline 紧急(最早 deadline) > 竞价(cmp_bid 最高)
//! ```
//!
//! - **D2**：`resource_id` 为 `&'static str`（无堆分配）。
//! - **D3**：`deadline` / `timestamp` 统一 `u64` ms，外部时间注入。
//! - **D5**：确定性仲裁——同 priority / 同 deadline / 同 bid 取**输入序首个**（手写循环 + 严格比较）。
//! - **D6**：`timestamp` 一律回显调用方传入的 `now_ms`。
//! - **D8**：[`ArbitrationResult`] 携带 `reason` + `conflict` 标记（仲裁可解释性）。
//! - **D9**：6 个计数器字段全 `pub`（仲裁路径可观测）。
//! - **D10**：`safety_critical` 请求数 ≥ 2 → `conflict = true`（仅标记与计数，不阻断仲裁）。
//! - **D11**：竞价分支经 [`cmp_bid`] 全序比较，NaN 恒最低（全 NaN 时首个胜出）。
//! - **D12**：urgent 窗口默认 1000ms（[`ArbiterPolicy::default`]），判定见 [`Claim::is_urgent`]。

use alloc::vec::Vec;

use eneros_agent::AgentId;

use crate::bid::{cmp_bid, Claim};

/// 仲裁请求（一次域内资源竞争；D2/D3）.
#[derive(Debug, Clone)]
pub struct ArbitrationRequest {
    /// 资源标识（D2：&'static str，如 `"pcc"` 并网点）.
    pub resource_id: &'static str,
    /// 竞争该资源的请求列表（按输入序处理，首个最大胜出保证确定性，D5）.
    pub claimants: Vec<Claim>,
    /// 请求级截止时间（u64 ms，D3；本版本仅记录不参与仲裁判定）.
    pub deadline: u64,
}

/// 仲裁原因（三级仲裁路径标记，D8）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbitrationReason {
    /// 安全第一级：`safety_critical` 请求中 [`crate::Priority`] 最高者胜出.
    SafetyFirst,
    /// 竞价第三级（常态主路径）：无安全/紧急请求，`bid` 最高者胜出.
    HighestBid,
    /// deadline 第二级：紧急请求（[`Claim::is_urgent`]）中最早 deadline 者胜出.
    Deadline,
    /// 默认（空请求列表，无胜出者）.
    Default,
}

/// 仲裁结果（D8/D10）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArbitrationResult {
    /// 胜出 Agent ID；空请求列表 → `None`.
    pub winner: Option<AgentId>,
    /// 仲裁原因（命中的仲裁级）.
    pub reason: ArbitrationReason,
    /// 仲裁时刻时间戳（u64 ms，回显传入的 `now_ms`，D6）.
    pub timestamp: u64,
    /// 安全冲突标记（safety_critical 请求数 ≥ 2，D10；仅安全级可能为 true）.
    pub conflict: bool,
}

/// 仲裁策略（D12）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArbiterPolicy {
    /// urgent 判定窗口（ms）：`deadline < now + window` 视为紧急（严格 <，D12）.
    pub urgent_window_ms: u64,
}

impl Default for ArbiterPolicy {
    /// 默认策略：urgent 窗口 1000ms（D12）.
    fn default() -> Self {
        Self {
            urgent_window_ms: 1000,
        }
    }
}

/// 域内仲裁器（D4：无 Send + Sync 约束；D9：计数器全 pub 可观测）.
#[derive(Debug, Clone)]
pub struct DomainArbiter {
    /// 仲裁策略（urgent 窗口等）.
    pub policy: ArbiterPolicy,
    /// 累计仲裁总次数.
    pub total_count: u64,
    /// 命中安全第一级的次数.
    pub safety_count: u64,
    /// 命中 deadline 第二级的次数.
    pub deadline_count: u64,
    /// 命中竞价第三级的次数.
    pub bid_count: u64,
    /// 空请求列表次数.
    pub empty_count: u64,
    /// 安全冲突次数（safety_critical 请求数 ≥ 2，D10）.
    pub conflict_count: u64,
}

impl DomainArbiter {
    /// 创建仲裁器：计数器全零，策略由调用方给定.
    pub fn new(policy: ArbiterPolicy) -> Self {
        Self {
            policy,
            total_count: 0,
            safety_count: 0,
            deadline_count: 0,
            bid_count: 0,
            empty_count: 0,
            conflict_count: 0,
        }
    }

    /// 执行一次三级仲裁（确定性：同输入同输出，D5）.
    ///
    /// 流程：
    /// 1. `total_count += 1`；
    /// 2. 空请求 → `empty_count += 1`，返回 `winner: None` + [`ArbitrationReason::Default`]；
    /// 3. 安全级：filter `safety_critical`，取 [`crate::Priority`] 最高（首个最大）；
    ///    安全请求数 ≥ 2 → `conflict = true` + `conflict_count += 1`（D10）；
    /// 4. deadline 级：filter [`Claim::is_urgent`]，取最早 deadline（首个最小）；
    /// 5. 竞价级：[`cmp_bid`] 取最高 bid（首个最大；NaN 恒最低，D11）。
    ///
    /// `timestamp` 一律回显 `now_ms`（D6）。
    pub fn arbitrate(&mut self, req: &ArbitrationRequest, now_ms: u64) -> ArbitrationResult {
        self.total_count += 1;

        // 空请求 → Default
        if req.claimants.is_empty() {
            self.empty_count += 1;
            return ArbitrationResult {
                winner: None,
                reason: ArbitrationReason::Default,
                timestamp: now_ms,
                conflict: false,
            };
        }

        // 第一级：安全（safety_critical 中 Priority 最高，首个最大；D5/D10）
        let mut safety_idx: Option<usize> = None;
        let mut safety_n: u64 = 0;
        for (i, c) in req.claimants.iter().enumerate() {
            if !c.safety_critical {
                continue;
            }
            safety_n += 1;
            match safety_idx {
                Some(bi) => {
                    if c.priority > req.claimants[bi].priority {
                        safety_idx = Some(i);
                    }
                }
                None => safety_idx = Some(i),
            }
        }
        if let Some(bi) = safety_idx {
            let conflict = safety_n >= 2;
            if conflict {
                self.conflict_count += 1;
            }
            self.safety_count += 1;
            return ArbitrationResult {
                winner: Some(req.claimants[bi].agent_id),
                reason: ArbitrationReason::SafetyFirst,
                timestamp: now_ms,
                conflict,
            };
        }

        // 第二级：deadline 紧急（is_urgent 中最早 deadline，首个最小；D5/D12）
        let mut urgent_idx: Option<usize> = None;
        for (i, c) in req.claimants.iter().enumerate() {
            if !c.is_urgent(now_ms, self.policy.urgent_window_ms) {
                continue;
            }
            match urgent_idx {
                Some(bi) => {
                    if c.deadline < req.claimants[bi].deadline {
                        urgent_idx = Some(i);
                    }
                }
                None => urgent_idx = Some(i),
            }
        }
        if let Some(bi) = urgent_idx {
            self.deadline_count += 1;
            return ArbitrationResult {
                winner: Some(req.claimants[bi].agent_id),
                reason: ArbitrationReason::Deadline,
                timestamp: now_ms,
                conflict: false,
            };
        }

        // 第三级：竞价（cmp_bid 最高，首个最大；NaN 恒最低，D5/D11）
        // claimants 非空（前面已拦截），best_idx 初始化为 0 安全
        let mut best_idx: usize = 0;
        for (i, c) in req.claimants.iter().enumerate().skip(1) {
            if cmp_bid(&c.bid, &req.claimants[best_idx].bid) == core::cmp::Ordering::Greater {
                best_idx = i;
            }
        }
        self.bid_count += 1;
        ArbitrationResult {
            winner: Some(req.claimants[best_idx].agent_id),
            reason: ArbitrationReason::HighestBid,
            timestamp: now_ms,
            conflict: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    use crate::bid::Priority;

    // ===== 测试辅助 =====

    /// 构造请求（5 参数快捷构造）.
    fn claim(id: u128, priority: Priority, bid: f32, safety: bool, deadline: u64) -> Claim {
        Claim {
            agent_id: AgentId(id),
            priority,
            bid,
            safety_critical: safety,
            deadline,
        }
    }

    /// 构造仲裁请求（resource_id="pcc", deadline=0）.
    fn req(claimants: Vec<Claim>) -> ArbitrationRequest {
        ArbitrationRequest {
            resource_id: "pcc",
            claimants,
            deadline: 0,
        }
    }

    // ===== T9: ArbiterPolicy::default().urgent_window_ms == 1000 =====
    #[test]
    fn t9_policy_default_window_1000() {
        assert_eq!(ArbiterPolicy::default().urgent_window_ms, 1000);
    }

    // ===== T10: new 计数器全零 + policy 回显 =====
    #[test]
    fn t10_new_counters_zero() {
        let policy = ArbiterPolicy {
            urgent_window_ms: 500,
        };
        let a = DomainArbiter::new(policy);
        assert_eq!(a.policy, policy);
        assert_eq!(a.total_count, 0);
        assert_eq!(a.safety_count, 0);
        assert_eq!(a.deadline_count, 0);
        assert_eq!(a.bid_count, 0);
        assert_eq!(a.empty_count, 0);
        assert_eq!(a.conflict_count, 0);
    }

    // ===== T11: 空 claimants → None + Default + conflict false + 计数器 =====
    #[test]
    fn t11_empty_claimants_default() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(&req(vec![]), 1000);
        assert_eq!(r.winner, None);
        assert_eq!(r.reason, ArbitrationReason::Default);
        assert!(!r.conflict);
        assert_eq!(a.empty_count, 1);
        assert_eq!(a.total_count, 1);
    }

    // ===== T12: 安全压制高 bid 与过期 deadline（蓝图 §7.3）=====
    #[test]
    fn t12_safety_suppresses_bid_and_deadline() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 999.0, false, 999_999), // 高 bid 非紧急
                claim(2, Priority::High, 0.0, true, 999_999),      // safety
                claim(3, Priority::Normal, 500.0, false, 0),       // deadline 已过
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(2)));
        assert_eq!(r.reason, ArbitrationReason::SafetyFirst);
    }

    // ===== T13: 2 safety（High vs Safety 级）→ Safety 级胜出 =====
    #[test]
    fn t13_safety_highest_priority_wins() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::High, 0.0, true, 999_999),
                claim(2, Priority::Safety, 0.0, true, 999_999),
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(2)));
        assert_eq!(r.reason, ArbitrationReason::SafetyFirst);
    }

    // ===== T14: 2 safety 同 priority → 输入序首个 + conflict==true + conflict_count==1 =====
    #[test]
    fn t14_safety_tie_first_wins_conflict() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::High, 0.0, true, 999_999),
                claim(2, Priority::High, 0.0, true, 999_999),
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(1)));
        assert!(r.conflict);
        assert_eq!(a.conflict_count, 1);
    }

    // ===== T15: safety 路径计数器（total==1 / safety==1 / 其余除 conflict 外==0）=====
    #[test]
    fn t15_safety_path_counters() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![claim(1, Priority::High, 0.0, true, 999_999)]),
            1000,
        );
        assert_eq!(r.reason, ArbitrationReason::SafetyFirst);
        assert!(!r.conflict);
        assert_eq!(a.total_count, 1);
        assert_eq!(a.safety_count, 1);
        assert_eq!(a.deadline_count, 0);
        assert_eq!(a.bid_count, 0);
        assert_eq!(a.empty_count, 0);
        assert_eq!(a.conflict_count, 0);
    }

    // ===== T16: deadline 紧急（过去 deadline=500, now=1000）→ Deadline =====
    #[test]
    fn t16_urgent_deadline_wins() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 999.0, false, 999_999), // 高 bid 非紧急
                claim(2, Priority::Normal, 0.0, false, 500),       // deadline 已过
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(2)));
        assert_eq!(r.reason, ArbitrationReason::Deadline);
        assert!(!r.conflict);
    }

    // ===== T17: 多 urgent（1500 vs 1200, now=1000, window=1000）→ 最早 deadline 1200 =====
    #[test]
    fn t17_earliest_deadline_wins() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 0.0, false, 1500),
                claim(2, Priority::Normal, 0.0, false, 1200),
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(2)));
        assert_eq!(r.reason, ArbitrationReason::Deadline);
    }

    // ===== T18: urgent + safety 同时存在 → safety 优先（三级顺序不可逾越）=====
    #[test]
    fn t18_safety_beats_urgent() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 0.0, false, 500), // urgent（已过）
                claim(2, Priority::Low, 0.0, true, 999_999), // safety 非 urgent
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(2)));
        assert_eq!(r.reason, ArbitrationReason::SafetyFirst);
    }

    // ===== T19: deadline 远超窗口（5000 > now+window=2000）→ 落 HighestBid =====
    #[test]
    fn t19_far_deadline_falls_to_bid() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 5.0, false, 5000),
                claim(2, Priority::Normal, 9.0, false, 6000),
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(2)));
        assert_eq!(r.reason, ArbitrationReason::HighestBid);
    }

    // ===== T20: 最高 bid 胜出（bid 5/9/7 → 9）=====
    #[test]
    fn t20_highest_bid_wins() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 5.0, false, 999_999),
                claim(2, Priority::Normal, 9.0, false, 999_999),
                claim(3, Priority::Normal, 7.0, false, 999_999),
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(2)));
        assert_eq!(r.reason, ArbitrationReason::HighestBid);
    }

    // ===== T21: 等 bid（5/5）→ 确定性取首个 =====
    #[test]
    fn t21_equal_bid_first_wins() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 5.0, false, 999_999),
                claim(2, Priority::Normal, 5.0, false, 999_999),
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(1)));
        assert_eq!(r.reason, ArbitrationReason::HighestBid);
    }

    // ===== T22: NaN bid 不胜出（[NaN, 1.0] → 1.0 者，D11）=====
    #[test]
    fn t22_nan_bid_never_wins() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, f32::NAN, false, 999_999),
                claim(2, Priority::Normal, 1.0, false, 999_999),
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(2)));
        assert_eq!(r.reason, ArbitrationReason::HighestBid);
    }

    // ===== T23: 全 NaN bid → 确定性首个胜出 + 不 panic =====
    #[test]
    fn t23_all_nan_first_wins() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, f32::NAN, false, 999_999),
                claim(2, Priority::Normal, f32::NAN, false, 999_999),
            ]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(1)));
        assert_eq!(r.reason, ArbitrationReason::HighestBid);
    }

    // ===== T24: timestamp == 传入 now_ms 回显（三路径各验一次）=====
    #[test]
    fn t24_timestamp_echo_now_ms() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r1 = a.arbitrate(
            &req(vec![claim(1, Priority::High, 0.0, true, 999_999)]),
            111,
        );
        assert_eq!(r1.timestamp, 111);
        assert_eq!(r1.reason, ArbitrationReason::SafetyFirst);
        let r2 = a.arbitrate(&req(vec![claim(1, Priority::Normal, 0.0, false, 500)]), 222);
        assert_eq!(r2.timestamp, 222);
        assert_eq!(r2.reason, ArbitrationReason::Deadline);
        let r3 = a.arbitrate(
            &req(vec![claim(1, Priority::Normal, 5.0, false, 999_999)]),
            333,
        );
        assert_eq!(r3.timestamp, 333);
        assert_eq!(r3.reason, ArbitrationReason::HighestBid);
    }

    // ===== T25: 计数器组合：safety/deadline/bid 各一次 → total==3 且分项和==3 =====
    #[test]
    fn t25_counter_combination() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let _ = a.arbitrate(
            &req(vec![claim(1, Priority::High, 0.0, true, 999_999)]),
            1000,
        );
        let _ = a.arbitrate(
            &req(vec![claim(1, Priority::Normal, 0.0, false, 500)]),
            1000,
        );
        let _ = a.arbitrate(
            &req(vec![claim(1, Priority::Normal, 5.0, false, 999_999)]),
            1000,
        );
        assert_eq!(a.total_count, 3);
        assert_eq!(a.safety_count + a.deadline_count + a.bid_count, 3);
        assert_eq!(a.safety_count, 1);
        assert_eq!(a.deadline_count, 1);
        assert_eq!(a.bid_count, 1);
    }

    // ===== T26: 单 claimant（非安全非紧急）→ HighestBid + winner 正确 =====
    #[test]
    fn t26_single_claimant_highest_bid() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let r = a.arbitrate(
            &req(vec![claim(7, Priority::Normal, 3.0, false, 999_999)]),
            1000,
        );
        assert_eq!(r.winner, Some(AgentId(7)));
        assert_eq!(r.reason, ArbitrationReason::HighestBid);
        assert!(!r.conflict);
    }

    // ===== T27: 自定义 policy window=0：deadline==now 不 urgent → 落 bid =====
    #[test]
    fn t27_zero_window_boundary_not_urgent() {
        let mut a = DomainArbiter::new(ArbiterPolicy {
            urgent_window_ms: 0,
        });
        // window=0：deadline < now 才 urgent；deadline==now=1000 → 不 urgent → 落 bid
        let r = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 5.0, false, 1000), // deadline == now → 非 urgent
                claim(2, Priority::Normal, 9.0, false, 2000),
            ]),
            1000,
        );
        assert_eq!(r.reason, ArbitrationReason::HighestBid);
        assert_eq!(r.winner, Some(AgentId(2)));
        // 对照：deadline=999 < now → urgent
        let r2 = a.arbitrate(
            &req(vec![
                claim(1, Priority::Normal, 5.0, false, 999),
                claim(2, Priority::Normal, 9.0, false, 2000),
            ]),
            1000,
        );
        assert_eq!(r2.reason, ArbitrationReason::Deadline);
        assert_eq!(r2.winner, Some(AgentId(1)));
    }

    // ===== T28: 确定性：同输入两个不同实例 → 结果全等 =====
    #[test]
    fn t28_deterministic_two_instances() {
        let mut a1 = DomainArbiter::new(ArbiterPolicy::default());
        let mut a2 = DomainArbiter::new(ArbiterPolicy::default());
        let r = req(vec![
            claim(1, Priority::Normal, 5.0, false, 999_999),
            claim(2, Priority::High, 0.0, true, 999_999),
            claim(3, Priority::Normal, 9.0, false, 500),
        ]);
        let r1 = a1.arbitrate(&r, 1234);
        let r2 = a2.arbitrate(&r, 1234);
        assert_eq!(r1.winner, r2.winner);
        assert_eq!(r1.reason, r2.reason);
        assert_eq!(r1.timestamp, r2.timestamp);
        assert_eq!(r1.conflict, r2.conflict);
        assert_eq!(r1, r2);
    }

    // ===== T29: ArbitrationResult 构造 + Clone + PartialEq + Debug 含 "SafetyFirst" =====
    #[test]
    fn t29_result_construct_clone_debug() {
        let r = ArbitrationResult {
            winner: Some(AgentId(1)),
            reason: ArbitrationReason::SafetyFirst,
            timestamp: 42,
            conflict: true,
        };
        // Clone trait 显式调用（Copy 类型避免 clone_on_copy lint）
        let r2 = Clone::clone(&r);
        assert_eq!(r, r2);
        let dbg = alloc::format!("{:?}", r);
        assert!(dbg.contains("SafetyFirst"));
    }

    // ===== T30: 100 claimants 大输入（99 普通 + 末位 safety）→ safety 胜出 =====
    #[test]
    fn t30_large_input_safety_wins() {
        let mut a = DomainArbiter::new(ArbiterPolicy::default());
        let mut claimants = Vec::new();
        for i in 1..100u128 {
            claimants.push(claim(i, Priority::Normal, i as f32, false, 999_999));
        }
        claimants.push(claim(100, Priority::High, 0.0, true, 999_999)); // 末位 safety
        let r = a.arbitrate(&req(claimants), 1000);
        assert_eq!(r.winner, Some(AgentId(100)));
        assert_eq!(r.reason, ArbitrationReason::SafetyFirst);
    }
}
