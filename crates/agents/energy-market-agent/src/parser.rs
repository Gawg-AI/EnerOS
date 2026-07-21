//! v0.85.0 市场数据订阅 —— 文本行解析器.
//!
//! 本模块实现 v0.85.0 市场数据订阅版本的文本行解析功能，将外部市场数据源
//! 的文本行（电价点行 `P,...`、需求响应信号行 `D,...`）解析为 [`PricePoint`] /
//! [`DrSignal`]，并聚合为 [`MarketFeed`]。
//!
//! # 偏差声明（D14）
//!
//! - **D14**：不派生 serde / 不引入 `serde_json`，改为手写 `core::str` 文本解析
//!   （`split` / `trim` / `parse`）。理由：no_std 全项目合规要求下减少依赖面，
//!   文本协议字段固定、结构简单，手写解析零分配压力且行为可控；时段解析使用
//!   `eq_ignore_ascii_case` 大小写不敏感匹配，避免 `to_lowercase()` 的 String 分配。
//!
//! # 数据格式
//!
//! - 电价点行：`P,<time>,<price>,<period>`（逗号分隔，字段允许空白，需 `trim()`）
//! - DR 信号行：`D,<event_id>,<target_mw>,<start>,<end>,<reward>`
//! - 聚合解析时，非法行按蓝图 §4.4「数据格式错误」策略跳过，不传播错误。

use alloc::vec::Vec;

use crate::market_feed::{DrSignal, MarketError, MarketFeed, MarketType, Period, PricePoint};

/// 解析时段字符串（大小写不敏感）.
///
/// 使用 `eq_ignore_ascii_case` 逐个匹配（core 方法，no_std 可用），
/// 避免 `to_lowercase()` 带来的 String 分配。
fn parse_period(s: &str) -> Result<Period, MarketError> {
    if s.eq_ignore_ascii_case("peak") {
        Ok(Period::Peak)
    } else if s.eq_ignore_ascii_case("flat") {
        Ok(Period::Flat)
    } else if s.eq_ignore_ascii_case("valley") {
        Ok(Period::Valley)
    } else {
        Err(MarketError::ParseFailed)
    }
}

/// 解析电价点文本行.
///
/// 格式：`P,<time>,<price>,<period>`（逗号分隔，字段允许空白，需 `trim()`）。
///
/// # 错误
///
/// 以下情况返回 `Err(MarketError::ParseFailed)`：
///
/// - 前缀非 `P`
/// - 字段数不等于 4
/// - `time`（u64）或 `price`（f32）数字解析失败
/// - `period` 未知（非 peak/flat/valley，大小写不敏感）
pub fn parse_price_point(line: &str) -> Result<PricePoint, MarketError> {
    let fields: Vec<&str> = line.split(',').collect();
    if fields.len() != 4 {
        return Err(MarketError::ParseFailed);
    }
    if fields[0].trim() != "P" {
        return Err(MarketError::ParseFailed);
    }
    let time = fields[1]
        .trim()
        .parse::<u64>()
        .map_err(|_| MarketError::ParseFailed)?;
    let price = fields[2]
        .trim()
        .parse::<f32>()
        .map_err(|_| MarketError::ParseFailed)?;
    let period = parse_period(fields[3].trim())?;
    Ok(PricePoint {
        time,
        price,
        period,
    })
}

/// 解析需求响应信号文本行.
///
/// 格式：`D,<event_id>,<target_mw>,<start>,<end>,<reward>`（逗号分隔，字段允许空白）。
///
/// # 错误
///
/// 以下情况返回 `Err(MarketError::ParseFailed)`：
///
/// - 前缀非 `D`
/// - 字段数不等于 6
/// - `event_id` / `start` / `end`（u64）或 `target_mw` / `reward`（f32）数字解析失败
pub fn parse_dr_signal(line: &str) -> Result<DrSignal, MarketError> {
    let fields: Vec<&str> = line.split(',').collect();
    if fields.len() != 6 {
        return Err(MarketError::ParseFailed);
    }
    if fields[0].trim() != "D" {
        return Err(MarketError::ParseFailed);
    }
    let event_id = fields[1]
        .trim()
        .parse::<u64>()
        .map_err(|_| MarketError::ParseFailed)?;
    let target_mw = fields[2]
        .trim()
        .parse::<f32>()
        .map_err(|_| MarketError::ParseFailed)?;
    let start = fields[3]
        .trim()
        .parse::<u64>()
        .map_err(|_| MarketError::ParseFailed)?;
    let end = fields[4]
        .trim()
        .parse::<u64>()
        .map_err(|_| MarketError::ParseFailed)?;
    let reward = fields[5]
        .trim()
        .parse::<f32>()
        .map_err(|_| MarketError::ParseFailed)?;
    Ok(DrSignal {
        event_id,
        target_mw,
        start,
        end,
        reward,
    })
}

