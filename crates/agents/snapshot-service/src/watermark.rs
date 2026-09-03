//! 来源水位（[08] §3.2 / [02] §4.3）：每 source 的最大 event time、
//! ingest time、epoch、sequence；epoch/sequence 回退与「迟到」判定。
//!
//! ## 与 ingestion-core `WatermarkTracker` 的分层关系（63.3 WP-63.3.1 消歧
//! 定名 `SnapshotWatermark`——原名与 ingestion-core 同名对撞，禁再混用）
//!
//! | | `ingestion_core::WatermarkTracker` | 本类型（SnapshotWatermark） |
//! |---|---|---|
//! | 域 | sink 准入守卫（InMemorySink/StoreSink/ObsShadow 钉扎） | 快照构建缺口/迟到判定 |
//! | 维度 | (epoch, sequence) 两维 | (epoch, sequence, event_time, ingest_time) 四维 |
//! | 判据形态 | check（非变异）/ advance（推进）分离 | observe 合体（check+mutate 一步） |
//! | epoch 语义 | 首见钉扎或水位推进 | 递增 = 可信重启（新代基线重置） |
//!
//! 两实现**不合并**：时间维是快照域真实需求，强行并入会污染 sink 准入
//! interface（63.3 裁决，CONTEXT.md 已登记）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// 单来源水位快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceWatermark {
    pub source_id: u32,
    /// 最大 event_time（水位切面基准）。
    pub max_event_time_ns: u64,
    /// 最大 ingest_time。
    pub max_ingest_time_ns: u64,
    /// 当前信任 epoch（事件必须匹配，否则重放/旧代拒绝）。
    pub epoch: u64,
    /// 同 epoch 最大序号。
    pub sequence: u64,
}

/// 快照构建水位（原 `WatermarkTracker`，63.3 WP-63.3.1 消歧改名——与
/// ingestion-core 同名 tracker 分层不同，见模块头对照表）：单调性纪律
/// （回拨/同 epoch 序回退拒绝——与 `TelemetryTrustEvaluator` 同一意图；
/// 本处按 source 逐源跟踪）。
pub struct SnapshotWatermark {
    by_source: BTreeMap<u32, SourceWatermark>,
    /// 每源「首见时刻」（epoch 初始基准；源冷启动用）。
    first_seen: BTreeMap<u32, u64>,
    /// 被拒事件计数（回拨/乱序/旧 epoch——可观测不静默）。
    pub rejected: u64,
}

impl SnapshotWatermark {
    pub fn new(_now_ns: u64) -> Self {
        Self {
            by_source: BTreeMap::new(),
            first_seen: BTreeMap::new(),
            rejected: 0,
        }
    }

    /// 观察事件（epoch/序列单调性纪律；违规 → 计数拒绝，不入水位）。
    ///
    /// 纪律：同源 event_time / ingest_time 不得回退（允许相等 =
    /// 同批次同时刻）；epoch 变化必须递增（重启换 epoch）；同 epoch
    /// sequence 不得回退。
    pub fn observe(
        &mut self,
        source_id: u32,
        event_time_ns: u64,
        ingest_time_ns: u64,
        epoch: u64,
        sequence: u64,
    ) -> bool {
        match self.by_source.get(&source_id) {
            None => {
                self.by_source.insert(
                    source_id,
                    SourceWatermark {
                        source_id,
                        max_event_time_ns: event_time_ns,
                        max_ingest_time_ns: ingest_time_ns,
                        epoch,
                        sequence,
                    },
                );
                self.first_seen.insert(source_id, ingest_time_ns);
                true
            }
            Some(prev) => {
                // 同 epoch 序列必须严格递增（同 (epoch,seq) = 重放拒绝）；
                // epoch 递增 = 可信重启（新序号空间，允许任意序列值）。
                let ok = ingest_time_ns >= prev.max_ingest_time_ns
                    && (epoch > prev.epoch
                        || (epoch == prev.epoch
                            && sequence > prev.sequence
                            && event_time_ns >= prev.max_event_time_ns));
                if !ok {
                    self.rejected += 1;
                    return false;
                }
                let w = self.by_source.get_mut(&source_id).unwrap();
                // 接受路径已保证同代 seq/event_time 单调；epoch 递增 =
                // 新代基线重置到本事件（不携带旧代 max——否则重启源在
                // 新代被旧基线永久拒绝）。
                w.max_event_time_ns = event_time_ns;
                w.sequence = sequence;
                w.max_ingest_time_ns = w.max_ingest_time_ns.max(ingest_time_ns);
                w.epoch = epoch;
                true
            }
        }
    }

