//! EnerOS RTOS 高频采样服务 — Phase 1 P1-H (v0.55.0).
//!
//! 本 crate 实现 RTOS 控制大区的高频采样服务，包括：
//! - [`snapshot::SampledPoint`] / [`snapshot::StateSnapshot`] — 采样点与状态快照（`#[repr(C)]`）
//! - [`shared_memory::SharedMemorySnapshot`] — 乒乓双缓冲无锁快照（原子切换 + 序列号校验）
//! - [`service::SamplingService`] — 周期采样服务（泛型 `<P: PointAccess>`，单步 `sample(now_us)`）
//! - [`stats::SamplingStats`] — 采样统计（次数/失败/时间戳）
//!
//! # 偏差声明（D1~D10）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 时间戳用 `u64` 微秒参数注入（蓝图 `MonotonicTime::now()` 在 no_std 不存在） |
//! | **D2** | crate 放入 `crates/kernel/rtos-sampling/`（P1-H RTOS 组件第二层） |
//! | **D3** | 不直接依赖 seL4 SharedMemory（用 in-memory 双缓冲，Phase 3 替换） |
//! | **D4** | `read()` 增加重试上限 `MAX_READ_RETRIES=10`（蓝图风险 8.3） |
//! | **D5** | 不实现阻塞式 `run()` 循环（`sample(now_us)` 单步接口） |
//! | **D6** | 不使用 `Box<dyn PointAccess>`（改为泛型 `<P: PointAccess>`） |
//! | **D7** | `SamplingStats` 不使用 `AtomicU64`（no_std 单线程） |
//! | **D8** | `SharedMemorySnapshot` 使用 `core::sync::atomic::{AtomicU8, AtomicU64}` |
//! | **D9** | `StateSnapshot.points` 用固定数组 `[SampledPoint; MAX_POINTS]`（`#[repr(C)]`） |
//! | **D10** | `PointQuality.valid` 映射为 `SampledPoint.quality` 的 `u8` |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，不 `use std::*`，不 `panic!`/`todo!`/`unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod error;
pub mod service;
pub mod shared_memory;
pub mod snapshot;
pub mod stats;

#[cfg(test)]
pub mod mock;

#[cfg(test)]
mod tests {
    use alloc::vec;

    use eneros_upa_model::PointValue;

    use crate::mock::MockPointAccess;
    use crate::service::SamplingService;
    use crate::shared_memory::SharedMemorySnapshot;
    use crate::snapshot::{SampledPoint, StateSnapshot, MAX_POINTS};
    use crate::stats::SamplingStats;

    /// 浮点近似比较（f64 不能直接 assert_eq）.
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // ===== T1：SampledPoint 构造与 Copy =====
    #[test]
    fn test_t1_sampled_point_construct_and_copy() {
        let p = SampledPoint {
            point_id: 1,
            value: 42.5,
            quality: 1,
        };
        let p2 = p; // Copy 语义
        assert_eq!(p.point_id, 1);
        assert_eq!(p.value, 42.5);
        assert_eq!(p.quality, 1);
        assert_eq!(p2.point_id, 1);
        assert_eq!(p2.value, 42.5);
        assert_eq!(p2.quality, 1);
    }

    // ===== T2：StateSnapshot 默认值与 get_points 切片访问 =====
    #[test]
    fn test_t2_state_snapshot_default_and_get_points() {
        let s = StateSnapshot::default();
        assert_eq!(s.timestamp, 0);
        assert_eq!(s.seq, 0);
        assert_eq!(s.point_count, 0);
        assert!(s.get_points().is_empty());
        assert_eq!(s.points.len(), MAX_POINTS);
    }

    // ===== T3：SharedMemorySnapshot 单次写入读取（seq=1）=====
    #[test]
    fn test_t3_shared_memory_single_write_read() {
        let sm = SharedMemorySnapshot::new();
        let pts = [SampledPoint {
            point_id: 1,
            value: 1.0,
            quality: 1,
        }];
        let seq = sm.write(1000, &pts);
        assert_eq!(seq, 1);
        let snap = sm.read().expect("read ok");
        assert_eq!(snap.seq, 1);
        assert_eq!(snap.timestamp, 1000);
        assert_eq!(snap.point_count, 1);
    }

    // ===== T4：SharedMemorySnapshot 序列号递增（3 次 write → seq=3）=====
    #[test]
    fn test_t4_shared_memory_seq_increment() {
        let sm = SharedMemorySnapshot::new();
        let pts: [SampledPoint; 0] = [];
        sm.write(1000, &pts);
        sm.write(2000, &pts);
        sm.write(3000, &pts);
        let snap = sm.read().expect("read ok");
        assert_eq!(snap.seq, 3);
    }

    // ===== T5：SharedMemorySnapshot 多次写入读取数据一致性 =====
    #[test]
    fn test_t5_shared_memory_data_consistency() {
        let sm = SharedMemorySnapshot::new();
        let pts = [
            SampledPoint {
                point_id: 10,
                value: 12.5,
                quality: 1,
            },
            SampledPoint {
                point_id: 20,
                value: 7.3,
                quality: 0,
            },
        ];
        sm.write(5000, &pts);
        let snap = sm.read().expect("read ok");
        assert_eq!(snap.point_count, 2);
        let read_pts = snap.get_points();
        assert_eq!(read_pts.len(), 2);
        assert_eq!(read_pts[0].point_id, 10);
        assert!(approx_eq(read_pts[0].value, 12.5));
        assert_eq!(read_pts[0].quality, 1);
        assert_eq!(read_pts[1].point_id, 20);
        assert!(approx_eq(read_pts[1].value, 7.3));
        assert_eq!(read_pts[1].quality, 0);
    }

