//! gPTP 时钟、Sync/FollowUp 报文与 BMCA 集成（IEEE 802.1AS）.
//!
//! - [`SyncMessage`] / [`FollowUpMessage`] — 时间同步报文
//! - [`GptpConfig`] — 时钟配置
//! - [`GptpClock`] — gPTP 时钟状态机（BMCA + 偏移滤波 + 时钟修正）

use core::cmp::Ordering;
use core::time::Duration;

use crate::bmca::{compare_priority, AnnounceMessage, BmcaResult};
use crate::clock::{ClockIdentity, MacAddr, PtpTime};
use crate::port::{Port, PortRole};

/// Sync 报文（携带粗略原时间戳）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncMessage {
    /// 报文发送时刻的粗略时间戳.
    pub origin_timestamp: PtpTime,
    /// 序列号（与 FollowUp 配对）.
    pub sequence_id: u16,
    /// 距祖时钟的跳数.
    pub steps_removed: u16,
}

/// FollowUp 报文（携带精确原时间戳，与 Sync 序列号配对）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowUpMessage {
    /// 与对应 Sync 报文一致的序列号.
    pub sequence_id: u16,
    /// 精确原时间戳.
    pub precise_origin_timestamp: PtpTime,
}

/// gPTP 时钟配置.
#[derive(Debug, Clone)]
pub struct GptpConfig {
    /// priority1.
    pub priority1: u8,
    /// priority2.
    pub priority2: u8,
    /// 端口列表.
    pub ports: alloc::vec::Vec<Port>,
}

impl Default for GptpConfig {
    fn default() -> Self {
        Self {
            priority1: 128,
            priority2: 0,
            ports: alloc::vec::Vec::new(),
        }
    }
}

/// gPTP 时钟状态机.
#[derive(Debug, Clone)]
pub struct GptpClock {
    /// 本时钟标识.
    pub identity: ClockIdentity,
    /// priority1.
    pub priority1: u8,
    /// clockClass（默认 248，从时钟）.
    pub clock_class: u8,
    /// clockAccuracy（默认 0xFE）.
    pub accuracy: u8,
    /// priority2.
    pub priority2: u8,
    /// 距祖时钟的跳数.
    pub steps_removed: u16,
    /// 当前祖时钟标识.
    pub grandmaster_identity: ClockIdentity,
    /// 本地自由运行时间（D7：由 `new` 注入，无 `PtpTime::now()`）.
    pub current_time: PtpTime,
    /// 估算的时间偏移（ns）.
    pub offset: i64,
    /// 端口列表.
    pub ports: alloc::vec::Vec<Port>,
    /// Sync 报文发送间隔.
    pub sync_interval: core::time::Duration,
    /// 频率偏移（小偏移修正用）.
    pub frequency_offset: i64,
    /// 最近一次大跳变（ns）；无 `warn!` 宏，用此字段记录（D6）.
    pub last_jump_ns: core::option::Option<i64>,
    /// 最近 Sync 序列号（FollowUp 配对用）.
    pub last_sync_seq_id: core::option::Option<u16>,
    /// 最近 Sync 接收时间戳（FollowUp 重算偏移用）.
    pub last_sync_rx_ts: core::option::Option<PtpTime>,
    /// 最近 Sync 的 `delay_ns`（FollowUp 重算偏移用，D9）.
    pub last_sync_delay_ns: i64,
    /// 最近 Sync 的原时间戳（FollowUp 配对校验用）.
    pub last_sync_origin_ts: core::option::Option<PtpTime>,
}

impl GptpClock {
    /// 构造 gPTP 时钟（D7：`initial_time` 显式注入）.
    pub fn new(identity: ClockIdentity, initial_time: PtpTime, config: &GptpConfig) -> Self {
        Self {
            identity,
            priority1: config.priority1,
            clock_class: 248,
            accuracy: 0xFE,
            priority2: config.priority2,
            steps_removed: 0,
            grandmaster_identity: identity,
            current_time: initial_time,
            offset: 0,
            ports: config.ports.clone(),
            sync_interval: Duration::from_millis(125),
            frequency_offset: 0,
            last_jump_ns: None,
            last_sync_seq_id: None,
            last_sync_rx_ts: None,
            last_sync_delay_ns: 0,
            last_sync_origin_ts: None,
        }
    }