    /// 全部水位（排序输出——确定性）。
    pub fn watermarks(&self) -> Vec<SourceWatermark> {
        self.by_source.values().copied().collect()
    }

    /// 某来源当前水位（未注册 → None）。
    pub fn watermark(&self, source_id: u32) -> Option<&SourceWatermark> {
        self.by_source.get(&source_id)
    }

    /// 缺口：`as_of - allowed_lateness` 之前 event 水位未达的来源列表
    /// （空 = 满足）。
    pub fn deficit_before(&self, as_of_ns: u64, allowed_lateness_ns: u64) -> Vec<u32> {
        let threshold = as_of_ns.saturating_sub(allowed_lateness_ns);
        self.by_source
            .iter()
            .filter(|(_, w)| {
                // 已见但水位低于阈值 → 缺口；「已注册未见事件」由
                // 装配方单独声明（来源注册表在这里不知道所需集合）。
                w.max_ingest_time_ns == 0 || w.max_event_time_ns < threshold
            })
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.by_source.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_source.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_watermark_and_replay_rejected() {
        let mut t = SnapshotWatermark::new(0);
        assert!(t.observe(1, 100, 100, 1, 1));
        assert!(t.observe(1, 200, 150, 1, 2));
        // event_time 回拨 → 拒绝。
        assert!(!t.observe(1, 199, 160, 1, 3));
        // ingest_time 回拨 → 拒绝。
        assert!(!t.observe(1, 300, 140, 1, 4));
        // 同 epoch 序列回退 → 拒绝。
        assert!(!t.observe(1, 400, 170, 1, 2));
        // 新 epoch 可以（重启）→ 接受。
        assert!(t.observe(1, 500, 180, 2, 1));
        assert_eq!(t.rejected, 3);
        let ws = t.watermarks();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].epoch, 2);
        assert_eq!(ws[0].max_event_time_ns, 500);
    }

    #[test]
    fn epoch_bump_resets_sequence_and_time_baseline() {
        let mut t = SnapshotWatermark::new(0);
        // epoch 1：序号与事件时间推高。
        for i in 1..=5u64 {
            assert!(t.observe(1, 1000 + i, 2000 + i, 1, i));
        }
        // 可信重启（epoch 2）：序号空间归 1、事件时间回落——新代基线，
        // 不得携带旧代 sequence/max_event_time（否则重启源被永久拒绝）。
        assert!(t.observe(1, 500, 3000, 2, 1));
        // 新 epoch 正常推进：旧实现经 max() 跨代携带会拒绝本事件。
        assert!(t.observe(1, 505, 3010, 2, 2));
        assert_eq!(t.rejected, 0);
        let w = t.watermark(1).unwrap();
        assert_eq!(w.epoch, 2);
        assert_eq!(w.sequence, 2);
        assert_eq!(w.max_event_time_ns, 505);
        // 旧 epoch 事件（重放/回代）仍拒绝。
        assert!(!t.observe(1, 600, 3020, 1, 6));
        assert_eq!(t.rejected, 1);
    }

    #[test]
    fn deficit_uses_lateness_window() {
        let mut t = SnapshotWatermark::new(0);
        t.observe(1, 90, 90, 1, 1);
        t.observe(2, 30, 30, 1, 1);
        // as_of=100，lateness=50：阈值 50——源 2 的 30 < 50 → 缺口。
        let d = t.deficit_before(100, 50);
        assert_eq!(d, alloc::vec![2]);
        let d2 = t.deficit_before(100, 80);
        assert!(d2.is_empty());
    }
}
