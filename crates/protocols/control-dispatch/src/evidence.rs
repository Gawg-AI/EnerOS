//! C7（2026-09-24 架构走查收口）：控制链串口证据行**词形单源表**。
//!
//! 迁移前词形分散在各 `println!` 字面里，词形冻结靠 `include_str!` 锁
//!（锁源文件——重构不友好：把 println 收进 helper 会打翻锁，而词形漂移
//! 却可能不改行为）；且唯一 Committed 前置事实（EXEC-JOURNAL
//! applied-confirmed）整段 `cfg(target_os = "none")`——host 对同一行为
//! 天然盲。现词形与格式化收进本表：调用点 `println!("{}", 表格式化(..))`，
//! **输出字节逐字节不变**；词形锁改锁表（golden 等值，见 dispatch_service
//! t22 与本模块测试）；formatter 即 host 可测的「记录不打印」形态。
//!
//! **冻结纪律**：QEMU 门按词形 grep——本表任何词形变化 = 验收契约变化，
//! 须另立卷（与 tools/ 判据同步改）。

use alloc::format;
use alloc::string::String;

use crate::dispatch::DispatchError;
use crate::dispatch::DispatchState;
use crate::saga::{BatchState, CmdState};

/// 服务前缀（本服务全部证据行共用）。
pub const PREFIX: &str = "[svc-control-dispatch] ";

/// 提交回执族入口词（dispatch_service 三入口：typed/trusted/intent）。
pub const WORD_TYPED: &str = "TYPED";
pub const WORD_TRUSTED: &str = "TRUSTED";
pub const WORD_INTENT: &str = "INTENT";

/// E4 证据 token（61 v0.314.24）：唯一 Committed 前置事实——独立回读
/// 匹配后才出现；readback 前出现 = 判据失效。整行静态词形（原
/// `cfg(none)` println 字面，现为 host 可断言的常量）。
pub const EXEC_JOURNAL_APPLIED_CONFIRMED: &str =
    "[svc-control-dispatch] EXEC-JOURNAL applied-confirmed（独立回读匹配→Committed）";

/// 提交回执行（非 Committed 时打印）：
/// `{PREFIX}{word} state={state:?} note={note:?}`——与旧调用点内联
/// `println!("[svc-control-dispatch] {word} state={:?} note={:?}", ..)`
/// 逐字节同形。
pub fn receipt_state_note(word: &str, state: DispatchState, note: &Option<String>) -> String {
    format!("{PREFIX}{word} state={state:?} note={note:?}")
}

/// 提交错误行：`{PREFIX}{word}-ERR {e:?}`（word 为入口词，`-ERR` 尾缀
/// 在表内拼装——调用点不出现 `-ERR` 字面）。
pub fn submit_err(word: &str, e: &DispatchError) -> String {
    format!("{PREFIX}{word}-ERR {e:?}")
}

/// 恢复对账行（原 `cfg(none)` 区字面；recovered > 0 才出现）。
pub fn exec_journal_recovered(pending: usize) -> String {
    format!("{PREFIX}EXEC-JOURNAL recovered pending={pending} → reconciling")
}

/// 安全拒绝行。
pub fn safety_rejected(reason: &str) -> String {
    format!("{PREFIX}SAFETY-REJECTED reason={reason}")
}

// ==========================================================================
// 蓝图 75 WP-75.1.2（R16-G）：typed/held 编译-持有-逐命令证据族收表。
//
// 九族原为 engine.rs / held_engine.rs 内手写 `println!` 字面（词形冻结
// 只靠 QEMU 门 grep）；迁入本表后词形冻结升级为「host golden 逐字节」
// （下方 `evidence_extended_families_golden`——期望值 = 迁移前现行字面
// 逐字节手转录，不从新代码取）。`r10_b_witness.py` 等 QEMU 门按词形
// grep 串口输出，本表输出**逐字节不变**。
// ==========================================================================

/// CERTIFIED 行（run_typed 独立认证观测面；4B 前缀仅辅助阅读）。
pub fn certified(
    plan_id4: &[u8],
    cert_id4: &[u8],
    constraints: u32,
    key_id: u16,
    key_version: u32,
) -> String {
    format!(
        "{PREFIX}CERTIFIED plan={:02x?} cert={:02x?} constraints={} key={}/{}",
        plan_id4, cert_id4, constraints, key_id, key_version
    )
}

/// PLAN-COMPILED 行（WP-5.1 编译批指纹：compiler/batch 双摘要 + 未动作
/// 证据数；plan = 完整 16B 机器身份）。
pub fn plan_compiled(
    batches: usize,
    intents: usize,
    skipped: usize,
    manual: usize,
    compiler4: &[u8],
    batch4: &[u8],
    plan_id: &[u8; 16],
) -> String {
    format!(
        "{PREFIX}PLAN-COMPILED batches={} intents={} skipped={} manual={} compiler={:02x?} batch={:02x?} plan={:02x?}",
        batches, intents, skipped, manual, compiler4, batch4, plan_id
    )
}

