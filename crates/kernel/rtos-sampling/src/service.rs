//! 高频采样服务 — SamplingService.
//!
//! [`SamplingService`] 周期性读取一组点，归一化为 [`SampledPoint`] 后写入
//! [`SharedMemorySnapshot`]，并更新 [`SamplingStats`].
//!
//! # 偏差 D5/D6
//!
//! - D5：不实现阻塞式 `run()` 循环（`sample(now_us)` 单步接口）.
//! - D6：不使用 `Box<dyn PointAccess>`（改为泛型 `<P: PointAccess>`）.

use alloc::vec::Vec;

use eneros_protocol_abstract::PointAccess;
use eneros_upa_model::{PointId, PointValue};

use crate::shared_memory::SharedMemorySnapshot;
use crate::snapshot::SampledPoint;
use crate::stats::SamplingStats;

/// 高频采样服务.
///
/// 泛型 `<P: PointAccess>` 避免动态分发（D6）.
pub struct SamplingService<P: PointAccess> {
    /// 待采样的点 ID 列表.
    point_ids: Vec<PointId>,
    /// 采样周期（微秒，D1）.
    period_us: u64,
    /// 双缓冲快照.
    snapshot: SharedMemorySnapshot,
    /// 点访问协议.
    protocol: P,
    /// 采样统计.
    stats: SamplingStats,
}

/// 单次采样报告.
#[derive(Debug, Clone, Default)]
pub struct SampleReport {
    /// 成功采样的点数.
    pub sampled_count: usize,
    /// 读取失败的点数.
    pub failed_count: usize,
    /// 本次写入的快照序列号.
    pub snapshot_seq: u64,
}

impl<P: PointAccess> SamplingService<P> {
    /// 创建采样服务.
    pub fn new(point_ids: Vec<PointId>, period_us: u64, protocol: P) -> Self {
        Self {
            point_ids,
            period_us,
            snapshot: SharedMemorySnapshot::new(),
            protocol,
            stats: SamplingStats::new(),
        }
    }

    /// 执行一次采样.
    ///
    /// 逐点读取协议，将 `PointValue` 归一化为 `f64`（Float/Int/Bool），
    /// 其余类型（Enum/String/Null）计为失败。成功点写入双缓冲快照.
    pub fn sample(&mut self, now_us: u64) -> SampleReport {
        let mut points: Vec<SampledPoint> = Vec::with_capacity(self.point_ids.len());
        let mut failed = 0usize;
        for &pid in &self.point_ids {
            match self.protocol.read_point(pid) {
                Ok(point) => {
                    let value = match point.value {
                        PointValue::Float(v) => v,
                        PointValue::Int(v) => v as f64,
                        PointValue::Bool(b) => {
                            if b {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        _ => {
                            failed += 1;
                            continue;
                        }
                    };
                    let quality = if point.quality.valid { 1 } else { 0 };
                    points.push(SampledPoint {
                        point_id: pid,
                        value,
                        quality,
                    });
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }
        let snapshot_seq = self.snapshot.write(now_us, &points);
        self.stats.record_sample(now_us, failed as u64);
        SampleReport {
            sampled_count: points.len(),
            failed_count: failed,
            snapshot_seq,
        }
    }

    /// 返回采样周期（微秒）.
    pub fn period_us(&self) -> u64 {
        self.period_us
    }

    /// 返回双缓冲快照引用.
    pub fn snapshot(&self) -> &SharedMemorySnapshot {
        &self.snapshot
    }

    /// 返回采样统计引用.
    pub fn stats(&self) -> &SamplingStats {
        &self.stats
    }
}
