//! Deepening-v1 §A：快照钉住生命周期深 module（原 `DispatchEngine` 五字段收拢）。
//!
//! 归属边界：本 module 只管**钉住账目**（代次、已释放事实、待重试队列、
//! 终态行一次性标记、钉面失败计数）。保留面本身（[`HeldInputRetainer`]
//! 装配）、持有计划介质（`DurablePlans`）与**快照存活引用计数**
//!（事实源 = 持久计划，`held_engine::live_snapshot_ref_count`）都在
//! engine 侧——引用计数经参数传入，不得误搬进来（风险 R-A2）。
//!
//! 有界淘汰单点化：收拢前六处四种惯用法（`while>16` 退到 16 /
//! `truncate(16)` 截到 16 / `<16` 才推入 / `push`+`>16 remove(0)`）在此
//! 逐容器对应为一个私有 `evict_bounded` 与一个入队闸门，语义逐位保持
//!（风险 R-A1 由本模块表级测试钉死每容器 16/17 边界）。
//!
//! F70-04（§7.2.6）代次安全不变量由注释维系升级为结构维系：代次只在
//! [`HoldRegistry::admit`] 递增，released 缓存只在
//! [`HoldRegistry::released_at_current_epoch`] 判定，旧代次完成事实不可能
//! 抑制新一轮释放。

use eneros_decision_contract::PlanId;

use super::*;

/// 代次表容量（>16 退到 16，淘汰最小键）。
const EPOCH_CAP: usize = 16;
/// 已释放事实容量（FIFO，最旧先出）。
const RELEASED_CAP: usize = 16;
/// 待重试释放队列容量（满额 = 本轮不释放，下轮引用态重算后再入队）。
const PENDING_CAP: usize = 16;
/// 终态行一次性标记容量（FIFO）。
const DONE_CAP: usize = 16;

/// 钉住账目（五字段全私有；观测面唯一入口 = 本模块方法）。
pub(crate) struct HoldRegistry {
    /// 快照持有代次（每次成功准入 +1；有界 ≤`EPOCH_CAP`）。
    epochs: BTreeMap<[u8; 16], u32>,
    /// 已解除钉的 (快照 ID, 持有代次)——防重复 release。
    released: Vec<([u8; 16], u32)>,
    /// 释放失败的快照（有界 ≤`PENDING_CAP`）——空闲周期持续重试。
    pending: Vec<[u8; 16]>,
    /// 已打印终态行的计划 ID（HELD-PLAN-DONE 一次性证据行）。
    done_logged: Vec<PlanId>,
    /// 钉面失败计数（hold 拒绝/装配缺失面——可观测不静默）。
    pin_failures: u64,
}

impl Default for HoldRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HoldRegistry {
    pub fn new() -> Self {
        Self {
            epochs: BTreeMap::new(),
            released: Vec::new(),
            pending: Vec::new(),
            done_logged: Vec::new(),
            pin_failures: 0,
        }
    }

    /// 本模块唯一的有界淘汰惯用法：环形尾部优先（`remove(0)` = 最旧）。
    fn evict_bounded<T>(buf: &mut Vec<T>, cap: usize) {
        while buf.len() > cap {
            buf.remove(0);
        }
    }

    /// 准入持有：代次 +1（wrapping），超容量淘汰最小键。返回新代次。
    ///
    /// 淘汰只引发至多一次幂等重释放，不损正确性（§7.2.6 原文承诺）。
    pub fn admit(&mut self, snapshot_id: [u8; 16]) -> u32 {
        let epoch = self.epochs.entry(snapshot_id).or_insert(0);
        *epoch = epoch.wrapping_add(1);
        while self.epochs.len() > EPOCH_CAP {
            let oldest = *self.epochs.keys().next().expect("nonempty");
            self.epochs.remove(&oldest);
        }
        self.epoch(&snapshot_id)
    }

    /// 当前代次（从未持有 = 0）。
    pub fn epoch(&self, snapshot_id: &[u8; 16]) -> u32 {
        self.epochs.get(snapshot_id).copied().unwrap_or(0)
    }

    /// §7.2.6 代次安全剪除：快照重新获得存活引用（新一轮合法持有）⇒
    /// 旧完成事实失效，按代次比对剪除。
    pub fn prune_releases_by_epoch(&mut self) {
        // 代次判据唯一经 `self.epoch`（避免与 epochs 字段双读漂移）；
        // 取出-回填只为解开同结构体内 `released` 可变借用与 `epochs`
        // 不可借用的冲突（收拢前两字段分居 engine 时不存在此约束）。
        let mut released = core::mem::take(&mut self.released);
        released.retain(|(id, released_epoch)| self.epoch(id) == *released_epoch);
        self.released = released;
    }

