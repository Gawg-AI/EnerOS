//! Deepening-v1 §B.2.1：`qa` 子命令参数解析的「四拍样板」收拢（FlagParser）。
//!
//! interface：`FlagParser::{bare, prefixed, next, take_path, take_string,
//! take_u64, take_usize, unknown, is_flag}`；implementation：每个 flag 的
//! `取值 → 缺失报错 → parse → 类型报错` 四拍 + 两套既有报错文案。
//!
//! 刻意**不做**声明式 flag 规格表（蓝图 §B.2「最小化，Karpathy 原则 2」）：
//! `match` 分支仍留在各命令里，被消除的只是逐字复演的取值/报错样板。
//!
//! 两套文案（`Wording`，R-B1 逐字冻结，见 `qa.rs` 的 `arg_contract` 锁定测试）：
//! - `Bare`（9 处循环）：`--out requires a value` / `--cycles must be usize` /
//!   `unknown soak-report arg '--bogus'`；
//! - `Prefixed`（perf-baseline / perf-measure）：
//!   `perf-baseline --out requires <path>` / `perf-baseline --slo-threshold
//!   must be u64` / `unknown perf-baseline arg '--bogus'`。
//!
//! 配对语义（R-B2）：flag 与**相邻**下一 arg 配对；重复 flag 后值覆盖前值
//! （调用方直接赋值即得）；位置参数由调用方处理（`--` 前缀者才报 unknown）。

use std::error::Error;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use super::{invalid, DynResult};

/// 报错文案样式（迁移前后逐字一致——CI/门日志可能匹配这些文案）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wording {
    /// `<flag> requires a value`：soak-report / interop-report / kat-report /
    /// compliance-report / sign-evidence / verify-evidence-sig / g7-audit /
    /// soak-run / perf-keygen。
    Bare,
    /// `<cmd> <flag> requires <kind>`：perf-baseline / perf-measure。
    Prefixed,
}

/// 单个命令的参数游标 + 命令名 + 报错文案样式。
///
/// 与旧式 `let mut it = args.iter(); while let Some(a) = it.next()` 逐点等价：
/// flag 与值共用同一游标，故「值缺失即报错」的发生位置、后续 arg 的消费顺序
/// 均不变。
pub(crate) struct FlagParser<'a> {
    args: &'a [String],
    cursor: usize,
    root: &'a Path,
    cmd: &'a str,
    wording: Wording,
}

impl<'a> FlagParser<'a> {
    /// `<flag> requires a value` 族（9 处循环）。
    pub(crate) fn bare(root: &'a Path, cmd: &'a str, args: &'a [String]) -> Self {
        Self::new(root, cmd, args, Wording::Bare)
    }

    /// `<cmd> <flag> requires <kind>` 族（perf-baseline / perf-measure）。
    pub(crate) fn prefixed(root: &'a Path, cmd: &'a str, args: &'a [String]) -> Self {
        Self::new(root, cmd, args, Wording::Prefixed)
    }

    fn new(root: &'a Path, cmd: &'a str, args: &'a [String], wording: Wording) -> Self {
        Self {
            args,
            cursor: 0,
            root,
            cmd,
            wording,
        }
    }

