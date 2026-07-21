//! v0.101.0 断网处理与孤岛模式：联邦网络分区检测。
//!
//! 提供 `PartitionState`（四态）与 `PartitionDetector`（心跳表 + quorum 判据
//! 状态机 + 交易冻结查询）。与 v0.84.0 grid_agent `IslandDetector`（电网物理
//! 并离网）**不同层**——本模块是 PBFT quorum 层面的联邦分区语义。
//!
//! ## 设计要点
//!
//! - **注入时钟**（D6）：`now_ms` 全部由参数注入，禁 `std::time::Duration` /
//!   全局 `now_ms()`，确定性可复现。
//! - **quorum 判据**（D8）：分区确认/恢复阈值复用 `pbft::quorum(total_nodes)`，
//!   与共识法定人数语义闭环（n=4 → 3，n=7 → 5，n=1 → 1）。
//! - **全失联直接升级**（C36）：Connected 态下 `alive < quorum` 直接落
//!   Partitioned，不经 Suspected 中间态；仅 `quorum <= alive < total` 才落
//!   Suspected 抖动容忍。
//! - **边界含等**（C31）：`now - last_contact <= timeout` 判活跃，
//!   `saturating_sub` 防时钟回拨下溢（v0.97.0 D9/D10 惯例）。
//! - **可观测**（D7）：`partition_count` 记录进入分区次数（含 Recovering
//!   再失联回退）。
//! - **冻结语义**（D9）：Partitioned 与 Recovering 均冻结交易，Recovering
//!   须经 `complete_recovery` 显式确认同步完成后方解冻回 Connected。

use alloc::collections::BTreeMap;

use crate::consensus::NodeId;
use crate::pbft::quorum;

/// 分区状态（蓝图 §4.1 PartitionState）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionState {
    /// 全网连通（全部节点活跃）
    Connected,
    /// 疑似分区（部分失联但活跃数 >= quorum，抖动容忍，不冻结交易）
    Suspected,
    /// 确认分区（活跃数 < quorum，冻结交易进入孤岛）
    Partitioned,
    /// 恢复中（活跃数回升 >= quorum，同步完成前保持冻结）
    Recovering,
}

/// 分区检测器（蓝图 §4.1 PartitionDetector，D6 注入时钟）
#[derive(Debug, Clone)]
pub struct PartitionDetector {
    /// 心跳超时（ms）
    pub heartbeat_timeout_ms: u64,
    /// 各节点最后联系时刻（BTreeMap，D6 替代 HashMap）
    pub last_contact: BTreeMap<NodeId, u64>,
    /// 当前状态
    pub state: PartitionState,
    /// 节点总数
    pub total_nodes: usize,
    /// 进入分区次数（D7 可观测）
    pub partition_count: u64,
}

impl PartitionDetector {
    /// 创建：全部节点 last_contact=now_ms，state=Connected
    pub fn new(nodes: &[NodeId], heartbeat_timeout_ms: u64, now_ms: u64) -> Self {
        let mut last_contact = BTreeMap::new();
        for &id in nodes {
            last_contact.insert(id, now_ms);
        }
        Self {
            heartbeat_timeout_ms,
            last_contact,
            state: PartitionState::Connected,
            total_nodes: nodes.len(),
            partition_count: 0,
        }
    }

    /// 心跳到达：已知节点更新 last_contact；未知节点忽略（C30）
    pub fn on_heartbeat(&mut self, from: NodeId, now_ms: u64) {
        if let Some(last) = self.last_contact.get_mut(&from) {
            *last = now_ms;
        }
    }

    /// 活跃节点数：now - last_contact <= timeout（边界含等，C31）
    pub fn alive_count(&self, now_ms: u64) -> usize {
        self.last_contact
            .values()
            .filter(|&&last| now_ms.saturating_sub(last) <= self.heartbeat_timeout_ms)
            .count()
    }

