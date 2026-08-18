//! JSONL 文件导出后端（33 OB-07：std feature 门控的仓内可运行导出链）.
//!
//! 日志不再只有内存环形缓冲——`FileStorage` 以追加式 JSONL 落盘（一条
//! JSON 一行），支持读回与按过滤器查询，构成
//! record → flush → JSONL 可读回 的 host 端到端商用验收证据链。
//!
//! no_std 合规：本模块整体 `#[cfg(feature = "std")]` 门控，默认（无 std
//! feature）构建不进入编译，交叉编译 `aarch64-unknown-none` 不受影响。

#![cfg(feature = "std")]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use heapless::String as HString;

use crate::log_filter::LogFilter;
use crate::logging::{LogEntry, LogError, LogLevel, LogStorage};

/// 追加型 JSONL 日志存储：每条 JSON 一行，进程重启不丢（33 OB-07）.
pub struct FileStorage {
    /// 追加写句柄.
    file: File,
    /// 文件路径（读回用）.
    path: String,
}

impl FileStorage {
    /// 打开（不存在则创建）JSONL 追加文件；路径不可用返回
    /// [`LogError::IoFailed`].
    pub fn open(path: &str) -> Result<Self, LogError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|_| LogError::IoFailed)?;
        Ok(Self {
            file,
            path: path.to_string(),
        })
    }

    /// 打开（不存在则创建）JSONL 追加文件（`AsRef<Path>` 重载）.
    pub fn open_path(path: &Path) -> Result<Self, LogError> {
        Self::open(&path.to_string_lossy())
    }

    /// 强制落盘（OS 缓冲刷到文件系统）.
    pub fn flush(&mut self) -> Result<(), LogError> {
        self.file.flush().map_err(|_| LogError::IoFailed)
    }

    /// 从文件读回全部条目（JSONL 反序列化；空行跳过）.
    pub fn read_all(&self) -> Result<Vec<LogEntry>, LogError> {
        let f = File::open(&self.path).map_err(|_| LogError::IoFailed)?;
        let reader = BufReader::new(f);
        let mut out = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|_| LogError::IoFailed)?;
            if line.trim().is_empty() {
                continue;
            }
            out.push(parse_log_json(&line)?);
        }
        Ok(out)
    }
}

impl LogStorage for FileStorage {
    fn append(&mut self, _entry: &LogEntry, json: &[u8]) -> Result<(), LogError> {
        // JSON 由本 crate to_json 生成，必为合法 UTF-8；写入一行 JSONL。
        self.file
            .write_all(json)
            .and_then(|()| self.file.write_all(b"\n"))
            .map_err(|_| LogError::IoFailed)
    }

    fn query(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, LogError> {
        let all = self.read_all()?;
        Ok(all.into_iter().filter(|e| filter.matches(e)).collect())
    }
}

// ===== JSONL 反序列化（识别本 crate to_json 的固定结构） =====

/// 将超长 `&str` 截断转换为定容 HString（与 logging.rs 内 hstr 同语义）.
fn hstr<const N: usize>(s: &str) -> HString<N> {
    match HString::try_from(s) {
        Ok(h) => h,
        Err(_) => {
            let mut h = HString::new();
            for ch in s.chars() {
                if h.push(ch).is_err() {
                    break;
                }
            }
            h
        }
    }
}

fn parse_level(s: &str) -> Result<LogLevel, LogError> {
    match s {
        "DEBUG" => Ok(LogLevel::Debug),
        "INFO" => Ok(LogLevel::Info),
        "WARN" => Ok(LogLevel::Warn),
        "ERROR" => Ok(LogLevel::Error),
        "FATAL" => Ok(LogLevel::Fatal),
        _ => Err(LogError::SerializeFailed),
    }
}

/// 单遍 JSON 扫描器（UTF-8 安全；字符串值处理 `\"` `\\` `\u00XX` 转义）.
struct Scan<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Scan<'a> {
    fn skip_ws(&mut self) {
        while let Some(ch) = self.s[self.i..].chars().next() {
            if ch.is_whitespace() {
                self.i += ch.len_utf8();
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.i += ch.len_utf8();
        Some(ch)
    }

    fn expect(&mut self, c: char) -> Result<(), LogError> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.i += c.len_utf8();
            Ok(())
        } else {
            Err(LogError::SerializeFailed)
        }
    }

