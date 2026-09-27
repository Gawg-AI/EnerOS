//! 状态投影（[08] §3.2）：按全局点键维护最近合格事件与版本。

use alloc::collections::BTreeMap;

use eneros_telemetry_model::event::ObservationEvent;

/// 点键（与目录 [`ObservationKey`] 同构；投影内嵌事件自持身份——无需
/// 外部键表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PointKey {
    pub point_id: u32,
    pub mapping_version: u64,
}

impl From<&ObservationEvent> for PointKey {
    fn from(ev: &ObservationEvent) -> Self {
        Self {
            point_id: ev.point_id,
            mapping_version: ev.mapping_version,
        }
    }
}

/// 投影容量（C1：10k 观测 / 100 源；BTreeMap 有界，超限拒绝新点——
/// 不静默丢）。
#[derive(Debug, Clone, Copy)]
pub struct ProjectorCapacity {
    pub max_points: usize,
    pub max_events_per_point: usize,
}

impl Default for ProjectorCapacity {
    fn default() -> Self {
        Self {
            max_points: 10_000,
            max_events_per_point: 4,
        }
    }
}

/// 状态投影器：每点保留**最近合格事件序列**（有界窗口；建快照时挑
/// `event_time <= cut` 的最新一档）。多源同点 → 冲突留给 resolver——
/// 投影保留多源各自最近事件（主备裁决需要主备两份证据）。
pub struct StateProjector {
    cap: ProjectorCapacity,
    /// point → (source → 最近事件)。
    states: BTreeMap<u32, BTreeMap<u32, ObservationEvent>>,
    /// 蓝图 77 卷一 WP-77.1.4：**在驻事件的日志落点镜像**——
    /// (point_id, source_id) → (partition, offset)。与 [`Self::observe`]
    /// 的 latest-wins 判定同规（被取代的旧落点随之失效）；重放路径
    /// 无日志位置 → 不入表。快照来源区间（provenance 取证/保留界）
    /// 由此派生——事件保留不再冒用只记最大值的消费位点。
    refs: BTreeMap<(u32, u32), (u32, u64)>,
    /// 已见事件总数（观测）。
    pub seen: u64,
    /// 超容量被拒点数（不静默丢）。
    pub overflow_rejected: u64,
}

impl StateProjector {
    pub fn new(cap: ProjectorCapacity) -> Self {
        Self {
            cap,
            states: BTreeMap::new(),
            refs: BTreeMap::new(),
            seen: 0,
            overflow_rejected: 0,
        }
    }

    /// 观察事件（同点同源：更新最近；新源：追加；每点源数满 → 拒绝
    /// 计数——容量界的是「每点源数」，不是同源更新次数）。
    pub fn observe(&mut self, ev: ObservationEvent) {
        self.observe_inner(ev, None);
    }

    /// 带日志落点的观察（蓝图 77 卷一 WP-77.1.4：事件面 OBS 追加 ACK 后
    /// 调用——ACK offset = 该事件的 durable 位置）。
    pub fn observe_tracked(&mut self, ev: ObservationEvent, partition: u32, offset: u64) {
        self.observe_inner(ev, Some((partition, offset)));
    }

    fn observe_inner(&mut self, ev: ObservationEvent, at: Option<(u32, u64)>) {
        self.seen += 1;
        let entry = self.states.entry(ev.point_id).or_default();
        // 同源乱序：只保留最大 event_time（同 event_time 取高序列）。
        match entry.get(&ev.source_id) {
            Some(p)
                if p.event_time_ns > ev.event_time_ns
                    || (p.event_time_ns == ev.event_time_ns && p.sequence >= ev.sequence) =>
            {
                return;
            }
            _ => {}
        }
        // 新源追加受每点源数上界约束（同源更新在槽位内原位替换）。
        if !entry.contains_key(&ev.source_id) && entry.len() >= self.cap.max_events_per_point {
            self.overflow_rejected += 1;
            return;
        }
        entry.insert(ev.source_id, ev);
        if let Some((partition, offset)) = at {
            self.refs
                .insert((ev.point_id, ev.source_id), (partition, offset));
        }
        if self.states.len() > self.cap.max_points {
            // 超点容量：淘汰最近更新的点（最不活跃者）——FIFO 式容量
            // 守恒，淘汰计数可见。
            let oldest_point = self
                .states
                .iter()
                .min_by_key(|(_, v)| v.values().map(|e| e.ingest_time_ns).max())
                .map(|(k, _)| *k);
            if let Some(k) = oldest_point {
                self.states.remove(&k);
                let victims: alloc::vec::Vec<(u32, u32)> = self
                    .refs
                    .keys()
                    .copied()
                    .filter(|(point, _)| *point == k)
                    .collect();
                for key in victims {
                    self.refs.remove(&key);
                }
                self.overflow_rejected += 1;
            }
        }
    }

