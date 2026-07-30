//! P2P 交易协议验证与匹配逻辑

use crate::types::{P2pAsk, P2pBid};
use crate::P2pError;

/// 验证卖方报价合法性
///
/// - volume_kwh > 0
/// - price >= 0
/// - 未过期 (now < valid_until)
pub fn validate_bid(bid: &P2pBid, now: u64) -> Result<(), P2pError> {
    if bid.volume_kwh <= 0.0 {
        return Err(P2pError::InvalidBid);
    }
    if bid.price < 0.0 {
        return Err(P2pError::InvalidBid);
    }
    if now >= bid.valid_until {
        return Err(P2pError::Expired);
    }
    Ok(())
}

/// 验证买方询价合法性
///
/// - volume_kwh > 0
/// - max_price >= 0
/// - 未过期 (now < valid_until)
pub fn validate_ask(ask: &P2pAsk, now: u64) -> Result<(), P2pError> {
    if ask.volume_kwh <= 0.0 {
        return Err(P2pError::InvalidAsk);
    }
    if ask.max_price < 0.0 {
        return Err(P2pError::InvalidAsk);
    }
    if now >= ask.valid_until {
        return Err(P2pError::Expired);
    }
    Ok(())
}

/// 判断报价与询价是否可撮合
///
/// 当卖方报价 <= 买方最高接受价时可成交。
pub fn can_match(bid: &P2pBid, ask: &P2pAsk) -> bool {
    bid.price <= ask.max_price
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bid() -> P2pBid {
        P2pBid {
            agent: 1,
            volume_kwh: 100.0,
            price: 0.65,
            valid_until: 10_000,
            seq: 1,
        }
    }

    fn sample_ask() -> P2pAsk {
        P2pAsk {
            agent: 2,
            volume_kwh: 50.0,
            max_price: 0.80,
            valid_until: 10_000,
            seq: 1,
        }
    }

    #[test]
    fn test_validate_bid_ok() {
        assert!(validate_bid(&sample_bid(), 5000).is_ok());
    }

    #[test]
    fn test_validate_bid_zero_volume() {
        let mut bid = sample_bid();
        bid.volume_kwh = 0.0;
        assert_eq!(validate_bid(&bid, 5000), Err(P2pError::InvalidBid));
    }

    #[test]
    fn test_validate_bid_negative_price() {
        let mut bid = sample_bid();
        bid.price = -0.1;
        assert_eq!(validate_bid(&bid, 5000), Err(P2pError::InvalidBid));
    }

    #[test]
    fn test_validate_bid_expired() {
        assert_eq!(validate_bid(&sample_bid(), 10_000), Err(P2pError::Expired));
        assert_eq!(validate_bid(&sample_bid(), 20_000), Err(P2pError::Expired));
    }

    #[test]
    fn test_validate_ask_ok() {
        assert!(validate_ask(&sample_ask(), 5000).is_ok());
    }

    #[test]
    fn test_validate_ask_zero_volume() {
        let mut ask = sample_ask();
        ask.volume_kwh = 0.0;
        assert_eq!(validate_ask(&ask, 5000), Err(P2pError::InvalidAsk));
    }

    #[test]
    fn test_validate_ask_negative_max_price() {
        let mut ask = sample_ask();
        ask.max_price = -1.0;
        assert_eq!(validate_ask(&ask, 5000), Err(P2pError::InvalidAsk));
    }

    #[test]
    fn test_validate_ask_expired() {
        assert_eq!(validate_ask(&sample_ask(), 10_000), Err(P2pError::Expired));
    }

    #[test]
    fn test_can_match_true() {
        assert!(can_match(&sample_bid(), &sample_ask()));
    }

    #[test]
    fn test_can_match_equal_price() {
        let mut ask = sample_ask();
        ask.max_price = 0.65;
        assert!(can_match(&sample_bid(), &ask));
    }

    #[test]
    fn test_can_match_false() {
        let mut bid = sample_bid();
        bid.price = 0.90;
        assert!(!can_match(&bid, &sample_ask()));
    }
}
