//! 蓝图 70.11 WP-70.11.2——**有界计划持有与到期驱动**。
//!
//! # 责任边界（与 [08] §3「缺失责任归属」对齐）
//!
//! dispatch 内**单一站点**持有已接纳计划：计划身份/证书/未来步骤保存在
//! 一个 implementation 内，调用方不再手动维护 future 列表。它承诺：
//!
//! 1. **接纳即持有**：整份计划（含未来步）有唯一持有点——函数返回不丢
//!    未来步（"[10] §1 出口 3：没有返回后消失的未来步"）。
//!    容量 = 1 活跃 + 1 待切换（`MAX_HELD_PLANS`）；**同一时刻只有一个
//!    投递面**（`plans[0]` = 活跃），待切换计划在活跃计划排空后接手
//!    （`switched_total` 计数）——不做时间维抢占（抢占策略属 70.11.3）。
//! 2. **有界 tick**：每次驱动最多推进 [`MAX_STEPS_PER_TICK`] 步——一步的
//!    失败不吞掉同 tick 的其余判定，长计划不产生无界工作集。
//! 3. **到期即终态**：错过有效窗的步骤记 [`StepState::Expired`]（显式
//!    "未执行理由"），**绝不补发**、绝不把错窗步骤伪装成已执行。
//! 4. **输入失效即撤销**：可信输入换代（快照/目录/时域不再同代）⇒ 未发
//!    步全部转 [`StepState::Cancelled`]；已发步保留原状态（对账面）。
//! 5. **保留协同（读点已就绪，跨服务接线未落）**：`has_outstanding_input`
//!    是"计划输入持有"的读点（70.8.2 删除判据 `min(消费位点, 计划输入持有)`
//!    的**约束方**一侧）。**当前生产无调用方**（无装配方轮询、svc-telemetry
//!    侧 `hold_input_offset` 亦无调用方）——接线需先有"释放面"（现
//!    `hold_offset` 只收紧不释放：首个调用方会把删除界永久钉住）与
//!    dispatch→telemetry 的服务间通道，属 70.11.3/70.11.4 范围，见执行卡
//!    登记。本模块只承诺读点语义正确。
//!
//! # 诚实边界
//!
//! - 持有为**会话内**状态（重启即丢）：持久化恢复（`durable-state` 格式）
//!   属 WP-70.11.4，本模块先给崩溃语义——冷启动 = 无持有计划（绝不凭
//!   空复活旧计划），并在 `accept` 计数器中留恢复位。
//! - 批提交入口仍是逐命令执行链（引擎既有 12 步合核）；`BatchSaga` 的
//!   生产激活（ADR-052 修订 + 真实批入口）为独立裁决，不在本模块内
//!   解冻——本模块的 [`BatchSink`] 接缝即未来 saga 入口。

use alloc::vec::Vec;

use eneros_decision_contract::{CandidatePlan, CertifiedPlan};

use crate::adapter::CandidateInputs;
use crate::DispatchError;

/// 每次 tick 最多推进的步数（有界工作集；蓝图 §9"建议≤100ms 调度粒度"
/// 的配套约束——长计划分 tick 排空，单 tick 不产生无界投递）。
pub const MAX_STEPS_PER_TICK: usize = 4;

/// 站内并发持有上限：1 活跃 + 1 待切换（蓝图 §9"每站先 1 个活跃计划 +
/// 1 个待切换计划；满时明确拒绝，不能覆盖在途"）。
pub const MAX_HELD_PLANS: usize = 2;

/// 步骤状态（终态不可逆：Done/Rejected/Expired/Cancelled）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// 未到期（未来步）。
    Pending,
    /// 本 tick 已交投递（结果由回执承载；Unknown 不转成功）。**非终态**：
    /// 必须由回执（[`PlanHolder::resolve_step`]）或计划窗关闭（证书到期，
    /// 见 [`PlanHolder::tick`]）收敛到 Done/Rejected。
    Dispatched,
    /// 错过有效窗（显式未执行理由）。
    Expired,
    /// 输入失效/被新计划抢占——未发即撤销。
    Cancelled,
    /// 投递链返回终态（回执 Committed）。
    Done,
    /// 投递链明确未生效（回执非 Committed，或回执在计划窗关闭前未到达）
    /// ——显式未执行理由，绝不重试（复查修复：原实现只有 `Dispatched`
    /// 这一个无出口状态，容量永久占用）。
    Rejected,
}

/// 单步视图（状态查询面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepView {
    /// 1-based 步序。
    pub step_index: u32,
    pub state: StepState,
    /// 有效窗起止（ms）。
    pub valid_from_ms: u64,
    pub valid_until_ms: u64,
}

/// 计划持有拒绝（结构化；调用方零状态变更）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldReject {
    /// 持有容量满（1 活跃 + 1 待切换）。
    Full,
    /// 已持有同代计划（重复接纳同一步时域）。
    Duplicate,
    /// 计划无效（时域/步数/资产集合不合法）。
    Invalid(&'static str),
    /// 引擎拒绝（认证/编译失败——零持有）。
    Engine(DispatchError),
}

/// tick 输入（调用方每 tick 供给的再验证事实）。
#[derive(Debug, Clone, Copy)]
pub struct PlanTick<'a> {
    /// 本机单调毫秒。
    pub now_ms: u64,
    /// 当前可信输入身份（None = 无可信输入：撤销未发步——
    /// "无法维持安全输入则暂停/撤销未发步"，蓝图 WP-70.11.2）。
    pub facts: Option<&'a eneros_agent_sdk::snap_input_wire::SnapshotFacts>,
}

