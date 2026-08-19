//! 蓝图 45 WP-45.3：采集调度器（周期表执行面）。
//!
//! 按点表周期**错峰**生成 poll 任务（确定性相位：同一输入 → 同一排程，
//! 可复现证据），经 `WorkBudget`（复用 protocol-abstract `plugin.rs`
//! 的预算类型）限额执行；预算不足的任务**顺延并计数**（不丢点不假
//! 成功——`deferred_count` 可观测）。纯状态机、确定性、无新线程/优先级
//! （root 单核调度事实）。
//!
//! 语义要点：
//! - **顺延 = 逾期**：预算不足未被取走的任务保持在「过期待执行」态，
//!   预算一旦恢复**立即补跑**（恢复即补新鲜度）；
//! - **deferred 优先**：顺延任务 FIFO 严格先于新鲜到期任务（无饿死）；
//! - **不追偿**：任务执行后下一 due = now + period——长停机不产生突发风暴；
//! - **调度延迟有界**（静态推导 WCET 上界）：任意单任务最多被顺延
//!   `ceil(总成本 / 单轮预算) - 1` 轮即被执行（`pending_orders` 保证每个
//!   点只有唯一调度来源，无重复取批）。

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use eneros_protocol_abstract::plugin::WorkBudget;
use eneros_upa_model::PointId;

/// 单点采集任务（点表/装配层构造；`cost_ms` 为该点单次 poll 的 WCET 上界）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollTask {
    /// 统一点 ID。
    pub point_id: PointId,
    /// 采集周期（毫秒）。
    pub period_ms: u32,
    /// 错峰相位（毫秒；首次到期 = 调度器创建时刻 + phase_ms）。
    pub phase_ms: u32,
    /// 单次 poll 名义耗时（毫秒；WCET 上界，装配/点表派生）。
    pub cost_ms: u32,
}

/// 错峰任务表生成（45.3）：按点表顺序确定性错开相位——
/// `phase[i] = (i * period[i]) / n`（`n` = 点数；同一输入 → 同一排程）。
/// `cost_ms` 缺省 1（配额单位；具体协议可覆写）。
pub fn staggered_tasks(points: &[(PointId, u32)], cost_ms: u32) -> Vec<PollTask> {
    let n = points.len().max(1) as u64;
    points
        .iter()
        .enumerate()
        .map(|(i, &(point_id, period))| PollTask {
            point_id,
            period_ms: period,
            phase_ms: ((i as u64 * period as u64) / n) as u32,
            cost_ms,
        })
        .collect()
}

/// 调度器内部到期项。
#[derive(Debug, Clone, Copy)]
struct DueItem {
    point_id: PointId,
    next_due: u64,
    period_ms: u32,
    cost_ms: u32,
    order: u64,
}

/// 单次取批结果（观测锁存）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStatus {
    /// 无到期任务。
    Idle,
    /// 发出本批（预算完全消化）。
    Issued,
    /// 存在因预算不足被顺延的任务（`deferred_count` 已增加）。
    Deferred,
}

/// 采集调度器（45.3）。
#[derive(Debug)]
pub struct AcqScheduler {
    /// 全部周期任务（首次 due 由错峰相位决定）。
    items: Vec<DueItem>,
    /// 顺延任务（FIFO 严格保序；预算恢复后最先执行——逾期即补跑）。
    pending: VecDeque<DueItem>,
    /// 顺延任务在 `items` 中的 order 集合——防止同一个点同时经
    /// pending 与 items 被重复取批（调度状态唯一事实）。
    pending_orders: alloc::collections::BTreeSet<u64>,
    /// 累计顺延次数（观测——不丢点不假成功）。
    pub deferred_count: u64,
}

