//! 竞价与优先级 — [`Priority`] / [`Claim`] / [`cmp_bid`]（v0.92.0）.
//!
//! - **D3**：`deadline` 统一 `u64` ms，外部时间注入，不读系统时钟。
//! - **D7**：[`Priority`] 变体按 `Low < Normal < High < Critical < Safety` **升序声明**，
//!   使派生 `Ord` 序即优先级序（`Safety` 最大），默认 `Normal`。
//! - **D11**：[`cmp_bid`] 为 f32 **全序**比较：双 NaN → Equal；NaN 恒最低；±Inf 保留偏序。
//!   解决 `f32: !Ord` 无法直接 `max` 的问题，NaN 永不胜出。
//! - **D12**：[`Claim::is_urgent`] urgent 判定窗口——`deadline < now_ms.saturating_add(window_ms)`
//!   （**严格 <**，过去 deadline 必 urgent；边界 `deadline == now + window` 不 urgent）；
//!   `saturating_add` 防 u64 溢出 panic。

use core::cmp::Ordering;

use eneros_agent::AgentId;

/// 请求优先级（D7：变体按升序声明，序即优先级；`Safety` 最大）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Priority {
    /// 低优先级（可延期后台任务）.
    Low,
    /// 常规优先级（默认，常态竞价请求）.
    #[default]
    Normal,
    /// 高优先级（重要调度任务）.
    High,
    /// 临界优先级（接近安全底线的关键任务）.
    Critical,
    /// 安全优先级（最高，安全底线相关任务）.
    Safety,
}

/// 资源请求（一个 Agent 对某资源的一次竞价/占用声明；D3/D7）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Claim {
    /// 发起请求的 Agent ID.
    pub agent_id: AgentId,
    /// 请求优先级（安全级仲裁时比较，D7）.
    pub priority: Priority,
    /// 竞价（f32；经 [`cmp_bid`] 全序比较，NaN 恒最低，D11）.
    pub bid: f32,
    /// 是否安全关键请求（true → 进入安全第一级仲裁，压制竞价与 deadline）.
    pub safety_critical: bool,
    /// 截止时间（u64 ms，D3；小于 now + urgent 窗口即视为紧急，D12）.
    pub deadline: u64,
}

impl Claim {
    /// 判定请求是否紧急（D12）.
    ///
    /// `deadline < now_ms.saturating_add(window_ms)`（**严格 <**）：
    /// 过去的 deadline 必为 urgent；边界 `deadline == now_ms + window_ms` 不 urgent；
    /// `saturating_add` 保证 `now_ms` 接近 `u64::MAX` 时不溢出 panic（饱和后任意 deadline 均 urgent）。
    pub fn is_urgent(&self, now_ms: u64, window_ms: u64) -> bool {
        self.deadline < now_ms.saturating_add(window_ms)
    }
}

