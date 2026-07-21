//! 高频采样错误类型.
//!
//! 定义 [`SamplingError`]，覆盖点读取失败、快照不一致、点数超限、未初始化等错误场景.

/// 高频采样错误（4 变体）.
///
/// 派生 `Debug`/`Clone`/`PartialEq`，便于在测试中精确匹配错误类型.
#[derive(Debug, Clone, PartialEq)]
pub enum SamplingError {
    /// 点读取失败（协议层返回错误或数据点不存在）.
    PointReadFailed,
    /// 快照不一致（双缓冲读取重试上限耗尽，检测到写者切换）.
    SnapshotInconsistent,
    /// 采样点数超过上限（超过 [`crate::snapshot::MAX_POINTS`]）.
    TooManyPoints,
    /// 采样服务未初始化.
    NotInitialized,
}

impl core::fmt::Display for SamplingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SamplingError::PointReadFailed => write!(f, "point read failed"),
            SamplingError::SnapshotInconsistent => write!(f, "snapshot inconsistent"),
            SamplingError::TooManyPoints => write!(f, "too many points"),
            SamplingError::NotInitialized => write!(f, "not initialized"),
        }
    }
}
