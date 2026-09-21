//! Deepening-v1 §A：快照钉住生命周期深 module（原 `DispatchEngine` 五字段收拢）。
//!
//! 归属边界：本 module 管**钉住账目**（代次、已释放事实、待重试队列、
//! 终态行一次性标记、钉面失败计数）及其**释放协议**。保留面本身
//!（[`HeldInputRetainer`] 装配）、持有计划介质（`DurablePlans`）与**存活
//! 引用计数的事实源**（持久计划）在 engine 侧——engine 据计划算出
//! `live_refs` 表传入，引用**派生**归 engine、引用**查表**归本模块
//!（唯一实现 `live_ref_count`，R-A2）。
//!
//! 协议归属（本卷 §11.3 第 2 条自查订正）：收拢首版只搬状态、把六条合取
//! 释放判据留在 `held_engine`，于是"剪除 → 取出重试集 → 收尾合并"的时序
//! 耦合靠注释维系——正是本候选宣称要消灭的形态。现版把释放协议整体收进
//! 本模块：调用方只需按序 `begin_finalize_pass` / `release_if_idle` /
//! `retry_pending`，重试集对内私有（外部不可见、不可漏步），
//! `pending`/`released`/`retry_batch` 三容器不再暴露任何可变面。
//!
//! 有界淘汰：收拢前六处四种惯用法（`while>16` 退到 16 / `truncate(16)`
//! 截到 16 / `<16` 才推入 / `push`+`>16 remove(0)`）在此按容器各自收拢为
//! 四处单点（`admit` 内代次表淘汰、`evict_bounded` 两处复用、`queue_pending`
//! 入队闸门、`retry_pending` 收尾截断），语义逐位保持——不是合并成"一个
//! 全局淘汰函数"（四种策略本就不同，强行合一会改行为）。R-A1 由本模块
//! 表级测试钉死每容器 16/17 边界**与淘汰受害者身份**。
//!
//! F70-04（§7.2.6）代次安全不变量由注释维系升级为结构维系：代次只在
//! [`HoldRegistry::admit`] 递增，幂等判定只在
//! [`HoldRegistry::released_at_current_epoch`] 发生，旧代次完成事实不可能
//! 抑制新一轮释放。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_decision_contract::PlanId;

use super::held_engine::HeldInputRetainer;

/// 代次表容量（>16 退到 16；受害者 = BTreeMap **最小键**（字节序），
/// 不等于"最早准入"——两者只在键与时间同序时重合）。
const EPOCH_CAP: usize = 16;
/// 已释放事实容量（FIFO，按插入序最旧先出）。
const RELEASED_CAP: usize = 16;
/// 待重试释放队列容量（入队闸门 `<16` + 收尾 `truncate(16)`）。
const PENDING_CAP: usize = 16;
/// 终态行一次性标记容量（FIFO）。
const DONE_CAP: usize = 16;

