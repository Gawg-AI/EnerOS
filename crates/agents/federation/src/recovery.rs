//! v0.101.0 断网处理与孤岛模式：网络恢复增量同步。
//!
//! 蓝图 phase2.md §v0.101.0 §4.5 RecoverySync：分区恢复后，孤岛期间本地
//! 缓存的业务事件按队序增量上传到联邦。上传语义分两类错误：
//! 时间戳冲突（sink 端仲裁）跳过计数继续；链路硬错误立即中止、缓存保留，
//! 待上层重试（蓝图 §8.5 重同步策略）。
//!
//! ## 设计要点
//!
//! - **同步化**（D4）：no_std 禁 async，蓝图 `async fn sync` 落地为同步
//!   `fn sync` + [`SyncSink<T>`] trait seam；生产由 channel/tunnel 适配
//!   注入真实传输实现，测试用 [`MockSyncSink<T>`] 故障注入。
//! - **队序遍历**（蓝图 §4.5）：按 `EventCache.events` 队首→队尾顺序上传，
//!   保持事件因果序（队首最旧）。
//! - **错误分类**（蓝图 §8.5）：[`SyncError::Conflict`] 软错误——跳过并计
//!   `conflicts` 继续后续上传；[`SyncError::UploadFailed`] 硬错误——立即
//!   返回 `Err` 中止本轮同步，已上传部分计入上下文但本轮报告不返回。
//! - **缓存保留**（蓝图 §7.2/§8.5）：本函数**不自动** `clear()` 缓存——
//!   调用方在确认 `Ok` 后自行 `cache.clear()`；若同步中止或 sink 侧需
//!   反查，缓存数据不丢。

use alloc::vec::Vec;

use crate::cache::EventCache;

/// 同步错误（D4 同步化）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncError {
    /// 硬错误（链路不可用/断点），需重试
    UploadFailed,
    /// 时间戳冲突（sink 端仲裁），跳过继续
    Conflict,
}

/// 同步报告
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncReport {
    /// 成功上传事件数
    pub uploaded: u64,
    /// 冲突跳过事件数
    pub conflicts: u64,
}

/// 同步目标 trait seam（D4：生产由 channel/tunnel 适配注入）
pub trait SyncSink<T> {
    /// 上传单条事件：Ok 成功 / Err(Conflict) 冲突跳过 / Err(UploadFailed) 硬错误
    fn upload(&mut self, event: &T) -> Result<(), SyncError>;
}

/// Mock 同步目标（测试用）
#[derive(Debug, Clone)]
pub struct MockSyncSink<T> {
    /// 已成功上传的事件（保序）
    pub uploaded: Vec<T>,
    /// 剩余应注入的 UploadFailed 次数（逐次递减）
    pub fail_times: u32,
    /// 剩余应注入的 Conflict 次数（逐次递减）
    pub conflict_times: u32,
}

impl<T: Clone> SyncSink<T> for MockSyncSink<T> {
    fn upload(&mut self, event: &T) -> Result<(), SyncError> {
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(SyncError::UploadFailed);
        }
        if self.conflict_times > 0 {
            self.conflict_times -= 1;
            return Err(SyncError::Conflict);
        }
        self.uploaded.push(event.clone());
        Ok(())
    }
}

/// 恢复同步器（蓝图 §4.5 RecoverySync）
pub struct RecoverySync;

impl RecoverySync {
    /// 按队序增量上传：Conflict 跳过计数继续，UploadFailed 立即 Err 保留缓存（蓝图 §8.5）
    ///
    /// - 空缓存 → `Ok(SyncReport { uploaded: 0, conflicts: 0 })`
    /// - 成功 → 返回 uploaded/conflicts 计数；缓存由上层显式 `clear()`
    ///   （不在本函数中清空，防 sync 被调用后 sink 侧反查）
    pub fn sync<T, S: SyncSink<T>>(
        cache: &EventCache<T>,
        sink: &mut S,
    ) -> Result<SyncReport, SyncError> {
        let mut uploaded = 0u64;
        let mut conflicts = 0u64;
        for event in cache.events.iter() {
            match sink.upload(event) {
                Ok(()) => uploaded += 1,
                Err(SyncError::Conflict) => conflicts += 1,
                Err(SyncError::UploadFailed) => return Err(SyncError::UploadFailed),
            }
        }
        Ok(SyncReport {
            uploaded,
            conflicts,
        })
    }
}

