//! 最佳主时钟算法（BMCA, IEEE 802.1AS）.
//!
//! - [`AnnounceMessage`] — Announce 报文（8 字段）
//! - [`BmcaResult`] — BMCA 选举结果
//! - [`compare_priority`] — 候选者优先级比较（值小者优）

use core::cmp::Ordering;
use core::fmt;

use crate::clock::{ClockIdentity, MacAddr};

/// Announce 报文（BMCA 候选者描述）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceMessage {
    /// 祖时钟标识.
    pub grandmaster_identity: ClockIdentity,
    /// priority1（全局优先级）.
    pub priority1: u8,
    /// clockClass.
    pub clock_class: u8,
    /// clockAccuracy.
    pub accuracy: u8,
    /// priority2（端口级优先级）.
    pub priority2: u8,
    /// 距祖时钟的跳数.
    pub steps_removed: u16,
    /// 源端口号.
    pub source_port_id: u16,
    /// 源端口 MAC.
    pub source_mac: MacAddr,
}

/// BMCA 选举结果.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BmcaResult {
    /// 本时钟当选为主时钟.
    ElectedAsMaster,
    /// 本时钟跟随指定祖时钟.
    FollowMaster(ClockIdentity),
}

impl fmt::Display for BmcaResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BmcaResult::ElectedAsMaster => write!(f, "ElectedAsMaster"),
            BmcaResult::FollowMaster(id) => write!(f, "FollowMaster({})", id),
        }
    }
}

/// 比较两个 Announce 候选者的优先级（值小者优）.
///
/// BMCA 优先级顺序（升序，返回 `Ordering::Less` 表示 `a` 更优）：
/// 1. `priority1`
/// 2. `clock_class`
/// 3. `accuracy`
/// 4. `priority2`
/// 5. `grandmaster_identity`（`[u8; 8]` 字典序）
pub fn compare_priority(a: &AnnounceMessage, b: &AnnounceMessage) -> Ordering {
    a.priority1
        .cmp(&b.priority1)
        .then_with(|| a.clock_class.cmp(&b.clock_class))
        .then_with(|| a.accuracy.cmp(&b.accuracy))
        .then_with(|| a.priority2.cmp(&b.priority2))
        .then_with(|| a.grandmaster_identity.cmp(&b.grandmaster_identity))
}