/// PLAN-COMMAND-DEFERRED 行（未到期步不当拍投递——对称证据行，
/// REG-70.FL-MULTISTEP-REPLAY）。
pub fn plan_command_deferred(intent_id4: &[u8], due_in_ms: u64) -> String {
    format!(
        "{PREFIX}PLAN-COMMAND-DEFERRED intent={:02x?} due_in_ms={}",
        intent_id4, due_in_ms
    )
}

/// PLAN-COMMAND-SKIP-EXPIRED 行（已过期步跳过留证）。
pub fn plan_command_skip_expired(intent_id4: &[u8]) -> String {
    format!(
        "{PREFIX}PLAN-COMMAND-SKIP-EXPIRED intent={:02x?}",
        intent_id4
    )
}

/// PLAN-COMMANDS-DEFERRED 行（延期/过期步数一次性对账；0/0 也打印）。
pub fn plan_commands_deferred(deferred: u32, expired: u32, delivered: u32) -> String {
    format!(
        "{PREFIX}PLAN-COMMANDS-DEFERRED deferred={} expired={} delivered={}",
        deferred, expired, delivered
    )
}

/// PLAN-HELD 行（准入落账；完整 plan id + 批/意图计数）。
pub fn plan_held(plan_id: &[u8; 16], batches: usize, intents: usize) -> String {
    format!(
        "{PREFIX}PLAN-HELD plan={:02x?} batches={} intents={}",
        plan_id, batches, intents
    )
}

/// HELD-BATCH 行（逐批驱动终态；batch = 4B 前缀供阅读，plan = 完整 id）。
pub fn held_batch(batch_id4: &[u8], state: &BatchState, events: u64, plan_id: &[u8; 16]) -> String {
    format!(
        "{PREFIX}HELD-BATCH batch={:02x?} state={:?} events={} plan={:02x?}",
        batch_id4, state, events, plan_id
    )
}

/// HELD-COMMAND 行（F70-05/RR2：完整 intent id + 所属完整 plan id +
/// dev/pt/val 绑定锚，9 个 kv 字段（卷册蓝图「11 字段」口径含
/// readback=Some/val 拆项，实数 9）；`reason` 缺省语义 `-` 由调用方
/// `as_deref().unwrap_or("-")` 供给）。
#[allow(clippy::too_many_arguments)]
pub fn held_command(
    intent_id: &[u8; 16],
    state: &CmdState,
    attempts: u32,
    readback: &Option<f64>,
    reason: &str,
    plan_id: &[u8; 16],
    device: u32,
    point: u32,
    value_kw: f64,
) -> String {
    format!(
        "{PREFIX}HELD-COMMAND intent={:02x?} state={:?} attempts={} readback={:?} reason={} plan={:02x?} dev=0x{:x} pt={} val={}",
        intent_id, state, attempts, readback, reason, plan_id, device, point, value_kw
    )
}

