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

use eneros_jsonl_wire::{hstr, JsonlError, Scan};
use heapless::String as HString;

use crate::log_filter::LogFilter;
use crate::logging::{LogEntry, LogError, LogLevel, LogStorage};

/// JSONL 骨架错误 → 本 crate 错误词形（变体名冻结：WP-74.3.3 映射表）.
impl From<JsonlError> for LogError {
    fn from(_: JsonlError) -> Self {
        Self::SerializeFailed
    }
}

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

/// 读取非负整数（骨架 read_number_text 原文 + u64 解析——领域数值形状
/// 留本 crate，WP-74.3.3）.
fn read_number(sc: &mut Scan) -> Result<u64, LogError> {
    let text = sc.read_number_text()?;
    text.parse().map_err(|_| LogError::SerializeFailed)
}

/// 读取 fields 子对象（"k":"v" 键值对）.
fn read_fields(sc: &mut Scan) -> Result<heapless::Vec<(HString<32>, HString<128>), 8>, LogError> {
    sc.expect('{')?;
    let mut fields = heapless::Vec::new();
    loop {
        sc.skip_ws();
        if sc.peek() == Some('}') {
            break;
        }
        let k = sc.read_string()?;
        sc.expect(':')?;
        let v = sc.read_string()?;
        let _ = fields.push((hstr::<32>(&k), hstr::<128>(&v)));
        sc.skip_ws();
        if sc.peek() == Some('}') {
            break;
        }
        sc.expect(',')?;
    }
    // 消费子对象收尾 '}'（否则外层解析会把 fields 的 '}' 误判为
    // 整个对象的结束，后续 trace_id/span_id/agent_id 全丢）。
    sc.expect('}')?;
    Ok(fields)
}

/// 将 to_json 生成的一行 JSON 反序列化为 `LogEntry`.
fn parse_log_json(line: &str) -> Result<LogEntry, LogError> {
    let mut sc = Scan::new(line);
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
            "ts" => entry.timestamp_ms = read_number(&mut sc)?,
            "level" => {
                let lv = sc.read_string()?;
                entry.level = parse_level(&lv)?;
            }
            "target" => entry.target = hstr::<64>(&sc.read_string()?),
            "msg" => entry.message = hstr::<256>(&sc.read_string()?),
            "fields" => entry.fields = read_fields(&mut sc)?,
            "trace_id" => entry.trace_id = read_number(&mut sc)?,
            "span_id" => entry.span_id = read_number(&mut sc)?,
            "agent_id" => entry.agent_id = read_number(&mut sc)? as u32,
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
                LogBuilder::new(
                    LogLevel::Warn,
                    "modbus.tcp",
                    "含\"引号\"与\\反斜杠及\u{01}控制符",
                    42,
                )
                .field("desc", "值\"含引号\"")
                .build(),
            )
            .unwrap();
        assert_eq!(logger.flush_to(&mut store).unwrap(), 1);

        let read_back = store.read_all().unwrap();
        assert_eq!(read_back.len(), 1);
        assert_eq!(
            read_back[0].message.as_str(),
            "含\"引号\"与\\反斜杠及\u{01}控制符"
        );
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

    /// 历史 bug 回归 fixture（WP-74.3.3；file_store.rs 收尾注释现场）：
    /// fields 收尾 `}` 未消费时被误判为整行结束，其后 trace_id/span_id/
    /// agent_id 全丢——非空 fields 且后随非零追踪三元组的条目必须逐字段
    /// 往返保全（jsonl-wire 单源化后此形态为双 crate 共用回归素材）.
    #[test]
    fn fields_closing_brace_regression_keeps_trace_identity() {
        let dir = std::env::temp_dir().join(format!("eneros-log-fs4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fields-regression.jsonl");
        let path_str = path.to_str().unwrap();

        let mut store = FileStorage::open(path_str).unwrap();
        let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
        logger
            .log(
                LogBuilder::new(LogLevel::Info, "agent.energy", "带字段与追踪身份", 3000)
                    .trace(99)
                    .span(7)
                    .agent(3)
                    .field("power_kw", "120.5")
                    .build(),
            )
            .unwrap();
        assert_eq!(logger.flush_to(&mut store).unwrap(), 1);
        store.flush().unwrap();

        let read_back = store.read_all().unwrap();
        assert_eq!(read_back.len(), 1);
        // 修复前：fields 收尾 '}' 误判整行结束 → 三元组全丢（恒 0）。
        assert_eq!(read_back[0].trace_id, 99);
        assert_eq!(read_back[0].span_id, 7);
        assert_eq!(read_back[0].agent_id, 3);
        assert_eq!(read_back[0].fields.len(), 1);
        assert_eq!(read_back[0].fields[0].0.as_str(), "power_kw");
        assert_eq!(read_back[0].fields[0].1.as_str(), "120.5");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