    /// 读取 JSON 字符串值（支持 `\"` `\\` `\u00XX` 转义；UTF-8 原样）.
    fn read_string(&mut self) -> Result<String, LogError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let ch = self.bump().ok_or(LogError::SerializeFailed)?;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let esc = self.bump().ok_or(LogError::SerializeFailed)?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'u' => {
                            let mut code = 0u32;
                            for _ in 0..4 {
                                let d = self.bump().ok_or(LogError::SerializeFailed)?;
                                code =
                                    code * 16 + d.to_digit(16).ok_or(LogError::SerializeFailed)?;
                            }
                            out.push(char::from_u32(code).ok_or(LogError::SerializeFailed)?);
                        }
                        _ => return Err(LogError::SerializeFailed),
                    }
                }
                c => out.push(c),
            }
        }
    }

    /// 读取非负整数.
    fn read_number(&mut self) -> Result<u64, LogError> {
        self.skip_ws();
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
        if self.i == start {
            return Err(LogError::SerializeFailed);
        }
        self.s[start..self.i]
            .parse()
            .map_err(|_| LogError::SerializeFailed)
    }

    /// 读取 fields 子对象（"k":"v" 键值对）.
    fn read_fields(&mut self) -> Result<heapless::Vec<(HString<32>, HString<128>), 8>, LogError> {
        self.expect('{')?;
        let mut fields = heapless::Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some('}') {
                break;
            }
            let k = self.read_string()?;
            self.expect(':')?;
            let v = self.read_string()?;
            let _ = fields.push((hstr::<32>(&k), hstr::<128>(&v)));
            self.skip_ws();
            if self.peek() == Some('}') {
                break;
            }
            self.expect(',')?;
        }
        // 消费子对象收尾 '}'（否则外层解析会把 fields 的 '}' 误判为
        // 整个对象的结束，后续 trace_id/span_id/agent_id 全丢）。
        self.expect('}')?;
        Ok(fields)
    }

    /// 跳过未知键的值（字符串/对象/数字）.
    fn skip_value(&mut self) -> Result<(), LogError> {
        self.skip_ws();
        match self.peek() {
            Some('"') => {
                self.read_string()?;
                Ok(())
            }
            Some('{') => {
                let mut depth = 0usize;
                loop {
                    let ch = self.bump().ok_or(LogError::SerializeFailed)?;
                    match ch {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        '"' => loop {
                            let c2 = self.bump().ok_or(LogError::SerializeFailed)?;
                            if c2 == '\\' {
                                let _ = self.bump().ok_or(LogError::SerializeFailed)?;
                            } else if c2 == '"' {
                                break;
                            }
                        },
                        _ => {}
                    }
                }
                Ok(())
            }
            _ => {
                while let Some(c) = self.peek() {
                    if c == ',' || c == '}' {
                        break;
                    }
                    self.i += c.len_utf8();
                }
                Ok(())
            }
        }
    }
}

