//! 统一 Verdict（蓝图 36 WP-36.8）。
//!
//! 唯一判定枚举：PASS / FAIL / PENDING / BLOCKED / NO_RESULT。
//! Release Gate 只接受 PASS；其余一律非零退出。
//! 只有显式 `--allow-non-pass`（信息报告模式）可 exit 0，
//! 且产物必须写 `non_release=true`，不得被 release evidence gate 消费。

use serde::Serialize;

/// 唯一判定枚举。`verdict 为 PASS` 是 release Gate 放行的唯一条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Verdict {
    /// 所有 oracle 对当前候选成立。
    Pass,
    /// 明确违反。
    Fail,
    /// 尚未执行 / 资源未就绪（如 placeholder、skip 推导）。
    Pending,
    /// 环境 / 外部条件阻塞（缺 SEL4_SRC、缺 peer、缺硬件、缺密钥）。
    Blocked,
    /// 缺报告 / 超时 / 解析失败。
    NoResult,
}

impl Verdict {
    /// release 命令退出码：只有 PASS 是 0。
    #[allow(dead_code)] // 44.2：release 退出码辅助，当前由 release 流程内联（44.4 冻结登记）
    pub fn release_exit_code(self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Fail | Self::Pending | Self::Blocked | Self::NoResult => 1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Pending => "PENDING",
            Self::Blocked => "BLOCKED",
            Self::NoResult => "NO_RESULT",
        }
    }

    /// 从任意字符串严格解析；未知值 → None（调用方必须按 FAIL 处理，
    /// 禁止把未知字符串折算为 PASS）。
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_uppercase().as_str() {
            "PASS" => Some(Self::Pass),
            "FAIL" => Some(Self::Fail),
            "PENDING" => Some(Self::Pending),
            "BLOCKED" => Some(Self::Blocked),
            "NO_RESULT" | "NO-RESULT" | "NONE" => Some(Self::NoResult),
            _ => None,
        }
    }
}

/// 信息报告允许非 PASS 的开关解析：只有显式 `--allow-non-pass` 才放行。
/// 返回 (allow, remaining_args)。
pub fn take_allow_non_pass(args: &[String]) -> (bool, Vec<String>) {
    let mut allow = false;
    let mut rest = Vec::with_capacity(args.len());
    for a in args.iter() {
        if a == "--allow-non-pass" {
            allow = true;
        } else {
            rest.push(a.clone());
        }
    }
    (allow, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pass_is_release_zero() {
        assert_eq!(Verdict::Pass.release_exit_code(), 0);
        for v in [
            Verdict::Fail,
            Verdict::Pending,
            Verdict::Blocked,
            Verdict::NoResult,
        ] {
            assert_ne!(v.release_exit_code(), 0, "{v:?} must be non-zero");
        }
    }

    #[test]
    fn parse_rejects_unknown_and_normalizes() {
        assert_eq!(Verdict::parse("PASS"), Some(Verdict::Pass));
        assert_eq!(Verdict::parse("pending"), Some(Verdict::Pending));
        assert_eq!(Verdict::parse("BLOCKED"), Some(Verdict::Blocked));
        assert_eq!(Verdict::parse("NO_RESULT"), Some(Verdict::NoResult));
        assert_eq!(Verdict::parse("VERIFIED"), None);
        assert_eq!(Verdict::parse("D2-PASS"), None);
        assert_eq!(Verdict::parse(""), None);
    }

    #[test]
    fn allow_non_pass_only_via_explicit_flag() {
        let (allow, rest) = take_allow_non_pass(&["--allow-non-pass".into(), "x".into()]);
        assert!(allow);
        assert_eq!(rest, vec!["x"]);
        let (allow, _) = take_allow_non_pass(&["x".into()]);
        assert!(!allow);
    }
}