    /// §7.2.6 待释放队列剪除：仍被在途/未决计划引用 ⇒ 不释放新持有人的
    /// 钉。`live_refs` 由 engine 侧从持久计划派生（R-A2：计数归属在
    /// engine，本模块只消费）。
    pub fn prune_pending_by_live_refs(&mut self, live_refs: &[([u8; 16], usize)]) {
        self.pending.retain(|id| {
            live_refs
                .iter()
                .find(|(x, _)| x == id)
                .map(|(_, n)| *n)
                .unwrap_or(0)
                == 0
        });
    }

    /// 进入收尾 pass：取出待重试队列（本 pass 新失败者入 `pending`，
    /// 重试队列在本 pass 内不再被写入——§7.2.4「下个实际调度到的周期」
    /// 语义）。
    pub fn begin_retry_cycle(&mut self) -> Vec<[u8; 16]> {
        core::mem::take(&mut self.pending)
    }

    /// 待重试队列（本 pass 新入队者）是否已含该快照。
    pub fn contains_pending(&self, snapshot_id: &[u8; 16]) -> bool {
        self.pending.contains(snapshot_id)
    }

    /// 本代次是否已完成释放（幂等重释放抑制判据）。
    pub fn released_at_current_epoch(&self, snapshot_id: &[u8; 16]) -> bool {
        let epoch = self.epoch(snapshot_id);
        self.released
            .iter()
            .any(|(id, e)| id == snapshot_id && *e == epoch)
    }

    /// 登记释放完成事实（带当代代次）。调用方负责先过
    /// [`HoldRegistry::released_at_current_epoch`] 幂等判据——与收拢前
    /// 两处调用点的判据位置逐字对应。
    pub fn note_released(&mut self, snapshot_id: [u8; 16]) {
        let epoch = self.epoch(&snapshot_id);
        self.released.push((snapshot_id, epoch));
        Self::evict_bounded(&mut self.released, RELEASED_CAP);
    }

    /// 释放失败入队（满额即丢弃本轮——有界，不无界积累）。
    pub fn queue_pending(&mut self, snapshot_id: [u8; 16]) {
        if self.pending.len() < PENDING_CAP {
            self.pending.push(snapshot_id);
        }
    }

    /// 收尾 pass 结束：本 pass 新失败者在前、重试仍失败者在后，截到
    /// `PENDING_CAP`（顺序与截断点与收拢前逐位一致）。
    pub fn end_retry_cycle(&mut self, retry_still_failed: Vec<[u8; 16]>) {
        let mut still_pending = core::mem::take(&mut self.pending);
        still_pending.extend(retry_still_failed);
        still_pending.truncate(PENDING_CAP);
        self.pending = still_pending;
    }

    /// 待重试数（观测面——空闲重试收敛的可观测事实）。
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// 终态行一次性判定：首次 true（并登记），重复 false。
    pub fn mark_done_if_first(&mut self, plan_id: PlanId) -> bool {
        if self.done_logged.contains(&plan_id) {
            return false;
        }
        self.done_logged.push(plan_id);
        Self::evict_bounded(&mut self.done_logged, DONE_CAP);
        true
    }

    /// 钉面失败一次（hold 拒绝/装配缺失）。
    pub fn note_pin_failure(&mut self) {
        self.pin_failures = self.pin_failures.saturating_add(1);
    }

