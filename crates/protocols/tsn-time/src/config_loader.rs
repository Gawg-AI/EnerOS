//! v0.80.0 TAS 配置构造器（D15：纯 Rust 构造，无 TOML 解析）.
//!
//! 提供 [`build_tas_config`] 函数，将 `cycle_us` / `base_time_s` /
//! 调度表条目 / 端口数组装为 [`TasConfig`]。TOML 解析由 eneros-config
//! v0.26.0 上层加载后调用本构造器；本版本不引入 `toml` / `serde`
//! 依赖.

use alloc::vec::Vec;

use crate::tas::{TasConfig, TasScheduleEntry};

/// 构造 [`TasConfig`]（由 eneros-config v0.26.0 上层加载 TOML 后调用）.
///
/// 直接组装 `TasConfig` 字段，不进行任何解析或校验（校验由
/// [`TasScheduler::validate_schedule`](crate::tas::TasScheduler::validate_schedule)
/// 在调度器构造后执行）.
pub fn build_tas_config(
    cycle_us: u64,
    base_time_s: u64,
    entries: Vec<TasScheduleEntry>,
    port_count: u8,
) -> TasConfig {
    TasConfig {
        cycle_us,
        base_time_s,
        schedule: entries,
        port_count,
    }
}
