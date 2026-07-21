//! 快照数据类型 — SampledPoint + StateSnapshot.
//!
//! [`SampledPoint`] 为单个采样点（`#[repr(C)]`，便于跨分区共享内存布局一致）；
//! [`StateSnapshot`] 为一帧完整快照（时间戳 + 序列号 + 采样点数组）.
//!
//! # 偏差 D9
//!
//! `StateSnapshot.points` 用固定数组 `[SampledPoint; MAX_POINTS]`（`#[repr(C)]`），
//! 避免动态分配，便于共享内存零拷贝读取.

/// 单快照最大采样点数（D9）.
pub const MAX_POINTS: usize = 256;

/// 单个采样点（`#[repr(C)]`，跨分区布局一致）.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SampledPoint {
    /// 点唯一标识.
    pub point_id: u32,
    /// 采样值（统一为 f64）.
    pub value: f64,
    /// 品质标志（1=有效，0=无效，D10）.
    pub quality: u8,
}

/// 一帧完整状态快照.
///
/// `points` 为固定数组 `[SampledPoint; MAX_POINTS]`，实际有效点数为 `point_count`.
/// `seq` 为单调递增序列号，用于双缓冲读取一致性校验.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateSnapshot {
    /// 快照时间戳（微秒，D1）.
    pub timestamp: u64,
    /// 序列号（写者每次写入递增）.
    pub seq: u64,
    /// 实际有效采样点数.
    pub point_count: u32,
    /// 采样点数组（固定容量 MAX_POINTS，前 point_count 个有效）.
    pub points: [SampledPoint; MAX_POINTS],
}

impl Default for StateSnapshot {
    /// 全零快照（`point_count=0`，`points` 全为 `SampledPoint::default()`）.
    fn default() -> Self {
        Self {
            timestamp: 0,
            seq: 0,
            point_count: 0,
            points: [SampledPoint::default(); MAX_POINTS],
        }
    }
}

impl StateSnapshot {
    /// 创建全零快照.
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回有效采样点切片（前 `point_count` 个）.
    pub fn get_points(&self) -> &[SampledPoint] {
        &self.points[..self.point_count as usize]
    }
}
