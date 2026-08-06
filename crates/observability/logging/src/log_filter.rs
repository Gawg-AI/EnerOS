//! 日志过滤条件与匹配逻辑（v0.121.0）.
//!
//! 对应蓝图 §4.2（LogFilter）与 §4.5（query 闭包过滤语义）：七个可选
//! 条件全部 AND；`LogFilter::default()` 全 None 表示匹配一切。

use crate::logging::{LogEntry, LogLevel};

/// 日志过滤条件（蓝图 §4.2；全 Option，AND 语义）.
#[derive(Debug, Clone, Default)]
pub struct LogFilter<'a> {
    /// 最低级别（含）.
    pub level_min: Option<LogLevel>,
    /// target 包含子串.
    pub target_contains: Option<&'a str>,
    /// message 包含子串.
    pub message_contains: Option<&'a str>,
    /// 追踪 ID 精确匹配.
    pub trace_id: Option<u64>,
    /// Agent ID 精确匹配.
    pub agent_id: Option<u32>,
    /// 时间窗下界（含）.
    pub start_ms: Option<u64>,
    /// 时间窗上界（含）.
    pub end_ms: Option<u64>,
}

impl<'a> LogFilter<'a> {
    /// 判断条目是否满足全部条件（AND 语义，照蓝图 §4.5 query 闭包）.
    pub fn matches(&self, entry: &LogEntry) -> bool {
        if let Some(min) = self.level_min {
            if entry.level < min {
                return false;
            }
        }
        if let Some(target) = self.target_contains {
            if !entry.target.as_str().contains(target) {
                return false;
            }
        }
        if let Some(msg) = self.message_contains {
            if !entry.message.as_str().contains(msg) {
                return false;
            }
        }
        if let Some(tid) = self.trace_id {
            if entry.trace_id != tid {
                return false;
            }
        }
        if let Some(aid) = self.agent_id {
            if entry.agent_id != aid {
                return false;
            }
        }
        if let Some(start) = self.start_ms {
            if entry.timestamp_ms < start {
                return false;
            }
        }
        if let Some(end) = self.end_ms {
            if entry.timestamp_ms > end {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::logging::LogBuilder;

    /// FLT8：单条件过滤（level_min）.
    #[test]
    fn flt8_single_condition() {
        let info = LogBuilder::new(LogLevel::Info, "t", "m", 0).build();
        let error = LogBuilder::new(LogLevel::Error, "t", "m", 0).build();
        let f = LogFilter {
            level_min: Some(LogLevel::Error),
            ..LogFilter::default()
        };
        assert!(!f.matches(&info));
        assert!(f.matches(&error));
        // 默认过滤器匹配一切
        assert!(LogFilter::default().matches(&info));
    }

    /// FLT9：组合条件 AND（level_min + agent_id + target_contains）.
    #[test]
    fn flt9_combined_and() {
        let e = LogBuilder::new(LogLevel::Error, "agent.pv", "越限", 0)
            .agent(7)
            .build();
        let ok = LogFilter {
            level_min: Some(LogLevel::Warn),
            target_contains: Some("pv"),
            agent_id: Some(7),
            ..LogFilter::default()
        };
        assert!(ok.matches(&e));
        // 任一条件不满足即不匹配
        let bad_agent = LogFilter {
            agent_id: Some(8),
            ..LogFilter::default()
        };
        assert!(!bad_agent.matches(&e));
        let bad_target = LogFilter {
            target_contains: Some("wind"),
            ..LogFilter::default()
        };
        assert!(!bad_target.matches(&e));
    }

    /// FLT10：时间窗边界（start<=ts<=end 含边界）.
    #[test]
    fn flt10_time_window_inclusive() {
        let e = LogBuilder::new(LogLevel::Info, "t", "m", 100).build();
        let win = LogFilter {
            start_ms: Some(100),
            end_ms: Some(100),
            ..LogFilter::default()
        };
        assert!(win.matches(&e));
        let before = LogFilter {
            start_ms: Some(101),
            ..LogFilter::default()
        };
        assert!(!before.matches(&e));
        let after = LogFilter {
            end_ms: Some(99),
            ..LogFilter::default()
        };
        assert!(!after.matches(&e));
    }
}
