//! 结构化日志数据模型、JSON 序列化、构建器与日志器实现（v0.121.0）.
//!
//! 对应蓝图 §4.1（数据结构）/ §4.2（接口定义）/ §4.4（错误处理）/ §4.5
//! （关键代码）。按偏差 D2，`LogBuilder::new` 通过 `now_ms` 参数传入
//! 时间戳；按偏差 D3，`LogStorage::append` 增加 `entry` 参数；实现期
//! 补充：JSON 输出含 `span_id`，且对字符串做 `"`/`\` 转义。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use heapless::String as HString;

use crate::log_filter::LogFilter;

/// 日志级别（蓝图 §4.1，repr 序与严重性一致）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd)]
#[repr(u8)]
pub enum LogLevel {
    /// 调试.
    Debug = 0,
    /// 信息.
    Info = 1,
    /// 警告.
    Warn = 2,
    /// 错误.
    Error = 3,
    /// 致命.
    Fatal = 4,
}

/// 日志条目（蓝图 §4.1）.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// 时间戳（毫秒）.
    pub timestamp_ms: u64,
    /// 日志级别.
    pub level: LogLevel,
    /// 模块路径（≤64 字节）.
    pub target: HString<64>,
    /// 消息（≤256 字节）.
    pub message: HString<256>,
    /// 结构化字段（≤8 个键值对，key≤32 字节，value≤128 字节）.
    pub fields: heapless::Vec<(HString<32>, HString<128>), 8>,
    /// 追踪 ID（关联 v0.122.0）.
    pub trace_id: u64,
    /// Span ID.
    pub span_id: u64,
    /// Agent ID.
    pub agent_id: u32,
}

/// 日志全流程错误（蓝图 §4.4）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogError {
    /// 序列化失败.
    SerializeFailed,
    /// 存储满.
    StorageFull,
    /// 过滤条件非法.
    FilterInvalid,
    /// 字段数超限.
    FieldLimitExceeded,
}