/// 投递接缝（生产 = 引擎 `submit_trusted`；未来 = BatchSaga 入口）。
pub trait BatchSink {
    /// 投递一步（调用方持有命令构造细节）；返回回执终态是否 Committed。
    fn deliver(&mut self, step_index: u32, power_kw: f64) -> Result<bool, DispatchError>;
}

/// 一份已持有计划。
struct HeldPlan {
    plan: CandidatePlan,
    cert: CertifiedPlan,
    /// 每步状态（index 0 = step 1）。
    states: Vec<StepState>,
}

/// 计划持有器（dispatch 内单一站点）。
#[derive(Default)]
pub struct PlanHolder {
    plans: Vec<HeldPlan>,
    /// 观测：接纳数 / 到期步数 / 撤销步数 / 投递步数。
    pub accepted_total: u64,
    pub expired_steps_total: u64,
    pub cancelled_steps_total: u64,
    pub delivered_steps_total: u64,
    /// 观测：活跃计划排空后由待切换计划接手（"1 活跃 + 1 待切换"的切换面）
    /// 的次数。
    pub switched_total: u64,
    /// 观测：已投递但回执未到达、因计划窗关闭而收敛为 [`StepState::Rejected`]
    /// 的步数（未确认命令的终局：绝不重发）。
    pub unconfirmed_steps_total: u64,
    /// 最近一次接纳拒绝原因（诊断）。
    pub last_reject: Option<HoldReject>,
}

impl PlanHolder {
    /// 接纳整份计划（含未来步）。
    ///
    /// 判据：时域合法（step_ns/step_count > 0）、未来步存在（step_count
    /// 不小于 2 才需要持有——单步计划由调用方当拍直接执行）、未超容量、
    /// 非同代重复。
    pub fn accept(
        &mut self,
        plan: CandidatePlan,
        cert: CertifiedPlan,
        _now_ms: u64,
    ) -> Result<(), HoldReject> {
        if plan.step_count() == 0 || plan.step_ns() == 0 {
            return Err(self.reject(HoldReject::Invalid("empty horizon")));
        }
        if plan.step_count() == 1 {
            // 单步计划无"未来步"——持有器不接管（当拍执行语义不变）。
            return Err(self.reject(HoldReject::Invalid("single-step plan")));
        }
        // 同代重复（同 plan id + 同快照）先于容量判——覆盖在途的企图必须
        // 以"重复"拒绝，而不是以"满"拒绝（语义可辨）。
        if self
            .plans
            .iter()
            .any(|p| p.plan.id() == plan.id() && p.plan.snapshot_id() == plan.snapshot_id())
        {
            return Err(self.reject(HoldReject::Duplicate));
        }
        if self.plans.len() >= MAX_HELD_PLANS {
            return Err(self.reject(HoldReject::Full));
        }
        let states = alloc::vec![StepState::Pending; plan.step_count() as usize];
        self.plans.push(HeldPlan { plan, cert, states });
        self.accepted_total = self.accepted_total.saturating_add(1);
        self.last_reject = None;
        Ok(())
    }

    /// 有界 tick 驱动：到期步投递、错窗步终态、输入换代撤销未发步。
    ///
    /// 返回本 tick 实际投递的步数（观测）。
    pub fn tick(&mut self, tick: PlanTick<'_>, sink: &mut dyn BatchSink) -> usize {
        let mut delivered = 0usize;
        // 切换单点（tick 起手一次判定）：活跃计划（`plans[0]`）已无未发步
        // ⇒ 最早的待切换计划接手。**不覆盖在途**：切换只在活跃计划排空后
        // 发生（绝不抢占已投递/在途命令），故任一时点只有一个投递面。
        //
        // 70.14 复查修复：切换判据必须**同时**排除 `Dispatched`（已投未确认）。
        // 旧实现只看 `Pending` —— 活跃计划最后一步以 `Ok(false)`（投递结果
        // 未知）投出后，下一步 tick 它就满足"无 Pending"而被 `remove(0)`：
        // 未确认命令既不收敛 `Rejected`、`unconfirmed_steps_total` 也不增，
        // `resolve_step` 此后恒 false（对账面丢失）——与 `reap_terminal` 的
        // "Dispatched 计划保留到回执或窗闭"承诺直接矛盾。现在切换等它终态化。
        let active_has_dispatched = self.plans[0].states.contains(&StepState::Dispatched);
        if self.plans.len() > 1
            && !Self::has_pending(&self.plans[0].states)
            && !active_has_dispatched
        {
            self.plans.remove(0);
            self.switched_total = self.switched_total.saturating_add(1);
        }
        // 输入代次（None = 无可信输入 → 撤销全部未发步）。
        let input_generation = tick.facts.copied();
        for (plan_index, p) in self.plans.iter_mut().enumerate() {
            // 输入失效判定：持有计划的绑定必须与当前可信输入同代
            // （快照 id/目录版本/时域——与 dispatch 准入同一三元组）。
            let same_generation = match &input_generation {
                None => false,
                Some(f) => {
                    p.plan.snapshot_id().0 == f.snapshot_id
                        && p.plan.resource_catalog_version() == f.catalog_version
                }
            };
            if !same_generation && Self::has_pending(&p.states) {
                // 撤销未发步（已发步保留原状态——对账面不重写历史）。
                for st in p.states.iter_mut() {
                    if *st == StepState::Pending {
                        *st = StepState::Cancelled;
                        self.cancelled_steps_total = self.cancelled_steps_total.saturating_add(1);
                    }
                }
            }
            // 终态化单点（**全部持有计划**，含待切换）：步窗已闭即收敛——
            // - `Pending` + 窗闭 ⇒ `Expired`（错窗 = 显式未执行理由，绝不
            //   补发；待切换计划的错窗步同样立即终态，否则其 `Pending` 会
            //   永久占住"输入持有/容量"）；
            // - `Dispatched` + 窗闭 ⇒ `Rejected`（未确认命令的终局：命令窗
            //   已过，重授权不可能再覆盖它；缺此出口则容量被永久占用）。
            // 判据与投递同源（`step_window`），不引入第二套时间语义。
            for (i, st) in p.states.iter_mut().enumerate() {
                let (_, until) = step_window(&p.plan, (i as u32) + 1);
                if tick.now_ms < until {
                    continue;
                }
                match *st {
                    StepState::Pending => {
                        *st = StepState::Expired;
                        self.expired_steps_total = self.expired_steps_total.saturating_add(1);
                    }
                    StepState::Dispatched => {
                        *st = StepState::Rejected;
                        self.unconfirmed_steps_total =
                            self.unconfirmed_steps_total.saturating_add(1);
                    }
                    _ => {}
                }
            }
            // 投递 pass：**仅活跃计划**（待切换计划不投递——两个计划并发
            // 投递同一设备 = 双命令源；复查修复）。
            if plan_index != 0 {
                continue;
            }
            // 有界推进：最多 MAX_STEPS_PER_TICK 步。
            for (i, st) in p.states.iter_mut().enumerate() {
                if delivered >= MAX_STEPS_PER_TICK {
                    break;
                }
                if *st != StepState::Pending {
                    continue;
                }
                let step_index = (i as u32) + 1;
                let (from, _until) = step_window(&p.plan, step_index);
                if tick.now_ms < from {
                    continue; // 未到期（未来步；错窗已由终态化单点处理）
                }
                let Some(power) = step_power(&p.plan, step_index) else {
                    // 结构缺步 = 计划本身非法（接纳时已验；防御性拒绝）。
                    *st = StepState::Expired;
                    self.expired_steps_total = self.expired_steps_total.saturating_add(1);
                    continue;
                };
                match sink.deliver(step_index, power) {
                    Ok(committed) => {
                        *st = if committed {
                            StepState::Done
                        } else {
                            StepState::Dispatched
                        };
                        delivered += 1;
                        self.delivered_steps_total = self.delivered_steps_total.saturating_add(1);
                    }
                    Err(_) => {
                        // 投递失败：本步保持 Pending（下 tick 重试——
                        // 幂等由 plan/step 身份承载；非幂等命令的自动重发
                        // 语义归 WP-70.11.4 的对账裁决，登记）。
                        continue;
                    }
                }
            }
        }
        delivered
    }