/// 钉住账目与释放协议（六字段全私有；观测面唯一入口 = 本模块方法）。
pub(crate) struct HoldRegistry {
    /// 快照持有代次（每次成功准入 +1；有界 ≤`EPOCH_CAP`）。
    epochs: BTreeMap<[u8; 16], u32>,
    /// 已解除钉的 (快照 ID, 持有代次)——防重复 release。
    released: Vec<([u8; 16], u32)>,
    /// 本 pass 新入队的释放失败快照（≤`PENDING_CAP`）。
    pending: Vec<[u8; 16]>,
    /// 进入本 pass 前已在册的待重试集（pass 内私有）。
    retry_batch: Vec<[u8; 16]>,
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
            retry_batch: Vec::new(),
            done_logged: Vec::new(),
            pin_failures: 0,
        }
    }

    /// FIFO 有界环形（`remove(0)` = 最早插入）——released/done_logged 共用。
    fn evict_bounded<T>(buf: &mut Vec<T>, cap: usize) {
        while buf.len() > cap {
            buf.remove(0);
        }
    }

    /// 准入持有：代次 +1（wrapping），超容量淘汰最小键。
    ///
    /// 淘汰只引发至多一次幂等重释放，不损正确性（§7.2.6 原文承诺）。
    pub fn admit(&mut self, snapshot_id: [u8; 16]) {
        let epoch = self.epochs.entry(snapshot_id).or_insert(0);
        *epoch = epoch.wrapping_add(1);
        while self.epochs.len() > EPOCH_CAP {
            let smallest = *self.epochs.keys().next().expect("nonempty");
            self.epochs.remove(&smallest);
        }
    }

    /// 当前代次（从未持有 = 0）。
    fn epoch(&self, snapshot_id: &[u8; 16]) -> u32 {
        self.epochs.get(snapshot_id).copied().unwrap_or(0)
    }

    /// 存活引用查表（表由 engine 依持久计划派生——派生归属在 engine）。
    fn live_ref_count(&self, live_refs: &BTreeMap<[u8; 16], usize>, id: &[u8; 16]) -> usize {
        live_refs.get(id).copied().unwrap_or(0)
    }

    /// 本代次是否已完成释放（幂等重释放抑制判据）。
    fn released_at_current_epoch(&self, snapshot_id: &[u8; 16]) -> bool {
        let epoch = self.epoch(snapshot_id);
        self.released
            .iter()
            .any(|(id, e)| id == snapshot_id && *e == epoch)
    }

    /// 登记释放完成事实（带当代代次）。
    fn note_released(&mut self, snapshot_id: [u8; 16]) {
        let epoch = self.epoch(&snapshot_id);
        self.released.push((snapshot_id, epoch));
        Self::evict_bounded(&mut self.released, RELEASED_CAP);
    }

    /// 释放失败入队（满额即丢弃本轮——有界，不无界积累）。被丢弃者不致
    /// 永久漏放：其计划仍终态 ⇒ 下个 pass 仍进 `release_if_idle` 重判。
    fn queue_pending(&mut self, snapshot_id: [u8; 16]) {
        if self.pending.len() < PENDING_CAP {
            self.pending.push(snapshot_id);
        }
    }

    /// 收尾 pass 第一步（engine 的 `finalize_held_plans` 入口调用）：
    /// ① §7.2.6 代次安全剪除——快照重新获得存活引用（新一轮合法持有）⇒
    ///    旧完成事实失效（覆盖"准入后同 pass 内完成"：此时引用已归零、
    ///    但新一轮释放不得被旧缓存抑制）；② 待释放队列按存活引用剪除
    ///   （仍被在途/未决引用 ⇒ 不撤销新持有人的钉）；③ 已在册失败项转入
    ///    本 pass 私有重试集（§7.2.4「下个实际调度到的周期」语义——本
    ///    pass 新失败者另账入队）。
    pub fn begin_finalize_pass(&mut self, live_refs: &BTreeMap<[u8; 16], usize>) {
        // 代次判据唯一经 `self.epoch`（避免与 epochs 字段双读漂移）；
        // 取出-回填只为解开同结构体内 `released` 可变借用与 `epochs`
        // 不可借用的冲突（收拢前两字段分居 engine 时不存在此约束）。
        let mut released = core::mem::take(&mut self.released);
        released.retain(|(id, released_epoch)| self.epoch(id) == *released_epoch);
        self.released = released;
        // 引用剪除与重试集取出合为一笔：剪后的 pending 整体转入 pass 私有
        // 重试集（`self.pending` 留空，本 pass 新失败者从零累计——与收拢前
        // `retain` 后 `mem::take` 的可达状态逐位一致）。
        let mut pending = core::mem::take(&mut self.pending);
        pending.retain(|id| self.live_ref_count(live_refs, id) == 0);
        self.retry_batch = pending;
    }

    /// 释放面（§7.2.3）单点：快照**无存活引用**（其他在途/未决计划仍引用
    /// ⇒ 保留）、本代次未记过完成、且不在本 pass 待重试集内，才发起释放；
    /// 成功记事实、失败入有界队列（§7.2.4/§7.2.7）。
    ///
    /// 调用方保留 `terminal && !unresolved` 前置——那是**计划**事实，不是
    /// 钉住账目。串口终态行不在此（G0：词形生成点留在 engine）。
    pub fn release_if_idle(
        &mut self,
        snapshot_id: [u8; 16],
        live_refs: &BTreeMap<[u8; 16], usize>,
        retainer: &mut Option<Box<dyn HeldInputRetainer>>,
    ) {
        if self.released_at_current_epoch(&snapshot_id)
            || self.pending.contains(&snapshot_id)
            || self.retry_batch.contains(&snapshot_id)
            || self.live_ref_count(live_refs, &snapshot_id) != 0
        {
            return;
        }
        let released = retainer
            .as_mut()
            .map(|r| r.release(snapshot_id))
            .unwrap_or(false);
        if released {
            self.note_released(snapshot_id);
        } else {
            self.queue_pending(snapshot_id);
        }
    }

    /// 收尾 pass 末步（§7.2.4：不依赖新提案，空闲周期也走到这里；
    /// §7.2.5：远端幂等确认，重试至收敛）。引用剪除已在
    /// [`HoldRegistry::begin_finalize_pass`] 完成，此处集合都无存活引用。
    /// 收尾合并 = 本 pass 新失败在前、重试仍失败在後，截到 `PENDING_CAP`
    ///（顺序与截断点与收拢前逐位一致）。
    pub fn retry_pending(&mut self, retainer: &mut Option<Box<dyn HeldInputRetainer>>) {
        let mut still_failed: Vec<[u8; 16]> = Vec::new();
        for id in core::mem::take(&mut self.retry_batch) {
            let released = retainer.as_mut().map(|r| r.release(id)).unwrap_or(false);
            if !released {
                still_failed.push(id);
            } else if !self.released_at_current_epoch(&id) {
                self.note_released(id);
            }
        }
        let mut still_pending = core::mem::take(&mut self.pending);
        still_pending.extend(still_failed);
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

    /// 钉面失败计数。§A.3.3 口径：原 `pub held_pin_failures` 字段两写
    /// 零点可读 ⇒ 改为只读方法；生产侧读者仍为零（本模块与 engine 观测面
    /// 提供读取通道，接入消费者随后续诊断面），**不宣称已闭合零读者**。
    pub fn pin_failures(&self) -> u64 {
        self.pin_failures
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    fn sid(n: u8) -> [u8; 16] {
        let mut id = [0u8; 16];
        id[0] = n;
        id
    }

    fn plan(n: u8) -> PlanId {
        PlanId([n; 16])
    }

    fn refs(v: &[([u8; 16], usize)]) -> BTreeMap<[u8; 16], usize> {
        v.iter().copied().collect()
    }

    #[derive(Default)]
    struct FakeRetainer {
        release_ok: bool,
    }
    impl HeldInputRetainer for FakeRetainer {
        fn hold(&mut self, _snapshot_id: [u8; 16]) -> bool {
            true
        }
        fn release(&mut self, _snapshot_id: [u8; 16]) -> bool {
            self.release_ok
        }
    }

    /// 装配保留面：`ok = true` 释放成功、`false` 恒失败（触发出队路径）。
    fn face(ok: bool) -> Option<Box<dyn HeldInputRetainer>> {
        Some(Box::new(FakeRetainer { release_ok: ok }))
    }

    /// R-A1 表级锁定：代次表 16/17 边界——第 17 个键准入后**最小键**出局，
    /// 其余 16 键代次保持（收拢前 `while len() > 16` 语义）。
    #[test]
    fn epoch_table_evicts_min_key_at_17th_admission() {
        let mut r = HoldRegistry::new();
        for n in 0..=16u8 {
            r.admit(sid(n));
            assert_eq!(r.epoch(&sid(n)), 1, "首轮准入代次 = 1");
        }
        assert_eq!(r.epoch(&sid(0)), 0, "最小键已淘汰 ⇒ 代次归零");
        assert_eq!(r.epoch(&sid(16)), 1, "最新键保留");
        r.admit(sid(16));
        assert_eq!(r.epoch(&sid(16)), 2, "重新准入 +1（不新增键）");
        assert_eq!(r.epoch(&sid(1)), 1, "16 容内代次保持");
        r.admit(sid(17));
        assert_eq!(r.epoch(&sid(1)), 0, "再淘汰一轮：次小键出局");
    }

    /// 同快照重复准入 ⇒ 代次单调 +1，新快照独立起算。
    #[test]
    fn epoch_increments_per_admission() {
        let mut r = HoldRegistry::new();
        for expected in 1..=5u32 {
            r.admit(sid(9));
            assert_eq!(r.epoch(&sid(9)), expected);
        }
        r.admit(sid(7));
        assert_eq!(r.epoch(&sid(7)), 1, "新快照独立代次");
    }

    /// R-A1：released 缓存 = push 后 `>16 remove(0)`（**插入序**淘汰，与
    /// 代次表的最小键判据不同族）。断言直验受害者身份——只断
    /// `released_at_current_epoch` 会被代次重置蒙过（自查第 4 条）。
    #[test]
    fn released_cache_evicts_earliest_inserted_at_17th() {
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
        assert!(
            !r.released.iter().any(|(id, _)| *id == sid(0)),
            "FIFO 受害者 = 最早插入的 sid(0)（身份直证）"
        );
        assert!(
            r.released.iter().any(|(id, _)| *id == sid(16)),
            "最新事实在册"
        );
        assert_eq!(r.released.len(), 16, "容量恰 16");
    }

    /// F70-04 代次安全：旧代次释放事实不得抑制新一轮释放——重新准入后
    /// 幂等判据转 false，pass 剪除后缓存清空。
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
        r.begin_finalize_pass(&refs(&[]));
        assert!(r.released.is_empty(), "剪除后缓存清空");
    }

    /// R-A1：待重试入队闸门 `<16`（第 17 个丢弃）+ 收尾 `truncate(16)`
    /// 的**受害者位置** + 合并顺序（本 pass 在前、重试在後）。
    #[test]
    fn pending_queue_bounds_and_merge_order() {
        let mut r = HoldRegistry::new();
        for n in 0..20u8 {
            r.queue_pending(sid(n));
        }
        assert_eq!(r.pending_count(), 16, "入队闸门 ≤16");
        assert_eq!(
            r.pending,
            (0..16u8).map(sid).collect::<Vec<_>>(),
            "闸门丢弃第 17 个之后的新失败"
        );

        // 顺序：本 pass 新失败在前、重试仍失败在後。
        let mut r = HoldRegistry::new();
        r.queue_pending(sid(2));
        r.retry_batch = vec![sid(1)];
        r.retry_pending(&mut face(false));
        assert_eq!(r.pending, vec![sid(2), sid(1)], "合并顺序逐位保持");

        // 截断受害者：满 16 时新失败全留、重试项出局（只断长度则
        // "留后 16 个"的错误实现会全绿——自查第 1 条）。
        let mut r = HoldRegistry::new();
        for n in 0..16u8 {
            r.queue_pending(sid(n));
        }
        r.retry_batch = (100..110u8).map(sid).collect();
        r.retry_pending(&mut face(false));
        assert_eq!(r.pending.len(), 16, "truncate(16) 截断点");
        assert_eq!(
            r.pending,
            (0..16u8).map(sid).collect::<Vec<_>>(),
            "被截断的是队尾重试项，本 pass 新失败项必须全留"
        );
    }

    /// 存活引用双用：pass 入口剪除重试集 + `release_if_idle` 拒放有引用者。
    #[test]
    fn live_refs_gate_requeue_and_release() {
        let mut r = HoldRegistry::new();
        r.queue_pending(sid(1));
        r.queue_pending(sid(2));
        r.begin_finalize_pass(&refs(&[(sid(1), 2), (sid(9), 0)]));
        assert_eq!(
            r.retry_batch,
            vec![sid(2)],
            "有存活引用 ⇒ 不撤销新持有人的钉"
        );

        let live = refs(&[(sid(5), 1)]);
        r.admit(sid(5));
        r.release_if_idle(sid(5), &live, &mut face(true));
        assert_eq!(r.pending_count(), 0, "仍被在途计划引用 ⇒ 不释放");
        assert!(
            !r.released_at_current_epoch(&sid(5)),
            "有引用 ⇒ 不得记完成事实"
        );
    }

    /// 幂等三重门：本代次已记完成 / 本 pass 已入队 / 已在重试集 ⇒ 不再发起。
    #[test]
    fn release_is_idempotent_across_three_gates() {
        let empty = refs(&[]);

        let mut r = HoldRegistry::new();
        r.admit(sid(4));
        r.release_if_idle(sid(4), &empty, &mut face(false));
        assert_eq!(r.pending_count(), 1, "首次失败入队");
        r.release_if_idle(sid(4), &empty, &mut face(false));
        assert_eq!(r.pending_count(), 1, "本 pass 重复请求不再入队");

        let mut r = HoldRegistry::new();
        r.admit(sid(4));
        r.note_released(sid(4));
        r.release_if_idle(sid(4), &empty, &mut face(false));
        assert_eq!(r.pending_count(), 0, "本代次已记完成 ⇒ 不重释放");

        let mut r = HoldRegistry::new();
        r.admit(sid(4));
        r.queue_pending(sid(4));
        r.begin_finalize_pass(&empty);
        assert_eq!(r.retry_batch, vec![sid(4)]);
        r.release_if_idle(sid(4), &empty, &mut face(false));
        assert_eq!(r.pending_count(), 0, "已在重试集 ⇒ 本 pass 不重复入队");
    }

    /// 释放成功记事实；重试收敛（释放转 OK 后事实登记、队列清空）。
    #[test]
    fn release_success_notes_fact_and_retry_converges() {
        let empty = refs(&[]);
        let mut r = HoldRegistry::new();
        r.admit(sid(6));
        r.release_if_idle(sid(6), &empty, &mut face(false));
        assert_eq!(r.pending_count(), 1);
        r.begin_finalize_pass(&empty);
        r.retry_pending(&mut face(true));
        assert_eq!(r.pending_count(), 0, "重试收敛");
        assert!(r.released_at_current_epoch(&sid(6)), "事实带当代代次");
        assert_eq!(r.epoch(&sid(6)), 1, "重试收敛不改代次");
    }

    /// 终态行一次性判定：首次 true、重复 false、容量 16 环形淘汰。
    #[test]
    fn done_marker_is_one_shot_and_bounded() {
        let mut r = HoldRegistry::new();
        assert!(r.mark_done_if_first(plan(99)));
        assert!(!r.mark_done_if_first(plan(99)), "重复终态不得二次打印");
        for n in 0..16u8 {
            r.mark_done_if_first(plan(n));
        }
        assert_eq!(r.done_logged.len(), 16, "容量恰 16");
        assert!(
            !r.done_logged.contains(&plan(99)),
            "FIFO 受害者 = 最早登记的 plan(99)"
        );
        assert!(
            !r.mark_done_if_first(plan(0)),
            "在册标记不得二次打印（plan(0) 未出局）"
        );
        assert!(
            r.mark_done_if_first(plan(99)),
            "环形淘汰掉的最旧标记视为新一轮（有界缓存的既定取舍）"
        );
    }

    /// 钉面失败计数 saturating + 只读观测面。
    #[test]
    fn pin_failures_are_counted_and_observed() {
        let mut r = HoldRegistry::new();
        assert_eq!(r.pin_failures(), 0);
        r.note_pin_failure();
        r.note_pin_failure();
        assert_eq!(r.pin_failures(), 2);
    }
}
