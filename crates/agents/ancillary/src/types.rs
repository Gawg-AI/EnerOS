//! 辅助服务数据类型
//!
//! 定义调频/调峰 + AGC 相关的数据结构。

/// 辅助服务类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AncillaryType {
    /// 调频（频率调节）
    FreqReg,
    /// 调峰（削峰）
    PeakShaving,
}

/// 辅助服务报价
#[derive(Debug, Clone, PartialEq)]
pub struct AncillaryBid {
    /// 报价唯一标识
    pub bid_id: u64,
    /// 服务类型
    pub service: AncillaryType,
    /// 容量 (kW)
    pub capacity_kw: f64,
    /// 爬坡速率 (kW/s)
    pub ramp_kw_per_s: f64,
    /// 报价 (元/kW)
    pub price_yuan_per_kw: f64,
    /// 持续时间 (min)
    pub duration_min: u32,
}

/// 报价确认
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BidAck {
    /// 对应报价 ID
    pub bid_id: u64,
    /// 是否被接受
    pub accepted: bool,
    /// 确认时间戳 (ms)
    pub ack_at: u64,
}

/// AGC 调度命令
#[derive(Debug, Clone, PartialEq)]
pub struct AgcCommand {
    /// 命令唯一标识
    pub cmd_id: u64,
    /// 目标功率 (kW)
    pub target_power_kw: f64,
    /// 执行截止 (ms)
    pub deadline_ms: u32,
    /// 接收时间戳 (ms)
    pub received_at: u64,
}

/// AGC 执行确认
#[derive(Debug, Clone, PartialEq)]
pub struct AgcAck {
    /// 对应命令 ID
    pub cmd_id: u64,
    /// 实际执行功率 (kW)
    pub actual_kw: f64,
    /// 确认时间戳 (ms)
    pub ack_at: u64,
}

/// 调频状态参数
#[derive(Debug, Clone, PartialEq)]
pub struct FreqRegState {
    /// 死区 (Hz)
    pub deadband_hz: f64,
    /// 下垂系数 (%)
    pub droop_pct: f64,
    /// 最大调节功率 (kW)
    pub max_power_kw: f64,
    /// 上次目标功率 (kW)
    pub last_target_kw: f64,
}

/// 削峰计划参数
#[derive(Debug, Clone, PartialEq)]
pub struct PeakShavePlan {
    /// 负荷阈值 (kW)
    pub threshold_kw: f64,
    /// 放电上限 (kWh)
    pub discharge_limit_kwh: f64,
    /// 最低 SOC
    pub soc_min: f64,
    /// 当前 SOC
    pub soc_current: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ancillary_type_eq() {
        assert_eq!(AncillaryType::FreqReg, AncillaryType::FreqReg);
        assert_ne!(AncillaryType::FreqReg, AncillaryType::PeakShaving);
    }

    #[test]
    fn test_ancillary_bid_creation() {
        let bid = AncillaryBid {
            bid_id: 1,
            service: AncillaryType::FreqReg,
            capacity_kw: 500.0,
            ramp_kw_per_s: 10.0,
            price_yuan_per_kw: 3.5,
            duration_min: 60,
        };
        assert_eq!(bid.bid_id, 1);
        assert_eq!(bid.service, AncillaryType::FreqReg);
        assert!((bid.capacity_kw - 500.0).abs() < f64::EPSILON);
        assert!((bid.ramp_kw_per_s - 10.0).abs() < f64::EPSILON);
        assert!((bid.price_yuan_per_kw - 3.5).abs() < f64::EPSILON);
        assert_eq!(bid.duration_min, 60);
    }

    #[test]
    fn test_bid_ack() {
        let ack = BidAck {
            bid_id: 1,
            accepted: true,
            ack_at: 1000,
        };
        assert!(ack.accepted);
        assert_eq!(ack.ack_at, 1000);
    }

    #[test]
    fn test_agc_command() {
        let cmd = AgcCommand {
            cmd_id: 42,
            target_power_kw: 200.0,
            deadline_ms: 5000,
            received_at: 9000,
        };
        assert_eq!(cmd.cmd_id, 42);
        assert!((cmd.target_power_kw - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_agc_ack() {
        let ack = AgcAck {
            cmd_id: 42,
            actual_kw: 198.5,
            ack_at: 9500,
        };
        assert!((ack.actual_kw - 198.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_freq_reg_state() {
        let state = FreqRegState {
            deadband_hz: 0.033,
            droop_pct: 5.0,
            max_power_kw: 1000.0,
            last_target_kw: 0.0,
        };
        assert!((state.deadband_hz - 0.033).abs() < 1e-6);
        assert!((state.droop_pct - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_peak_shave_plan() {
        let plan = PeakShavePlan {
            threshold_kw: 800.0,
            discharge_limit_kwh: 2000.0,
            soc_min: 0.2,
            soc_current: 0.8,
        };
        assert!((plan.threshold_kw - 800.0).abs() < f64::EPSILON);
        assert!(plan.soc_current > plan.soc_min);
    }
}
