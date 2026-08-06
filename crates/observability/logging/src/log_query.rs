//! 日志存储实现：容量可配的内存环形存储（v0.121.0）.
//!
//! 对应蓝图 §4.2（LogStorage trait）与 §4.5（批量刷入）。`MemoryLogStorage`
//! 以 `alloc::collections::VecDeque` 保存（条目 + JSON），写满轮转丢弃最旧。

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use crate::log_filter::LogFilter;
use crate::logging::{LogEntry, LogError, LogStorage};

/// 内存环形日志存储：容量可配，写满丢弃最旧（对应蓝图 §6.5 故障注入）.
pub struct MemoryLogStorage {
    /// 存储条目（日志条目 + JSON 序列化结果），按插入顺序排列.
    entries: VecDeque<(LogEntry, String)>,
    /// 容量上限.
    capacity: usize,
}

impl MemoryLogStorage {
    /// 创建存储；`capacity` 为容量上限.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// 当前存储条数.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 存储是否为空.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 读取第 `index` 条（按插入顺序）的 JSON 文本；越界返回 None.
    pub fn json_of(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(|(_, j)| j.as_str())
    }
}

impl LogStorage for MemoryLogStorage {
    fn append(&mut self, entry: &LogEntry, json: &[u8]) -> Result<(), LogError> {
        // 写满轮转：丢弃最旧一条
        while self.entries.len() >= self.capacity {
            let _ = self.entries.pop_front();
        }
        // JSON 由本 crate to_json 生成，必为合法 UTF-8；非 UTF-8 视为序列化失败
        let text = core::str::from_utf8(json).map_err(|_| LogError::SerializeFailed)?;
        self.entries.push_back((entry.clone(), String::from(text)));
        Ok(())
    }

    fn query(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, LogError> {
        // 按插入顺序返回匹配子集
        let results: Vec<LogEntry> = self
            .entries
            .iter()
            .filter(|(e, _)| filter.matches(e))
            .map(|(e, _)| e.clone())
            .collect();
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::logging::{to_json, LogBuilder, LogLevel};

    /// STO17：append 后可 query 到.
    #[test]
    fn sto17_append_then_query() {
        let mut store = MemoryLogStorage::new(8);
        let e = LogBuilder::new(LogLevel::Info, "agent.pv", "启动", 1)
            .agent(3)
            .build();
        let json = to_json(&e);
        store.append(&e, json.as_bytes()).unwrap();
        let got = store.query(&LogFilter::default()).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].agent_id, 3);
        // JSON 可读回且含关键字段
        assert!(store.json_of(0).unwrap().contains("\"agent_id\":3"));
    }

    /// STO18：容量轮转丢弃最旧，总数保持 N.
    #[test]
    fn sto18_rotation_drops_oldest() {
        let mut store = MemoryLogStorage::new(4);
        for i in 0..6u64 {
            let e = LogBuilder::new(LogLevel::Info, "t", "m", i).build();
            let json = to_json(&e);
            store.append(&e, json.as_bytes()).unwrap();
        }
        assert_eq!(store.len(), 4);
        let got = store.query(&LogFilter::default()).unwrap();
        // 最旧的 2 条（ts=0,1）已轮转丢弃
        assert_eq!(got[0].timestamp_ms, 2);
        assert_eq!(got[3].timestamp_ms, 5);
    }

    /// STO19：存储 query 组合过滤（level_min + agent_id）.
    #[test]
    fn sto19_query_combined_filter() {
        let mut store = MemoryLogStorage::new(16);
        for i in 0..6u64 {
            let (level, agent) = if i % 2 == 0 {
                (LogLevel::Error, 7)
            } else {
                (LogLevel::Info, 8)
            };
            let e = LogBuilder::new(level, "t", "m", i).agent(agent).build();
            let json = to_json(&e);
            store.append(&e, json.as_bytes()).unwrap();
        }
        let f = LogFilter {
            level_min: Some(LogLevel::Error),
            agent_id: Some(7),
            ..LogFilter::default()
        };
        let got = store.query(&f).unwrap();
        assert_eq!(got.len(), 3);
        for e in &got {
            assert_eq!(e.level, LogLevel::Error);
            assert_eq!(e.agent_id, 7);
        }
    }

    /// STO20：空存储查询返回空.
    #[test]
    fn sto20_empty_storage_returns_empty() {
        let store = MemoryLogStorage::new(8);
        let got = store.query(&LogFilter::default()).unwrap();
        assert!(got.is_empty());
    }
}
