//! JSONL 线码单源叶 crate（蓝图 74 卷三 WP-74.3.3，R15-L）.
//!
//! `eneros-logging` 与 `eneros-metrics` 的 file_store 曾各自维护逐行同构的
//! JSON 扫描器与转义函数（双胞胎不同步的现场证据：logging 曾因 fields
//! 收尾 `}` 误判丢 trace_id/span_id 而修复，metrics 侧无此锁）。本 crate
//! 收编**骨架与字符串/转义**契约：
//!
//! - [`escape_json_into`]：`"`/`\` 转义 + 控制字符 `\u00xx`（写侧唯一实现）；
//! - [`Scan`]：单遍扫描骨架（skip_ws/peek/bump/expect/read_string/
//!   read_number_text/skip_value，读侧唯一实现）；
//! - [`hstr`]：超长截断转定容字符串（observability 族四处收编）。
//!
//! 数值解析与领域形状（logging 的 u64 fields 表 / metrics 的 labels 数组）
//! 留各 crate——共享层只收 wire 骨架。各 crate 错误词形经 `From<JsonlError>`
//! 映射且变体名冻结（`LogError::SerializeFailed` / `MetricsError::InvalidValue`）.
//!
//! 登记不扩面（异族）：security/recovery 两处 `hstr_truncate`、pentest
//! 自定义 `HString` 别位不在本单源（蓝图 74 卷三 §1 R15-L）.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt::Write as _;

/// 定容字符串别名（observability 族 wire 承载形态）.
pub type HString<const N: usize> = heapless::String<N>;

/// 将超长 `&str` 截断转换为定容 [`HString`]（放得下全量；放不下按字符
/// 边界截断到容量，不 panic）——原 logging.rs hstr 语义.
pub fn hstr<const N: usize>(s: &str) -> HString<N> {
    match HString::<N>::try_from(s) {
        Ok(h) => h,
        Err(_) => {
            let mut h = HString::<N>::new();
            for ch in s.chars() {
                if h.push(ch).is_err() {
                    break;
                }
            }
            h
        }
    }
}

/// JSON 字符串转义（写侧唯一实现）：`"` 与 `\` 转义，控制字符转
/// `\u00xx`——防注入破坏 JSONL 行结构.
pub fn escape_json_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                // 格式化失败仅理论可能，忽略写入结果
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// 骨架级错误（各 crate 经 `From` 映射到自有错误词形，变体名冻结）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonlError {
    /// 语法不合法（期望字符失配 / 引号未闭合 / 转义集外 / `\u` 截断或
    /// 非十六进制 / 数值文本为空）.
    Syntax,
}

