//! 蓝图 76 卷三 WP-76.3.5（体检 R17 候选 5）：`run_typed` 分类单源。
//!
//! 病灶（收编前，engine.rs `run_typed` 一次调用五遍扫描）：
//! - 扫① 涉备集 / 扫② 未决普查 / 扫③ 逐设备重放判定 / 扫④ 投递 / 扫⑤ 清账；
//! - 幂等键 `(correlation, 语义摘要)` 在扫②③**各算一遍**（SHA-256 摘要实算
//!   2 遍），correlation 另有扫④第三处派生；
//! - 「判定」（Deferred/Expired/replay_only）在循环体内手写，「计数」
//!   （deferred_steps/expired_steps）在另一处累加——REG-70.FL-MULTISTEP-REPLAY
//!   的「多步计划只走了首步，串口看不出来」病灶即此分离。
//!
//! 本模块 = 分类的**纯函数**面：[`classify`] 一遍扫描产出 [`PreparedPlan`]
//! （涉备集 + 每步幂等键 + 投递时态 + 冲突/未决回执 + replay_only 聚合），
//! `run_typed` 只消费不再判。纯 = host 可表测（体检报告的收益点：不必再为
//! 「一步到期与否」搭整套引擎夹具）。
//!
//! **幂等语义红线**（卷三 §1D）：摘要吃 `intent.power_kw()`（计划态功率），
//! 而执行面功率可被 `power_override_kw` 替换（saga_exec.rs）——两值**不得
//! 合并**，[`PreparedStep`] 刻意不携带任何「执行功率」字段，也不得为省一次
//! 计算而把投递功率喂回摘要。
//!
//! 与蓝图表列的差异（如实登记，卷三 §6 设计裁决）：`StepKind` 落为**三态**
//! （Due/Deferred/Expired）而非四态——蓝图的第四态 `Replayed` 与时间轴
//! **正交**（journal Current 命中的步照样按窗口决定投不投，命中半边由
//! execute_core 的 `IdemNote::PlanReplay` 出回执）。把正交两轴塞进同一枚举
//! 等于再造一个「表行装饰」（WP-76.3.4 刚拆掉的那种）：`Replayed` 要么吞掉
//! Deferred/Expired 的计数面，要么永远不可达。故命中半边由
//! [`PreparedStep::replay_current`] 承载，聚合面给出 [`PreparedPlan::replay_only`]。

use alloc::vec::Vec;

use eneros_decision_contract::{CommandIntent, PlanId};

use crate::compiler::intent_journal_identity;
use crate::journal::{ExecutionJournal, IdempotencyRecord, JournalHit};

/// 投递时态（判据单源 = [`kind_at`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepKind {
    /// 当拍可投（`valid_from ≤ now < valid_until`）。
    Due,
    /// 未到期：`until_ms` = `valid_from`（投递点复检按 fresh now 重算等待）。
    Deferred { until_ms: u64 },
    /// 窗口已过（单调——分类时点已过期者投递点不再重判）。
    Expired,
}

/// 时态判据**唯一**定义点（原 run_typed 投递循环体内的两行 `if` 收编于此）。
/// 边界语义逐字保形：`now < valid_from` 未到期、`now >= valid_until` 过期。
pub(crate) fn kind_at(now: u64, valid_from_ms: u64, valid_until_ms: u64) -> StepKind {
    if now < valid_from_ms {
        StepKind::Deferred {
            until_ms: valid_from_ms,
        }
    } else if now >= valid_until_ms {
        StepKind::Expired
    } else {
        StepKind::Due
    }
}