    /// 状态机检查（D8 quorum 判据），返回迁移后状态：
    ///
    /// - Connected：alive == total → 保持 Connected；
    ///   alive < quorum → **直接升级** Partitioned + partition_count+=1（C36）；
    ///   否则（quorum <= alive < total）→ Suspected
    /// - Suspected：alive == total → Connected；
    ///   alive < quorum → Partitioned + partition_count+=1；否则保持 Suspected
    /// - Partitioned：alive >= quorum → Recovering
    /// - Recovering：alive < quorum → Partitioned + partition_count+=1（再失联回退）
    pub fn check(&mut self, now_ms: u64) -> PartitionState {
        let alive = self.alive_count(now_ms);
        match self.state {
            PartitionState::Connected => {
                if alive == self.total_nodes {
                    // 全网连通，保持 Connected
                } else if alive < quorum(self.total_nodes) {
                    // 全失联直接升级（C36，不经 Suspected）
                    self.state = PartitionState::Partitioned;
                    self.partition_count += 1;
                } else {
                    self.state = PartitionState::Suspected;
                }
            }
            PartitionState::Suspected => {
                if alive == self.total_nodes {
                    self.state = PartitionState::Connected;
                } else if alive < quorum(self.total_nodes) {
                    self.state = PartitionState::Partitioned;
                    self.partition_count += 1;
                }
                // 否则 quorum <= alive < total，保持 Suspected 抖动容忍
            }
            PartitionState::Partitioned => {
                if alive >= quorum(self.total_nodes) {
                    self.state = PartitionState::Recovering;
                }
            }
            PartitionState::Recovering => {
                if alive < quorum(self.total_nodes) {
                    self.state = PartitionState::Partitioned;
                    self.partition_count += 1;
                }
            }
        }
        self.state
    }

    /// 交易冻结查询（D9）：Partitioned | Recovering → true
    pub fn trading_frozen(&self) -> bool {
        matches!(
            self.state,
            PartitionState::Partitioned | PartitionState::Recovering
        )
    }

