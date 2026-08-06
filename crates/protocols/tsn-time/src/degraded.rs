//! TSN 降级路径：软时钟同步宣称（v0.223.0，B2-06 裁决）。
//!
//! # 裁决结论（v0.223.0）
//!
//! 目标板网卡不支持硬件时间戳/taprio（QEMU virt 与首款交付板评估结论），
//! 按蓝图 v0.223.0 选项 ②执行：**降级为"软时钟同步"宣称**——
//!
//! - 移除 TSN 硬实时承诺（不再宣称 gPTP 硬件时间戳精度、taprio 门控
//!   确定性时隙）；
//! - gPTP 状态机（BMCA 选举 + 偏移滤波）保留为软件时间同步原语；
//! - 本模块提供 [`SoftSyncClaim`]：能力与宣称一致性的机器可查结构。
//!
//! 真实硬件路径（taprio/gPTP 硬件时间戳）在具备能力的网卡接入后
//! 另行评估（蓝图 v0.223.0 选项 ①）。

use crate::clock::{ClockIdentity, PtpTime};
use crate::gptp::{FollowUpMessage, GptpClock, SyncMessage};

/// 软时钟同步宣称（能力一致性，降级路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoftSyncClaim {
    /// 是否为降级模式（true = 软同步，无 TSN 硬实时承诺）。
    pub degraded: bool,
    /// 是否支持硬件时间戳（降级后恒 false）。
    pub hardware_timestamp: bool,
    /// 是否支持 taprio 门控下发（降级后恒 false）。
    pub taprio: bool,
    /// 宣称的同步精度（纳秒；软同步不承诺硬实时界）。
    pub claimed_accuracy_ns: u64,
}

impl Default for SoftSyncClaim {
    fn default() -> Self {
        Self {
            degraded: true,
            hardware_timestamp: false,
            taprio: false,
            // 软同步：宣称精度 = 软件滤波可实现的典型量级（亚毫秒级
            // 受调度抖动影响，不作为硬实时保证）。
            claimed_accuracy_ns: 500_000,
        }
    }
}

/// 软时钟同步：在 gPTP 状态机之上提供软件时间同步。
///
/// 与 `GptpClock` 的差异：不依赖硬件时间戳，`rx_ts` 由软件层
/// （协议栈轮询时刻）提供；精度受调度抖动影响（软宣称）。
pub struct SoftSyncClock {
    /// 底层 gPTP 时钟状态机。
    pub inner: GptpClock,
    /// 能力宣称。
    pub claim: SoftSyncClaim,
}

impl SoftSyncClock {
    /// 创建软同步时钟。
    pub fn new(identity: ClockIdentity, initial_time: PtpTime, config: &crate::gptp::GptpConfig) -> Self {
        Self {
            inner: GptpClock::new(identity, initial_time, config),
            claim: SoftSyncClaim::default(),
        }
    }

    /// 处理 SYNC 报文（软时间戳）。
    pub fn handle_sync(&mut self, sync: &SyncMessage, rx_ts: PtpTime, delay_ns: i64) {
        self.inner.handle_sync(sync, rx_ts, delay_ns);
    }

    /// 处理 FOLLOW_UP 报文。
    pub fn handle_follow_up(&mut self, follow_up: &FollowUpMessage) {
        self.inner.handle_follow_up(follow_up);
    }

    /// 返回当前时间修正偏移（纳秒）。
    pub fn offset_ns(&self) -> i64 {
        self.inner.offset
    }

    /// 能力一致性检查：宣称 vs 实际。
    ///
    /// 降级路径下：`hardware_timestamp == false` 且 `taprio == false`，
    /// 与实现能力一致（无 TSN 硬实时承诺）。
    pub fn verify_claim(&self) -> bool {
        if self.claim.degraded {
            !self.claim.hardware_timestamp && !self.claim.taprio
        } else {
            // 非降级路径必须声明硬件能力（当前未提供）。
            false
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_claim_is_degraded() {
        let claim = SoftSyncClaim::default();
        assert!(claim.degraded);
        assert!(!claim.hardware_timestamp);
        assert!(!claim.taprio);
    }

    #[test]
    fn test_soft_sync_clock_claim_consistent() {
        let clock = SoftSyncClock::new(
            ClockIdentity::new([0u8; 8]),
            PtpTime::new(0, 0),
            &crate::gptp::GptpConfig::default(),
        );
        assert!(clock.verify_claim(), "降级宣称必须与能力一致");
        assert_eq!(clock.claim.claimed_accuracy_ns, 500_000);
    }

    #[test]
    fn test_soft_sync_processes_sync() {
        let mut clock = SoftSyncClock::new(
            ClockIdentity::new([0u8; 8]),
            PtpTime::new(0, 0),
            &crate::gptp::GptpConfig::default(),
        );
        // 初始无偏移。
        assert_eq!(clock.offset_ns(), 0);
        // 构造 SYNC 报文并处理（软时间戳路径不 panic）。
        let sync = SyncMessage {
            origin_timestamp: PtpTime::new(1_700_000_000, 0),
            sequence_id: 1,
            steps_removed: 0,
        };
        let rx_ts = PtpTime::new(1_700_000_000, 0);
        clock.handle_sync(&sync, rx_ts, 0);
        // 处理后偏移已被滤波更新（值随实现，仅验证可调用）。
        let _ = clock.offset_ns();
    }

    #[test]
    fn test_non_degraded_claim_inconsistent() {
        // 若错误地宣称硬件能力（非降级），verify_claim 必须失败。
        let claim = SoftSyncClaim {
            degraded: false,
            hardware_timestamp: true,
            taprio: true,
            claimed_accuracy_ns: 100,
        };
        let clock = SoftSyncClock {
            inner: GptpClock::new(
                ClockIdentity::new([0u8; 8]),
                PtpTime::new(0, 0),
                &crate::gptp::GptpConfig::default(),
            ),
            claim,
        };
        assert!(!clock.verify_claim(), "非降级宣称当前不可验证");
    }
}
