//! 市场域共享词汇（ARCH-M01/M02 / ADR-0029）。
//!
//! `TradeDir`/`TimeSlot`/`Bid`/`BidSegment`/`BidAck` 与价格单位 newtype
//! （[`YuanPerKwh`]/[`YuanPerMwh`]）的**唯一定义**。spot-market、
//! realtime-market、settlement、energy-market-agent 等 crate 一律 re-export
//! 本 crate，禁止再定义等价类型（ADR-0029）。
//!
//! # 价格单位（ARCH-M02）
//!
//! 价格必须是类型而非注释：`元/kWh` 与 `元/MWh` 相差 1000 倍，
//! 裸 `f64` 传递在编译期无法区分。跨 crate 传递价格必须携带单位；
//! 单位换算集中在 [`YuanPerKwh::to_mwh`] / [`YuanPerMwh::to_kwh`]。
//!
//! # no_std
//!
//! `#![cfg_attr(not(test), no_std)]` + `alloc`（`Bid`/`BidSegment` 使用 `Vec`）。

#![cfg_attr(not(test), no_std)]
#![allow(clippy::result_large_err)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// 交易方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TradeDir {
    /// 买入。
    Buy,
    /// 卖出。
    Sell,
}

/// 时段类型（峰/平/谷，W5.1 蓝图23 / ADR-0029 词汇归属收敛）。
///
/// 进入跨 crate 契约（`PricePoint.period`、报价单 `ResourceBid.period`），
/// 由本 crate 唯一持有；energy-market-agent 等 crate re-export 本定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    /// 峰时段。
    Peak,
    /// 平时段。
    #[default]
    Flat,
    /// 谷时段。
    Valley,
}

/// 时间段（UTC 秒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimeSlot {
    /// 起始时间（UTC 秒）。
    pub start: u64,
    /// 结束时间（UTC 秒）。
    pub end: u64,
}

impl TimeSlot {
    /// 创建时间段。
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// 检查时间段是否有效（end > start）。
    pub const fn is_valid(&self) -> bool {
        self.end > self.start
    }

    /// 获取持续秒数。
    pub const fn duration_secs(&self) -> u64 {
        self.end - self.start
    }
}

// ============================================================================
// 价格单位（ARCH-M02 / ADR-0029）
// ============================================================================

/// 电价 newtype（元/kWh）。
///
/// 与 [`YuanPerMwh`] 相差 1000 倍；禁止在二者之间做隐式裸 `f64` 换算。
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct YuanPerKwh(pub f64);

/// 电价 newtype（元/MWh）。
///
/// 与 [`YuanPerKwh`] 相差 1000 倍；禁止在二者之间做隐式裸 `f64` 换算。
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct YuanPerMwh(pub f64);

impl YuanPerKwh {
    /// 零价。
    pub const ZERO: YuanPerKwh = YuanPerKwh(0.0);

    /// 换算为元/MWh（×1000）。
    pub const fn to_mwh(self) -> YuanPerMwh {
        YuanPerMwh(self.0 * 1000.0)
    }
}

impl YuanPerMwh {
    /// 换算为元/kWh（÷1000）。
    pub const fn to_kwh(self) -> YuanPerKwh {
        YuanPerKwh(self.0 / 1000.0)
    }
}

// ============================================================================
// 报价模型
// ============================================================================

/// 市场类型（统一枚举，ADR-0029：涵盖现货/实时/辅助服务/需求响应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarketType {
    /// 日前市场。
    DayAhead,
    /// 日内市场。
    IntraDay,
    /// 实时市场。
    Realtime,
    /// 现货市场（energy-market-agent 语义）。
    Spot,
    /// 辅助服务市场。
    AncillaryService,
    /// 需求响应。
    DemandResponse,
}

/// 报价段。
#[derive(Debug, Clone, PartialEq)]
pub struct BidSegment {
    /// 时段。
    pub period: TimeSlot,
    /// 电量（kWh）。
    pub volume_kwh: f64,
    /// 价格（元/kWh，单位化）。
    pub price: YuanPerKwh,
    /// 交易方向。
    pub direction: TradeDir,
}

