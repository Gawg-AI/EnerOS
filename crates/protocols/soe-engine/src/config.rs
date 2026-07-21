//! SOE 引擎配置与统计.

/// SOE 引擎配置.
#[derive(Debug, Clone)]
pub struct SoeConfig {
    /// 事件队列最大长度.
    pub max_queue_size: usize,
    /// 是否启用持久化.
    pub persist_enabled: bool,
    /// 持久化批量大小（队列达到此长度触发批量写入）.
    pub persist_batch_size: usize,
    /// 上传间隔（毫秒）.
    pub upload_interval_ms: u32,
    /// 事件保留天数（超期自动清理）.
    pub retention_days: u32,
}

impl Default for SoeConfig {
    fn default() -> Self {
        Self {
            max_queue_size: 10_000,
            persist_enabled: true,
            persist_batch_size: 100,
            upload_interval_ms: 5_000,
            retention_days: 90,
        }
    }
}

/// SOE 引擎运行时统计.
#[derive(Debug, Clone, Default)]
pub struct SoeStats {
    /// 累计记录事件数.
    pub total_events: u64,
    /// 已持久化事件数.
    pub persisted_events: u64,
    /// 已上传事件数.
    pub uploaded_events: u64,
    /// 丢弃事件数（队列满等）.
    pub dropped_events: u64,
}