/// 聚合解析市场数据馈送.
///
/// 逐行解析输入文本：
///
/// - 每行先 `trim()`，空行跳过
/// - 行首为 `P` → 尝试 [`parse_price_point`]，成功 push 到 `prices`，失败跳过
/// - 行首为 `D` → 尝试 [`parse_dr_signal`]，成功 push 到 `dr_signals`，失败跳过
/// - 其他前缀行跳过
///
/// 非法行按蓝图 §4.4「数据格式错误」策略跳过，不传播错误；本函数永不 panic。
pub fn parse_feed(input: &str, market_type: MarketType, timestamp: u64) -> MarketFeed {
    let mut prices: Vec<PricePoint> = Vec::new();
    let mut dr_signals: Vec<DrSignal> = Vec::new();
    for raw_line in input.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('P') {
            if let Ok(point) = parse_price_point(line) {
                prices.push(point);
            }
        } else if line.starts_with('D') {
            if let Ok(signal) = parse_dr_signal(line) {
                dr_signals.push(signal);
            }
        }
    }
    MarketFeed {
        market_type,
        timestamp,
        prices,
        dr_signals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== T13: 解析峰时段电价点 =====
    #[test]
    fn t13_parse_price_point_peak() {
        let p = parse_price_point("P,1000,0.85,peak").unwrap();
        assert_eq!(p.time, 1000);
        assert!((p.price - 0.85).abs() < f32::EPSILON);
        assert_eq!(p.period, Period::Peak);
    }

    // ===== T14: 解析平时段电价点 =====
    #[test]
    fn t14_parse_price_point_flat() {
        let p = parse_price_point("P,2000,0.50,flat").unwrap();
        assert_eq!(p.time, 2000);
        assert!((p.price - 0.50).abs() < f32::EPSILON);
        assert_eq!(p.period, Period::Flat);
    }

    // ===== T15: 解析谷时段电价点 =====
    #[test]
    fn t15_parse_price_point_valley() {
        let p = parse_price_point("P,3000,0.30,valley").unwrap();
        assert_eq!(p.time, 3000);
        assert!((p.price - 0.30).abs() < f32::EPSILON);
        assert_eq!(p.period, Period::Valley);
    }

    // ===== T16: 时段解析大小写不敏感 =====
    #[test]
    fn t16_parse_price_point_period_case_insensitive() {
        let p1 = parse_price_point("P,1000,0.85,PEAK").unwrap();
        assert_eq!(p1.period, Period::Peak);
        let p2 = parse_price_point("P,1000,0.85,Peak").unwrap();
        assert_eq!(p2.period, Period::Peak);
    }

    // ===== T17: 前缀非 P → ParseFailed =====
    #[test]
    fn t17_parse_price_point_wrong_prefix() {
        assert_eq!(
            parse_price_point("X,1000,0.85,peak"),
            Err(MarketError::ParseFailed)
        );
    }

    // ===== T18: time / price 数字解析失败 → ParseFailed =====
    #[test]
    fn t18_parse_price_point_number_failure() {
        assert_eq!(
            parse_price_point("P,abc,0.85,peak"),
            Err(MarketError::ParseFailed)
        );
        assert_eq!(
            parse_price_point("P,1000,xyz,peak"),
            Err(MarketError::ParseFailed)
        );
    }

    // ===== T19: 字段不足 → ParseFailed =====
    #[test]
    fn t19_parse_price_point_missing_field() {
        assert_eq!(
            parse_price_point("P,1000,0.85"),
            Err(MarketError::ParseFailed)
        );
    }

    // ===== T20: 未知时段 → ParseFailed =====
    #[test]
    fn t20_parse_price_point_unknown_period() {
        assert_eq!(
            parse_price_point("P,1000,0.85,unknown"),
            Err(MarketError::ParseFailed)
        );
    }

    // ===== T21: 解析 DR 信号 =====
    #[test]
    fn t21_parse_dr_signal_ok() {
        let s = parse_dr_signal("D,42,2.5,1000,2000,500.0").unwrap();
        assert_eq!(s.event_id, 42);
        assert!((s.target_mw - 2.5).abs() < f32::EPSILON);
        assert_eq!(s.start, 1000);
        assert_eq!(s.end, 2000);
        assert!((s.reward - 500.0).abs() < f32::EPSILON);
    }

    // ===== T22: DR 信号前缀非 D → ParseFailed =====
    #[test]
    fn t22_parse_dr_signal_wrong_prefix() {
        assert_eq!(
            parse_dr_signal("P,42,2.5,1000,2000,500.0"),
            Err(MarketError::ParseFailed)
        );
    }

    // ===== T23: DR 信号数字解析失败 / 字段不足 → ParseFailed =====
    #[test]
    fn t23_parse_dr_signal_failures() {
        assert_eq!(
            parse_dr_signal("D,abc,2.5,1000,2000,500.0"),
            Err(MarketError::ParseFailed)
        );
        assert_eq!(
            parse_dr_signal("D,42,2.5,1000"),
            Err(MarketError::ParseFailed)
        );
    }

    // ===== T24: parse_feed 混合输入（合法/非法/空行）=====
    #[test]
    fn t24_parse_feed_mixed_input() {
        let input = "P,1000,0.85,peak\n\
                     P,2000,0.50,flat\n\
                     D,42,2.5,1000,2000,500.0\n\
                     X,foo\n\
                     \n";
        let feed = parse_feed(input, MarketType::AncillaryService, 12345);
        assert_eq!(feed.prices.len(), 2);
        assert_eq!(feed.dr_signals.len(), 1);
        assert_eq!(feed.market_type, MarketType::AncillaryService);
        assert_eq!(feed.timestamp, 12345);
    }

    // ===== T25: parse_feed 空输入 → 空馈送，不 panic =====
    #[test]
    fn t25_parse_feed_empty_input() {
        let feed = parse_feed("", MarketType::Spot, 0);
        assert!(feed.prices.is_empty());
        assert!(feed.dr_signals.is_empty());
        assert_eq!(feed.market_type, MarketType::Spot);
        assert_eq!(feed.timestamp, 0);
    }

    // ===== T26: 字段含空白 → trim 生效，解析成功 =====
    #[test]
    fn t26_parse_feed_trimmed_fields() {
        let feed = parse_feed("P, 1000 , 0.85 , peak", MarketType::Spot, 5);
        assert_eq!(feed.prices.len(), 1);
        let p = feed.prices[0];
        assert_eq!(p.time, 1000);
        assert!((p.price - 0.85).abs() < f32::EPSILON);
        assert_eq!(p.period, Period::Peak);
        assert_eq!(feed.timestamp, 5);
    }
}
