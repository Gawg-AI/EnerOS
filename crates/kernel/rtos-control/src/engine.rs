//! 控制循环引擎 — 多循环调度与错误隔离.
//!
//! [`ControlLoopEngine`] 持有多个 [`ControlLoop`]，按周期调度执行.
//! 不实现阻塞式 `run() -> !`（D3：改为 `tick` 单步驱动）.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::loop_trait::{ControlLoop, LoopStats};
use crate::stats::EngineStats;

/// 控制循环引擎.
///
/// 遍历所有注册的循环，对到期的循环（`now_us - last_execute_us >= period_us`）
/// 调用 `execute(elapsed_us)`。错误隔离：单个循环返回 `Err` 不影响其他循环.
pub struct ControlLoopEngine {
    /// 注册的控制循环列表.
    loops: Vec<Box<dyn ControlLoop>>,
    /// 每循环的统计信息.
    loop_stats: Vec<LoopStats>,
    /// 每循环的上次执行时间（微秒），初始为 0.
    last_execute_us: Vec<u64>,
    /// 引擎全局统计.
    stats: EngineStats,
}

/// 单次 tick 的执行报告.
#[derive(Debug, Clone, Default)]
pub struct EngineTickReport {
    /// 本次 tick 执行的循环数.
    pub executed_loops: usize,
    /// 本次 tick 的错误数.
    pub errors: usize,
}

impl ControlLoopEngine {
    /// 创建引擎（空）.
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册控制循环.
    ///
    /// 追加循环到列表，初始化 `LoopStats` 和 `last_execute_us = 0`.
    pub fn register(&mut self, ctrl: Box<dyn ControlLoop>) {
        let name = alloc::string::String::from(ctrl.name());
        let period = ctrl.period_us();
        self.loop_stats.push(LoopStats::new(name, period));
        self.last_execute_us.push(0);
        self.loops.push(ctrl);
    }

    /// 推进一个 tick.
    ///
    /// 遍历所有循环，对到期的循环调用 `execute(elapsed_us)`.
    /// 抖动 = `elapsed_us.saturating_sub(period_us)`（首次 tick 为 0）.
    /// 返回执行报告.
    pub fn tick(&mut self, now_us: u64, elapsed_us: u64) -> EngineTickReport {
        self.stats.total_ticks += 1;
        let mut executed = 0usize;
        let mut errors = 0usize;

        for i in 0..self.loops.len() {
            let period = self.loops[i].period_us();
            let last = self.last_execute_us[i];

            if now_us.saturating_sub(last) >= period {
                let is_first = last == 0;
                let jitter = if is_first {
                    0
                } else {
                    elapsed_us.saturating_sub(period)
                };

                let result = self.loops[i].execute(elapsed_us);
                let is_error = result.is_err();

                // 更新引擎统计
                let name_str = self.loops[i].name();
                self.stats.update(name_str, jitter, elapsed_us, is_error);

                // 更新循环统计
                let ls = &mut self.loop_stats[i];
                ls.last_exec_time_us = elapsed_us;
                ls.last_jitter_us = jitter;
                if jitter > ls.max_jitter_us {
                    ls.max_jitter_us = jitter;
                }
                ls.total_jitter_us += jitter;
                ls.exec_count += 1;
                if is_error {
                    ls.error_count += 1;
                }

                self.last_execute_us[i] = now_us;

                executed += 1;
                if is_error {
                    errors += 1;
                }
            }
        }

        EngineTickReport {
            executed_loops: executed,
            errors,
        }
    }

    /// 获取引擎统计引用.
    pub fn stats(&self) -> &EngineStats {
        &self.stats
    }
}

impl Default for ControlLoopEngine {
    fn default() -> Self {
        Self {
            loops: Vec::new(),
            loop_stats: Vec::new(),
            last_execute_us: Vec::new(),
            stats: EngineStats::new(),
        }
    }
}