// ============================================================
// Unit Tests TR28~TR36
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::detector::{PartitionDetector, PartitionState};
    use crate::partition::IslandMode;

    /// 便捷构造：按序压入事件的无溢出缓存（max_size 足够大）
    fn make_cache(items: &[u64]) -> EventCache<u64> {
        let mut c: EventCache<u64> = EventCache::new(items.len().max(1));
        for &e in items {
            c.push(e);
        }
        c
    }

    /// 便捷构造：无故障注入的 Mock sink
    fn ok_sink<T>() -> MockSyncSink<T> {
        MockSyncSink {
            uploaded: Vec::new(),
            fail_times: 0,
            conflict_times: 0,
        }
    }

    // TR28: 空缓存 sync → Ok(SyncReport { uploaded: 0, conflicts: 0 })
    #[test]
    fn tr28_sync_empty_cache() {
        let cache: EventCache<u64> = EventCache::new(4);
        let mut sink = ok_sink::<u64>();
        let report = RecoverySync::sync(&cache, &mut sink);
        assert_eq!(
            report,
            Ok(SyncReport {
                uploaded: 0,
                conflicts: 0
            })
        );
        assert!(sink.uploaded.is_empty());
    }

    // TR29: 全量上传：[1,2,3] 无故障 → uploaded==3 conflicts==0，sink 保序
    #[test]
    fn tr29_sync_full_upload() {
        let cache = make_cache(&[1, 2, 3]);
        let mut sink = ok_sink::<u64>();
        let report = RecoverySync::sync(&cache, &mut sink);
        assert_eq!(
            report,
            Ok(SyncReport {
                uploaded: 3,
                conflicts: 0
            })
        );
        assert_eq!(sink.uploaded, vec![1, 2, 3]);
    }

    // TR30: UploadFailed 立即中止 + 后续重试成功
    #[test]
    fn tr30_upload_failed_aborts() {
        let cache = make_cache(&[1, 2, 3]);
        // 第一轮：fail_times=1 → upload(&1) 即 Err(UploadFailed)，立即中止
        let mut fail_sink = MockSyncSink {
            uploaded: Vec::new(),
            fail_times: 1,
            conflict_times: 0,
        };
        assert_eq!(
            RecoverySync::sync(&cache, &mut fail_sink),
            Err(SyncError::UploadFailed)
        );
        assert!(fail_sink.uploaded.is_empty()); // 无一上传
        assert_eq!(cache.len(), 3); // 缓存保留
                                    // 第二轮：换无故障 sink 重试同一缓存 → 全部上传成功
        let mut ok = ok_sink::<u64>();
        assert_eq!(
            RecoverySync::sync(&cache, &mut ok),
            Ok(SyncReport {
                uploaded: 3,
                conflicts: 0
            })
        );
        assert_eq!(ok.uploaded, vec![1, 2, 3]);
    }

    // TR31: Conflict 跳过计数：conflict_times=1 仅对第一个事件生效
    #[test]
    fn tr31_conflict_skip_count() {
        let cache = make_cache(&[10, 20, 30]);
        let mut sink = MockSyncSink {
            uploaded: Vec::new(),
            fail_times: 0,
            conflict_times: 1,
        };
        let report = RecoverySync::sync(&cache, &mut sink);
        assert_eq!(
            report,
            Ok(SyncReport {
                uploaded: 2,
                conflicts: 1
            })
        );
        // 10 被 Conflict 跳过，20/30 上传
        assert_eq!(sink.uploaded, vec![20, 30]);
    }

    // TR32: 多 Conflict 连续：conflict_times=2 → uploaded==2 conflicts==2
    #[test]
    fn tr32_multiple_conflicts() {
        let cache = make_cache(&[1, 2, 3, 4]);
        let mut sink = MockSyncSink {
            uploaded: Vec::new(),
            fail_times: 0,
            conflict_times: 2,
        };
        let report = RecoverySync::sync(&cache, &mut sink);
        assert_eq!(
            report,
            Ok(SyncReport {
                uploaded: 2,
                conflicts: 2
            })
        );
        assert_eq!(sink.uploaded, vec![3, 4]);
    }

    // TR33: UploadFailed 后缓存不丢（sync 不自动 clear）
    #[test]
    fn tr33_upload_failed_cache_preserved() {
        let cache = make_cache(&[1, 2, 3]);
        let mut sink = MockSyncSink {
            uploaded: Vec::new(),
            fail_times: 2,
            conflict_times: 0,
        };
        assert_eq!(
            RecoverySync::sync(&cache, &mut sink),
            Err(SyncError::UploadFailed)
        );
        assert_eq!(cache.len(), 3); // 未自动 clear
        assert_eq!(
            cache.events.iter().copied().collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    // TR34: 队序保持：上传顺序与缓存队序一致
    #[test]
    fn tr34_order_preserved() {
        let cache = make_cache(&[5, 6, 7]);
        let mut sink = ok_sink::<u64>();
        assert!(RecoverySync::sync(&cache, &mut sink).is_ok());
        assert_eq!(sink.uploaded, vec![5, 6, 7]);
    }

    // TR35: 泛型 struct Mock：自定义事件类型实例化
    #[test]
    fn tr35_generic_struct_mock() {
        #[derive(Debug, Clone, PartialEq)]
        struct TestRecord {
            id: u32,
            seq: u64,
        }
        let r1 = TestRecord { id: 1, seq: 100 };
        let r2 = TestRecord { id: 2, seq: 200 };
        let mut cache: EventCache<TestRecord> = EventCache::new(4);
        cache.push(r1.clone());
        cache.push(r2.clone());
        let mut sink = ok_sink::<TestRecord>();
        assert_eq!(
            RecoverySync::sync(&cache, &mut sink),
            Ok(SyncReport {
                uploaded: 2,
                conflicts: 0
            })
        );
        assert_eq!(sink.uploaded, vec![r1, r2]);
    }

    // TR36: e2e 断网全流程（蓝图 §6.2）：4 节点联邦 n=4 quorum=3，
    //       Connected → Partitioned（孤岛缓存）→ Recovering → sync → Connected
    #[test]
    fn tr36_e2e_partition_to_recovery() {
        // 步骤 1：初始 Connected
        let mut det = PartitionDetector::new(&[1, 2, 3, 4], 1000, 0);
        assert_eq!(det.state, PartitionState::Connected);
        assert!(!det.trading_frozen());

        // 步骤 2：1,2 活跃，3,4 失联 → 直接升级 Partitioned（alive=2 < quorum=3）
        det.on_heartbeat(1, 500);
        det.on_heartbeat(2, 500);
        assert_eq!(det.check(1001), PartitionState::Partitioned);
        assert_eq!(det.partition_count, 1);
        assert!(det.trading_frozen());

        // 步骤 3：进入孤岛，缓存 3 条本地事件
        let mut island: IslandMode<u64> = IslandMode::new(3);
        island.activate(1001);
        assert!(island.active);
        assert!(island.cache_event(101));
        assert!(island.cache_event(102));
        assert!(island.cache_event(103));
        assert_eq!(island.cache.len(), 3);

        // 步骤 4：心跳恢复到 >= quorum → Recovering（仍冻结）
        det.on_heartbeat(3, 1500);
        // 1,2：1500-500=1000<=1000 活跃（边界含等）；3 活跃；4 超时 → alive=3
        assert_eq!(det.check(1500), PartitionState::Recovering);
        assert!(det.trading_frozen());

        // 步骤 5：恢复同步上传孤岛缓存
        let mut sink = ok_sink::<u64>();
        assert_eq!(
            RecoverySync::sync(&island.cache, &mut sink),
            Ok(SyncReport {
                uploaded: 3,
                conflicts: 0
            })
        );
        assert_eq!(sink.uploaded, vec![101, 102, 103]);

        // 步骤 6：显式完成恢复 → Connected 解冻
        assert!(det.complete_recovery(1600));
        assert_eq!(det.state, PartitionState::Connected);
        assert!(!det.trading_frozen());

        // 步骤 7：退出孤岛模式
        island.deactivate();
        assert!(!island.active);

        // 步骤 8：缓存保留（sync 不自动 clear），计数器可观测
        assert_eq!(island.cache.len(), 3);
        assert_eq!(det.partition_count, 1);
        assert_eq!(island.activated_count, 1);
        assert_eq!(island.cache.overflow_count, 0);

        // 步骤 9：上层确认 Ok 后显式 clear
        island.cache.clear();
        assert!(island.cache.is_empty());
    }
}