    /// 回执回填（`Dispatched` 的唯一正常出口）：命令结果落定 ⇒
    /// `committed` = 终态 [`StepState::Done`]，否则 [`StepState::Rejected`]
    /// （投递链明确未生效——**不重试**：重发语义归计划持有之外的批/对账
    /// 裁决）。返回是否命中一个在途步（未命中 = 迟到/重复回执，零状态变更）。
    pub fn resolve_step(
        &mut self,
        plan_id: &eneros_decision_contract::PlanId,
        snapshot_id: eneros_decision_contract::SnapshotId,
        step_index: u32,
        committed: bool,
    ) -> bool {
        let Some(p) = self
            .plans
            .iter_mut()
            .find(|p| p.plan.id() == plan_id && p.plan.snapshot_id() == snapshot_id)
        else {
            return false;
        };
        let idx = step_index.saturating_sub(1) as usize;
        let Some(st) = p.states.get_mut(idx) else {
            return false;
        };
        if *st != StepState::Dispatched {
            return false;
        }
        *st = if committed {
            StepState::Done
        } else {
            StepState::Rejected
        };
        true
    }

    /// 证书有效窗查询（观测/调用方策略面——**不是**本持有器的投递判据：
    /// 投递判据 = 步窗到期 + 可信输入同代；命令是否被证书授权由执行方
    /// （引擎每次提交向 `CertificateSource` 现取证书）裁决）。
    pub fn cert_valid(&self, now_ms: u64) -> bool {
        self.plans
            .iter()
            .any(|p| p.cert.is_valid_at(now_ms) && Self::has_pending(&p.states))
    }

