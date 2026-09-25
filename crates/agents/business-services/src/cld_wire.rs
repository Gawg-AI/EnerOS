//! 云同步服务 0x01 UPLOAD 载荷 codec（WP-75.2.6 自 bin 手抄下沉；层位
//! 声明同 [`crate::svc_wire`]——本模块只裁**载荷层**）。
//!
//! 不变式：先总长门后切片；变长段钳制逻辑照搬 bin 现状
//! （`5 + 55 + plen > req.len()` → ERR_LEN）。

#[cfg(test)]
mod tests {
    use super::*;

    /// 合法 0x01 UPLOAD 请求（tenant/class/schema/seq/hash/payload）。
    fn valid_upload_req(plen: usize) -> alloc::vec::Vec<u8> {
        let mut p = alloc::vec![0u8; 55];
        p[0..4].copy_from_slice(&3u32.to_le_bytes());
        p[4] = 2; // data_class
        p[5..13].copy_from_slice(b"gridload");
        p[13..21].copy_from_slice(&42u64.to_le_bytes());
        p[21..53].copy_from_slice(&[0x77; 32]);
        p[53..55].copy_from_slice(&(plen as u16).to_le_bytes());
        let mut req = alloc::vec::Vec::from(&b"CLD1"[..]);
        req.push(0x01);
        req.extend_from_slice(&p);
        req.extend(core::iter::repeat(0xAEu8).take(plen));
        req
    }

    /// golden 正向：全字段逐项断言（含变长 payload）。
    #[test]
    fn golden_upload_req_roundtrip_fields() {
        let req = valid_upload_req(4);
        let item = parse_upload_req(&req).expect("合法请求");
        assert_eq!(item.tenant, 3);
        assert_eq!(item.data_class, 2);
        assert_eq!(item.schema, "gridload");
        assert_eq!(item.sequence, 42);
        assert_eq!(item.hash, [0x77; 32]);
        assert_eq!(item.payload, alloc::vec![0xAE; 4]);
    }

    /// 变长边界（bin 现状保形）：定长门 = 5+56（比 55B 头多 1B）——
    /// plen=0（总长 60 < 61）现行即拒；plen=1 为最小可接受；满幅接受。
    #[test]
    fn varlen_boundaries_bin_faithful() {
        assert_eq!(
            parse_upload_req(&valid_upload_req(0)),
            Err(crate::svc_wire::ERR_LEN),
            "plen=0 被 5+56 定长门拒绝（bin 现状保形）"
        );
        let min = parse_upload_req(&valid_upload_req(1)).expect("plen=1 最小可接受");
        assert_eq!(min.payload, alloc::vec![0xAE]);
        let big = parse_upload_req(&valid_upload_req(300)).expect("plen=300");
        assert_eq!(big.payload.len(), 300);
    }

    /// 负例族：总长 < 5+56 → ERR_LEN；plen 超出实际载荷 → ERR_LEN。
    #[test]
    fn negative_lengths_and_clamp_rejected() {
        let mut short = valid_upload_req(4);
        short.truncate(5 + 55); // 差 1B（不足定长头部）
        assert_eq!(parse_upload_req(&short), Err(crate::svc_wire::ERR_LEN));
        // plen 声称 300，实际载荷 0 → 钳制门拒绝。
        let mut p = alloc::vec![0u8; 55];
        p[53..55].copy_from_slice(&300u16.to_le_bytes());
        let mut req = alloc::vec::Vec::from(&b"CLD1"[..]);
        req.push(0x01);
        req.extend_from_slice(&p);
        assert_eq!(parse_upload_req(&req), Err(crate::svc_wire::ERR_LEN));
    }
}

/// 0x01 UPLOAD 定长头部（bin 手抄门值保形：`req.len() < 5 + 56` 拒）。
pub const UPLOAD_FIXED: usize = 56;

/// 解析 0x01 UPLOAD 请求（`req` = 完整信封；信封层由调用方先经
/// [`crate::svc_wire::parse_envelope`] 判定）。
///
/// 变长段钳制照搬 bin 现状：`plen` u16 在 p[53..55]，实际载荷不足时
/// （`5 + 55 + plen > req.len()`）→ [`crate::svc_wire::ERR_LEN`]。
pub fn parse_upload_req(req: &[u8]) -> Result<crate::cloud_sync::UploadItem, &'static [u8]> {
    if req.len() < 5 + UPLOAD_FIXED || req[4] != 0x01 {
        return Err(crate::svc_wire::ERR_LEN);
    }
    let p = &req[5..];
    let mut schema = [0u8; 8];
    schema.copy_from_slice(&p[5..13]);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&p[21..53]);
    let plen = u16::from_le_bytes(p[53..55].try_into().unwrap()) as usize;
    if 5 + 55 + plen > req.len() {
        return Err(crate::svc_wire::ERR_LEN);
    }
    Ok(crate::cloud_sync::UploadItem {
        tenant: u32::from_le_bytes(p[0..4].try_into().unwrap()),
        data_class: p[4],
        schema: alloc::string::String::from_utf8_lossy(&schema).into_owned(),
        sequence: u64::from_le_bytes(p[13..21].try_into().unwrap()),
        hash,
        payload: req[5 + 55..5 + 55 + plen].to_vec(),
    })
}
