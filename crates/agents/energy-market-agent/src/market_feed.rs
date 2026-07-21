//! v0.85.0 市场数据订阅 —— 市场数据模型.
//!
//! 本模块定义 v0.85.0 市场数据订阅版本的核心数据类型：市场类型、时段类型、
//! 电价点、需求响应信号、市场数据馈送以及市场错误。
//!
//! # 偏差声明（D3~D7）
//!
//! - **D3**：命名 `MarketFeed` 避开 v0.72.0 既有 `MarketData`，防止类型冲突。
//! - **D4**：`DrSignal.event_id` 使用 `u64` 而非蓝图中的 `String`，保持 `Copy` 语义。
//! - **D5**：蓝图交付物 `PriceSignal` 视为 `PricePoint` 别名，本版本命名 `PricePoint`。
//! - **D6**：蓝图引用 `Period` 但未定义，本版本定义峰/平/谷三时段。
//! - **D7**：`MarketError` MVP 收敛为 3 变体（SourceFailed/ParseFailed/PublishFailed）。

use alloc::vec::Vec;

/// 市场类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarketType {
    /// 现货市场.
    #[default]
    Spot,
    /// 辅助服务市场.
    AncillaryService,
    /// 需求响应.
    DemandResponse,
}

/// 时段类型（D6：蓝图引用未定义，本版本定义峰/平/谷）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    /// 峰时段.
    Peak,
    /// 平时段.
    #[default]
    Flat,
    /// 谷时段.
    Valley,
}

/// 电价点（D5：蓝图交付物 PriceSignal 视为 PricePoint 别名）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PricePoint {
    /// 时间戳（ms）.
    pub time: u64,
    /// 电价（元/kWh）.
    pub price: f32,
    /// 时段.
    pub period: Period,
}

/// 需求响应信号（D4：event_id 用 u64 而非 String，保持 Copy）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DrSignal {
    /// 事件 ID.
    pub event_id: u64,
    /// 目标功率（MW）.
    pub target_mw: f32,
    /// 开始时间（ms）.
    pub start: u64,
    /// 结束时间（ms）.
    pub end: u64,
    /// 补偿价格（元）.
    pub reward: f32,
}

/// 市场数据馈送（D3：命名 MarketFeed 避开 v0.72.0 既有 MarketData）.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MarketFeed {
    /// 市场类型.
    pub market_type: MarketType,
    /// 时间戳（ms）.
    pub timestamp: u64,
    /// 电价点列表.
    pub prices: Vec<PricePoint>,
    /// DR 信号列表.
    pub dr_signals: Vec<DrSignal>,
}

/// 市场错误（D7：MVP 3 变体）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketError {
    /// 数据源失败.
    SourceFailed,
    /// 解析失败.
    ParseFailed,
    /// 发布失败.
    PublishFailed,
}

#[cfg(test)]
mod tests {
    use alloc::format;

    use super::*;

    // ===== T1: MarketType 默认值为 Spot =====
    #[test]
    fn t1_market_type_default_spot() {
        assert_eq!(MarketType::default(), MarketType::Spot);
    }

    // ===== T2: MarketType 3 变体 Debug 输出非空 =====
    #[test]
    fn t2_market_type_debug_nonempty() {
        assert!(!format!("{:?}", MarketType::Spot).is_empty());
        assert!(!format!("{:?}", MarketType::AncillaryService).is_empty());
        assert!(!format!("{:?}", MarketType::DemandResponse).is_empty());
    }

    // ===== T3: Period 默认值为 Flat =====
    #[test]
    fn t3_period_default_flat() {
        assert_eq!(Period::default(), Period::Flat);
    }

    // ===== T4: Period 3 变体 Debug 输出非空 =====
    #[test]
    fn t4_period_debug_nonempty() {
        assert!(!format!("{:?}", Period::Peak).is_empty());
        assert!(!format!("{:?}", Period::Flat).is_empty());
        assert!(!format!("{:?}", Period::Valley).is_empty());
    }

    // ===== T5: PricePoint::default 字段为 0/0.0/Flat =====
    #[test]
    fn t5_price_point_default() {
        let p = PricePoint::default();
        assert_eq!(p.time, 0);
        assert!((p.price - 0.0).abs() < f32::EPSILON);
        assert_eq!(p.period, Period::Flat);
    }

    // ===== T6: PricePoint 构造与字段访问 =====
    #[test]
    fn t6_price_point_fields() {
        let p = PricePoint {
            time: 1000,
            price: 0.85,
            period: Period::Peak,
        };
        assert_eq!(p.time, 1000);
        assert!((p.price - 0.85).abs() < f32::EPSILON);
        assert_eq!(p.period, Period::Peak);
    }

    // ===== T7: PricePoint 是 Copy =====
    #[test]
    fn t7_price_point_copy() {
        let p1 = PricePoint {
            time: 1000,
            price: 0.85,
            period: Period::Peak,
        };
        let p2 = p1;
        assert_eq!(p2, p1);
    }

    // ===== T8: DrSignal::default 全字段为 0/0.0 =====
    #[test]
    fn t8_dr_signal_default() {
        let s = DrSignal::default();
        assert_eq!(s.event_id, 0);
        assert!((s.target_mw - 0.0).abs() < f32::EPSILON);
        assert_eq!(s.start, 0);
        assert_eq!(s.end, 0);
        assert!((s.reward - 0.0).abs() < f32::EPSILON);
    }

    // ===== T9: DrSignal 构造与字段断言 =====
    #[test]
    fn t9_dr_signal_fields() {
        let s = DrSignal {
            event_id: 42,
            target_mw: 2.5,
            start: 1000,
            end: 2000,
            reward: 500.0,
        };
        assert_eq!(s.event_id, 42);
        assert!((s.target_mw - 2.5).abs() < f32::EPSILON);
        assert_eq!(s.start, 1000);
        assert_eq!(s.end, 2000);
        assert!((s.reward - 500.0).abs() < f32::EPSILON);
    }

    // ===== T10: DrSignal 是 Copy =====
    #[test]
    fn t10_dr_signal_copy() {
        let s1 = DrSignal {
            event_id: 42,
            target_mw: 2.5,
            start: 1000,
            end: 2000,
            reward: 500.0,
        };
        let s2 = s1;
        assert_eq!(s2, s1);
    }

    // ===== T11: MarketFeed::default 空馈送 =====
    #[test]
    fn t11_market_feed_default() {
        let feed = MarketFeed::default();
        assert!(feed.prices.is_empty());
        assert!(feed.dr_signals.is_empty());
        assert_eq!(feed.market_type, MarketType::Spot);
        assert_eq!(feed.timestamp, 0);
    }

    // ===== T12: MarketError 3 变体 PartialEq 与 Debug =====
    #[test]
    fn t12_market_error_variants() {
        assert_eq!(MarketError::SourceFailed, MarketError::SourceFailed);
        assert_eq!(MarketError::ParseFailed, MarketError::ParseFailed);
        assert_eq!(MarketError::PublishFailed, MarketError::PublishFailed);
        assert_ne!(MarketError::SourceFailed, MarketError::ParseFailed);
        assert_ne!(MarketError::ParseFailed, MarketError::PublishFailed);
        assert_ne!(MarketError::SourceFailed, MarketError::PublishFailed);
        assert!(!format!("{:?}", MarketError::SourceFailed).is_empty());
        assert!(!format!("{:?}", MarketError::ParseFailed).is_empty());
        assert!(!format!("{:?}", MarketError::PublishFailed).is_empty());
    }
}