    /// 钉面失败计数（Deepening-v1 §A.3.3：原 `pub held_pin_failures`
    /// 字段两写入点零读者——观测意图在此兑现，字段已私有）。
    pub fn pin_failures(&self) -> u64 {
        self.pin_failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(n: u8) -> [u8; 16] {
        let mut id = [0u8; 16];
        id[0] = n;
        id
    }

    fn plan(n: u8) -> PlanId {
        PlanId([n; 16])
    }

    /// R-A1 表级锁定：代次表 16/17 边界——第 17 个键准入后最小键被
    /// 淘汰，其余 16 键代次保持（收拢前 `while len() > 16` 语义）。
    #[test]
    fn epoch_table_evicts_to_16_and_keeps_survivors() {
        let mut r = HoldRegistry::new();
        for n in 0..=16u8 {
            assert_eq!(r.admit(sid(n)), 1, "首轮准入代次 = 1");
        }
        assert_eq!(r.epoch(&sid(0)), 0, "最小键已淘汰 ⇒ 代次归零");
        assert_eq!(r.epoch(&sid(16)), 1, "最新键保留");
        assert_eq!(r.admit(sid(16)), 2, "重新准入 +1");
        assert_eq!(r.epoch(&sid(1)), 1, "在 16 容内代次保持");
        r.admit(sid(17));
        assert_eq!(r.epoch(&sid(1)), 0, "再淘汰一轮：次小键出局");
    }

    /// 同快照重复准入 ⇒ 代次单调 +1（wrapping），不新增键。
    #[test]
    fn epoch_increments_per_admission() {
        let mut r = HoldRegistry::new();
        for expected in 1..=5u32 {
            assert_eq!(r.admit(sid(9)), expected);
        }
        assert_eq!(r.admit(sid(7)), 1, "新快照独立代次");
    }

    /// R-A1：released 缓存 = push 后 `>16 remove(0)`（环形最旧先出，
    /// 容量恰 16 时可增至 16 不截）。
    #[test]
    fn released_cache_is_fifo_bounded_at_16() {
        let mut r = HoldRegistry::new();
        for n in 0..16u8 {
            r.admit(sid(n));
            r.note_released(sid(n));
        }
        assert!(
            (0..16).all(|n| r.released_at_current_epoch(&sid(n))),
            "16 项全部在册"
        );
        r.admit(sid(16));
        r.note_released(sid(16));
        assert!(!r.released_at_current_epoch(&sid(0)), "最旧事实出局");
        assert!(
            (1..=16).all(|n| r.released_at_current_epoch(&sid(n))),
            "余 16 项在册"
        );
        assert_eq!(r.released.len(), 16, "容量恰 16");
    }

    /// F70-04 代次安全：旧代次的释放事实不得抑制新一轮释放——重新准入
    /// 后 `released_at_current_epoch` 转 false，剪除后缓存清空。
    #[test]
    fn re_admission_invalidates_stale_release_fact() {
        let mut r = HoldRegistry::new();
        r.admit(sid(3));
        r.note_released(sid(3));
        assert!(r.released_at_current_epoch(&sid(3)));
        r.admit(sid(3));
        assert!(
            !r.released_at_current_epoch(&sid(3)),
            "新代次 ⇒ 旧完成事实失效"
        );
        r.prune_releases_by_epoch();
        assert!(r.released.is_empty(), "剪除后缓存清空");
    }

    /// R-A1：待重试队列两处界限——入队闸门 `<16` 与收尾 `truncate(16)`
    /// 顺序（本 pass 新失败在前、重试仍失败在后）。
    #[test]
    fn pending_queue_bounds_and_merge_order() {
        let mut r = HoldRegistry::new();
        for n in 0..20u8 {
            r.queue_pending(sid(n));
        }
        assert_eq!(r.pending_count(), 16, "入队闸门 ≤16");
        assert!(r.contains_pending(&sid(0)) && r.contains_pending(&sid(15)));
        assert!(!r.contains_pending(&sid(16)), "满额后丢弃");

        let mut r = HoldRegistry::new();
        r.queue_pending(sid(1));
        let retry = r.begin_retry_cycle();
        assert_eq!(retry, alloc::vec![sid(1)]);
        assert_eq!(r.pending_count(), 0, "pass 内 pending 从零累计");
        r.queue_pending(sid(2));
        r.end_retry_cycle(alloc::vec![sid(3)]);
        assert_eq!(
            r.pending,
            alloc::vec![sid(2), sid(3)],
            "本 pass 在前、重试仍失败在后"
        );

        let mut r = HoldRegistry::new();
        for n in 0..16u8 {
            r.queue_pending(sid(n));
        }
        r.end_retry_cycle((100..110u8).map(sid).collect());
        assert_eq!(r.pending_count(), 16, "truncate(16) 截断点");
    }

    /// 存活引用剪除：仍被引用者留在待重试集，引用归零者放行重试。
    #[test]
    fn pending_pruned_by_live_refs_only() {
        let mut r = HoldRegistry::new();
        r.queue_pending(sid(1));
        r.queue_pending(sid(2));
        r.prune_pending_by_live_refs(&[(sid(1), 2), (sid(9), 0)]);
        assert_eq!(
            r.pending,
            alloc::vec![sid(2)],
            "有存活引用 ⇒ 不撤销新持有人的钉"
        );
    }

    /// 终态行一次性判定：首次 true、重复 false、容量 16 环形。
    #[test]
    fn done_marker_is_one_shot_and_bounded() {
        let mut r = HoldRegistry::new();
        assert!(r.mark_done_if_first(plan(99)));
        assert!(!r.mark_done_if_first(plan(99)), "重复终态不得二次打印");
        for n in 0..16u8 {
            r.mark_done_if_first(plan(n));
        }
        assert_eq!(r.done_logged.len(), 16, "容量恰 16，最旧 plan(99) 出局");
        assert!(
            !r.mark_done_if_first(plan(0)),
            "在册标记不得二次打印（plan(0) 未出局）"
        );
        assert!(
            r.mark_done_if_first(plan(99)),
            "环形淘汰掉的最旧标记视为新一轮（有界缓存的既定取舍）"
        );
    }

    /// 钉面失败计数：saturating，观测面兑现零读者缺口。
    #[test]
    fn pin_failures_are_counted_and_observed() {
        let mut r = HoldRegistry::new();
        assert_eq!(r.pin_failures(), 0);
        r.note_pin_failure();
        r.note_pin_failure();
        assert_eq!(r.pin_failures(), 2);
    }
}
