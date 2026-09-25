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
}
