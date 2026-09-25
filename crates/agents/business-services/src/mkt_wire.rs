//! 市场服务 0x01 SUBMIT 载荷 codec（WP-75.2.6 自 bin 手抄下沉；层位
//! 声明同 [`crate::svc_wire`]——本模块只裁**载荷层**：信封层归
//! svc_wire，opcode 分派与业务执行归 bin）。
//!
//! 不变式：先总长门后切片（93B 定长载荷——bin 现状依赖长度门前置，
//! 下沉后由本模块独立执法，`try_into().unwrap()` 不越界）；错误类别
//! 与序逐字保形：先总长门（ERR_LEN）后枚举域（ERR:market）。

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_market_types::MarketType;

    /// 合法 0x01 SUBMIT 请求（全字段独特值）。
    fn valid_submit_req() -> alloc::vec::Vec<u8> {
        let mut p = alloc::vec![0u8; 93];
        p[0..8].copy_from_slice(&0x0102030405060708u64.to_le_bytes());
        p[8..24].copy_from_slice(&[0x11; 16]);
        p[24..32].copy_from_slice(&12345u64.to_le_bytes());
        p[32] = 3; // Realtime
        p[33..37].copy_from_slice(&7u32.to_le_bytes());
        p[37..41].copy_from_slice(&9u32.to_le_bytes());
        p[41..49].copy_from_slice(&1000u64.to_le_bytes());
        p[49..57].copy_from_slice(&3600u64.to_le_bytes());
        p[57..65].copy_from_slice(&55u64.to_le_bytes());
        p[65..73].copy_from_slice(&350.5f64.to_le_bytes());
        p[73..81].copy_from_slice(&(-1250i64).to_le_bytes());
        let mut req = alloc::vec::Vec::from(&b"MKT1"[..]);
        req.push(0x01);
        req.extend_from_slice(&p);
        req
    }

    /// golden 正向：全字段逐项断言（偏移/枚举映射保形）。
    #[test]
    fn golden_submit_req_roundtrip_fields() {
        use eneros_market_types::MarketType;
        let req = valid_submit_req();
        let q = parse_submit_req(&req).expect("合法请求");
        assert_eq!(q.quote_ref.external_id, 0x0102030405060708);
        assert_eq!(q.quote_ref.idempotency.0, [0x11; 16]);
        assert_eq!(q.quote_ref.deadline_ms, 12345);
        assert_eq!(q.quote_ref.market, MarketType::Realtime);
        assert_eq!(q.quote_ref.tenant, 7);
        assert_eq!(q.quote_ref.product, 9);
        assert_eq!(q.slot.start, 1000);
        assert_eq!(q.slot.end, 3600);
        assert_eq!(q.volume_kwh, 55);
        assert!((q.price.0 - 350.5).abs() < 1e-9, "price 字段");
        assert_eq!(q.amount.0, -1250);
    }

    /// 枚举域 1-6 显式映射逐项。
    #[test]
    fn market_enum_domain_mapping() {
        for (b, want) in [
            (1u8, MarketType::DayAhead),
            (2, MarketType::IntraDay),
            (3, MarketType::Realtime),
            (4, MarketType::Spot),
            (5, MarketType::AncillaryService),
            (6, MarketType::DemandResponse),
        ] {
            let mut req = valid_submit_req();
            req[5 + 32] = b;
            let q = parse_submit_req(&req).expect("合法枚举");
            assert_eq!(q.quote_ref.market, want, "market={b}");
        }
    }

    /// 负例族：长度 92/94 → ERR_LEN。
    #[test]
    fn negative_lengths_rejected() {
        for bad in [92usize, 94] {
            let mut req = valid_submit_req();
            req.truncate(5 + bad);
            req.resize(5 + bad, 0);
            assert_eq!(parse_submit_req(&req), Err(crate::svc_wire::ERR_LEN));
        }
    }

    /// 负例族：market=0/7（越界拒，不放行）→ ERR:market。
    #[test]
    fn negative_market_domains_rejected() {
        for bad in [0u8, 7, 255] {
            let mut req = valid_submit_req();
            req[5 + 32] = bad;
            assert_eq!(parse_submit_req(&req), Err(b"ERR:market" as &[u8]));
        }
    }

    /// 错误序锁：长度门先于枚举域——92B 且 market=7 时得 ERR_LEN
    /// 而非 ERR:market（保现行序）。
    #[test]
    fn error_order_length_before_domain() {
        let mut req = valid_submit_req();
        req.truncate(5 + 92);
        req.resize(5 + 92, 0);
        req[5 + 32] = 7; // 越界枚举 + 短长并存 → 长度门先行
        assert_eq!(parse_submit_req(&req), Err(crate::svc_wire::ERR_LEN));
    }
}

/// 0x01 SUBMIT 载荷长度（bin 手抄门值保形：`req.len() != 5 + 93`）。
pub const SUBMIT_BODY: usize = 93;

/// 解析 0x01 SUBMIT 请求（`req` = 完整信封；信封层由调用方先经
/// [`crate::svc_wire::parse_envelope`] 判定）。
///
/// 错误序保形：先总长门（[`crate::svc_wire::ERR_LEN`]）后 market 枚举
/// 域（`ERR:market`，1-6 显式映射、越界拒不放行）。
pub fn parse_submit_req(req: &[u8]) -> Result<crate::market::QuoteMessage, &'static [u8]> {
    use eneros_durable_state::IdempotencyKey;
    use eneros_market_types::{MarketType, MilliYuan, TimeSlot, YuanPerMwh};

    if req.len() != 5 + SUBMIT_BODY || req[4] != 0x01 {
        return Err(crate::svc_wire::ERR_LEN);
    }
    let p = &req[5..];
    let market = match p[32] {
        1 => MarketType::DayAhead,
        2 => MarketType::IntraDay,
        3 => MarketType::Realtime,
        4 => MarketType::Spot,
        5 => MarketType::AncillaryService,
        6 => MarketType::DemandResponse,
        _ => return Err(b"ERR:market"),
    };
    Ok(crate::market::QuoteMessage {
        quote_ref: crate::market::QuoteRef {
            external_id: u64::from_le_bytes(p[0..8].try_into().unwrap()),
            idempotency: IdempotencyKey(p[8..24].try_into().unwrap()),
            deadline_ms: u64::from_le_bytes(p[24..32].try_into().unwrap()),
            market,
            tenant: u32::from_le_bytes(p[33..37].try_into().unwrap()),
            product: u32::from_le_bytes(p[37..41].try_into().unwrap()),
        },
        slot: TimeSlot {
            start: u64::from_le_bytes(p[41..49].try_into().unwrap()),
            end: u64::from_le_bytes(p[49..57].try_into().unwrap()),
        },
        volume_kwh: u64::from_le_bytes(p[57..65].try_into().unwrap()),
        price: YuanPerMwh(f64::from_le_bytes(p[65..73].try_into().unwrap())),
        amount: MilliYuan(i64::from_le_bytes(p[73..81].try_into().unwrap())),
    })
}
