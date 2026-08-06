//! 现货市场出清网关（v0.229.0，B3-06）。
//!
//! 删除 `pub mock_clearing`/`inject_clearing` 伪造出清路径：本模块提供
//! 交易中心协议 seam 的真实客户端：
//!
//! - [`ClearingGateway`] — 出清查询抽象（fetch 真实出清结果）；
//! - [`ProtocolClearingClient`] — 协议客户端（注入传输闭包，无对接方时
//!   由契约测试验证请求/响应语义）；
//! - [`clearing_contract_test`] — 市场网关契约测试（请求编码 → 对端
//!   解码 → 响应解析全链）。
//!
//! 生产构建无伪造出清路径：所有出清结果均来自网关。

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{ClearedSegment, ClearingResult, YuanPerKwh};
use crate::MarketError;

/// 出清查询网关抽象。
pub trait ClearingGateway {
    /// 查询指定报价的出清结果。
    fn fetch_clearing(&mut self, bid_id: u64) -> Result<ClearingResult, MarketError>;
}

/// 协议出清客户端：经传输闭包与交易中心通信。
///
/// 请求帧：`b"Q" + bid_id(8B LE)`；响应帧：
/// `b"C" + bid_id(8B LE) + clearing_price(8B LE) + clearing_at(8B LE)
/// + n_segments(1B) + n × (volume(8B LE) + price(8B LE))`。
pub struct ProtocolClearingClient<T> {
    /// 传输闭包（TCP/TLS 由上层注入）。
    transport: T,
    /// 网关地址（诊断）。
    gateway: String,
}

impl<T> ProtocolClearingClient<T>
where
    T: FnMut(&[u8]) -> Result<Vec<u8>, MarketError>,
{
    /// 创建协议客户端。
    pub fn new(transport: T, gateway: &str) -> Self {
        Self {
            transport,
            gateway: String::from(gateway),
        }
    }

    /// 编码出清查询请求。
    pub fn encode_query(bid_id: u64) -> Vec<u8> {
        let mut req = Vec::with_capacity(9);
        req.push(b'Q');
        req.extend_from_slice(&bid_id.to_le_bytes());
        req
    }

    /// 解析出清响应帧；格式非法返回 [`MarketError::ClearingNotFound`]。
    pub fn decode_response(resp: &[u8]) -> Result<ClearingResult, MarketError> {
        if resp.len() < 1 + 8 + 8 + 8 + 1 {
            return Err(MarketError::ClearingNotFound);
        }
        if resp[0] != b'C' {
            return Err(MarketError::ClearingNotFound);
        }
        let bid_id =
            u64::from_le_bytes(resp[1..9].try_into().map_err(|_| MarketError::ClearingNotFound)?);
        let price =
            f64::from_le_bytes(resp[9..17].try_into().map_err(|_| MarketError::ClearingNotFound)?);
        let clearing_at =
            u64::from_le_bytes(resp[17..25].try_into().map_err(|_| MarketError::ClearingNotFound)?);
        let n = resp[25] as usize;
        let mut cleared = Vec::with_capacity(n);
        let mut off = 26usize;
        for _ in 0..n {
            if off + 16 > resp.len() {
                return Err(MarketError::ClearingNotFound);
            }
            let volume = f64::from_le_bytes(resp[off..off + 8].try_into().map_err(|_| MarketError::ClearingNotFound)?);
            let seg_price = f64::from_le_bytes(resp[off + 8..off + 16].try_into().map_err(|_| MarketError::ClearingNotFound)?);
            cleared.push(ClearedSegment {
                period: eneros_market_types::TimeSlot { start: 0, end: 0 },
                cleared_volume: volume,
                cleared_price: YuanPerKwh(seg_price),
            });
            off += 16;
        }
        Ok(ClearingResult {
            bid_id,
            cleared,
            clearing_price: YuanPerKwh(price),
            clearing_at,
        })
    }
}