    /// 运行 BMCA：本时钟 Announce + 远端 Announce 中选最优.
    pub fn run_bmca(&mut self, announces: &[AnnounceMessage]) -> BmcaResult {
        let own = self.to_announce();
        // 候选者：自身 Announce 在前，随后是所有远端 Announce.
        let mut candidates: alloc::vec::Vec<&AnnounceMessage> = alloc::vec::Vec::new();
        candidates.push(&own);
        for a in announces {
            candidates.push(a);
        }
        // 选最小者（compare_priority 返回 Less 表示更优）.
        let mut best_idx = 0;
        for i in 1..candidates.len() {
            if compare_priority(candidates[i], candidates[best_idx]) == Ordering::Less {
                best_idx = i;
            }
        }
        let best = candidates[best_idx];
        if best.grandmaster_identity == self.identity {
            for p in self.ports.iter_mut() {
                p.role = PortRole::Master;
            }
            BmcaResult::ElectedAsMaster
        } else {
            self.grandmaster_identity = best.grandmaster_identity;
            self.steps_removed = best.steps_removed + 1;
            BmcaResult::FollowMaster(best.grandmaster_identity)
        }
    }

    /// 处理 Sync 报文（D9：`delay_ns` 为参数，修正蓝图 bug）.
    ///
    /// 低通滤波：`offset = (offset * 7 + new_offset) / 8`，
    /// 其中 `new_offset = rx_ts.diff_ns(origin_timestamp) - delay_ns`.
    pub fn handle_sync(&mut self, sync: &SyncMessage, rx_ts: PtpTime, delay_ns: i64) {
        let new_offset = rx_ts.diff_ns(&sync.origin_timestamp) - delay_ns;
        self.offset = (self.offset * 7 + new_offset) / 8;
        self.last_sync_seq_id = Some(sync.sequence_id);
        self.last_sync_rx_ts = Some(rx_ts);
        self.last_sync_delay_ns = delay_ns;
        self.last_sync_origin_ts = Some(sync.origin_timestamp);
    }

    /// 处理 FollowUp 报文：序列号匹配时用精确时间戳重算偏移.
    pub fn handle_follow_up(&mut self, fu: &FollowUpMessage) {
        if self.last_sync_seq_id == Some(fu.sequence_id) {
            if let Some(rx_ts) = self.last_sync_rx_ts {
                let new_offset =
                    rx_ts.diff_ns(&fu.precise_origin_timestamp) - self.last_sync_delay_ns;
                self.offset = (self.offset * 7 + new_offset) / 8;
            }
            // 序列号匹配，消费之.
            self.last_sync_seq_id = None;
        }
    }

    /// 时钟修正（D6：无 `warn!` 宏，用 `last_jump_ns` 记录跳变）.
    ///
    /// - 偏移 < 1ms：仅调整 `frequency_offset`，不改 `current_time`
    /// - 偏移 ≥ 1ms：直接跳变 `current_time`，记录 `last_jump_ns`
    pub fn adjust_clock(&mut self, offset: i64) {
        if offset.abs() < 1_000_000 {
            self.frequency_offset = offset / 100;
        } else {
            self.current_time.add_ns(offset);
            self.last_jump_ns = Some(offset);
        }
    }

    /// 返回当前估算偏移（ns）.
    pub fn compute_offset(&self) -> i64 {
        self.offset
    }

    /// 返回修正后的当前时间（`current_time + offset`）.
    pub fn current_time(&self) -> PtpTime {
        let mut t = self.current_time;
        t.add_ns(self.offset);
        t
    }

    /// 生成本时钟的 Announce 报文.
    pub fn to_announce(&self) -> AnnounceMessage {
        AnnounceMessage {
            grandmaster_identity: self.grandmaster_identity,
            priority1: self.priority1,
            clock_class: self.clock_class,
            accuracy: self.accuracy,
            priority2: self.priority2,
            steps_removed: self.steps_removed,
            source_port_id: 0,
            source_mac: MacAddr::new([0; 6]),
        }
    }
}