/// 将 to_json 生成的一行 JSON 反序列化为 `LogEntry`.
fn parse_log_json(line: &str) -> Result<LogEntry, LogError> {
    let mut sc = Scan { s: line, i: 0 };
    let mut entry = LogEntry {
        timestamp_ms: 0,
        level: LogLevel::Info,
        target: HString::new(),
        message: HString::new(),
        fields: heapless::Vec::new(),
        trace_id: 0,
        span_id: 0,
        agent_id: 0,
    };
    sc.expect('{')?;
    loop {
        sc.skip_ws();
        if sc.peek() == Some('}') {
            break;
        }
        let key = sc.read_string()?;
        sc.expect(':')?;
        sc.skip_ws();
        match key.as_str() {
            "ts" => entry.timestamp_ms = sc.read_number()?,
            "level" => {
                let lv = sc.read_string()?;
                entry.level = parse_level(&lv)?;
            }
            "target" => entry.target = hstr::<64>(&sc.read_string()?),
            "msg" => entry.message = hstr::<256>(&sc.read_string()?),
            "fields" => entry.fields = sc.read_fields()?,
            "trace_id" => entry.trace_id = sc.read_number()?,
            "span_id" => entry.span_id = sc.read_number()?,
            "agent_id" => entry.agent_id = sc.read_number()? as u32,
            _ => sc.skip_value()?,
        }
        sc.skip_ws();
        if sc.peek() == Some('}') {
            break;
        }
        sc.expect(',')?;
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::logging::{LogBuilder, StructuredLogger, StructuredLoggerImpl};

    /// 33 OB-07 判据：record → flush → JSONL 读回（逐字段一致）.
    #[test]
    fn jsonl_roundtrip_preserves_entry() {
        let dir = std::env::temp_dir().join(format!("eneros-log-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.jsonl");
        let path_str = path.to_str().unwrap();

        let mut store = FileStorage::open(path_str).unwrap();
        let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
        logger
            .log(
                LogBuilder::new(LogLevel::Info, "agent.energy", "启动完成", 1000)
                    .trace(99)
                    .span(7)
                    .agent(3)
                    .build(),
            )
            .unwrap();
        logger
            .log(
                LogBuilder::new(LogLevel::Error, "agent.pv", "功率越限", 2000)
                    .field("power_kw", "120.5")
                    .field("threshold", "100")
                    .build(),
            )
            .unwrap();
        let n = logger.flush_to(&mut store).unwrap();
        assert_eq!(n, 2);
        store.flush().unwrap();

        let read_back = store.read_all().unwrap();
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0].timestamp_ms, 1000);
        assert_eq!(read_back[0].level, LogLevel::Info);
        assert_eq!(read_back[0].target.as_str(), "agent.energy");
        assert_eq!(read_back[0].message.as_str(), "启动完成");
        assert_eq!(read_back[0].trace_id, 99);
        assert_eq!(read_back[0].span_id, 7);
        assert_eq!(read_back[0].agent_id, 3);
        assert_eq!(read_back[1].level, LogLevel::Error);
        assert_eq!(read_back[1].message.as_str(), "功率越限");
        assert_eq!(read_back[1].fields.len(), 2);
        assert_eq!(read_back[1].fields[0].0.as_str(), "power_kw");
        assert_eq!(read_back[1].fields[0].1.as_str(), "120.5");
        assert_eq!(read_back[1].fields[1].0.as_str(), "threshold");
        assert_eq!(read_back[1].fields[1].1.as_str(), "100");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 33 OB-07：中文（UTF-8 多字节）与引号/反斜杠转义往返.
    #[test]
    fn jsonl_roundtrip_escapes_and_utf8() {
        let dir = std::env::temp_dir().join(format!("eneros-log-fs2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("esc.jsonl");
        let path_str = path.to_str().unwrap();

        let mut store = FileStorage::open(path_str).unwrap();
        let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
        logger
            .log(
                LogBuilder::new(LogLevel::Warn, "modbus.tcp", "含\"引号\"与\\反斜杠", 42)
                    .field("desc", "值\"含引号\"")
                    .build(),
            )
            .unwrap();
        assert_eq!(logger.flush_to(&mut store).unwrap(), 1);

        let read_back = store.read_all().unwrap();
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].message.as_str(), "含\"引号\"与\\反斜杠");
        assert_eq!(read_back[0].fields[0].1.as_str(), "值\"含引号\"");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 33 OB-07：flush 后经 LogStorage::query 过滤可查（重启语义）.
    #[test]
    fn jsonl_query_through_logstorage() {
        let dir = std::env::temp_dir().join(format!("eneros-log-fs3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("q.jsonl");
        let path_str = path.to_str().unwrap();

        let mut store = FileStorage::open(path_str).unwrap();
        let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
        logger
            .log(LogBuilder::new(LogLevel::Info, "a.b", "m1", 1).build())
            .unwrap();
        logger
            .log(LogBuilder::new(LogLevel::Error, "a.c", "m2", 2).build())
            .unwrap();
        logger.flush_to(&mut store).unwrap();
        store.flush().unwrap();

        // 从全新 FileStorage 实例读回（模拟重启后查询）
        let reopened = FileStorage::open(path_str).unwrap();
        let filter = LogFilter {
            level_min: Some(LogLevel::Error),
            ..Default::default()
        };
        let got = reopened.query(&filter).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target.as_str(), "a.c");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
