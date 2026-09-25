//! 账本服务 0x01 POST 载荷 codec（WP-75.2.6 自 bin 手抄下沉；层位
//! 声明同 [`crate::svc_wire`]——本模块只裁**载荷层**）。
//!
//! 不变式：先总长门后切片（64B 定长载荷；`try_into().unwrap()` 不越界）。

#[cfg(test)]
mod tests {
    use super::*;

    /// 合法 0x01 POST 请求（全字段独特值）。
    fn valid_post_req() -> alloc::vec::Vec<u8> {
        let mut p = alloc::vec![0u8; 64];
        p[0..8].copy_from_slice(&0x99u64.to_le_bytes());
        p[8..12].copy_from_slice(&1u32.to_le_bytes());
        p[12..16].copy_from_slice(&2u32.to_le_bytes());
        p[16..24].copy_from_slice(&(-500i64).to_le_bytes());
        p[24..32].copy_from_slice(&555u64.to_le_bytes());
        p[32..64].copy_from_slice(&[0xAB; 32]);
        let mut req = alloc::vec::Vec::from(&b"LDG1"[..]);
        req.push(0x01);
        req.extend_from_slice(&p);
        req
    }

    /// golden 正向：全字段逐项断言。
    #[test]
    fn golden_post_req_roundtrip_fields() {
        let r = parse_post_req(&valid_post_req()).expect("合法请求");
        assert_eq!(r.0, crate::ledger::TransactionId(0x99));
        assert_eq!(r.1, crate::ledger::AccountId(1));
        assert_eq!(r.2, crate::ledger::AccountId(2));
        assert_eq!(r.3, eneros_market_types::MilliYuan(-500));
        assert_eq!(r.4, 555);
        assert_eq!(r.5, crate::ledger::ReceiptDigest([0xAB; 32]));
    }

    /// 负例族：长度 63/65 → ERR_LEN。
    #[test]
    fn negative_lengths_rejected() {
        for bad in [63usize, 65] {
            let mut req = valid_post_req();
            req.truncate(5 + bad);
            req.resize(5 + bad, 0);
            assert_eq!(parse_post_req(&req), Err(crate::svc_wire::ERR_LEN));
        }
    }
}

/// 0x01 POST 载荷长度（bin 手抄门值保形：`req.len() != 5 + 64`）。
pub const POST_BODY: usize = 64;

/// POST 请求字段组（bin 手抄顺序保形）。
pub type PostFields = (
    crate::ledger::TransactionId,
    crate::ledger::AccountId,
    crate::ledger::AccountId,
    eneros_market_types::MilliYuan,
    u64,
    crate::ledger::ReceiptDigest,
);

/// 解析 0x01 POST 请求（`req` = 完整信封；信封层由调用方先经
/// [`crate::svc_wire::parse_envelope`] 判定）。总长门错误 =
/// [`crate::svc_wire::ERR_LEN`]。
pub fn parse_post_req(req: &[u8]) -> Result<PostFields, &'static [u8]> {
    if req.len() != 5 + POST_BODY || req[4] != 0x01 {
        return Err(crate::svc_wire::ERR_LEN);
    }
    let p = &req[5..];
    Ok((
        crate::ledger::TransactionId(u64::from_le_bytes(p[0..8].try_into().unwrap())),
        crate::ledger::AccountId(u32::from_le_bytes(p[8..12].try_into().unwrap())),
        crate::ledger::AccountId(u32::from_le_bytes(p[12..16].try_into().unwrap())),
        eneros_market_types::MilliYuan(i64::from_le_bytes(p[16..24].try_into().unwrap())),
        u64::from_le_bytes(p[24..32].try_into().unwrap()),
        crate::ledger::ReceiptDigest(p[32..64].try_into().unwrap()),
    ))
}
