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
use crate::gptp::{GptpClock, GptpConfig};

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

impl SoftSyncClaim {
    /// 能力一致性检查：宣称 vs 实际。
    ///
    /// 降级路径下：`hardware_timestamp == false` 且 `taprio == false`，
    /// 与实现能力一致（无 TSN 硬实时承诺）。非降级路径必须声明硬件能力
    /// （当前未提供 → 不一致）。
    ///
    /// W3.4（蓝图23，ADR-0040）：原 `SoftSyncClock::verify_claim` 的逻辑收敛到此处
    /// （`SoftSyncClock` 纯委托 wrapper 已删除；gPTP 状态机由 `GptpClock` 直接提供）。
    pub fn verify_claim(&self) -> bool {
        if self.degraded {
            !self.hardware_timestamp && !self.taprio
        } else {
            false
        }
    }
}

/// 构造降级软同步宣称 + gPTP 软时钟状态机（替代已删的 `SoftSyncClock`）。
///
/// 返回 `(GptpClock, SoftSyncClaim)`——调用方直接用 `GptpClock` 的 `handle_sync`/
/// `offset_ns`，用 `SoftSyncClaim::verify_claim` 做能力一致性检查。
pub fn soft_sync_clock(
    identity: ClockIdentity,
    initial_time: PtpTime,
    config: &GptpConfig,
) -> (GptpClock, SoftSyncClaim) {
    (
        GptpClock::new(identity, initial_time, config),
        SoftSyncClaim::default(),
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gptp::SyncMessage;

    #[test]
    fn test_default_claim_is_degraded() {
        let claim = SoftSyncClaim::default();
        assert!(claim.degraded);
        assert!(!claim.hardware_timestamp);
        assert!(!claim.taprio);
    }

    #[test]
    fn test_soft_sync_clock_claim_consistent() {
        let (_clock, claim) = soft_sync_clock(
            ClockIdentity::new([0u8; 8]),
            PtpTime::new(0, 0),
            &crate::gptp::GptpConfig::default(),
        );
        assert!(claim.verify_claim(), "降级宣称必须与能力一致");
        assert_eq!(claim.claimed_accuracy_ns, 500_000);
    }

    #[test]
    fn test_soft_sync_processes_sync() {
        let (mut clock, _claim) = soft_sync_clock(
            ClockIdentity::new([0u8; 8]),
            PtpTime::new(0, 0),
            &crate::gptp::GptpConfig::default(),
        );
        // 初始无偏移。
        assert_eq!(clock.offset, 0);
        // 构造 SYNC 报文并处理（软时间戳路径不 panic）。
        let sync = SyncMessage {
            origin_timestamp: PtpTime::new(1_700_000_000, 0),
            sequence_id: 1,
            steps_removed: 0,
        };
        let rx_ts = PtpTime::new(1_700_000_000, 0);
        clock.handle_sync(&sync, rx_ts, 0);
        // 处理后偏移已被滤波更新（值随实现，仅验证可调用）。
        let _ = clock.offset;
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
        assert!(!claim.verify_claim(), "非降级宣称当前不可验证");
    }
}