    /// 取下一个 arg（循环头）。返回引用的生命周期与 `args` 同，故可与
    /// `&mut self` 并存（`take_*` 可接收刚取出的 flag 本身）。
    pub(crate) fn next(&mut self) -> Option<&'a str> {
        let args: &'a [String] = self.args;
        let arg = args.get(self.cursor)?;
        self.cursor += 1;
        Some(arg.as_str())
    }

    /// `--flag <path>`：值按 `root` 相对解析（等价旧 `root.join(val()?)`）。
    pub(crate) fn take_path(&mut self, flag: &str) -> DynResult<PathBuf> {
        let value = self.value(flag, "<path>")?;
        Ok(self.root.join(value))
    }

    /// `--flag <string>`：值原样取用、不做路径拼接（等价旧
    /// `val()?.to_owned()` / `val()?.clone()` / `PathBuf::from(val()?)`）。
    pub(crate) fn take_string(&mut self, flag: &str) -> DynResult<String> {
        let value = self.value(flag, "<value>")?;
        Ok(value.to_owned())
    }

    /// `--flag <u64>`；`kind` = 类型报错文案中的类型描述（现网为 `"u64"`）。
    pub(crate) fn take_u64(&mut self, flag: &str, kind: &str) -> DynResult<u64> {
        self.take_parsed(flag, kind)
    }

    /// `--flag <usize>`；`kind` 同上（现网为 `"usize"`）。
    pub(crate) fn take_usize(&mut self, flag: &str, kind: &str) -> DynResult<usize> {
        self.take_parsed(flag, kind)
    }

    /// 四拍本体：取值 → 缺失报错 → `parse` → 类型报错。
    ///
    /// `FromStr::Err` 一律丢弃（现网文案不含底层解析错误），只报
    /// `<flag> must be <kind>`——`kind` 由调用方逐字给出（如
    /// `"u64 (bytes/day)"`），故无需为个别尾注新开变体（R-B1）。
    fn take_parsed<T: FromStr>(&mut self, flag: &str, kind: &str) -> DynResult<T> {
        let value = self.value(flag, &format!("<{kind}>"))?;
        value
            .parse()
            .map_err(|_| invalid(self.type_error_text(flag, kind)))
    }

    /// `unknown <cmd> arg '<arg>'`（11 处循环同形）。
    pub(crate) fn unknown(&self, arg: &str) -> Box<dyn Error> {
        invalid(format!("unknown {} arg '{arg}'", self.cmd))
    }

    /// 位置参数命令的判据：仅 `--` 前缀者算 flag（其余按位置值消费）。
    pub(crate) fn is_flag(arg: &str) -> bool {
        arg.starts_with("--")
    }

    fn value(&mut self, flag: &str, placeholder: &str) -> DynResult<&'a str> {
        match self.next() {
            Some(value) => Ok(value),
            None => Err(invalid(match self.wording {
                Wording::Bare => format!("{flag} requires a value"),
                Wording::Prefixed => format!("{} {flag} requires {placeholder}", self.cmd),
            })),
        }
    }

    fn type_error_text(&self, flag: &str, kind: &str) -> String {
        match self.wording {
            Wording::Bare => format!("{flag} must be {kind}"),
            Wording::Prefixed => format!("{} {flag} must be {kind}", self.cmd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn err_of(r: DynResult<()>) -> String {
        match r {
            Ok(()) => "«命令成功：期望 fail-closed 报错»".to_owned(),
            Err(e) => e.to_string(),
        }
    }

    /// 消费整串参数（每个已知 flag 各取其型），只为把报错文案跑出来。
    fn drain(parser: &mut FlagParser<'_>) -> DynResult<()> {
        while let Some(a) = parser.next() {
            match a {
                "--path" => {
                    parser.take_path(a)?;
                }
                "--u64" => {
                    parser.take_u64(a, "u64")?;
                }
                "--usize" => {
                    parser.take_usize(a, "usize")?;
                }
                other => return Err(parser.unknown(other)),
            }
        }
        Ok(())
    }

    /// R-B1：`Bare` 族三类文案与现网逐字一致。
    #[test]
    fn bare_wording_is_frozen() {
        let root = Path::new("/tmp/root");
        let cases: Vec<(Vec<&str>, &str)> = vec![
            (vec!["--path"], "--path requires a value"),
            (vec!["--u64"], "--u64 requires a value"),
            (vec!["--usize"], "--usize requires a value"),
            (vec!["--nope"], "unknown soak-run arg '--nope'"),
            (vec!["--u64", "abc"], "--u64 must be u64"),
            (vec!["--usize", "abc"], "--usize must be usize"),
            (vec!["--path", "a", "--u64"], "--u64 requires a value"),
        ];
        for (args, want) in cases {
            let owned = argv(&args);
            let mut parser = FlagParser::bare(root, "soak-run", &owned);
            assert_eq!(err_of(drain(&mut parser)), want, "args={args:?}");
        }
    }

    /// R-B1：`Prefixed` 族（perf-baseline / perf-measure）文案逐字一致。
    #[test]
    fn prefixed_wording_is_frozen() {
        let root = Path::new("/tmp/root");
        let cases: Vec<(&str, Vec<&str>, &str)> = vec![
            (
                "perf-baseline",
                vec!["--path"],
                "perf-baseline --path requires <path>",
            ),
            (
                "perf-baseline",
                vec!["--u64"],
                "perf-baseline --u64 requires <u64>",
            ),
            (
                "perf-measure",
                vec!["--u64"],
                "perf-measure --u64 requires <u64>",
            ),
            (
                "perf-baseline",
                vec!["--u64", "abc"],
                "perf-baseline --u64 must be u64",
            ),
            (
                "perf-baseline",
                vec!["--usize", "abc"],
                "perf-baseline --usize must be usize",
            ),
            (
                "perf-measure",
                vec!["--zzz"],
                "unknown perf-measure arg '--zzz'",
            ),
        ];
        for (cmd, args, want) in cases {
            let owned = argv(&args);
            let mut parser = FlagParser::prefixed(root, cmd, &owned);
            assert_eq!(err_of(drain(&mut parser)), want, "args={args:?}");
        }
    }

    /// R-B1：类型报错的类型描述由调用方逐字给出（`--leak-threshold` 带单位）。
    #[test]
    fn kind_label_is_verbatim_in_type_error() {
        let root = Path::new("/tmp/root");
        let args = argv(&["--leak-threshold", "abc"]);
        let mut parser = FlagParser::bare(root, "soak-run", &args);
        parser.next();
        let got = parser.take_u64("--leak-threshold", "u64 (bytes/day)");
        assert_eq!(
            err_of(got.map(|_| ())),
            "--leak-threshold must be u64 (bytes/day)"
        );
    }

    /// 路径值按 root 拼接、字符串值原样保留（`--out` 与 `--image` 之别）。
    #[test]
    fn paths_join_to_root_while_strings_stay_verbatim() {
        let root = Path::new("/tmp/root");
        let args = argv(&["--path", "sub/out.json", "--str", "rel/image.raw"]);
        let mut parser = FlagParser::bare(root, "soak-run", &args);
        let mut got: Vec<String> = Vec::new();
        while let Some(a) = parser.next() {
            match a {
                "--path" => got.push(parser.take_path(a).unwrap().display().to_string()),
                "--str" => got.push(parser.take_string(a).unwrap()),
                other => got.push(format!("UNEXPECTED {other}")),
            }
        }
        assert_eq!(
            got,
            vec![
                "/tmp/root/sub/out.json".to_owned(),
                "rel/image.raw".to_owned()
            ]
        );
    }

    /// R-B2：重复 flag —— 前值各自配对消费，后值覆盖前值。
    #[test]
    fn repeated_flags_last_value_wins() {
        let root = Path::new("/tmp/root");
        let args = argv(&["--path", "first.json", "--path", "second.json"]);
        let mut parser = FlagParser::bare(root, "perf-keygen", &args);
        let mut seen: Vec<String> = Vec::new();
        while let Some(a) = parser.next() {
            match a {
                "--path" => seen.push(parser.take_path(a).unwrap().display().to_string()),
                other => seen.push(format!("UNEXPECTED {other}")),
            }
        }
        assert_eq!(
            seen,
            vec![
                "/tmp/root/first.json".to_owned(),
                "/tmp/root/second.json".to_owned()
            ]
        );
        // 末次出现无值 → 仍按该次出现报缺值（不得回退到前值）。
        let args = argv(&["--path", "a.bin", "--path"]);
        let mut parser = FlagParser::bare(root, "perf-keygen", &args);
        assert_eq!(
            err_of(drain(&mut parser)),
            "--path requires a value",
            "R-B2：末次出现的缺值须报错"
        );
    }

    #[test]
    fn numeric_values_are_parsed() {
        let root = Path::new("/tmp/root");
        let args = argv(&["--u64", "67108864", "--usize", "3"]);
        let mut parser = FlagParser::bare(root, "soak-run", &args);
        let mut got: Vec<String> = Vec::new();
        while let Some(a) = parser.next() {
            match a {
                "--u64" => got.push(parser.take_u64(a, "u64").unwrap().to_string()),
                "--usize" => got.push(parser.take_usize(a, "usize").unwrap().to_string()),
                other => got.push(format!("UNEXPECTED {other}")),
            }
        }
        assert_eq!(got, vec!["67108864".to_owned(), "3".to_owned()]);
    }

    #[test]
    fn is_flag_distinguishes_positional_args() {
        assert!(FlagParser::is_flag("--sign-key"));
        assert!(!FlagParser::is_flag("evidence.txt"));
        assert!(!FlagParser::is_flag(""));
    }
}