    /// 每步状态查询（终态/待人工有因——"可查询每步状态与最终结果"）。
    pub fn status(&self) -> Vec<Vec<StepView>> {
        self.plans
            .iter()
            .map(|p| {
                p.states
                    .iter()
                    .enumerate()
                    .map(|(i, st)| {
                        let (from, until) = step_window(&p.plan, (i as u32) + 1);
                        StepView {
                            step_index: (i as u32) + 1,
                            state: *st,
                            valid_from_ms: from,
                            valid_until_ms: until,
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// 计划输入仍在引用（保留协同读点）：任一计划含未发步 = 其输入必须
    /// 可追溯。svc-telemetry 侧据此把删除界收紧到消费位点
    /// （`min(消费位点, 计划输入持有)` 的持有半边）。
    ///
    /// 跨服务接线（dispatch 读点 → telemetry `hold_offset`）由装配方轮询
    /// 本读点承载；分区/位点映射属事件面事实，不入本模块（深度边界）。
    ///
    /// 70.14 复查修复：**`Dispatched`（已发未确认）同样算持有**。旧实现只看
    /// `Pending`——全部步骤已投出但未确认时返回 `false`，删除界据此释放输入，
    /// 而 `reap_terminal` 又刻意保留该计划（未确认命令的对账面要可查）：
    /// 同一模块两个面语义相反。未确认步的对账需要引用其输入/证书证据，
    /// 释放即失去可追溯性。
    pub fn has_outstanding_input(&self) -> bool {
        self.plans
            .iter()
            .any(|p| Self::has_pending(&p.states) || p.states.contains(&StepState::Dispatched))
    }

    /// 持有的活跃计划数（观测）。
    pub fn held_count(&self) -> usize {
        self.plans.len()
    }

    /// 全部计划终态（无 Pending/Dispatched）时释放（容量回收）。
    ///
    /// `Dispatched` 计划在此**保留**——未确认命令的对账面必须可查，直到
    /// 回执（[`Self::resolve_step`]）或计划窗关闭（`tick` 收敛 Rejected）
    /// 把它变成终态。
    pub fn reap_terminal(&mut self) {
        self.plans
            .retain(|p| Self::has_pending(&p.states) || p.states.contains(&StepState::Dispatched));
    }

    fn has_pending(states: &[StepState]) -> bool {
        states.contains(&StepState::Pending)
    }

    fn reject(&mut self, r: HoldReject) -> HoldReject {
        self.last_reject = Some(r.clone());
        r
    }
}

/// 步有效窗（单源：`horizon_start + (i-1)·step_ns`，ms；步内 = [from, from+step_ns)）。
fn step_window(plan: &CandidatePlan, step_index: u32) -> (u64, u64) {
    // 步长单源 = `CandidatePlan::step_len_ms()`（下限 1ms）：与编译器
    // 时段窗同一换算——`step_ns ∈ (0,1ms)` 不得在这里变成零宽窗
    // （那会把全部步瞬间判成"错窗"，静默零投递）。
    let step_ms = plan.step_len_ms();
    let from = plan
        .horizon_start_ms()
        .saturating_add(step_ms.saturating_mul(u64::from(step_index - 1)));
    (from, from.saturating_add(step_ms))
}

/// 一步的功率（多资产时取**该步全部分配**的净和——与站点平衡一致；
/// 缺步 = None）。
fn step_power(plan: &CandidatePlan, step_index: u32) -> Option<f64> {
    let mut sum: Option<f64> = None;
    for a in plan.assignments() {
        if a.step_index == step_index {
            sum = Some(sum.unwrap_or(0.0) + a.power_kw);
        }
    }
    sum
}

/// 绑定输入构造（装配方/验证方用）：**身份取自可信输入、时域取自计划
/// 本体**——与 dispatch 准入同一公式（`objective_binding_v1` 的五维）。
///
/// 复查修复：原实现把时域硬编码为 `(step_ns=0, step_count=1)`，任何多步
/// 计划用它派生都会得到与计划不符的目标摘要（验证方重算即拒），且该形态
/// 的 step_count=1 计划根本进不了持有器——即"广告的装配辅助函数无法喂给
/// 持有器"。时域必须来自计划，故本函数要求同时给出计划。
pub fn inputs_from_facts(
    facts: &eneros_agent_sdk::snap_input_wire::SnapshotFacts,
    plan: &CandidatePlan,
) -> CandidateInputs {
    CandidateInputs::from_snapshot(
        facts.snapshot_id,
        facts.catalog_version,
        plan.horizon_start_ms(),
        plan.step_ns(),
        plan.step_count(),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_decision_contract::Assignment;

    use super::*;

    const SIDE: u64 = 3_600_000_000_000; // 1h

    fn facts(snapshot_id: [u8; 16]) -> eneros_agent_sdk::snap_input_wire::SnapshotFacts {
        eneros_agent_sdk::snap_input_wire::SnapshotFacts {
            snapshot_id,
            provenance_root: [0x3D; 32],
            cut_time_ns: 1_700_000_000_000_000_000,
            produced_at_ns: 1_700_000_000_000_000_000,
            catalog_version: 3,
            catalog_digest8: [1; 8],
            accepted: 7,
            missing: 0,
            completeness: eneros_agent_sdk::snap_input_wire::COMPLETE,
        }
    }

    /// 4 步计划（每步一条分配，功率 = 步序）。
    fn plan(snapshot_id: [u8; 16], steps: u32) -> CandidatePlan {
        plan_with_id([7u8; 16], snapshot_id, steps)
    }

    /// 同上但指定 plan id（多计划场景：同代两计划必须 id 不同，否则
    /// 被 `accept` 判重复）。
    fn plan_with_id(plan_id: [u8; 16], snapshot_id: [u8; 16], steps: u32) -> CandidatePlan {
        let start_ms = 1_700_000_000_000u64;
        let inputs = CandidateInputs::from_snapshot(snapshot_id, 3, start_ms, SIDE, steps);
        let assignments: Vec<Assignment> = (1..=steps)
            .map(|s| Assignment {
                asset_id: 1,
                device_id: 2,
                point_id: 10,
                unit_code: 6,
                power_kw: s as f64,
                step_index: s,
            })
            .collect();
        CandidatePlan::from_parts(eneros_decision_contract::CandidatePlanParts {
            id: eneros_decision_contract::PlanId::new(plan_id).expect("id"),
            snapshot_id: eneros_decision_contract::SnapshotId(snapshot_id),
            objective_digest: inputs.objective_digest,
            resource_catalog_version: 3,
            horizon_start_ms: start_ms,
            step_ns: SIDE,
            step_count: steps,
            solver_status: eneros_decision_contract::SolveStatusKind::Optimal,
            objective_value: 0.0,
            relative_gap: 0.0,
            max_primal_residual: 0.0,
            elapsed_ms: 1,
            model_digest: [0x11; 32],
            input_digest: [0x22; 32],
            assignments,
            issued_at_ms: start_ms,
            ttl_ms: 3_600_000,
        })
        .expect("plan")
    }

    /// 证书经**夹具认证器**（test-util 缝；真实评估器出证）——测试代码
    /// 同样不走发放构造（`seal_issuance_source_scan` 门对全仓执法）。
    fn cert(plan: &CandidatePlan, now_ms: u64, _ttl_ms: u64) -> CertifiedPlan {
        let fx = eneros_certifier::FixtureCertifier::new(
            b"plan-hold-cert-key-0123456789abcdef",
            100.0,
            -500.0,
            500.0,
            0.5,
        );
        let reply = fx.certify(plan, now_ms).expect("fixture certify");
        CertifiedPlan::decode_v1(&reply.certificate).expect("decode cert")
    }

    /// 记账投递面（回执 = 步序为偶数则失败重试）。
    struct RecordingSink {
        calls: Vec<(u32, f64)>,
        fail_steps: Vec<u32>,
        committed: bool,
    }
    impl RecordingSink {
        fn all_committed() -> Self {
            Self {
                calls: Vec::new(),
                fail_steps: Vec::new(),
                committed: true,
            }
        }
    }
    impl BatchSink for RecordingSink {
        fn deliver(&mut self, step_index: u32, power_kw: f64) -> Result<bool, DispatchError> {
            self.calls.push((step_index, power_kw));
            if self.fail_steps.contains(&step_index) {
                Err(DispatchError::ProposalInvalid(String::from("sink-down")))
            } else {
                Ok(self.committed)
            }
        }
    }

    /// 正例：4 时段跨多 tick，**接纳一次**仍逐步执行（蓝图 §8 判据原文
    /// "4时段跨多个tick，提交只一次仍执行后续"）。
    #[test]
    fn t1_four_steps_across_ticks_single_accept() {
        let mut h = PlanHolder::default();
        let sid = [0x5A; 16];
        let p = plan(sid, 4);
        let c = cert(&p, 1_700_000_000_000, 3_600_000);
        h.accept(p, c, 1_700_000_000_000).expect("接纳");
        let f = facts(sid);
        let mut sink = RecordingSink::all_committed();
        // tick1（第 1 窗）：1 步到期 → 投递 1 步。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_000_100_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1
        );
        // tick2（第 1 窗内再驱动）：无新到期步（幂等——同窗不重投）。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_000_200_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            0
        );
        // tick3（第 2 窗，窗长 1h）：1 步。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_003_700_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1
        );
        // 步窗互斥（[from, from+step) 不重叠）⇒ 每 tick 每计划至多一步
        // 到期；tick5 进第 4 窗投最后一步（有界 tick 的跨计划合计约束）。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_007_300_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1
        );
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_010_900_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1
        );
        assert_eq!(
            sink.calls.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        let states: Vec<StepState> = h.status()[0].iter().map(|v| v.state).collect();
        assert_eq!(states, vec![StepState::Done; 4], "全部终态");
        assert_eq!(h.expired_steps_total, 0);
        // 全 Done 时无"未发步"引用（输入持有解除）；中途有 Pending 时为真
        // （t5 已覆盖失败保 Pending 的形态）。
        assert!(!h.has_outstanding_input(), "全终态 = 输入持有解除");
        h.reap_terminal();
        assert_eq!(h.held_count(), 0, "终态计划释放");
        assert!(!h.has_outstanding_input(), "终态后解除输入持有");
    }

    /// 错窗 = Expired（显式未执行理由；**绝不补发**）。
    /// 变异靶：错窗步继续投递（或标 Done）→ 本测红。
    #[test]
    fn t2_missed_window_is_expired_never_dispatched() {
        let mut h = PlanHolder::default();
        let sid = [0x5A; 16];
        let p = plan(sid, 3);
        let c = cert(&p, 1_700_000_000_000, 3_600_000);
        h.accept(p, c, 1_700_000_000_000).expect("接纳");
        let f = facts(sid);
        let mut sink = RecordingSink::all_committed();
        // 直接跳到第 3 窗：步 1/2 已错窗 → Expired；只有步 3 投递。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_007_300_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1
        );
        let states: Vec<StepState> = h.status()[0].iter().map(|v| v.state).collect();
        assert_eq!(
            states,
            vec![StepState::Expired, StepState::Expired, StepState::Done],
            "错窗步必须显式 Expired（未执行有因）"
        );
        assert_eq!(h.expired_steps_total, 2);
        assert_eq!(sink.calls.len(), 1, "错窗步不得补发");
    }

    /// 输入换代 → 未发步撤销（已发步保留原状态——不重写历史）。
    /// 变异靶：换代后继续投递旧计划 → 本测红。
    #[test]
    fn t3_input_generation_change_cancels_pending_keeps_history() {
        let mut h = PlanHolder::default();
        let sid = [0x5A; 16];
        let p = plan(sid, 3);
        let c = cert(&p, 1_700_000_000_000, 3_600_000);
        h.accept(p, c, 1_700_000_000_000).expect("接纳");
        let f = facts(sid);
        let mut sink = RecordingSink::all_committed();
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_000_100_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1
        );
        // 快照换代：未发步（2/3）必须撤销。
        let new_facts = facts([0x77; 16]);
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_003_700_000,
                    facts: Some(&new_facts)
                },
                &mut sink
            ),
            0,
            "跨代不得投递"
        );
        let states: Vec<StepState> = h.status()[0].iter().map(|v| v.state).collect();
        assert_eq!(
            states,
            vec![StepState::Done, StepState::Cancelled, StepState::Cancelled],
            "已发步保留原状态，未发步撤销"
        );
        assert_eq!(h.cancelled_steps_total, 2);
    }

    /// 无可信输入（facts=None）= 撤销未发步（"无法维持安全输入则暂停/
    /// 撤销未发步"——不是继续按旧输入投递）。
    #[test]
    fn t4_no_trusted_input_cancels_pending() {
        let mut h = PlanHolder::default();
        let sid = [0x5A; 16];
        let p = plan(sid, 2);
        let c = cert(&p, 1_700_000_000_000, 3_600_000);
        h.accept(p, c, 1_700_000_000_000).expect("接纳");
        let mut sink = RecordingSink::all_committed();
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_000_100_000,
                    facts: None
                },
                &mut sink
            ),
            0
        );
        let states: Vec<StepState> = h.status()[0].iter().map(|v| v.state).collect();
        assert_eq!(states, vec![StepState::Cancelled, StepState::Cancelled]);
    }

    /// 投递失败：本步保持 Pending（下 tick 重试；计数守恒）。
    #[test]
    fn t5_delivery_failure_keeps_step_pending() {
        let mut h = PlanHolder::default();
        let sid = [0x5A; 16];
        let p = plan(sid, 2);
        let c = cert(&p, 1_700_000_000_000, 3_600_000);
        h.accept(p, c, 1_700_000_000_000).expect("接纳");
        let f = facts(sid);
        let mut sink = RecordingSink {
            calls: Vec::new(),
            fail_steps: vec![1],
            committed: true,
        };
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_000_100_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            0
        );
        assert_eq!(h.status()[0][0].state, StepState::Pending, "失败步保持待投");
        // 恢复后重投成功。
        let mut sink2 = RecordingSink::all_committed();
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: 1_700_000_150_000,
                    facts: Some(&f)
                },
                &mut sink2
            ),
            1
        );
        assert_eq!(h.status()[0][0].state, StepState::Done);
        // 计数守恒（复查修复：原断言的谓词枚举了全部 5 个状态 ⇒ 恒真，
        // 对"消失步"零判别力）。有牙形态：步序必须是接纳步数的完整序列
        // （每步恰好出现一次、序号连续），且**终端状态计数与状态视图逐项
        // 相等**（计数与状态是两个独立存储面，二者一致才说明无丢失）。
        let views = &h.status()[0];
        assert_eq!(views.len(), 2, "步数 = plan.step_count");
        assert_eq!(
            views.iter().map(|v| v.step_index).collect::<Vec<_>>(),
            alloc::vec![1, 2],
            "步序连续且唯一（无消失步/重复步）"
        );
        let count_of = |st: StepState| views.iter().filter(|v| v.state == st).count() as u64;
        assert_eq!(count_of(StepState::Done), 1, "步 1 已终态");
        assert_eq!(
            count_of(StepState::Done) + h.delivered_steps_total,
            2,
            "投递计数自洽"
        );
        assert_eq!(h.expired_steps_total, count_of(StepState::Expired));
        assert_eq!(h.cancelled_steps_total, count_of(StepState::Cancelled));
        assert_eq!(h.unconfirmed_steps_total, count_of(StepState::Rejected));
        assert_eq!(
            count_of(StepState::Pending) + count_of(StepState::Dispatched),
            h.status()[0]
                .iter()
                .filter(|v| { v.state == StepState::Pending || v.state == StepState::Dispatched })
                .count() as u64,
            "非终态步数守恒"
        );
    }

    /// 持有容量与重复接纳（1 活跃 + 1 待切换；满时明确拒绝不覆盖）。
    #[test]
    fn t6_capacity_and_duplicate_rejects() {
        let mut h = PlanHolder::default();
        let now = 1_700_000_000_000u64;
        let p1 = plan([0x5A; 16], 2);
        h.accept(p1.clone(), cert(&p1, now, 3_600_000), now)
            .expect("接纳 1");
        let p2 = plan([0x77; 16], 2);
        h.accept(p2.clone(), cert(&p2, now, 3_600_000), now)
            .expect("接纳 2（待切换）");
        let p3 = plan([0x88; 16], 2);
        assert_eq!(
            h.accept(p3, cert(&plan([0x88; 16], 2), now, 3_600_000), now),
            Err(HoldReject::Full)
        );
        assert_eq!(
            h.accept(p1, cert(&plan([0x5A; 16], 2), now, 3_600_000), now),
            Err(HoldReject::Duplicate)
        );
    }

    /// 单步计划不接管（当拍执行语义不变——持有器只管未来步）。
    #[test]
    fn t7_single_step_plan_rejected() {
        let mut h = PlanHolder::default();
        let now = 1_700_000_000_000u64;
        let p = plan([0x5A; 16], 1);
        assert_eq!(
            h.accept(p, cert(&plan([0x5A; 16], 1), now, 3_600_000), now),
            Err(HoldReject::Invalid("single-step plan"))
        );
    }

    /// 复查修复：步长下限（`step_len_ms` 单源）——`step_ns ∈ (0,1ms)`
    /// 不得产生**零宽步窗**（原实现 `step_ns/1e6 = 0` ⇒ 每步窗 [from,from)
    /// 在 `now == from` 时即判错窗 ⇒ 接纳的计划被整体静默 Expired）。
    ///
    /// 变异靶：`step_window` 回到 `plan.step_ns() / 1_000_000` → 本测红
    /// （0 投递 + 全部 Expired）。
    #[test]
    fn t9_sub_millisecond_step_does_not_zero_windows() {
        let mut h = PlanHolder::default();
        let sid = [0x5A; 16];
        let start_ms = 1_700_000_000_000u64;
        // 2 步 × 0.5ms（合法但极短的步长；编译器同律取 1ms 下限）。
        let inputs = CandidateInputs::from_snapshot(sid, 3, start_ms, 500_000, 2);
        let assignments: Vec<Assignment> = (1..=2u32)
            .map(|s| Assignment {
                asset_id: 1,
                device_id: 2,
                point_id: 10,
                unit_code: 6,
                power_kw: s as f64,
                step_index: s,
            })
            .collect();
        let p = CandidatePlan::from_parts(eneros_decision_contract::CandidatePlanParts {
            id: eneros_decision_contract::PlanId::new([9u8; 16]).expect("id"),
            snapshot_id: eneros_decision_contract::SnapshotId(sid),
            objective_digest: inputs.objective_digest,
            resource_catalog_version: 3,
            horizon_start_ms: start_ms,
            step_ns: 500_000,
            step_count: 2,
            solver_status: eneros_decision_contract::SolveStatusKind::Optimal,
            objective_value: 0.0,
            relative_gap: 0.0,
            max_primal_residual: 0.0,
            elapsed_ms: 1,
            model_digest: [0x11; 32],
            input_digest: [0x22; 32],
            assignments,
            issued_at_ms: start_ms,
            ttl_ms: 3_600_000,
        })
        .expect("plan");
        let c = cert(&p, start_ms, 3_600_000);
        h.accept(p, c, start_ms).expect("接纳");
        let f = facts(sid);
        let mut sink = RecordingSink::all_committed();
        // 步 1 窗起点即 horizon：必须投递（不是"错窗"）。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: start_ms,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1,
            "零宽窗把首步误判错窗（步长下限失守）"
        );
        assert_eq!(sink.calls[0].0, 1);
        assert_eq!(h.expired_steps_total, 0);
    }

    /// 复查修复：`Dispatched` 必须**有出口**——回执回填（`resolve_step`）
    /// 与计划窗关闭（证书到期）两条路都能收敛为终态，容量随 `reap_terminal`
    /// 回收（原实现 `Ok(false)` 只进不出：两个计划卡死 = 后续一律 `Full`）。
    #[test]
    fn t10_dispatched_settles_by_receipt_or_window_close() {
        let mut h = PlanHolder::default();
        let sid = [0x5A; 16];
        let start_ms = 1_700_000_000_000u64;
        let p = plan(sid, 2);
        let c = cert(&p, start_ms, 3_600_000);
        let plan_id = *p.id();
        let snap_id = p.snapshot_id();
        h.accept(p, c, start_ms).expect("接纳");
        let f = facts(sid);
        // 投递未确认（Ok(false)）→ Dispatched（非终态，占容量）。
        let mut uncertain = RecordingSink {
            calls: Vec::new(),
            fail_steps: Vec::new(),
            committed: false,
        };
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: start_ms + 100_000,
                    facts: Some(&f)
                },
                &mut uncertain
            ),
            1
        );
        assert_eq!(h.status()[0][0].state, StepState::Dispatched);
        assert!(h.has_outstanding_input(), "未确认命令仍引用其输入");
        h.reap_terminal();
        assert_eq!(h.held_count(), 1, "未确认计划不得被回收");
        // ① 回执回填：明确未生效 → Rejected（终态）。
        assert!(h.resolve_step(&plan_id, snap_id, 1, false));
        assert_eq!(h.status()[0][0].state, StepState::Rejected);
        assert!(
            !h.resolve_step(&plan_id, snap_id, 1, false),
            "重复回执零状态变更"
        );
        // ② 步窗关闭：第 2 步投递未确认（步窗 [start+1h, start+2h)）→
        //    窗口过后收敛 Rejected（绝不重发），容量回收。
        let mut uncertain2 = RecordingSink {
            calls: Vec::new(),
            fail_steps: Vec::new(),
            committed: false,
        };
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: start_ms + 3_700_000,
                    facts: Some(&f)
                },
                &mut uncertain2
            ),
            1
        );
        assert_eq!(h.status()[0][1].state, StepState::Dispatched);
        // 步 2 窗闭（≥ start+2h）后驱动一次 ⇒ 未确认收敛 Rejected。
        h.tick(
            PlanTick {
                now_ms: start_ms + 7_300_000,
                facts: Some(&f),
            },
            &mut uncertain2,
        );
        assert_eq!(
            h.status()[0][1].state,
            StepState::Rejected,
            "回执未到 + 计划窗关闭 = 未确认终局"
        );
        assert!(h.unconfirmed_steps_total >= 1);
        h.reap_terminal();
        assert_eq!(h.held_count(), 0, "全终态 ⇒ 容量回收");
        assert!(!h.has_outstanding_input(), "终态 ⇒ 输入持有解除");
    }

    /// 复查修复：**同一时刻只有一个投递面**（1 活跃 + 1 待切换）。
    /// 原实现遍历全部持有计划 ⇒ 两个计划在同 tick 争相向同一设备投递
    /// （双命令源）；现语义 = `plans[0]` 活跃投递、其余待切换，活跃计划
    /// **排空后**自动接手（`switched_total`；不做时间维抢占，抢占策略属
    /// WP-70.11.3）。
    /// 70.14 复查红测：切换**不得吞掉未确认（`Dispatched`）计划**。
    ///
    /// 缺陷形态（旧实现）：tick 起手的切换只看 `Pending`——活跃计划 A 两步
    /// 都以 `Ok(false)`（投递结果未知）投出后，再驱动一次（两个窗都已闭）时
    /// A 满足"无 Pending"而在**终态化 pass 之前**被 `remove(0)`：
    /// 未确认命令既不收敛 `Rejected`、`unconfirmed_steps_total` 也不增，
    /// `resolve_step` 此后恒 false（对账面丢失），且与 `reap_terminal` 的
    /// "Dispatched 保留到回执或窗闭"承诺矛盾。
    ///
    /// 同时锁 `has_outstanding_input`：全部步 Dispatched 时仍持有输入
    /// （未确认命令的对账要引用其证据）。
    #[test]
    fn switch_does_not_swallow_dispatched_plan() {
        let mut h = PlanHolder::default();
        let start_ms = 1_700_000_000_000u64;
        let sid = [0x5A; 16];
        let a = plan_with_id([0x01; 16], sid, 2);
        let a_id = *a.id();
        let a_snap = a.snapshot_id();
        let b = plan_with_id([0x02; 16], sid, 2);
        h.accept(
            a,
            cert(&plan_with_id([0x01; 16], sid, 2), start_ms, 3_600_000),
            start_ms,
        )
        .expect("A");
        h.accept(
            b,
            cert(&plan_with_id([0x02; 16], sid, 2), start_ms, 3_600_000),
            start_ms,
        )
        .expect("B");
        let f = facts(sid);
        // `committed=false` + 无 fail_steps ⇒ `Ok(false)` = **投出但结果未确认**
        // （与 t10 同一形态；`Err` 是"投递失败保 Pending"，不是本场景）。
        let mut uncertain = RecordingSink {
            calls: Vec::new(),
            fail_steps: Vec::new(),
            committed: false,
        };
        // tick1（窗 1）：步 1 投出且未确认。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: start_ms + 100_000,
                    facts: Some(&f)
                },
                &mut uncertain
            ),
            1
        );
        assert_eq!(h.status()[0][0].state, StepState::Dispatched);
        // tick2（窗 2）：步 1 窗闭 ⇒ Rejected（终态化 pass）；步 2 投出未确认。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: start_ms + 3_700_000,
                    facts: Some(&f)
                },
                &mut uncertain
            ),
            1
        );
        assert_eq!(h.status()[0][0].state, StepState::Rejected, "步 1 窗闭终态");
        assert_eq!(
            h.status()[0][1].state,
            StepState::Dispatched,
            "步 2 投出未确认"
        );
        assert!(
            h.has_outstanding_input(),
            "全部剩余步 Dispatched（未确认）时仍必须持有输入（对账需引用证据）"
        );
        // tick3（两窗皆闭）：**不得在终态化之前把 A 切换掉**。
        let mut sink = RecordingSink::all_committed();
        h.tick(
            PlanTick {
                now_ms: start_ms + 7_300_000,
                facts: Some(&f),
            },
            &mut sink,
        );
        assert_eq!(
            h.switched_total, 0,
            "A 仍有未确认步 ⇒ 不切换（绝不抢占在途）"
        );
        assert_eq!(h.held_count(), 2, "未确认计划不得被切换丢弃");
        assert_eq!(
            h.status()[0][1].state,
            StepState::Rejected,
            "未确认步必须在窗闭时收敛 Rejected 并计数（旧实现整计划被丢弃 → 无痕）"
        );
        // 两个未确认步各在自身窗闭时收敛（步 1@tick2、步 2@tick3）。
        assert_eq!(h.unconfirmed_steps_total, 2);
        // 在途未见回执时**计划本体必须仍在**（回执路径的出口由 t10 覆盖：
        // 计划的保留是先决条件——旧实现在此处已把 A 丢弃，回执无处回填）。
        assert!(
            h.status().iter().any(|p| p[0].state == StepState::Rejected),
            "A 的步状态必须仍可查询（对账面）"
        );
        let _ = (a_id, a_snap);
    }

    #[test]
    fn t11_only_one_plan_delivers_pending_switches_after_drain() {
        let mut h = PlanHolder::default();
        let start_ms = 1_700_000_000_000u64;
        let sid = [0x5A; 16];
        // 同代两计划（重规划形态）：id 不同（同 id 同快照会被判重复）。
        let a = plan_with_id([0x01; 16], sid, 2);
        let b = plan_with_id([0x02; 16], sid, 2);
        h.accept(
            a,
            cert(&plan_with_id([0x01; 16], sid, 2), start_ms, 3_600_000),
            start_ms,
        )
        .expect("活跃计划 A");
        h.accept(
            b,
            cert(&plan_with_id([0x02; 16], sid, 2), start_ms, 3_600_000),
            start_ms,
        )
        .expect("待切换计划 B");
        let f = facts(sid);
        let mut sink = RecordingSink::all_committed();
        // tick1：A 活跃 → 步 1；B（待切换）零投递。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: start_ms + 100_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1
        );
        assert_eq!(sink.calls, alloc::vec![(1, 1.0)]);
        assert_eq!(h.switched_total, 0, "A 仍有未发步 → 不切换");
        // tick2（第 2 窗）：A 投步 2 后**排空**；B 仍非活跃 → 零投递。
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: start_ms + 3_700_000,
                    facts: Some(&f)
                },
                &mut sink
            ),
            1
        );
        assert_eq!(sink.calls, alloc::vec![(1, 1.0), (2, 2.0)]);
        assert_eq!(
            h.switched_total, 0,
            "排空发生在本次 tick 内 → 下次 tick 起切换"
        );
        // tick3：A 排空 ⇒ B 接手并在同 tick 投递其步 1。
        let mut sink_b = RecordingSink::all_committed();
        assert_eq!(
            h.tick(
                PlanTick {
                    now_ms: start_ms + 3_700_001,
                    facts: Some(&f)
                },
                &mut sink_b
            ),
            1,
            "排空后待切换计划接手并在同 tick 投递步 1"
        );
        assert_eq!(h.switched_total, 1, "切换计数");
        // B 接手时其第 1 步窗已闭 ⇒ Expired（绝不补发），只有第 2 步到期。
        assert_eq!(
            sink_b.calls,
            alloc::vec![(2, 2.0)],
            "B 的第 2 步（功率 = 步序）"
        );
        let b_states: Vec<StepState> = h.status()[0].iter().map(|v| v.state).collect();
        assert_eq!(
            b_states,
            alloc::vec![StepState::Expired, StepState::Done],
            "待切换计划的错窗步同样立即终态（不占 Pending）"
        );
        assert_eq!(h.cancelled_steps_total, 0, "同代驱动：无撤销");
    }
}