    /// 显式完成恢复（D9）：仅 Recovering → Connected 返回 true；
    /// 其余状态返回 false（幂等保护）。
    ///
    /// `_now_ms` 为保留参数（语义一致性/未来扩展），当前实现不使用。
    pub fn complete_recovery(&mut self, _now_ms: u64) -> bool {
        if self.state == PartitionState::Recovering {
            self.state = PartitionState::Connected;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 常用 4 节点（n=4 → quorum=3）
    const N: [u64; 4] = [1, 2, 3, 4];

    fn det(nodes: &[u64], timeout: u64, now: u64) -> PartitionDetector {
        PartitionDetector::new(nodes, timeout, now)
    }

    /// TD8: new 初始态
    #[test]
    fn td8_new_initial() {
        let d = det(&N, 1000, 100);
        assert_eq!(d.state, PartitionState::Connected);
        assert_eq!(d.total_nodes, 4);
        assert_eq!(d.last_contact.len(), 4);
        for &id in N.iter() {
            assert_eq!(d.last_contact.get(&id), Some(&100));
        }
        assert_eq!(d.partition_count, 0);
    }

    /// TD9: on_heartbeat 已知节点更新 last_contact
    #[test]
    fn td9_heartbeat_updates_known() {
        let mut d = det(&N, 1000, 100);
        d.on_heartbeat(2, 500);
        assert_eq!(d.last_contact.get(&2), Some(&500));
        assert_eq!(d.last_contact.get(&1), Some(&100));
        assert_eq!(d.last_contact.get(&3), Some(&100));
        assert_eq!(d.last_contact.get(&4), Some(&100));
    }

    /// TD10: on_heartbeat 未知节点忽略（C30）
    #[test]
    fn td10_heartbeat_unknown_ignored() {
        let mut d = det(&N, 1000, 100);
        d.on_heartbeat(99, 500);
        assert_eq!(d.last_contact.len(), 4);
        assert!(!d.last_contact.contains_key(&99));
    }

    /// TD11: alive_count 边界含等（C31）：now - last == timeout 仍活跃
    #[test]
    fn td11_alive_count_boundary_inclusive() {
        let d = det(&N, 1000, 100);
        // 1100-100=1000 <= 1000 活跃
        assert_eq!(d.alive_count(1100), 4);
        // 1101-100=1001 > 1000 不活跃
        assert_eq!(d.alive_count(1101), 0);
    }

    /// TD12: Connected→Suspected（quorum <= alive < total）
    #[test]
    fn td12_connected_to_suspected() {
        let mut d = det(&N, 1000, 0);
        // 节点 1,2,3 心跳到 900，节点 4 停在 0
        d.on_heartbeat(1, 900);
        d.on_heartbeat(2, 900);
        d.on_heartbeat(3, 900);
        // 1,2,3：1001-900=101<=1000 活跃；4：1001-0=1001>1000 超时
        // alive=3，quorum(4)=3 <= 3 < 4 → Suspected
        assert_eq!(d.check(1001), PartitionState::Suspected);
        assert_eq!(d.partition_count, 0);
    }

    /// TD13: Suspected→Connected 回退（全部恢复活跃）
    #[test]
    fn td13_suspected_back_to_connected() {
        let mut d = det(&N, 1000, 0);
        d.on_heartbeat(1, 900);
        d.on_heartbeat(2, 900);
        d.on_heartbeat(3, 900);
        assert_eq!(d.check(1001), PartitionState::Suspected);
        // 节点 4 心跳恢复
        d.on_heartbeat(4, 1500);
        // 1,2,3：1500-900=600<=1000；4：1500-1500=0 → alive=4 == total
        assert_eq!(d.check(1500), PartitionState::Connected);
        assert_eq!(d.partition_count, 0);
    }

    /// TD14: Suspected 保持（>= quorum 未全恢复，不降级）
    #[test]
    fn td14_suspected_holds_above_quorum() {
        let mut d = det(&N, 1000, 0);
        d.on_heartbeat(1, 900);
        d.on_heartbeat(2, 900);
        d.on_heartbeat(3, 900);
        assert_eq!(d.check(1001), PartitionState::Suspected);
        // 无新心跳：1600-900=700<=1000 仍活跃，1600-0=1600>1000 节点 4 超时
        // alive=3，quorum <= 3 < 4 → 保持 Suspected
        assert_eq!(d.check(1600), PartitionState::Suspected);
        assert_eq!(d.partition_count, 0);
    }

    /// TD15: Suspected→Partitioned（失联扩大至 alive < quorum）
    #[test]
    fn td15_suspected_to_partitioned() {
        let mut d = det(&N, 1000, 0);
        d.on_heartbeat(1, 900);
        d.on_heartbeat(2, 900);
        d.on_heartbeat(3, 900);
        assert_eq!(d.check(1001), PartitionState::Suspected);
        // 节点 3 也失联：仅节点 1,2 心跳到 1500
        d.on_heartbeat(1, 1500);
        d.on_heartbeat(2, 1500);
        // 1,2：2500-1500=1000<=1000 活跃（边界含等）；3：2500-900=1600>1000 超时；4 早已超时
        // alive=2 < quorum(4)=3 → Partitioned
        assert_eq!(d.check(2500), PartitionState::Partitioned);
        assert_eq!(d.partition_count, 1);
    }

    /// TD16: Connected→Partitioned 直接升级（C36，不经 Suspected）
    #[test]
    fn td16_connected_to_partitioned_direct() {
        let mut d = det(&N, 1000, 0);
        // 仅节点 1 心跳到 500，2,3,4 停在 0
        d.on_heartbeat(1, 500);
        // 1：1001-500=501<=1000 活跃；2,3,4：1001-0=1001>1000 全超时
        // alive=1 < quorum(4)=3 → Connected 直接升级 Partitioned
        assert_eq!(d.check(1001), PartitionState::Partitioned);
        assert_eq!(d.partition_count, 1);

        // C45: n=1, quorum=1，唯一节点失联 → Partitioned
        let mut d1 = det(&[1u64], 1000, 0);
        d1.on_heartbeat(1, 500);
        // 1501-500=1001>1000，唯一节点超时 → alive=0 < quorum(1)=1 → Partitioned
        assert_eq!(d1.check(1501), PartitionState::Partitioned);
        assert_eq!(d1.partition_count, 1);
    }

    /// TD17: trading_frozen 四态真值表（D9）
    #[test]
    fn td17_trading_frozen_truth_table() {
        // Connected → 不冻结
        let d = det(&N, 1000, 0);
        assert!(!d.trading_frozen());

        // Suspected → 不冻结
        let mut d = det(&N, 1000, 0);
        d.on_heartbeat(1, 900);
        d.on_heartbeat(2, 900);
        d.on_heartbeat(3, 900);
        assert_eq!(d.check(1001), PartitionState::Suspected);
        assert!(!d.trading_frozen());

        // Partitioned → 冻结
        let mut d = det(&N, 1000, 0);
        d.on_heartbeat(1, 500);
        assert_eq!(d.check(1001), PartitionState::Partitioned);
        assert!(d.trading_frozen());

        // Recovering → 冻结（同步完成前不解冻）
        d.on_heartbeat(2, 1500);
        d.on_heartbeat(3, 1500);
        // 1：1500-500=1000<=1000 活跃；2,3 活跃；4：1500-0=1500>1000 超时
        // alive=3 >= quorum(4)=3 → Recovering
        assert_eq!(d.check(1500), PartitionState::Recovering);
        assert!(d.trading_frozen());
    }

    /// TD18: Partitioned→Recovering + complete_recovery 解冻
    #[test]
    fn td18_partitioned_recovering_complete_recovery() {
        let mut d = det(&N, 1000, 0);
        d.on_heartbeat(1, 500);
        assert_eq!(d.check(1001), PartitionState::Partitioned);
        // 心跳恢复到 >= quorum → Recovering（仍冻结）
        d.on_heartbeat(2, 1500);
        d.on_heartbeat(3, 1500);
        assert_eq!(d.check(1500), PartitionState::Recovering);
        assert!(d.trading_frozen());
        // 显式完成恢复 → Connected 解冻
        assert!(d.complete_recovery(1600));
        assert_eq!(d.state, PartitionState::Connected);
        assert!(!d.trading_frozen());
        // 幂等保护：非 Recovering 态再调用返回 false
        assert!(!d.complete_recovery(1700));
        assert_eq!(d.state, PartitionState::Connected);

        // C46: n=7, quorum=5，4 活跃 → Partitioned；5 活跃 → Recovering
        let n7 = [1u64, 2, 3, 4, 5, 6, 7];
        let mut d7 = det(&n7, 1000, 0);
        d7.on_heartbeat(1, 500);
        d7.on_heartbeat(2, 500);
        d7.on_heartbeat(3, 500);
        d7.on_heartbeat(4, 500);
        // 1500-500=1000<=1000（边界含等）→ 4 活跃 < quorum(7)=5 → Partitioned
        assert_eq!(d7.check(1500), PartitionState::Partitioned);
        assert_eq!(d7.partition_count, 1);
        // 恢复 1 个节点到 5 活跃
        d7.on_heartbeat(5, 1500);
        // 1~5 活跃（5：1500-1500=0），6,7 超时 → alive=5 >= quorum(7)=5 → Recovering
        assert_eq!(d7.check(1500), PartitionState::Recovering);
        // 显式恢复
        assert!(d7.complete_recovery(1600));
        assert_eq!(d7.state, PartitionState::Connected);
    }

    /// TD19: Recovering 再失联回退 Partitioned（partition_count 累加）
    #[test]
    fn td19_recovering_relapse_to_partitioned() {
        let mut d = det(&N, 1000, 0);
        d.on_heartbeat(1, 500);
        assert_eq!(d.check(1001), PartitionState::Partitioned);
        d.on_heartbeat(2, 1500);
        d.on_heartbeat(3, 1500);
        assert_eq!(d.check(1500), PartitionState::Recovering);
        // 心跳停在 1500/500：1500+1000=2500 含等仍活跃，2600 全部超时
        // alive=0 < quorum(4)=3 → 回退 Partitioned
        assert_eq!(d.check(2600), PartitionState::Partitioned);
        assert_eq!(d.partition_count, 2);
    }
}