/// 单遍 JSON 扫描骨架（UTF-8 安全；字符串值处理 `\"` `\\` `\u00XX` 转义集）
/// ——原 logging/metrics 双胞胎 Scan 的逐行同构收编.
pub struct Scan<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Scan<'a> {
    /// 从一整行 JSON 出发.
    pub fn new(s: &'a str) -> Self {
        Self { s, i: 0 }
    }

    pub fn skip_ws(&mut self) {
        while let Some(ch) = self.s[self.i..].chars().next() {
            if ch.is_whitespace() {
                self.i += ch.len_utf8();
            } else {
                break;
            }
        }
    }

    pub fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    pub fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.i += ch.len_utf8();
        Some(ch)
    }

    /// 跳过空白后期望字符 `c`（失配即 [`JsonlError::Syntax`]）.
    pub fn expect(&mut self, c: char) -> Result<(), JsonlError> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.i += c.len_utf8();
            Ok(())
        } else {
            Err(JsonlError::Syntax)
        }
    }

    /// 读取 JSON 字符串值（支持 `\"` `\\` `\u00XX` 转义；UTF-8 原样）.
    pub fn read_string(&mut self) -> Result<String, JsonlError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let ch = self.bump().ok_or(JsonlError::Syntax)?;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let esc = self.bump().ok_or(JsonlError::Syntax)?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'u' => {
                            let mut code = 0u32;
                            for _ in 0..4 {
                                let d = self.bump().ok_or(JsonlError::Syntax)?;
                                code = code * 16 + d.to_digit(16).ok_or(JsonlError::Syntax)?;
                            }
                            out.push(char::from_u32(code).ok_or(JsonlError::Syntax)?);
                        }
                        _ => return Err(JsonlError::Syntax),
                    }
                }
                c => out.push(c),
            }
        }
    }

    /// 读取数值原文本（直到逗号/收尾括号；u64/f64 由调用方解析——领域
    /// 数值形状留各 crate）.
    pub fn read_number_text(&mut self) -> Result<String, JsonlError> {
        self.skip_ws();
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == ',' || c == '}' {
                break;
            }
            self.i += c.len_utf8();
        }
        if self.i == start {
            return Err(JsonlError::Syntax);
        }
        Ok(self.s[start..self.i].to_string())
    }

    /// 跳过未知键的值（字符串/对象/数组——括号配平计数 + 字符串转义
    /// 感知；其余按数值原文本消费至逗号/收尾括号）.
    pub fn skip_value(&mut self) -> Result<(), JsonlError> {
        self.skip_ws();
        match self.peek() {
            Some('"') => {
                self.read_string()?;
                Ok(())
            }
            Some('{') | Some('[') => {
                let (open, close) = if self.peek() == Some('{') {
                    ('{', '}')
                } else {
                    ('[', ']')
                };
                let mut depth = 0usize;
                loop {
                    let ch = self.bump().ok_or(JsonlError::Syntax)?;
                    if ch == open {
                        depth += 1;
                    } else if ch == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if ch == '"' {
                        loop {
                            let c2 = self.bump().ok_or(JsonlError::Syntax)?;
                            if c2 == '\\' {
                                let _ = self.bump().ok_or(JsonlError::Syntax)?;
                            } else if c2 == '"' {
                                break;
                            }
                        }
                    }
                }
                Ok(())
            }
            _ => {
                let _ = self.read_number_text()?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// round-trip 契约套件（两 crate 共用素材）：escape → `"…"` →
    /// Scan::read_string 恒等往返.
    #[test]
    fn escape_scan_roundtrip_is_identity() {
        let samples = [
            "",
            "plain",
            "启动完成（UTF-8 多字节）",
            "含\"引号\"",
            "含\\反斜杠",
            "混合\"引号\"与\\反斜杠及中文",
            "控制符\u{01}\u{1f}结尾",
            "tab\t分隔",
        ];
        for sample in samples {
            let mut json = String::new();
            json.push('"');
            escape_json_into(&mut json, sample);
            json.push('"');
            let mut sc = Scan::new(&json);
            assert_eq!(
                sc.read_string().unwrap(),
                sample,
                "round-trip 失配: {sample:?}"
            );
        }
    }

    /// 转义集形态：控制字符落盘为 `\u00xx`（消费者可 grep 的 wire 形态）.
    #[test]
    fn escape_encodes_control_chars_as_unicode_hex() {
        let mut json = String::new();
        escape_json_into(&mut json, "a\u{01}b\u{1f}");
        assert_eq!(json, "a\\u0001b\\u001f");
    }

    /// 转义集负例：骨架只认 `\"` `\\` `\u`——其余转义拒绝.
    #[test]
    fn scan_rejects_escape_set_violations() {
        // 未闭合引号。
        let mut sc = Scan::new("\"未闭合");
        assert_eq!(sc.read_string(), Err(JsonlError::Syntax));
        // 转义集外（\n 不在骨架转义集）。
        let mut sc = Scan::new("\"a\\nb\"");
        assert_eq!(sc.read_string(), Err(JsonlError::Syntax));
        // \u 截断（不足 4 位十六进制）。
        let mut sc = Scan::new("\"a\\u00\"");
        assert_eq!(sc.read_string(), Err(JsonlError::Syntax));
        // \u 非十六进制位。
        let mut sc = Scan::new("\"a\\u00zz\"");
        assert_eq!(sc.read_string(), Err(JsonlError::Syntax));
    }

    /// hstr 语义：放得下全量；放不下按字符边界截断（多字节不撕裂）.
    #[test]
    fn hstr_fits_and_truncates_on_char_boundary() {
        assert_eq!(hstr::<8>("abc").as_str(), "abc");
        assert_eq!(hstr::<8>("12345678").as_str(), "12345678");
        // 多字节截断：8 字节容量装 3 字节汉字 → 2 个（6 字节）。
        assert_eq!(hstr::<8>("功率越限告警").as_str(), "功率");
        // 7 字节容量：第 3 个汉字（3 字节）装不下即止步（不撕裂）。
        assert_eq!(hstr::<7>("中文中文").as_str(), "中文");
    }

    /// read_number_text 消费至逗号/收尾括号（u64/f64 由调用方解析）.
    #[test]
    fn read_number_text_stops_at_delimiters() {
        let mut sc = Scan::new("123,\"x\":45.5}");
        assert_eq!(sc.read_number_text().unwrap(), "123");
        sc.skip_ws();
        assert_eq!(sc.expect(','), Ok(()));
        // 空文本（start==i）→ Syntax。
        let mut sc = Scan::new("}");
        assert_eq!(sc.read_number_text(), Err(JsonlError::Syntax));
    }

    /// skip_value：对象/数组括号配平 + 字符串转义感知 + 数值原文本.
    #[test]
    fn skip_value_balances_structures_and_strings() {
        // 嵌套对象 + 内嵌字符串（含转义引号）。
        let mut sc = Scan::new("{\"a\":{\"b\":\"含\\\"引号\\\"\"}},\"after\"");
        sc.skip_value().unwrap();
        sc.skip_ws();
        assert_eq!(sc.expect(','), Ok(()));
        // 数组配平。
        let mut sc = Scan::new("[1,[2,3],\"x\"],\"after\"");
        sc.skip_value().unwrap();
        sc.skip_ws();
        assert_eq!(sc.expect(','), Ok(()));
        // 数值原文本。
        let mut sc = Scan::new("42,\"after\"");
        sc.skip_value().unwrap();
        sc.skip_ws();
        assert_eq!(sc.expect(','), Ok(()));
    }

    /// ASCII 全域（0x00..=0x7F）逐字节 round-trip——防写读两表漂移的
    /// 粗压面（任一转义/还原图漏项即红）.
    #[test]
    fn escape_output_always_scan_readable() {
        for byte in 0x00u8..=0x7f {
            let mut s = String::new();
            s.push('x');
            s.push(byte as char);
            let mut json = String::new();
            json.push('"');
            escape_json_into(&mut json, &s);
            json.push('"');
            let mut sc = Scan::new(&json);
            assert_eq!(
                sc.read_string().unwrap(),
                s,
                "ASCII 0x{byte:02x} round-trip 失配"
            );
        }
    }
}