/// HELD-PLAN-DONE 行（一次性收尾证据：逐命令终态三桶计数守恒 +
/// committed 终态词）。
pub fn held_plan_done(
    plan_id: &[u8; 16],
    batches: usize,
    applied: usize,
    blocked: usize,
    failed: usize,
    state: &str,
) -> String {
    format!(
        "{PREFIX}HELD-PLAN-DONE plan={:02x?} batches={} applied={} blocked={} failed={} state={}",
        plan_id, batches, applied, blocked, failed, state
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    /// 词形 golden 锁（本表 = 词形唯一事实源；QEMU 门 grep 的对象）。
    /// 变异验证：表内任一字符漂移 ⇒ 本测红（调用点零字面，漂移只可能
    /// 发生在表）。
    #[test]
    fn evidence_word_forms_golden() {
        assert_eq!(
            EXEC_JOURNAL_APPLIED_CONFIRMED,
            "[svc-control-dispatch] EXEC-JOURNAL applied-confirmed（独立回读匹配→Committed）"
        );
        assert_eq!(
            receipt_state_note(WORD_TYPED, DispatchState::Failed, &None),
            "[svc-control-dispatch] TYPED state=Failed note=None"
        );
        assert_eq!(
            receipt_state_note(
                WORD_TRUSTED,
                DispatchState::Indeterminate,
                &Some(String::from("timeout"))
            ),
            "[svc-control-dispatch] TRUSTED state=Indeterminate note=Some(\"timeout\")"
        );
        assert_eq!(
            exec_journal_recovered(3),
            "[svc-control-dispatch] EXEC-JOURNAL recovered pending=3 → reconciling"
        );
        assert_eq!(
            safety_rejected("ramp-contract"),
            "[svc-control-dispatch] SAFETY-REJECTED reason=ramp-contract"
        );
        // `-ERR` 尾缀拼装（调用点只传入口词）。
        assert_eq!(
            submit_err(WORD_TYPED, &DispatchError::DeliveryTimeout),
            "[svc-control-dispatch] TYPED-ERR DeliveryTimeout"
        );
    }

    /// 蓝图 75 WP-75.1.2（R16-G）九族 golden：期望值 = 迁移前 engine.rs /
    /// held_engine.rs 各 `println!` 字面**逐字节手转录**（卷册 §4：期望值
    /// 从现行为取，不从新代码取——防新实现自证）。变异验证：任改一族
    /// formatter 一个字符 ⇒ 本测红。
    #[test]
    fn evidence_extended_families_golden() {
        let plan16 = [0xa5u8; 16];
        let expect_plan16 = "[a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5]";
        let intent16 = [0x77u8; 16];
        let expect_intent16 = "[77, 77, 77, 77, 77, 77, 77, 77, 77, 77, 77, 77, 77, 77, 77, 77]";
        // CERTIFIED（engine.rs run_typed；key_id u16 / key_version u32）。
        assert_eq!(
            certified(
                &[0x12, 0x34, 0x56, 0x78],
                &[0xaa, 0xbb, 0xcc, 0xdd],
                7,
                3,
                5
            ),
            "[svc-control-dispatch] CERTIFIED plan=[12, 34, 56, 78] cert=[aa, bb, cc, dd] constraints=7 key=3/5"
        );
        // PLAN-COMPILED（engine.rs run_typed）。
        assert_eq!(
            plan_compiled(
                2,
                5,
                1,
                0,
                &[0xca, 0xfe, 0xba, 0xbe],
                &[0xde, 0xad, 0xbe, 0xef],
                &plan16
            ),
            "[svc-control-dispatch] PLAN-COMPILED batches=2 intents=5 skipped=1 manual=0 compiler=[ca, fe, ba, be] batch=[de, ad, be, ef] plan=[a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5, a5]"
        );
        // PLAN-COMMAND-DEFERRED / PLAN-COMMAND-SKIP-EXPIRED
        // / PLAN-COMMANDS-DEFERRED（engine.rs 逐命令循环）。
        assert_eq!(
            plan_command_deferred(&[0x11, 0x22, 0x33, 0x44], 1_500),
            "[svc-control-dispatch] PLAN-COMMAND-DEFERRED intent=[11, 22, 33, 44] due_in_ms=1500"
        );
        assert_eq!(
            plan_command_skip_expired(&[0x11, 0x22, 0x33, 0x44]),
            "[svc-control-dispatch] PLAN-COMMAND-SKIP-EXPIRED intent=[11, 22, 33, 44]"
        );
        assert_eq!(
            plan_commands_deferred(2, 1, 1),
            "[svc-control-dispatch] PLAN-COMMANDS-DEFERRED deferred=2 expired=1 delivered=1"
        );
        // PLAN-HELD（held_engine.rs accept_held_plan）。
        assert_eq!(
            plan_held(&plan16, 2, 6),
            alloc::format!(
                "[svc-control-dispatch] PLAN-HELD plan={expect_plan16} batches=2 intents=6"
            )
        );
        // HELD-BATCH（held_engine.rs drive_held_plans）。
        assert_eq!(
            held_batch(
                &[0xbe, 0xef, 0xca, 0xfe],
                &crate::saga::BatchState::Completed,
                3,
                &plan16
            ),
            alloc::format!(
                "[svc-control-dispatch] HELD-BATCH batch=[be, ef, ca, fe] state=Completed events=3 plan={expect_plan16}"
            )
        );
        // HELD-COMMAND（held_engine.rs；9 个 kv 字段，reason 缺省 `-`）。
        assert_eq!(
            held_command(
                &intent16,
                &crate::saga::CmdState::Applied,
                2,
                &Some(1.05),
                "-",
                &plan16,
                1,
                10,
                1.5
            ),
            alloc::format!(
                "[svc-control-dispatch] HELD-COMMAND intent={expect_intent16} state=Applied attempts=2 readback=Some(1.05) reason=- plan={expect_plan16} dev=0x1 pt=10 val=1.5"
            )
        );
        // HELD-PLAN-DONE（held_engine.rs finalize_held_plans）。
        assert_eq!(
            held_plan_done(&plan16, 2, 3, 1, 0, "Committed"),
            alloc::format!(
                "[svc-control-dispatch] HELD-PLAN-DONE plan={expect_plan16} batches=2 applied=3 blocked=1 failed=0 state=Committed"
            )
        );
    }
}