    // ===== T6：SharedMemorySnapshot 无写入时 read 返回 Some（seq=0）=====
    #[test]
    fn test_t6_shared_memory_read_without_write() {
        let sm = SharedMemorySnapshot::new();
        let snap = sm.read().expect("read ok");
        assert_eq!(snap.seq, 0);
        assert_eq!(snap.point_count, 0);
    }

    // ===== T7：SamplingStats 更新 =====
    #[test]
    fn test_t7_sampling_stats_update() {
        let mut stats = SamplingStats::new();
        assert_eq!(stats.sample_count, 0);
        stats.record_sample(1000, 2);
        assert_eq!(stats.sample_count, 1);
        assert_eq!(stats.read_failures, 2);
        assert_eq!(stats.last_sample_time_us, 1000);
        stats.record_sample(2000, 0);
        assert_eq!(stats.sample_count, 2);
        assert_eq!(stats.read_failures, 2);
        assert_eq!(stats.last_sample_time_us, 2000);
    }

    // ===== T8：SamplingService 正常采样（3 点全部成功）=====
    #[test]
    fn test_t8_service_normal_sampling() {
        let mut protocol = MockPointAccess::new();
        protocol.set_point(1, 10.0, true);
        protocol.set_point(2, 20.0, true);
        protocol.set_point(3, 30.0, true);
        let mut svc = SamplingService::new(vec![1, 2, 3], 1000, protocol);
        let report = svc.sample(1000);
        assert_eq!(report.sampled_count, 3);
        assert_eq!(report.failed_count, 0);
        assert_eq!(report.snapshot_seq, 1);
        // 验证快照内容与品质映射（D10，valid=true → quality=1）
        let snap = svc.snapshot().read().expect("read ok");
        let pts = snap.get_points();
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[0].point_id, 1);
        assert!(approx_eq(pts[0].value, 10.0));
        assert_eq!(pts[0].quality, 1);
        assert_eq!(pts[2].point_id, 3);
        assert!(approx_eq(pts[2].value, 30.0));
        // 验证统计
        assert_eq!(svc.stats().sample_count, 1);
        assert_eq!(svc.stats().read_failures, 0);
    }

    // ===== T9：SamplingService 部分点读取失败（1 点 fail_on_read）=====
    #[test]
    fn test_t9_service_partial_failure() {
        let mut protocol = MockPointAccess::new();
        protocol.set_point(1, 10.0, true);
        protocol.set_point(2, 20.0, true);
        protocol.fail_on_read(3);
        let mut svc = SamplingService::new(vec![1, 2, 3], 1000, protocol);
        let report = svc.sample(1000);
        assert_eq!(report.sampled_count, 2);
        assert_eq!(report.failed_count, 1);
        assert_eq!(svc.stats().read_failures, 1);
    }

    // ===== T10：SamplingService 空采样点列表 =====
    #[test]
    fn test_t10_service_empty_point_list() {
        let protocol = MockPointAccess::new();
        let mut svc = SamplingService::new(vec![], 1000, protocol);
        let report = svc.sample(1000);
        assert_eq!(report.sampled_count, 0);
        assert_eq!(report.failed_count, 0);
        assert_eq!(report.snapshot_seq, 1);
    }

    // ===== T11：SamplingService PointValue 类型转换（Float/Int/Bool/Null）=====
    #[test]
    fn test_t11_service_point_value_conversion() {
        let mut protocol = MockPointAccess::new();
        protocol.set_point_value(1, PointValue::Float(1.5), true);
        protocol.set_point_value(2, PointValue::Int(42), true);
        protocol.set_point_value(3, PointValue::Bool(true), true);
        protocol.set_point_value(4, PointValue::Bool(false), true);
        protocol.set_point_value(5, PointValue::Null, true);
        let mut svc = SamplingService::new(vec![1, 2, 3, 4, 5], 1000, protocol);
        let report = svc.sample(1000);
        // Float/Int/Bool(true)/Bool(false) 成功；Null 计为失败
        assert_eq!(report.sampled_count, 4);
        assert_eq!(report.failed_count, 1);
        // 验证转换结果
        let snap = svc.snapshot().read().expect("read ok");
        let pts = snap.get_points();
        assert_eq!(pts.len(), 4);
        assert!(approx_eq(pts[0].value, 1.5)); // Float
        assert!(approx_eq(pts[1].value, 42.0)); // Int
        assert!(approx_eq(pts[2].value, 1.0)); // Bool(true)
        assert!(approx_eq(pts[3].value, 0.0)); // Bool(false)
    }

    // ===== T12：SamplingService 多次采样后 snapshot.seq 递增（3 次 → seq=3）=====
    #[test]
    fn test_t12_service_multi_sample_seq_increment() {
        let mut protocol = MockPointAccess::new();
        protocol.set_point(1, 10.0, true);
        let mut svc = SamplingService::new(vec![1], 1000, protocol);
        svc.sample(1000);
        svc.sample(2000);
        svc.sample(3000);
        let snap = svc.snapshot().read().expect("read ok");
        assert_eq!(snap.seq, 3);
        assert_eq!(svc.stats().sample_count, 3);
    }
}
