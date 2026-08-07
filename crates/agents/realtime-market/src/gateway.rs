//! 实时市场出清网关（v0.229.0，B3-06）。
//!
//! `RealtimeLoop::mock_clearing` 已收编 cfg(test)；生产出清经本模块：
//!
//! - [`RealtimeClearingGateway`] — 出清获取抽象；
//! - [`RealtimeProtocolClient`] — 协议客户端（传输闭包注入）；
//! - [`realtime_contract_test`] — 契约测试。
//!
//! 请求帧：`b"R" + session_id(8B LE) + seq(8B LE)`；
//! 响应帧：`b"P" + price(8B LE f64) + volume(8B LE f64) + cleared_at(8B LE)`。

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{RealtimeClearing, YuanPerMwh};
use crate::RealtimeError;

/// 实时出清网关抽象。
pub trait RealtimeClearingGateway {
    /// 查询指定会话+序号的出清结果。
    fn fetch_clearing(
        &mut self,
        session_id: u64,
        seq: u64,
    ) -> Result<RealtimeClearing, RealtimeError>;
}

/// 协议出清客户端。
pub struct RealtimeProtocolClient<T> {
    /// 传输闭包。
    transport: T,
    /// 交易中心地址（诊断）。
    #[allow(dead_code)]
    gateway: String,
}

impl<T> RealtimeProtocolClient<T>
where
    T: FnMut(&[u8]) -> Result<Vec<u8>, RealtimeError>,
{
    /// 创建客户端。
    pub fn new(transport: T, gateway: &str) -> Self {
        Self {
            transport,
            gateway: String::from(gateway),
        }
    }

    /// 编码查询请求。
    pub fn encode_query(session_id: u64, seq: u64) -> Vec<u8> {
        let mut req = Vec::with_capacity(17);
        req.push(b'R');
        req.extend_from_slice(&session_id.to_le_bytes());
        req.extend_from_slice(&seq.to_le_bytes());
        req
    }

    /// 解析出清响应。
    pub fn decode_response(
        session_id: u64,
        seq: u64,
        resp: &[u8],
    ) -> Result<RealtimeClearing, RealtimeError> {
        if resp.len() < 1 + 8 + 8 + 8 || resp[0] != b'P' {
            return Err(RealtimeError::ClearingNotFound);
        }
        let price = f64::from_le_bytes(
            resp[1..9]
                .try_into()
                .map_err(|_| RealtimeError::ClearingNotFound)?,
        );
        let volume = f64::from_le_bytes(
            resp[9..17]
                .try_into()
                .map_err(|_| RealtimeError::ClearingNotFound)?,
        );
        let cleared_at = u64::from_le_bytes(
            resp[17..25]
                .try_into()
                .map_err(|_| RealtimeError::ClearingNotFound)?,
        );
        Ok(RealtimeClearing {
            session_id,
            seq,
            clearing_price: YuanPerMwh(price),
            cleared_volume: volume,
            cleared_at,
        })
    }
}

impl<T> RealtimeClearingGateway for RealtimeProtocolClient<T>
where
    T: FnMut(&[u8]) -> Result<Vec<u8>, RealtimeError>,
{
    fn fetch_clearing(
        &mut self,
        session_id: u64,
        seq: u64,
    ) -> Result<RealtimeClearing, RealtimeError> {
        let req = Self::encode_query(session_id, seq);
        let resp = (self.transport)(&req)?;
        Self::decode_response(session_id, seq, &resp)
    }
}

/// 实时市场契约测试。
pub fn realtime_contract_test<G: RealtimeClearingGateway>(
    gateway: &mut G,
    session_id: u64,
    seq: u64,
) -> Result<RealtimeClearing, RealtimeError> {
    gateway.fetch_clearing(session_id, seq)
}

// ============================================================================
// Tests（契约测试）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_query_layout() {
        let req =
            RealtimeProtocolClient::<fn(&[u8]) -> Result<Vec<u8>, RealtimeError>>::encode_query(
                3, 9,
            );
        assert_eq!(req[0], b'R');
        assert_eq!(&req[1..9], &3u64.to_le_bytes());
        assert_eq!(&req[9..17], &9u64.to_le_bytes());
    }

    #[test]
    fn test_decode_response_roundtrip() {
        let mut resp = vec![b'P'];
        resp.extend_from_slice(&150.0f64.to_le_bytes());
        resp.extend_from_slice(&100.0f64.to_le_bytes());
        resp.extend_from_slice(&2000u64.to_le_bytes());
        let r =
            RealtimeProtocolClient::<fn(&[u8]) -> Result<Vec<u8>, RealtimeError>>::decode_response(
                3, 9, &resp,
            )
            .expect("decode");
        assert_eq!(r.session_id, 3);
        assert_eq!(r.seq, 9);
        assert_eq!(r.clearing_price.0, 150.0);
        assert_eq!(r.cleared_volume, 100.0);
        assert_eq!(r.cleared_at, 2000);
    }

    #[test]
    fn test_decode_response_truncated_rejected() {
        let resp = vec![b'P', 0x01];
        let r =
            RealtimeProtocolClient::<fn(&[u8]) -> Result<Vec<u8>, RealtimeError>>::decode_response(
                1, 1, &resp,
            );
        assert!(r.is_err());
    }

    #[test]
    fn test_gateway_fetch_end_to_end() {
        let mut client = RealtimeProtocolClient::new(
            |req: &[u8]| {
                assert_eq!(req[0], b'R');
                let sid = u64::from_le_bytes(req[1..9].try_into().unwrap());
                let seq = u64::from_le_bytes(req[9..17].try_into().unwrap());
                let mut resp = vec![b'P'];
                resp.extend_from_slice(&(sid as f64).to_le_bytes());
                resp.extend_from_slice(&(seq as f64).to_le_bytes());
                resp.extend_from_slice(&3000u64.to_le_bytes());
                Ok(resp)
            },
            "exchange:8802",
        );
        let r = realtime_contract_test(&mut client, 5, 2).expect("contract");
        assert_eq!(r.session_id, 5);
        assert_eq!(r.seq, 2);
        assert_eq!(r.cleared_at, 3000);
    }

    #[test]
    fn test_gateway_transport_error_propagates() {
        let mut client = RealtimeProtocolClient::new(
            |_req: &[u8]| Err(RealtimeError::ClearingNotFound),
            "exchange:8802",
        );
        assert!(realtime_contract_test(&mut client, 1, 1).is_err());
    }
}
