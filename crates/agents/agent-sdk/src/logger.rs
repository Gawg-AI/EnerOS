//! 日志 API（SDK v1.0 稳定 API）。
//!
//! 提供结构化日志接口。SDK 层提供 Mock 实现（计数），
//! 真实日志输出由 Agent Runtime 注入。

use heapless::String as HString;

use crate::agent::AgentId;

/// 日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// 跟踪（最详细）
    Trace = 0,
    /// 调试
    Debug = 1,
    /// 信息
    Info = 2,
    /// 警告
    Warn = 3,
    /// 错误
    Error = 4,
}

impl LogLevel {
    /// 获取级别名称。
    pub fn name(&self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

/// 日志器。
///
/// Agent 通过此结构输出日志。
pub struct Logger {
    /// 所属 Agent ID
    agent_id: AgentId,
    /// 最低输出级别
    min_level: LogLevel,
    /// 日志计数（统计用）
    log_count: u64,
    /// 最后一条日志（Mock 用）
    last_message: HString<256>,
}

impl Logger {
    /// 创建新的日志器。
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            min_level: LogLevel::Info,
            log_count: 0,
            last_message: HString::<256>::new(),
        }
    }

    /// 设置最低输出级别。
    pub fn with_min_level(mut self, level: LogLevel) -> Self {
        self.min_level = level;
        self
    }

    /// 输出日志。
    pub fn log(&mut self, level: LogLevel, msg: &str) {
        if level < self.min_level {
            return;
        }
        self.log_count += 1;
        self.last_message = HString::<256>::try_from(msg).unwrap_or_default();
        // SDK 层 Mock：仅记录，真实输出由 Runtime 注入
    }

    /// 跟踪日志。
    pub fn trace(&mut self, msg: &str) {
        self.log(LogLevel::Trace, msg);
    }

    /// 调试日志。
    pub fn debug(&mut self, msg: &str) {
        self.log(LogLevel::Debug, msg);
    }

    /// 信息日志。
    pub fn info(&mut self, msg: &str) {
        self.log(LogLevel::Info, msg);
    }

    /// 警告日志。
    pub fn warn(&mut self, msg: &str) {
        self.log(LogLevel::Warn, msg);
    }

    /// 错误日志。
    pub fn error(&mut self, msg: &str) {
        self.log(LogLevel::Error, msg);
    }

    /// 获取日志计数。
    pub fn log_count(&self) -> u64 {
        self.log_count
    }

    /// 获取最后一条日志。
    pub fn last_message(&self) -> &str {
        self.last_message.as_str()
    }

    /// 获取所属 Agent ID。
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// 获取最低输出级别。
    pub fn min_level(&self) -> LogLevel {
        self.min_level
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// LOG1：基本日志。
    #[test]
    fn log1_basic_logging() {
        let mut logger = Logger::new(AgentId(1));
        logger.info("hello");
        assert_eq!(logger.log_count(), 1);
        assert_eq!(logger.last_message(), "hello");
    }

    /// LOG2：级别过滤。
    #[test]
    fn log2_level_filtering() {
        let mut logger = Logger::new(AgentId(1)).with_min_level(LogLevel::Warn);
        logger.debug("should be filtered");
        assert_eq!(logger.log_count(), 0);

        logger.warn("should pass");
        assert_eq!(logger.log_count(), 1);

        logger.error("should also pass");
        assert_eq!(logger.log_count(), 2);
    }

    /// LOG3：所有级别。
    #[test]
    fn log3_all_levels() {
        let mut logger = Logger::new(AgentId(1)).with_min_level(LogLevel::Trace);
        logger.trace("t");
        logger.debug("d");
        logger.info("i");
        logger.warn("w");
        logger.error("e");
        assert_eq!(logger.log_count(), 5);
    }

    /// LOG4：级别名称。
    #[test]
    fn log4_level_names() {
        assert_eq!(LogLevel::Trace.name(), "TRACE");
        assert_eq!(LogLevel::Debug.name(), "DEBUG");
        assert_eq!(LogLevel::Info.name(), "INFO");
        assert_eq!(LogLevel::Warn.name(), "WARN");
        assert_eq!(LogLevel::Error.name(), "ERROR");
    }

    /// LOG5：级别排序。
    #[test]
    fn log5_level_ordering() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
    }
}