/// 单步预制品：幂等键 + 投递时态 + 重放命中事实。
#[derive(Debug)]
pub(crate) struct PreparedStep<'a> {
    pub intent: &'a CommandIntent,
    /// 分类时点（`classified_at`）的时态。
    pub kind: StepKind,
    /// 幂等键唯一派生点（[`intent_journal_identity`] 的返回值原样）。
    ///
    /// **correlation 即本元组前 16B**（收编前三处各派生一遍：扫②③走
    /// identity 内含、扫④另调 `plan_command_correlation`）——现零第二遍，
    /// 故本结构刻意不再单列 `correlation` 字段（单列 = 把同一事实表两遍）。
    pub identity: ([u8; 16], [u8; 32]),
    /// journal 已有同键同摘要的**当前**记录（终态与否皆算命中）——按设备
    /// 聚合成 replay_only（免仲裁判据）。
    pub replay_current: bool,
}

impl PreparedStep<'_> {
    /// 投递关联键（= 幂等键前 16B，非第二派生点）。
    pub(crate) fn correlation(&self) -> [u8; 16] {
        self.identity.0
    }

    /// 涉备键（与仲裁/清账同口径：u16 device → u32）。
    pub(crate) fn device(&self) -> u32 {
        u32::from(self.intent.device_id())
    }

    /// 投递时点的时态复检（判据同源 [`kind_at`]）。
    ///
    /// **为什么不能省这一次重查**：分类与投递之间 `execute_core` 走真实
    /// IPC（ms 级），窗口可在期间关闭；`Expired` 单调不回退故免检，其余按
    /// fresh now 重判——「到期即不投」语义与收编前逐 intent 现取 `now` 的
    /// 形态零漂移（蓝图 76 卷三 §6 红线④的裁决面）。
    pub(crate) fn kind_at(&self, now: u64) -> StepKind {
        match self.kind {
            StepKind::Expired => StepKind::Expired,
            _ => kind_at(
                now,
                self.intent.valid_from_ms(),
                self.intent.valid_until_ms(),
            ),
        }
    }
}

/// 一批的预制品（`run_typed` 扫①②③⑤的产物同源）。
#[derive(Debug)]
pub(crate) struct PreparedPlan<'a> {
    /// 步序 = 批序 × 批内序 = 编译序（可观察行为：投递与失败阻断按此序）。
    pub steps: Vec<PreparedStep<'a>>,
    /// 涉备集（批序首次出现去重；仲裁登记与清账按此序）。
    pub involved: Vec<u32>,
    /// 任一 intent 语义摘要与 journal 既有记录冲突 → 整体拒绝（早于仲裁、
    /// 早于未决回执返回——与收编前扫描次序一致）。
    pub conflict: bool,
    /// 首个**非终态** Current 记录 → 早于仲裁返回既有回执（`get_or_insert`
    /// 语义 = 扫描序第一个命中者胜出）。
    pub unresolved: Option<IdempotencyRecord>,
}

impl PreparedPlan<'_> {
    /// replay_only 聚合（原扫③手写 `all(...)`）：设备全部 intent 均 journal
    /// Current 命中 ⇒ 免仲裁——已知命令查账不得抢占资源；反之新命令必须过
    /// 仲裁（不可覆盖未知效果）。
    ///
    /// journal 状态在分类与仲裁之间无写入（分类只做 `lookup`），故聚合用
    /// 分类产物而非第二次 `lookup`。
    pub(crate) fn replay_only(&self, device: u32) -> bool {
        self.steps
            .iter()
            .filter(|s| s.device() == device)
            .all(|s| s.replay_current)
    }
}