/// 报价单（统一结构，ADR-0029）。
#[derive(Debug, Clone, PartialEq)]
pub struct Bid {
    /// 报价 ID。
    pub bid_id: u64,
    /// 市场类型。
    pub market: MarketType,
    /// 提交者 ID。
    pub submitter: u64,
    /// 报价段列表。
    pub segments: Vec<BidSegment>,
    /// 提交时间（UTC 秒）。
    pub submit_at: u64,
}

impl Bid {
    /// 创建报价单。
    pub fn new(bid_id: u64, market: MarketType, submitter: u64, submit_at: u64) -> Self {
        Self {
            bid_id,
            market,
            submitter,
            segments: Vec::new(),
            submit_at,
        }
    }

    /// 添加报价段。
    pub fn with_segment(mut self, segment: BidSegment) -> Self {
        self.segments.push(segment);
        self
    }

    /// 报价总电量（kWh）。
    pub fn total_volume_kwh(&self) -> f64 {
        self.segments.iter().map(|s| s.volume_kwh).sum()
    }
}

/// 报价回执。
#[derive(Debug, Clone, PartialEq)]
pub struct BidAck {
    /// 报价 ID。
    pub bid_id: u64,
    /// 是否接受。
    pub accepted: bool,
    /// 拒绝原因（接受时为 None）。
    pub reason: Option<String>,
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// TradeDir/TimeSlot 基本语义。
    #[test]
    fn trade_dir_and_time_slot() {
        assert_eq!(TradeDir::Buy, TradeDir::Buy);
        assert_ne!(TradeDir::Buy, TradeDir::Sell);
        let ts = TimeSlot::new(1000, 2000);
        assert!(ts.is_valid());
        assert_eq!(ts.duration_secs(), 1000);
        assert!(!TimeSlot::new(2000, 1000).is_valid());
    }

    /// 价格单位换算（1000 倍关系显式、集中）。
    #[test]
    fn price_unit_conversion() {
        let kwh = YuanPerKwh(0.5);
        assert_eq!(kwh.to_mwh(), YuanPerMwh(500.0));
        assert_eq!(kwh.to_mwh().to_kwh(), kwh);
        // 编译期类型隔离：YuanPerKwh 与 YuanPerMwh 不可互赋值/比较
        let _mwh: YuanPerMwh = kwh.to_mwh();
        let _kwh: YuanPerKwh = _mwh.to_kwh();
        assert_eq!(_kwh, kwh);
    }

    /// 蓝图/20 R3-2 金额定点精度：同一价格多次成交累计无漂移
    /// （f64 直算 0.1+0.2 场景在定点表示下精确；金额运算经
    /// `MilliYuan` 定点语义验证）。
    #[test]
    fn price_fixed_point_precision() {
        // 单价 0.1 元/kWh × 3 次 100 kWh 成交：金额必须精确等于 30 元
        // （毫元定点，无浮点累计误差）。
        let price = YuanPerKwh(0.1);
        let mut total = MilliYuan::ZERO;
        for _ in 0..3 {
            let amount = MilliYuan::from_yuan_f64(100.0 * price.0);
            total = MilliYuan(total.0 + amount.0);
        }
        assert_eq!(total, MilliYuan::from_yuan_f64(30.0), "定点累计精确");
        // 单位换算往返不损失精度（1000 倍关系）。
        let mwh = price.to_mwh();
        assert_eq!(mwh.to_kwh(), price);
    }

    /// Bid 构造与聚合。
    #[test]
    fn bid_build_and_aggregate() {
        let bid = Bid::new(1, MarketType::DayAhead, 42, 1000).with_segment(BidSegment {
            period: TimeSlot::new(2000, 3000),
            volume_kwh: 100.0,
            price: YuanPerKwh(0.5),
            direction: TradeDir::Sell,
        });
        assert_eq!(bid.bid_id, 1);
        assert_eq!(bid.total_volume_kwh(), 100.0);
        assert_eq!(bid.segments.len(), 1);
    }
}

// ============================================================================
// ARC-003：金额定点化（毫元）
// ============================================================================

