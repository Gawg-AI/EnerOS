//! 采样统计 — SamplingStats.
//!
//! [`SamplingStats`] 记录采样次数、读取失败次数与最近采样时间戳.
//!
//! # 偏差 D7
//!
//! 不使用 `AtomicU64`（no_std 单线程无需，与 v0.54.0 D8 一致）.

/// 采样统计.
#[derive(Debug, Clone, Default)]
pub struct SamplingStats {
    /// 累计采样次数.
    pub sample_count: u64,
    /// 累计读取失败次数.
    pub read_failures: u64,
    /// 最近一次采样时间戳（微秒，D1）.
    pub last_sample_time_us: u64,
}

impl SamplingStats {
    /// 创建空统计.
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次采样.
    ///
    /// - `now_us`：本次采样时间戳（微秒）.
    /// - `failure_count`：本次采样中读取失败的点数.
    pub fn record_sample(&mut self, now_us: u64, failure_count: u64) {
        self.sample_count += 1;
        self.read_failures += failure_count;
        self.last_sample_time_us = now_us;
    }
}