/// 一遍分类（纯函数：只读 `journal.lookup` 与 intent 窗口，零写、零副作用）。
///
/// `batches` 取「每批一截意图」的迭代器形态，使本函数可脱离
/// `CommandBatch`（需证书 + 注册表构造）单测——这是本 WP 的 host 可测性收益点。
pub(crate) fn classify<'a>(
    batches: impl IntoIterator<Item = &'a [CommandIntent]>,
    plan_id: &PlanId,
    legacy_single: bool,
    journal: &ExecutionJournal,
    now: u64,
) -> PreparedPlan<'a> {
    let mut steps: Vec<PreparedStep<'a>> = Vec::new();
    let mut involved: Vec<u32> = Vec::new();
    let mut conflict = false;
    let mut unresolved: Option<IdempotencyRecord> = None;
    for intents in batches {
        for intent in intents {
            let d = u32::from(intent.device_id());
            if !involved.contains(&d) {
                involved.push(d);
            }
            let identity = intent_journal_identity(plan_id, intent, legacy_single);
            let hit = journal.lookup(&identity.0, &identity.1);
            let replay_current = matches!(hit, Some(JournalHit::Current(_)));
            if matches!(hit, Some(JournalHit::Conflict)) {
                conflict = true;
            }
            if let Some(JournalHit::Current(record)) = hit {
                if !record.status.is_terminal() {
                    unresolved.get_or_insert(record);
                }
            }
            steps.push(PreparedStep {
                intent,
                kind: kind_at(now, intent.valid_from_ms(), intent.valid_until_ms()),
                identity,
                replay_current,
            });
        }
    }
    PreparedPlan {
        steps,
        involved,
        conflict,
        unresolved,
    }
}

/// 分类面表测（蓝图 76 卷三 §6 步骤 1：四态构造——不搭引擎夹具，纯函数直喂）。
#[cfg(test)]
mod tests {
    use alloc::vec;

    use eneros_decision_contract::{CommandIntentParts, CompensationHint, IntentId, Precondition};

    use super::*;
    use crate::journal::ExecutionJournal;

    fn plan_id() -> PlanId {
        PlanId([0x61; 16])
    }

    fn intent(
        id_byte: u8,
        device: u16,
        point: u32,
        kw: f64,
        from: u64,
        until: u64,
    ) -> CommandIntent {
        CommandIntent::from_parts(CommandIntentParts {
            intent_id: IntentId([id_byte; 16]),
            plan_id: plan_id(),
            plan_sha256: [0xAB; 32],
            asset_id: u64::from(device),
            device_id: device,
            point_id: point,
            power_kw: kw,
            unit_code: 6,
            valid_from_ms: from,
            valid_until_ms: until,
            precondition: Precondition::None,
            compensation: CompensationHint::None,
        })
        .expect("intent")
    }