    /// 每分区**最早仍被在驻事件引用**的日志 offset（蓝图 77 卷一
    /// WP-77.1.4：快照来源区间下界的单源——"每分区最早仍需事件
    /// offset"出自事件落点本身，不再冒用消费位点）。
    pub fn provenance_floors(&self) -> BTreeMap<u32, u64> {
        let mut floors: BTreeMap<u32, u64> = BTreeMap::new();
        for &(partition, offset) in self.refs.values() {
            let slot = floors.entry(partition).or_insert(u64::MAX);
            if offset < *slot {
                *slot = offset;
            }
        }
        floors
    }

    /// 每点每源最近事件（建快照输入）。
    pub fn latest_by_source(&self, point_id: u32) -> impl Iterator<Item = &ObservationEvent> {
        self.states
            .get(&point_id)
            .map(|m| m.values())
            .into_iter()
            .flatten()
    }

    /// 所有点键（枚举）。
    pub fn point_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.states.keys().copied()
    }

    /// 当前投影点数（观测）。
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;
    use crate::tests::obs7;

    fn event(point: u32, source: u32, time_ns: u64, seq: u64) -> ObservationEvent {
        let mut evs = obs7(
            1_000_000_000,
            0,
            source,
            &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0],
        );
        let mut ev = evs.remove(((point - 1) % 7) as usize);
        ev.point_id = point;
        ev.source_id = source;
        ev.event_time_ns = time_ns;
        ev.sequence = seq;
        ev
    }

    #[test]
    fn latest_event_kept_per_source() {
        let mut p = StateProjector::new(ProjectorCapacity::default());
        p.observe(event(1, 2, 100, 1));
        p.observe(event(1, 2, 200, 2));
        // 乱序旧事件不得回退最新。
        p.observe(event(1, 2, 150, 3));
        let latest: Vec<_> = p.latest_by_source(1).collect();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].event_time_ns, 200);
        // 第二源介入（冲突素材）。
        p.observe(event(1, 3, 180, 1));
        let latest: Vec<_> = p.latest_by_source(1).collect();
        assert_eq!(latest.len(), 2);
    }

    #[test]
    fn same_source_update_not_blocked_by_capacity() {
        let mut p = StateProjector::new(ProjectorCapacity {
            max_points: 10,
            max_events_per_point: 2,
        });
        p.observe(event(1, 2, 100, 1));
        p.observe(event(1, 3, 200, 1)); // 两源已满
                                        // 同源更新（source 2 更新事件）——容量界的是
                                        // 「每点源数」，不是同源更新次数。
        p.observe(event(1, 2, 300, 2));
        let latest: Vec<_> = p.latest_by_source(1).collect();
        assert_eq!(latest.len(), 2);
        let s2 = latest.iter().find(|e| e.source_id == 2).unwrap();
        assert_eq!(s2.event_time_ns, 300);
        assert_eq!(p.overflow_rejected, 0);
        // 第三源追加 → 容量满拒绝计数。
        p.observe(event(1, 4, 400, 1));
        assert_eq!(p.overflow_rejected, 1);
        let latest: Vec<_> = p.latest_by_source(1).collect();
        assert_eq!(latest.len(), 2);
    }

    #[test]
    fn capacity_bounded_not_silent() {
        let mut p = StateProjector::new(ProjectorCapacity {
            max_points: 10,
            max_events_per_point: 2,
        });
        for point in 1..=20u32 {
            p.observe(event(point, 2, 100, 1));
        }
        assert!(p.len() <= 10);
        assert!(p.overflow_rejected > 0);
        // 不静默：拒绝被计数且仍可查询存量。
        assert!(p.seen >= 20);
    }
}