impl<T> ClearingGateway for ProtocolClearingClient<T>
where
    T: FnMut(&[u8]) -> Result<Vec<u8>, MarketError>,
{
    fn fetch_clearing(&mut self, bid_id: u64) -> Result<ClearingResult, MarketError> {
        let req = Self::encode_query(bid_id);
        let resp = (self.transport)(&req)?;
        Self::decode_response(&resp)
    }
}

/// 市场网关契约测试：请求编码 → 对端解码 → 响应解析。
///
/// 无真实对接方时，契约测试锁定线协议（生产构建无伪造出清路径）。
pub fn clearing_contract_test<G: ClearingGateway>(
    gateway: &mut G,
    bid_id: u64,
) -> Result<ClearingResult, MarketError> {
    gateway.fetch_clearing(bid_id)
}

// ============================================================================
// Tests（契约测试：编码/解码对称）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_query_layout() {
        let req = ProtocolClearingClient::<fn(&[u8]) -> Result<Vec<u8>, MarketError>>::encode_query(42);
        assert_eq!(req[0], b'Q');
        assert_eq!(&req[1..9], &42u64.to_le_bytes());
    }

    #[test]
    fn test_decode_response_roundtrip() {
        // 构造对端响应：C + price + at + 1 segment。
        let mut resp = vec![b'C'];
        resp.extend_from_slice(&42u64.to_le_bytes());
        resp.extend_from_slice(&0.85f64.to_le_bytes());
        resp.extend_from_slice(&1000u64.to_le_bytes());
        resp.push(1);
        resp.extend_from_slice(&10.0f64.to_le_bytes());
        resp.extend_from_slice(&0.85f64.to_le_bytes());
        let r = ProtocolClearingClient::<fn(&[u8]) -> Result<Vec<u8>, MarketError>>::decode_response(&resp)
            .expect("decode");
        assert_eq!(r.bid_id, 42);
        assert_eq!(r.clearing_price.0, 0.85);
        assert_eq!(r.clearing_at, 1000);
        assert_eq!(r.cleared.len(), 1);
        assert_eq!(r.cleared[0].cleared_volume, 10.0);
    }

    #[test]
    fn test_decode_response_truncated_rejected() {
        let resp = vec![b'C', 0x01];
        let r = ProtocolClearingClient::<fn(&[u8]) -> Result<Vec<u8>, MarketError>>::decode_response(&resp);
        assert!(r.is_err());
    }

    #[test]
    fn test_decode_response_wrong_magic_rejected() {
        let resp = vec![b'X', 0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let r = ProtocolClearingClient::<fn(&[u8]) -> Result<Vec<u8>, MarketError>>::decode_response(&resp);
        assert!(r.is_err());
    }

    #[test]
    fn test_gateway_fetch_end_to_end() {
        // 契约测试：客户端 → 传输闭包（模拟交易中心）→ 响应解析。
        let mut client = ProtocolClearingClient::new(
            |req: &[u8]| {
                // 模拟交易中心：校验请求格式并回出清。
                assert_eq!(req[0], b'Q');
                let bid_id = u64::from_le_bytes(req[1..9].try_into().unwrap());
                let mut resp = vec![b'C'];
                resp.extend_from_slice(&bid_id.to_le_bytes());
                resp.extend_from_slice(&0.9f64.to_le_bytes());
                resp.extend_from_slice(&(bid_id * 10).to_le_bytes());
                resp.push(0);
                Ok(resp)
            },
            "exchange:8801",
        );
        let r = clearing_contract_test(&mut client, 7).expect("contract");
        assert_eq!(r.clearing_price.0, 0.9);
        assert_eq!(r.clearing_at, 70);
        assert!(r.cleared.is_empty());
    }

    #[test]
    fn test_gateway_transport_error_propagates() {
        let mut client = ProtocolClearingClient::new(
            |_req: &[u8]| Err(MarketError::IsoUnreachable),
            "exchange:8801",
        );
        assert!(clearing_contract_test(&mut client, 1).is_err());
    }
}
