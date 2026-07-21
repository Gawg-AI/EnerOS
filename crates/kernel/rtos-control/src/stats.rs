//! 引擎统计 — 抖动统计与全局统计.
//!
//! [`JitterStats`] 记录单循环的抖动/执行/错误统计；
//! [`EngineStats`] 汇总所有循环的统计信息.
//!
//! # 偏差 D8/D9
//!
//! - D8：不使用 `AtomicU64`（no_std 单线程无需）.
//! - D9：不使用 `BTreeMap<&str, u64>`，用 `Vec<(String, JitterStats)>`.

use alloc::string::String;
use alloc::vec::Vec;

/// 单循环抖动统计.
#[derive(Clone, Default)]
pub struct JitterStats {
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
    /// 上次执行耗时（微秒）.
    pub last_exec_time_us: u64,
}

/// 引擎全局统计.
///
/// 按 `Vec<(String, JitterStats)>` 存储每循环统计（D9）.
#[derive(Clone, Default)]
pub struct EngineStats {
    /// 按循环名称索引的抖动统计列表.
    pub per_loop: Vec<(String, JitterStats)>,
    /// 总 tick 次数.
    pub total_ticks: u64,
}

impl EngineStats {
    /// 创建引擎统计（空）.
    pub fn new() -> Self {
        Self::default()
    }

    /// 更新指定循环的统计.
    ///
    /// 若该循环名称不存在则新建条目.
    pub fn update(&mut self, name: &str, jitter_us: u64, exec_time_us: u64, is_error: bool) {
        let idx = self.per_loop.iter().position(|(n, _)| n == name);
        let idx = match idx {
            Some(i) => i,
            None => {
                self.per_loop
                    .push((String::from(name), JitterStats::default()));
                self.per_loop.len() - 1
            }
        };

        let stats = &mut self.per_loop[idx].1;
        stats.last_jitter_us = jitter_us;
        if jitter_us > stats.max_jitter_us {
            stats.max_jitter_us = jitter_us;
        }
        stats.total_jitter_us += jitter_us;
        stats.exec_count += 1;
        if is_error {
            stats.error_count += 1;
        }
        stats.last_exec_time_us = exec_time_us;
    }

    /// 查询指定循环的统计.
    pub fn get(&self, name: &str) -> Option<&JitterStats> {
        self.per_loop
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
    }
}