    /// 单批夹具（`classify` 的迭代器面在此收口——生产传 `b.intents()` 切片，
    /// 测试传同一形状，判据不分叉）。
    fn one_batch<'a>(
        intents: &'a [CommandIntent],
        j: &ExecutionJournal,
        now: u64,
    ) -> PreparedPlan<'a> {
        classify([intents], &plan_id(), false, j, now)
    }

    /// 时态判据的窗口边界（`valid_from` 闭、`valid_until` 开——逐字保形）。
    #[test]
    fn kind_at_window_boundaries() {
        assert_eq!(
            kind_at(1_000, 2_000, 3_000),
            StepKind::Deferred { until_ms: 2_000 }
        );
        assert_eq!(kind_at(2_000, 2_000, 3_000), StepKind::Due, "到点当拍可投");
        assert_eq!(kind_at(2_999, 2_000, 3_000), StepKind::Due);
        assert_eq!(
            kind_at(3_000, 2_000, 3_000),
            StepKind::Expired,
            "valid_until 开区间"
        );
    }

    /// 投递点复检：Deferred 到期转 Due（「到期即投」），窗口关闭转 Expired；
    /// 分类即 Expired 的步单调不回退（时钟早于窗口也不复活）。
    #[test]
    fn recheck_defers_then_delivers_and_expired_stays() {
        let j = ExecutionJournal::new(8);
        let future = vec![intent(1, 7, 101, 5.0, 2_000, 3_000)];
        let p = one_batch(&future, &j, 1_000);
        assert_eq!(p.steps[0].kind, StepKind::Deferred { until_ms: 2_000 });
        assert_eq!(
            p.steps[0].kind_at(2_500),
            StepKind::Due,
            "复检：窗口已开 → 可投（分类瞬间不得把步永久判死）"
        );
        assert_eq!(
            p.steps[0].kind_at(3_500),
            StepKind::Expired,
            "复检：窗口已关"
        );
        let gone = vec![intent(2, 7, 102, 5.0, 100, 200)];
        let p2 = one_batch(&gone, &j, 500);
        assert_eq!(p2.steps[0].kind, StepKind::Expired);
        assert_eq!(
            p2.steps[0].kind_at(0),
            StepKind::Expired,
            "过期步免重判（收编前后同形）"
        );
    }

    /// 涉备集 = 批序首次出现去重（仲裁登记与清账按此序 = 可观察行为）。
    #[test]
    fn involved_set_keeps_first_seen_batch_order() {
        let j = ExecutionJournal::new(8);
        let b0 = vec![
            intent(1, 12, 1, 5.0, 0, 10_000),
            intent(2, 9, 2, 5.0, 0, 10_000),
        ];
        let b1 = vec![intent(3, 12, 3, 5.0, 0, 10_000)];
        let p = classify([b0.as_slice(), b1.as_slice()], &plan_id(), false, &j, 1_000);
        assert_eq!(p.involved, vec![12u32, 9], "首次出现序 + 去重");
        assert_eq!(p.steps.len(), 3, "步序 = 批序×批内序（不按设备折叠）");
        assert!(!p.conflict);
        assert!(p.unresolved.is_none());
        assert!(p.steps.iter().all(|s| !s.replay_current));
    }

    /// journal Current 命中半边：`replay_current` + 非终态命中置 `unresolved`
    /// （扫描序首个命中者胜出）+ `replay_only` 按设备聚合（全命中才免仲裁）。
    #[test]
    fn journal_current_hit_marks_replay_and_unresolved() {
        let known = intent(1, 12, 1, 5.0, 0, 10_000);
        let fresh = intent(2, 12, 2, 6.0, 0, 10_000);
        let (key, digest) = intent_journal_identity(&plan_id(), &known, false);
        let mut j = ExecutionJournal::new(16);
        j.authorize(key, [7u8; 16], digest, 900).expect("authorize");
        let batch = vec![known, fresh];
        let p = one_batch(&batch, &j, 1_000);
        assert!(p.steps[0].replay_current, "同键同摘要 = Current 命中");
        assert!(!p.steps[1].replay_current, "未落账步不命中");
        assert!(!p.conflict);
        let rec = p
            .unresolved
            .as_ref()
            .expect("非终态 Current → 未决回执（早于仲裁返回）");
        assert_eq!(rec.key, key);
        assert!(
            !rec.status.is_terminal(),
            "未决半边只对非终态成立（本例 Authorized）"
        );
        assert!(
            !p.replay_only(12),
            "设备 12 含未命中步 ⇒ 不免仲裁（新命令不得越过裁决）"
        );

        let solo = intent(4, 20, 4, 5.0, 0, 10_000);
        let (k2, d2) = intent_journal_identity(&plan_id(), &solo, false);
        let mut j2 = ExecutionJournal::new(16);
        j2.authorize(k2, [8u8; 16], d2, 900).expect("authorize");
        let s = vec![solo];
        let p2 = one_batch(&s, &j2, 1_000);
        assert!(p2.replay_only(20), "设备全部步 Current 命中 ⇒ 免仲裁");
        assert!(p2.unresolved.is_some());
    }

    /// 冲突半边：同键不同摘要 → 整体拒绝面（不置 Current/未决）；旧记录无
    /// 摘要（`[0;32]` 哨兵）不参与比对——journal 单源语义在分类面的如实投影。
    #[test]
    fn digest_mismatch_marks_conflict_and_zero_digest_never_does() {
        let a = intent(1, 12, 1, 5.0, 0, 10_000);
        let (ka, _) = intent_journal_identity(&plan_id(), &a, false);
        let mut j = ExecutionJournal::new(16);
        j.authorize(ka, [7u8; 16], [9u8; 32], 900)
            .expect("authorize");
        let v = vec![a];
        let p = one_batch(&v, &j, 1_000);
        assert!(p.conflict, "同键不同摘要必须标冲突");
        assert!(
            p.unresolved.is_none(),
            "冲突记录不是 Current——拒绝面优先于未决回执"
        );

        let b = intent(2, 13, 1, 5.0, 0, 10_000);
        let (kb, _) = intent_journal_identity(&plan_id(), &b, false);
        let mut j2 = ExecutionJournal::new(16);
        j2.authorize(kb, [7u8; 16], [0u8; 32], 900)
            .expect("authorize 无摘要");
        let v2 = vec![b];
        let p2 = one_batch(&v2, &j2, 1_000);
        assert!(!p2.conflict, "无摘要哨兵记录不得判冲突（无法比对内容）");
        assert!(p2.steps[0].replay_current);
    }

    /// 幂等语义红线（卷三 §1D）：**键**不含功率、**摘要**含功率。执行面
    /// 功率可被 `power_override_kw` 替换（saga_exec.rs）——若哪天有人把投递
    /// 功率喂回摘要，本测即红（幂等语义静默改变 = 重放判定漂移）。
    #[test]
    fn identity_binds_planned_power_in_digest_not_key() {
        let at5 = intent(1, 12, 1, 5.0, 0, 10_000);
        let at6 = intent(1, 12, 1, 6.0, 0, 10_000);
        let (k5, d5) = intent_journal_identity(&plan_id(), &at5, false);
        let (k6, d6) = intent_journal_identity(&plan_id(), &at6, false);
        assert_eq!(k5, k6, "同 intent_id ⇒ 同 correlation（键面不含功率）");
        assert_ne!(d5, d6, "功率入摘要（改功率 = 改语义）");
        let mut j = ExecutionJournal::new(16);
        j.authorize(k5, [7u8; 16], d5, 900).expect("authorize");
        let v = vec![at6];
        assert!(
            one_batch(&v, &j, 1_000).conflict,
            "以 6 kW 重放 5 kW 必须判冲突"
        );
    }

    /// legacy 单命令计划的键口径（分类只是将 `legacy_single` 透传给单源派生
    /// 点，不另判）：全部步 correlation = plan_id。
    #[test]
    fn legacy_single_plan_keys_all_steps_by_plan_id() {
        let a = intent(1, 12, 1, 5.0, 0, 10_000);
        let b = intent(2, 13, 2, 5.0, 0, 10_000);
        let batch = vec![a, b];
        let p = classify(
            [batch.as_slice()],
            &plan_id(),
            true,
            &ExecutionJournal::new(8),
            1_000,
        );
        assert_eq!(p.steps[0].correlation(), plan_id().0);
        assert_eq!(p.steps[1].correlation(), plan_id().0);
        assert_ne!(
            p.steps[0].identity.1, p.steps[1].identity.1,
            "摘要仍按 (device,point,power) 区分——legacy 只并键不并义"
        );
    }

    /// 单源性负证（防回潮）：`run_typed` 投递段不得再二次派生 correlation
    /// （键只从预制品取）。针形运行期拼接——本测源文本不含完整针形（守卫
    /// 自咬纪律）。
    #[test]
    fn correlation_derivation_is_single_sourced() {
        let src = include_str!("engine.rs");
        let start = src
            .find(concat!("for step in &prepared", ".steps"))
            .expect("run_typed 消费预制品的投递循环在位");
        let tail = &src[start..];
        assert!(
            !tail.contains(concat!("plan_command_correlat", "ion(")),
            "投递段不得二次派生 correlation（幂等键单源 = classify）"
        );
        assert!(
            tail.contains(concat!("let correlation = step.", "correlation()")),
            "correlation 取自预制品（唯一消费点）"
        );
    }
}
