//! 控制循环 trait + 循环统计.
//!
//! [`ControlLoop`] 是 RTOS 控制闭环的核心 trait，定义控制循环的生命周期：
//! 初始化 → 执行 → 关闭。不要求 `Send + Sync`（D5：no_std 单线程无需）.

use alloc::string::String;

use crate::error::ControlError;

/// 控制循环 trait.
///
/// 每个控制循环声明自己的名称和周期（微秒），由 [`crate::engine::ControlLoopEngine`]
/// 按周期调度执行。`execute` 接收 `elapsed_us`（距上次 tick 的流逝时间）.
///
/// # 偏差 D5
///
/// 不要求 `Send + Sync`（no_std 单线程无需）.
pub trait ControlLoop {
    /// 循环名称（人类可读）.
    fn name(&self) -> &str;
    /// 循环周期（微秒）.
    fn period_us(&self) -> u64;
    /// 初始化循环.
    fn init(&mut self) -> Result<(), ControlError>;
    /// 执行一步控制.
    fn execute(&mut self, elapsed_us: u64) -> Result<(), ControlError>;
    /// 关闭循环（释放资源）.
    fn shutdown(&mut self);
}

/// 单循环统计信息.
///
/// 记录循环的执行次数、抖动、错误次数等运行时统计.
#[derive(Clone)]
pub struct LoopStats {
    /// 循环名称.
    pub name: String,
    /// 循环周期（微秒）.
    pub period_us: u64,
    /// 上次执行耗时（微秒）.
    pub last_exec_time_us: u64,
    /// 上次抖动（微秒）.
    pub last_jitter_us: u64,
    /// 最大抖动（微秒）.
    pub max_jitter_us: u64,
    /// 累计抖动（微秒）.
    pub total_jitter_us: u64,
    /// 执行次数.
    pub exec_count: u64,
    /// 错误次数.
    pub error_count: u64,
}

impl LoopStats {
    /// 创建循环统计（初始值全为 0，除 name/period_us）.
    pub fn new(name: String, period_us: u64) -> Self {
        Self {
            name,
            period_us,
            last_exec_time_us: 0,
            last_jitter_us: 0,
            max_jitter_us: 0,
            total_jitter_us: 0,
            exec_count: 0,
            error_count: 0,
        }
    }
}