/// 结构化日志器 trait（蓝图 §4.2；偏差 D1 查询返回 alloc Vec）.
pub trait StructuredLogger {
    /// 记录一条日志；低于 min_level 的条目静默丢弃（返回 Ok）.
    fn log(&mut self, entry: LogEntry) -> Result<(), LogError>;
    /// 动态调整最低记录级别.
    fn set_level(&mut self, level: LogLevel);
    /// 按过滤条件查询缓冲中的条目.
    fn query(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, LogError>;
}

/// 日志存储 trait（偏差 D3：append 增加 entry 参数以支撑按条目查询）.
pub trait LogStorage {
    /// 追加一条日志及其 JSON 序列化结果.
    fn append(&mut self, entry: &LogEntry, json: &[u8]) -> Result<(), LogError>;
    /// 按过滤条件查询存储中的条目.
    fn query(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, LogError>;
}

/// 将 `&str` 截断转换为定容 HString（超长截断，不 panic）.
fn hstr<const N: usize>(s: &str) -> HString<N> {
    match HString::try_from(s) {
        Ok(h) => h,
        Err(_) => {
            // 超长时按字符边界截断到容量
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

/// JSON 字符串转义：`"` 与 `\` 转义，控制字符转 `\u00XX`（防注入破坏结构）.
fn escape_json_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                // 控制字符转义为 \u00XX；格式化失败仅理论可能，忽略写入结果
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// 序列化日志条目为 JSON（蓝图 §4.5；补充 span_id 与字符串转义）.
pub fn to_json(entry: &LogEntry) -> String {
    let mut json = String::new();
    json.push_str("{\"ts\":");
    let _ = write!(json, "{}", entry.timestamp_ms);
    json.push_str(",\"level\":\"");
    json.push_str(match entry.level {
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
        LogLevel::Fatal => "FATAL",
    });
    json.push_str("\",\"target\":\"");
    escape_json_into(&mut json, entry.target.as_str());
    json.push_str("\",\"msg\":\"");
    escape_json_into(&mut json, entry.message.as_str());
    json.push_str("\",\"fields\":{");
    for (i, (k, v)) in entry.fields.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push('"');
        escape_json_into(&mut json, k.as_str());
        json.push_str("\":\"");
        escape_json_into(&mut json, v.as_str());
        json.push('"');
    }
    json.push_str("},\"trace_id\":");
    let _ = write!(json, "{}", entry.trace_id);
    json.push_str(",\"span_id\":");
    let _ = write!(json, "{}", entry.span_id);
    json.push_str(",\"agent_id\":");
    let _ = write!(json, "{}", entry.agent_id);
    json.push('}');
    json
}

/// 日志构建器（蓝图 §4.5；偏差 D2：new 增加 now_ms 参数）.
pub struct LogBuilder {
    entry: LogEntry,
}

impl LogBuilder {
    /// 创建构建器；`now_ms` 为日志时间戳（替代蓝图系统时钟调用）.
    pub fn new(level: LogLevel, target: &str, message: &str, now_ms: u64) -> Self {
        Self {
            entry: LogEntry {
                timestamp_ms: now_ms,
                level,
                target: hstr(target),
                message: hstr(message),
                fields: heapless::Vec::new(),
                trace_id: 0,
                span_id: 0,
                agent_id: 0,
            },
        }
    }

    /// 追加结构化字段；超过 8 个截断丢弃.
    pub fn field(mut self, key: &str, value: &str) -> Self {
        let _ = self.entry.fields.push((hstr(key), hstr(value)));
        self
    }

    /// 设置追踪 ID.
    pub fn trace(mut self, trace_id: u64) -> Self {
        self.entry.trace_id = trace_id;
        self
    }

    /// 设置 Span ID.
    pub fn span(mut self, span_id: u64) -> Self {
        self.entry.span_id = span_id;
        self
    }

    /// 设置 Agent ID.
    pub fn agent(mut self, agent_id: u32) -> Self {
        self.entry.agent_id = agent_id;
        self
    }

    /// 构建日志条目.
    pub fn build(self) -> LogEntry {
        self.entry
    }
}

/// 结构化日志器实现（蓝图 §4.5；128 条内存缓冲 + 批量刷入）.
pub struct StructuredLoggerImpl {
    /// 最低记录级别.
    min_level: LogLevel,
    /// 内存缓冲（满 128 丢弃最旧）.
    buffer: heapless::Vec<LogEntry, 128>,
    /// 累计刷入次数.
    flush_count: usize,
}

impl StructuredLoggerImpl {
    /// 创建日志器；`min_level` 为最低记录级别.
    pub fn new(min_level: LogLevel) -> Self {
        Self {
            min_level,
            buffer: heapless::Vec::new(),
            flush_count: 0,
        }
    }

    /// 累计刷入次数（供测试与运维观测）.
    pub fn flush_count(&self) -> usize {
        self.flush_count
    }

    /// 缓冲当前条目数（供测试与运维观测）.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// 批量刷入存储：逐条序列化并 append 后清空缓冲，返回刷入条数.
    pub fn flush_to<S: LogStorage>(&mut self, store: &mut S) -> Result<usize, LogError> {
        // 33 OB-02：按成功进度收缩——逐条 append，成功即 remove（旧实现
        // 批量刷入失败后 buffer 未收缩 → 已成功条目下次重复写）。
        let mut flushed = 0usize;
        while !self.buffer.is_empty() {
            let entry = self.buffer[0].clone();
            let json = to_json(&entry);
            match store.append(&entry, json.as_bytes()) {
                Ok(()) => {
                    self.buffer.remove(0);
                    flushed += 1;
                }
                Err(e) => return Err(e),
            }
        }
        self.flush_count += 1;
        Ok(flushed)
    }
}

impl StructuredLogger for StructuredLoggerImpl {
    fn log(&mut self, entry: LogEntry) -> Result<(), LogError> {
        // 低于最低级别的条目静默丢弃
        if entry.level < self.min_level {
            return Ok(());
        }
        // 缓冲满 128 时丢弃最旧一条（对应蓝图 §6.5 故障注入）
        if self.buffer.is_full() {
            self.buffer.remove(0);
        }
        let _ = self.buffer.push(entry);
        Ok(())
    }

    fn set_level(&mut self, level: LogLevel) {
        self.min_level = level;
    }

    fn query(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, LogError> {
        let results: Vec<LogEntry> = self
            .buffer
            .iter()
            .filter(|e| filter.matches(e))
            .cloned()
            .collect();
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::log_query::MemoryLogStorage;

    /// LVL1：级别排序 Error > Info（派生 PartialOrd 与 repr 序一致）.
    #[test]
    fn lvl1_level_ordering() {
        assert!(LogLevel::Error > LogLevel::Info);
        assert!(LogLevel::Fatal > LogLevel::Error);
        assert!(LogLevel::Debug < LogLevel::Warn);
        assert_eq!(LogLevel::Info as u8, 1);
    }

    /// LVL2：builder 默认级别为 new 传入的级别，trace/span/agent 默认 0.
    #[test]
    fn lvl2_builder_default_level() {
        let e = LogBuilder::new(LogLevel::Warn, "agent.sched", "msg", 100).build();
        assert_eq!(e.level, LogLevel::Warn);
        assert_eq!(e.timestamp_ms, 100);
        assert_eq!(e.trace_id, 0);
        assert_eq!(e.span_id, 0);
        assert_eq!(e.agent_id, 0);
    }

    /// JSON3：固定字段齐全且值正确.
    #[test]
    fn json3_fixed_fields_present() {
        let e = LogBuilder::new(LogLevel::Info, "agent.energy", "启动完成", 1234)
            .trace(99)
            .span(7)
            .agent(3)
            .build();
        let json = to_json(&e);
        assert!(json.contains("\"ts\":1234"));
        assert!(json.contains("\"level\":\"INFO\""));
        assert!(json.contains("\"target\":\"agent.energy\""));
        assert!(json.contains("\"msg\":\"启动完成\""));
        assert!(json.contains("\"fields\":{"));
        assert!(json.contains("\"trace_id\":99"));
        assert!(json.contains("\"span_id\":7"));
        assert!(json.contains("\"agent_id\":3"));
        assert!(json.starts_with('{') && json.ends_with('}'));
    }

    /// JSON4：fields 子对象键值正确.
    #[test]
    fn json4_fields_subobject() {
        let e = LogBuilder::new(LogLevel::Error, "agent.pv", "功率越限", 10)
            .field("power_kw", "120.5")
            .field("threshold", "100")
            .build();
        let json = to_json(&e);
        assert!(json.contains("\"power_kw\":\"120.5\""));
        assert!(json.contains("\"threshold\":\"100\""));
        assert!(json.contains("\",\"fields\":{"));
    }

    /// JSON5：含引号/反斜杠的消息转义后不破坏 JSON 结构.
    #[test]
    fn json5_escapes_quotes_and_backslash() {
        let e = LogBuilder::new(LogLevel::Warn, "agent.io", "路径 \"C:\\temp\" 不可写", 1).build();
        let json = to_json(&e);
        // 转义后的序列不应出现未转义的裸引号破坏结构
        assert!(json.contains("\\\"C:\\\\temp\\\""));
        // 结构收尾完整：首尾大括号成对，msg 字段可被定位
        assert!(json.starts_with('{') && json.ends_with('}'));
        let msg_pos = json.find("\"msg\":\"").unwrap();
        let fields_pos = json.find("\",\"fields\":{").unwrap();
        assert!(msg_pos < fields_pos);
    }

    /// BLD6：builder field/trace/span/agent 链式正确.
    #[test]
    fn bld6_builder_chaining() {
        let e = LogBuilder::new(LogLevel::Debug, "t", "m", 5)
            .field("k1", "v1")
            .trace(11)
            .span(22)
            .agent(33)
            .build();
        assert_eq!(e.trace_id, 11);
        assert_eq!(e.span_id, 22);
        assert_eq!(e.agent_id, 33);
        assert_eq!(e.fields.len(), 1);
        assert_eq!(e.fields[0].0.as_str(), "k1");
        assert_eq!(e.fields[0].1.as_str(), "v1");
    }

    /// BLD7：field 超 8 个截断丢弃.
    #[test]
    fn bld7_fields_truncated_at_eight() {
        let mut b = LogBuilder::new(LogLevel::Info, "t", "m", 0);
        for i in 0..12 {
            let mut key = String::new();
            write!(key, "k{i}").unwrap();
            b = b.field(&key, "v");
        }
        let e = b.build();
        assert_eq!(e.fields.len(), 8);
        assert_eq!(e.fields[7].0.as_str(), "k7");
    }

    /// LOG11：低于 min_level 的条目静默丢弃（返回 Ok 且缓冲无新增）.
    #[test]
    fn log11_below_level_dropped() {
        let mut logger = StructuredLoggerImpl::new(LogLevel::Warn);
        let e = LogBuilder::new(LogLevel::Info, "t", "m", 0).build();
        assert_eq!(logger.log(e), Ok(()));
        assert_eq!(logger.buffered(), 0);
    }

    /// LOG12：达到级别的条目入缓冲.
    #[test]
    fn log12_at_level_buffered() {
        let mut logger = StructuredLoggerImpl::new(LogLevel::Warn);
        let e = LogBuilder::new(LogLevel::Error, "t", "m", 0).build();
        logger.log(e).unwrap();
        assert_eq!(logger.buffered(), 1);
    }

    /// LOG13：set_level 动态生效.
    #[test]
    fn log13_set_level_dynamic() {
        let mut logger = StructuredLoggerImpl::new(LogLevel::Error);
        logger
            .log(LogBuilder::new(LogLevel::Warn, "t", "m", 0).build())
            .unwrap();
        assert_eq!(logger.buffered(), 0);
        logger.set_level(LogLevel::Debug);
        logger
            .log(LogBuilder::new(LogLevel::Warn, "t", "m", 1).build())
            .unwrap();
        assert_eq!(logger.buffered(), 1);
    }

    /// LOG14：缓冲满 128 时丢弃最旧.
    #[test]
    fn log14_buffer_full_drops_oldest() {
        let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
        for i in 0..130u64 {
            logger
                .log(LogBuilder::new(LogLevel::Info, "t", "m", i).build())
                .unwrap();
        }
        assert_eq!(logger.buffered(), 128);
        // 最旧的 2 条（ts=0,1）已被丢弃，首条为 ts=2
        let all = logger.query(&LogFilter::default()).unwrap();
        assert_eq!(all[0].timestamp_ms, 2);
        assert_eq!(all[127].timestamp_ms, 129);
    }

    /// LOG15：flush 批量刷入、清空缓冲并返回条数.
    #[test]
    fn log15_flush_batch_and_clear() {
        let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
        let mut store = MemoryLogStorage::new(16);
        for i in 0..10u64 {
            logger
                .log(LogBuilder::new(LogLevel::Info, "t", "m", i).build())
                .unwrap();
        }
        let n = logger.flush_to(&mut store).unwrap();
        assert_eq!(n, 10);
        assert_eq!(logger.buffered(), 0);
        assert_eq!(store.len(), 10);
    }

    /// LOG16：flush_count 累加.
    #[test]
    fn log16_flush_count_accumulates() {
        let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
        let mut store = MemoryLogStorage::new(16);
        logger
            .log(LogBuilder::new(LogLevel::Info, "t", "m", 0).build())
            .unwrap();
        logger.flush_to(&mut store).unwrap();
        logger
            .log(LogBuilder::new(LogLevel::Info, "t", "m", 1).build())
            .unwrap();
        logger.flush_to(&mut store).unwrap();
        assert_eq!(logger.flush_count(), 2);
    }
}
