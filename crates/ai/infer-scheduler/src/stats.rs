//! 调度器统计（D5：普通 u64，不使用原子类型）.

/// 调度器运行统计.
///
/// 单线程 no_std 下使用普通 `u64` 计数器，无需原子操作（D5）。
#[derive(Debug, Clone, Default)]
pub struct SchedulerStats {
    /// 总提交请求数.
    pub total_requests: u64,
    /// 已完成请求数（推理成功）.
    pub completed_requests: u64,
    /// 超时请求数.
    pub timed_out_requests: u64,
    /// 失败请求数（推理错误或 Cache 耗尽）.
    pub failed_requests: u64,
    /// KV Cache 淘汰次数.
    pub cache_evictions: u64,
}

impl SchedulerStats {
    /// 记录一次提交.
    pub fn record_submit(&mut self) {
        self.total_requests += 1;
    }

    /// 记录一次完成.
    pub fn record_complete(&mut self) {
        self.completed_requests += 1;
    }

    /// 记录一次超时.
    pub fn record_timeout(&mut self) {
        self.timed_out_requests += 1;
    }

    /// 记录一次失败.
    pub fn record_failure(&mut self) {
        self.failed_requests += 1;
    }

    /// 记录一次 Cache 淘汰.
    pub fn record_eviction(&mut self) {
        self.cache_evictions += 1;
    }
}