impl AcqScheduler {
    /// 构造（`now_ms` 为调度器创建时刻；首次到期 = now + phase_ms 错峰）。
    pub fn new(tasks: &[PollTask], now_ms: u64) -> Self {
        let items = tasks
            .iter()
            .enumerate()
            .map(|(i, t)| DueItem {
                point_id: t.point_id,
                next_due: now_ms.saturating_add(t.phase_ms as u64),
                period_ms: t.period_ms,
                cost_ms: t.cost_ms,
                order: i as u64,
            })
            .collect();
        Self {
            items,
            pending: VecDeque::new(),
            pending_orders: alloc::collections::BTreeSet::new(),
            deferred_count: 0,
        }
    }

    /// 任务总数。
    pub fn task_count(&self) -> usize {
        self.items.len()
    }

    /// 顺延中任务数（= 上轮预算未消化的到期任务）。
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// 取本 tick 到期且受预算限额的任务（`WorkBudget` 双维：`max_polls`
    /// 计数 + `max_work_ms` 成本累计；`max_work_ms = 0` → 仅计数量限）。
    ///
    /// 返回 (批次 point_id 列表, 状态)。到期未取完的 → 顺延入 pending 并计数。
    /// 公平性：**顺延任务优先**（FIFO 保序，不得被新鲜到期任务饿死）；顺延
    /// 任务为逾期状态——预算一旦恢复立即补跑（恢复即补新鲜度）。
    pub fn take_batch(&mut self, now_ms: u64, budget: WorkBudget) -> (Vec<PointId>, BatchStatus) {
        // 1. 收集到期，区分 deferred（先入）与 fresh（后入）。
        let mut due_deferred: Vec<DueItem> = Vec::new();
        let held: VecDeque<DueItem> = core::mem::take(&mut self.pending);
        for item in held {
            if item.next_due <= now_ms {
                due_deferred.push(item);
            } else {
                self.pending.push_back(item);
            }
        }
        // fresh：items 中 next_due ≤ now 且未处于顺延（pending_orders 防重复）。
        let mut due_fresh: Vec<DueItem> = Vec::new();
        for item in self.items.iter() {
            if item.next_due <= now_ms && !self.pending_orders.contains(&item.order) {
                due_fresh.push(*item);
            }
        }
        if due_deferred.is_empty() && due_fresh.is_empty() {
            return (Vec::new(), BatchStatus::Idle);
        }
        // 2. 合并（确定性）：deferred 保持 pending FIFO 顺序（VecDeque 即
        //    保序——禁再按 order 排序，否则破坏「先顺延先补跑」公平）；
        //    fresh 按 (next_due, order) 稳定排序。
        due_fresh.sort_by_key(|i| (i.next_due, i.order));
        let mut due: Vec<DueItem> = due_deferred;
        due.extend(due_fresh);

        // 3. 限额挑选：max_polls 计数 + max_work_ms 成本累计（双维）。
        let mut taken: Vec<PointId> = Vec::with_capacity(budget.max_polls as usize);
        let mut work: u64 = 0;
        let mut deferred: Vec<DueItem> = Vec::new();
        for item in due {
            let count_ok = (taken.len() as u64) < budget.max_polls as u64;
            let cost_ok = budget.max_work_ms == 0
                || work.saturating_add(item.cost_ms as u64) <= budget.max_work_ms;
            if count_ok && cost_ok {
                work = work.saturating_add(item.cost_ms as u64);
                // 执行：推进调度态（下一 due = now + period）+ 解除顺延标记。
                self.items[item.order as usize].next_due =
                    now_ms.saturating_add(item.period_ms as u64);
                self.pending_orders.remove(&item.order);
                taken.push(item.point_id);
            } else {
                deferred.push(item);
            }
        }

        // 4. 顺延入 FIFO 并计数（不丢点不假成功）；顺延点在 items 中
        //    保持过期态并登记 order——下一轮仅经 pending 一个来源。
        if !deferred.is_empty() {
            let n = deferred.len() as u64;
            for item in &deferred {
                self.pending_orders.insert(item.order);
                self.pending.push_back(*item);
            }
            self.deferred_count = self.deferred_count.saturating_add(n);
            return (taken, BatchStatus::Deferred);
        }
        (taken, BatchStatus::Issued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(polls: u32, work_ms: u64) -> WorkBudget {
        WorkBudget {
            max_polls: polls,
            max_work_ms: work_ms,
        }
    }

    fn tasks(n: u32, period: u32) -> Vec<PollTask> {
        (1..=n)
            .map(|i| PollTask {
                point_id: i,
                period_ms: period,
                phase_ms: 0,
                cost_ms: 1,
            })
            .collect()
    }

    // ---- 45.3 DoD：错峰 ----

    #[test]
    fn staggered_tasks_phases_are_deterministic_and_within_period() {
        let points: Vec<(PointId, u32)> = (1..=6).map(|i| (i, 6000)).collect();
        let st = staggered_tasks(&points, 1);
        // 6 点、周期 6000 → 相位 0/1000/2000/3000/4000/5000（互异、< period）。
        let mut phases: Vec<u32> = st.iter().map(|t| t.phase_ms).collect();
        phases.sort_unstable();
        assert_eq!(phases, vec![0, 1000, 2000, 3000, 4000, 5000]);
        // 确定性：同输入两次生成一致。
        let st2 = staggered_tasks(&points, 1);
        assert_eq!(st, st2);
        // 全部 phase < period。
        for t in &st {
            assert!(t.phase_ms < t.period_ms);
        }
    }

    #[test]
    fn schedule_staggers_first_due() {
        let st = staggered_tasks(&(1..=6u32).map(|i| (i, 6000)).collect::<Vec<_>>(), 1);
        let mut sched = AcqScheduler::new(&st, 1_000);
        // 首轮（now=1000）：仅 phase=0 的点到期。
        let (batch, status) = sched.take_batch(1_000, budget(6, 100));
        assert_eq!(status, BatchStatus::Issued);
        assert_eq!(batch, vec![1]);
        // 下一点 phase=1000 → now=2000 到期。
        let (batch, _) = sched.take_batch(2_000, budget(6, 100));
        assert_eq!(batch, vec![2]);
    }

    // ---- 45.3 DoD：超限顺延 ----

    #[test]
    fn budget_exhaustion_defers_and_counts() {
        // 10 点全 0 相位 → 首轮全部到期；预算 max_polls=3 → 顺延 7。
        let mut sched = AcqScheduler::new(&tasks(10, 1000), 0);
        let mut seen: alloc::collections::BTreeSet<PointId> = alloc::collections::BTreeSet::new();
        {
            let (batch, status) = sched.take_batch(0, budget(3, 100));
            assert_eq!(status, BatchStatus::Deferred);
            assert_eq!(batch.len(), 3);
            assert_eq!(sched.pending_count(), 7);
            assert_eq!(sched.deferred_count, 7);
            seen.extend(batch);
        }
        // 有界（静态推导 ⌈N/B⌉=4 轮）：随后 3 轮内全部 10 点至少执行一次，
        // 无饿死；顺延计数单调、pending 有界不丢点。
        for round in 1..=3 {
            let (batch, status) = sched.take_batch(round * 1000, budget(3, 100));
            assert_eq!(status, BatchStatus::Deferred);
            assert_eq!(batch.len(), 3, "round {round}");
            assert!(sched.pending_count() <= 10);
            seen.extend(batch);
        }
        // FIFO 公平：4 轮内 10 点全覆盖（BTreeSet 去重后恰为 1..=10）。
        let all: alloc::collections::BTreeSet<PointId> = (1..=10).collect();
        assert_eq!(seen, all, "4 轮内全部点至少执行一次（有界延迟）");
        assert!(sched.deferred_count >= 7, "顺延计数单调可观测");
    }

    /// 双维预算：max_work_ms 也限额（cost 累计）。
    #[test]
    fn work_budget_cap_limits_by_cost() {
        let heavy: Vec<PollTask> = (1..=4)
            .map(|i| PollTask {
                point_id: i,
                period_ms: 1000,
                phase_ms: 0,
                cost_ms: 20,
            })
            .collect();
        let mut sched = AcqScheduler::new(&heavy, 0);
        // max_work_ms=45 → 每轮最多 2 点（20+20=40 ≤ 45；+20=60 > 45）。
        let (batch, status) = sched.take_batch(0, budget(4, 45));
        assert_eq!(status, BatchStatus::Deferred);
        assert_eq!(batch.len(), 2);
        assert_eq!(sched.pending_count(), 2);
    }

    // ---- 45.3 DoD：恢复（预算恢复后顺延最先执行，无饿死） ----

    #[test]
    fn recovery_runs_deferred_first_with_fresh_tasks_deferred_after() {
        // 2 点短周期；首轮预算 0 → 全部顺延（逾期待补）。
        let st = vec![
            PollTask {
                point_id: 1,
                period_ms: 100,
                phase_ms: 0,
                cost_ms: 1,
            },
            PollTask {
                point_id: 2,
                period_ms: 100,
                phase_ms: 0,
                cost_ms: 1,
            },
        ];
        let mut sched = AcqScheduler::new(&st, 0);
        let (batch, status) = sched.take_batch(0, budget(0, 0));
        assert_eq!(batch.len(), 0);
        assert_eq!(status, BatchStatus::Deferred);
        assert_eq!(sched.pending_count(), 2);
        assert_eq!(sched.deferred_count, 2);
        // 预算恢复（now=50）→ 逾期顺延任务立即补跑（恢复即补新鲜度）。
        let (batch, status) = sched.take_batch(50, budget(2, 100));
        assert_eq!(batch, vec![1, 2]);
        assert_eq!(status, BatchStatus::Issued);
        assert_eq!(sched.pending_count(), 0);
        // 观察计数保持（累计顺延 2 次）。
        assert_eq!(sched.deferred_count, 2);
        // 不追偿：执行后下一 due = now+period = 150（50 期已过不补）。
        let (batch, status) = sched.take_batch(100, budget(2, 100));
        assert_eq!(batch, Vec::<PointId>::new());
        assert_eq!(status, BatchStatus::Idle);
        let (batch, _) = sched.take_batch(150, budget(2, 100));
        assert_eq!(batch, vec![1, 2]);
    }

    /// 公平性（恢复核心）：顺延任务优先级高于新鲜到期——低序号点被顺延后
    /// 不得被高序号新鲜点反复插队饿死。
    #[test]
    fn deferred_has_priority_over_fresh_tasks() {
        // 3 点同周期；首轮预算 1 → 取 point 1，顺延 2、3。
        let st = vec![
            PollTask {
                point_id: 1,
                period_ms: 1000,
                phase_ms: 0,
                cost_ms: 1,
            },
            PollTask {
                point_id: 2,
                period_ms: 1000,
                phase_ms: 0,
                cost_ms: 1,
            },
            PollTask {
                point_id: 3,
                period_ms: 1000,
                phase_ms: 0,
                cost_ms: 1,
            },
        ];
        let mut sched = AcqScheduler::new(&st, 0);
        let (batch, _) = sched.take_batch(0, budget(1, 100));
        assert_eq!(batch, vec![1]);
        assert_eq!(sched.pending_count(), 2); // 顺延 2、3
                                              // 次轮预算 3：到期 = 顺延(2,3) + 新鲜(1 又到期)；全部取走时断言
                                              // 顺延 2、3 排在新鲜 1 之前（FIFO 公平，无饿死）。
        let (batch, status) = sched.take_batch(1_000, budget(3, 100));
        assert_eq!(status, BatchStatus::Issued);
        assert_eq!(batch, vec![2, 3, 1], "顺延任务优先于新鲜到期，无饿死");
    }
}