/// f32 竞价全序比较（D11：用于 max 比较）.
///
/// - 双 NaN → [`Ordering::Equal`]
/// - `a` 为 NaN → [`Ordering::Less`]（NaN 恒最低，永不胜出）
/// - `b` 为 NaN → [`Ordering::Greater`]
/// - 否则按偏序比较（±Inf 保留偏序；理论上不可比时兜底 Equal）
pub fn cmp_bid(a: &f32, b: &f32) -> Ordering {
    if a.is_nan() && b.is_nan() {
        Ordering::Equal
    } else if a.is_nan() {
        Ordering::Less
    } else if b.is_nan() {
        Ordering::Greater
    } else {
        a.partial_cmp(b).unwrap_or(Ordering::Equal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== T1: Priority 序 Low<Normal<High<Critical<Safety + Default==Normal + Copy 语义 =====
    #[test]
    fn t1_priority_ordering_default_copy() {
        assert!(Priority::Low < Priority::Normal);
        assert!(Priority::Normal < Priority::High);
        assert!(Priority::High < Priority::Critical);
        assert!(Priority::Critical < Priority::Safety);
        assert_eq!(Priority::default(), Priority::Normal);
        let p = Priority::High;
        let pc = p; // Copy
        assert_eq!(p, pc);
    }

    // ===== T2: Claim 构造 + Copy + 5 字段回显 =====
    #[test]
    fn t2_claim_construct_copy_fields() {
        let c = Claim {
            agent_id: AgentId(42),
            priority: Priority::Critical,
            bid: 9.5,
            safety_critical: true,
            deadline: 5000,
        };
        let cc = c; // Copy
        assert_eq!(c, cc);
        assert_eq!(c.agent_id, AgentId(42));
        assert_eq!(c.priority, Priority::Critical);
        assert!((c.bid - 9.5).abs() < 1e-6);
        assert!(c.safety_critical);
        assert_eq!(c.deadline, 5000);
    }

    // ===== T3: cmp_bid 正常比较 Less / Greater / Equal =====
    #[test]
    fn t3_cmp_bid_normal() {
        assert_eq!(cmp_bid(&1.0, &2.0), Ordering::Less);
        assert_eq!(cmp_bid(&2.0, &1.0), Ordering::Greater);
        assert_eq!(cmp_bid(&1.5, &1.5), Ordering::Equal);
    }

    // ===== T4: cmp_bid 单侧 NaN → NaN 恒最低 =====
    #[test]
    fn t4_cmp_bid_nan_lowest() {
        assert_eq!(cmp_bid(&f32::NAN, &1.0), Ordering::Less);
        assert_eq!(cmp_bid(&1.0, &f32::NAN), Ordering::Greater);
    }

    // ===== T5: cmp_bid 双 NaN → Equal；±Inf 保留偏序 =====
    #[test]
    fn t5_cmp_bid_double_nan_and_inf() {
        assert_eq!(cmp_bid(&f32::NAN, &f32::NAN), Ordering::Equal);
        assert_eq!(cmp_bid(&f32::INFINITY, &1.0), Ordering::Greater);
        assert_eq!(cmp_bid(&f32::NEG_INFINITY, &1.0), Ordering::Less);
    }

    // ===== T6: is_urgent 过去 deadline 必 urgent；now+window 内 urgent =====
    #[test]
    fn t6_is_urgent_past_and_within_window() {
        let mut c = Claim {
            agent_id: AgentId(1),
            priority: Priority::Normal,
            bid: 0.0,
            safety_critical: false,
            deadline: 500,
        };
        // 过去 deadline（500 < now=1000）必 urgent
        assert!(c.is_urgent(1000, 1000));
        // now+window 内（1500 < 1000+1000=2000）urgent
        c.deadline = 1500;
        assert!(c.is_urgent(1000, 1000));
    }

    // ===== T7: is_urgent 窗口外 false；边界 deadline==now+window 严格 < → false =====
    #[test]
    fn t7_is_urgent_outside_and_boundary() {
        let mut c = Claim {
            agent_id: AgentId(1),
            priority: Priority::Normal,
            bid: 0.0,
            safety_critical: false,
            deadline: 5000,
        };
        // 5000 远超 now+window=2000 → false
        assert!(!c.is_urgent(1000, 1000));
        // 边界 deadline == now+window（2000）→ 严格 < 不成立 → false
        c.deadline = 2000;
        assert!(!c.is_urgent(1000, 1000));
    }

    // ===== T8: is_urgent now=u64::MAX saturating 不 panic 且任意 deadline → true =====
    #[test]
    fn t8_is_urgent_saturating_no_panic() {
        let mut c = Claim {
            agent_id: AgentId(1),
            priority: Priority::Normal,
            bid: 0.0,
            safety_critical: false,
            deadline: u64::MAX,
        };
        // saturating_add 饱和到 u64::MAX，deadline < MAX 不成立……但 deadline=MAX 时
        // MAX < MAX 为 false，故取更小 deadline 验证"任意普通 deadline → true"
        c.deadline = u64::MAX - 1;
        assert!(c.is_urgent(u64::MAX, 1000));
        c.deadline = 0;
        assert!(c.is_urgent(u64::MAX, 1000));
    }
}
