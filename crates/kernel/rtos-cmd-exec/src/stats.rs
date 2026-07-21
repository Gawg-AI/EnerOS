//! ExecutorStats / ExecutorReport — 执行统计与单次报告（D7）.

/// 累计执行统计（跨 tick 累加，单线程，不用 AtomicU64，D7）.
#[derive(Debug, Clone, Default)]
pub struct ExecutorStats {
    /// 成功下发次数.
    pub success_count: u64,
    /// 失败下发次数.
    pub failure_count: u64,
    /// TTL 过期次数.
    pub expired_count: u64,
    /// 约束拒绝次数.
    pub rejected_count: u64,
    /// 截断下发次数.
    pub truncated_count: u64,
    /// 未映射次数.
    pub unmapped_count: u64,
    /// 总执行次数（含所有分支）.
    pub total_executed: u64,
}

/// 单次 tick 执行报告.
#[derive(Debug, Clone, Default)]
pub struct ExecutorReport {
    /// 本次 tick 处理的命令总数.
    pub total: usize,
    /// 成功下发数.
    pub success: usize,
    /// 失败下发数.
    pub failed: usize,
    /// TTL 过期数.
    pub expired: usize,
    /// 约束拒绝数.
    pub rejected: usize,
    /// 截断下发数.
    pub truncated: usize,
    /// 未映射数.
    pub unmapped: usize,
}