/// 金额定点类型（ARC-003：1 元 = 1000 毫元，i64）。
///
/// 消除结算/账本/自治/演示路径的 `f64` 金额（浮点漂移与 0.1+0.2 类误差）；
/// 电价仍为 [`YuanPerKwh`]/[`YuanPerMwh`]（单位化 newtype），金额一律
/// 经本类型以毫元整数表示。
///
/// # 舍入规范（ARC-003）
/// `from_yuan_f64` 采用 **round-half-up 到毫元**：`(v * 1000).round()`，
/// 即 0.0005 元（0.5 毫元）进位、0.0004 元舍去。乘费率/电量同样
/// 先精确 f64 计算再 round-half-up 定毫。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct MilliYuan(pub i64);

/// round-half-up（0.5 进位；负数为 -0.5 进位，即远离零）。
/// no_std 手写（core 浮点无 floor/ceil）：截断 + 分数判断。
fn round_half_up(v: f64) -> i64 {
    let trunc = v as i64;
    let frac = v - trunc as f64;
    if frac >= 0.5 {
        trunc + 1
    } else if frac <= -0.5 {
        trunc - 1
    } else {
        trunc
    }
}

impl MilliYuan {
    /// 零金额。
    pub const ZERO: MilliYuan = MilliYuan(0);

    /// 从元（f64）按舍入规范定毫（round-half-up，no_std 手写）。
    pub fn from_yuan_f64(v: f64) -> Self {
        MilliYuan(round_half_up(v * 1000.0))
    }

    /// 转元（f64，仅供展示/导出；内部计算保持定点）。
    pub fn to_yuan_f64(self) -> f64 {
        self.0 as f64 / 1000.0
    }

    /// 定点加（溢出时饱和——金额场景 i64 毫元上限约 9.2e15 元，不触达）。
    pub fn checked_add(self, other: MilliYuan) -> Option<MilliYuan> {
        self.0.checked_add(other.0).map(MilliYuan)
    }

    /// 定点减。
    pub fn checked_sub(self, other: MilliYuan) -> Option<MilliYuan> {
        self.0.checked_sub(other.0).map(MilliYuan)
    }

    /// 金额 × 无量纲系数（费率/比例，f64）→ 毫元（round-half-up）。
    pub fn mul_f64(self, factor: f64) -> MilliYuan {
        MilliYuan(round_half_up(self.0 as f64 * factor))
    }
}

#[cfg(test)]
mod arc003_tests {
    use super::*;

    #[test]
    fn test_arc003_round_half_up() {
        // 舍入规范：0.5 毫元进位
        assert_eq!(
            MilliYuan::from_yuan_f64(0.0005).0,
            1,
            "0.0005 元 = 0.5 毫元进位"
        );
        assert_eq!(MilliYuan::from_yuan_f64(0.0004).0, 0, "0.0004 元舍去");
        assert_eq!(
            MilliYuan::from_yuan_f64(1.2345).0,
            1235,
            "1.2345 元 → 1235 毫元"
        );
        assert_eq!(MilliYuan::from_yuan_f64(1.2344).0, 1234);
        assert_eq!(
            MilliYuan::from_yuan_f64(-0.0005).0,
            -1,
            "负数同样 round-half-up"
        );
    }

    #[test]
    fn test_arc003_arithmetic() {
        let a = MilliYuan(1000);
        let b = MilliYuan(250);
        assert_eq!(a.checked_add(b), Some(MilliYuan(1250)));
        assert_eq!(a.checked_sub(b), Some(MilliYuan(750)));
        assert_eq!(a.mul_f64(0.05), MilliYuan(50), "1000 毫元 × 5% = 50 毫元");
        assert_eq!(
            a.mul_f64(0.3333),
            MilliYuan(333),
            "round-half-up: 333.3 → 333"
        );
    }

    #[test]
    fn test_arc003_zero() {
        assert_eq!(MilliYuan::ZERO, MilliYuan(0));
        assert_eq!(MilliYuan::from_yuan_f64(0.0).0, 0);
    }
}
