//! OTA1 探针 wire（agent-sdk 共享模块，蓝图 58 C3）。
//!
//! 单一事实：root-task 探针（storage_persist_probe / ota_download_probe）
//! 的手写请求构造 + 响应解析，与 svc-ota handler 的手写请求解析曾各自实现
//! 同一布局——漂移只能在 QEMU 日志现形。本模块收归一处，root 与 svc-ota
//! 共同消费（WP-45.5 sample_wire 先例疗法）。
//!
//! 布局（小端；magic = `OTA1`）：
//! - 0x06 PERSIST-TRIAL：req = magic‖0x06‖remaining u32‖version 16B；
//!   resp = gen u64 或 `ERR:...`
//! - 0x07 LOAD-TRIAL：req = magic‖0x07；
//!   resp = found u8(=1)‖remaining u32‖version 16B 或 `ERR:...`
//! - 0x02 DOWNLOAD：req = magic‖0x02‖target 32B‖sku 16B‖secver u32‖
//!   image_len u64‖hash 32B‖sig‖chunk_size u32‖image_len u32‖image；
//!   resp = 子码回显 u8(=2) 或 `ERR:...`
//! - 0x03 TRIAL：req = magic‖0x03‖flag u8‖version 32B；resp 回显 3
//! - 0x04 COMMIT：req = magic‖0x04；resp 回显 4
//!
//! version 字段为定长 NUL 填充 UTF-8（截断到字段宽）。

use alloc::string::String;
use alloc::vec::Vec;

/// wire magic。
pub const MAGIC: [u8; 4] = *b"OTA1";
/// PERSIST-TRIAL 子码。
pub const SUB_PERSIST_TRIAL: u8 = 0x06;
/// LOAD-TRIAL 子码。
pub const SUB_LOAD_TRIAL: u8 = 0x07;
/// DOWNLOAD 子码。
pub const SUB_DOWNLOAD: u8 = 0x02;
/// TRIAL 子码。
pub const SUB_TRIAL: u8 = 0x03;
/// COMMIT 子码。
pub const SUB_COMMIT: u8 = 0x04;

/// version 字段宽（persist/load 用短版）。
pub const VERSION_LEN_SHORT: usize = 16;
/// version 字段宽（download/trial 用长版）。
pub const VERSION_LEN_LONG: usize = 32;
/// sku 字段宽。
pub const SKU_LEN: usize = 16;
/// 服务标识字段宽。
pub const SERVICE_LEN: usize = 16;

/// 解析失败类型（`ERR:` 前缀语义随迁——41 卷实弹教训：
/// 不检查前缀会把错误串前 8 字节解成数字打印假成功行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtaWireError {
    /// 服务侧拒绝响应（`ERR:` 前缀，载荷为可读消息）。
    Rejected(String),
    /// 响应短于布局要求。
    Short(usize),
    /// 子码回显不符。
    UnexpectedSubcode(u8),
    /// found 标志非 1（trial 未找到）。
    NotFound,
    /// 定长字符串含非 UTF-8。
    BadUtf8,
}

/// NUL 填充定长字段编码（超长截断）。
fn fill_field(buf: &mut Vec<u8>, value: &[u8], width: usize) {
    let n = core::cmp::min(value.len(), width);
    buf.extend_from_slice(&value[..n]);
    buf.resize(buf.len() + (width - n), 0);
}

/// 定长字段解码为去 NUL 字符串。
fn field_to_string(raw: &[u8]) -> Result<String, OtaWireError> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    core::str::from_utf8(&raw[..end])
        .map(alloc::string::String::from)
        .map_err(|_| OtaWireError::BadUtf8)
}

/// 公共响应检查：`ERR:` 前缀 → Rejected。
fn check_err_prefix(resp: &[u8]) -> Result<(), OtaWireError> {
    if resp.starts_with(b"ERR:") {
        return Err(OtaWireError::Rejected(alloc::string::String::from(
            core::str::from_utf8(resp).unwrap_or("<bad-utf8>"),
        )));
    }
    Ok(())
}

/// 0x06 PERSIST-TRIAL 请求：remaining ‖ version(16)。
pub fn encode_persist_trial(remaining: u32, version: &str) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_PERSIST_TRIAL);
    req.extend_from_slice(&remaining.to_le_bytes());
    fill_field(&mut req, version.as_bytes(), VERSION_LEN_SHORT);
    req
}

/// 服务侧解析 0x06 请求 → `(remaining, version)`。
pub fn parse_persist_trial_req(req: &[u8]) -> Result<(u32, String), OtaWireError> {
    // 布局：magic(4)+sub(1)+remaining(4)+version(16)。
    if req.len() != 5 + 4 + VERSION_LEN_SHORT || req[..4] != MAGIC || req[4] != SUB_PERSIST_TRIAL {
        return Err(OtaWireError::Short(req.len()));
    }
    let p = &req[5..];
    let remaining = u32::from_le_bytes(p[0..4].try_into().expect("u32"));
    let version = field_to_string(&p[4..20])?;
    Ok((remaining, version))
}

/// 0x06 响应解析 → 持久化代次。
pub fn parse_persist_resp(resp: &[u8]) -> Result<u64, OtaWireError> {
    check_err_prefix(resp)?;
    if resp.len() < 8 {
        return Err(OtaWireError::Short(resp.len()));
    }
    Ok(u64::from_le_bytes(resp[..8].try_into().expect("u64")))
}

/// 0x07 LOAD-TRIAL 请求。
pub fn encode_load_trial() -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_LOAD_TRIAL);
    req
}

/// 0x07 响应解析 → `(remaining, version)`（found != 1 → NotFound）。
pub fn parse_load_resp(resp: &[u8]) -> Result<(u32, String), OtaWireError> {
    check_err_prefix(resp)?;
    if resp.len() < 1 + 4 {
        return Err(OtaWireError::Short(resp.len()));
    }
    if resp[0] != 1 {
        return Err(OtaWireError::NotFound);
    }
    let remaining = u32::from_le_bytes(resp[1..5].try_into().expect("u32"));
    let version = field_to_string(&resp[5..])?;
    Ok((remaining, version))
}

/// 0x02 DOWNLOAD 请求（hash 由调用方计算注入——codec 保持无加密依赖）。
pub fn encode_download(
    target_version: &[u8],
    sku: &[u8],
    secver: u32,
    image: &[u8],
    hash: &[u8; 32],
    sig: &[u8],
    chunk_size: u32,
) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_DOWNLOAD);
    fill_field(&mut req, target_version, VERSION_LEN_LONG);
    fill_field(&mut req, sku, SKU_LEN);
    req.extend_from_slice(&secver.to_le_bytes());
    req.extend_from_slice(&(image.len() as u64).to_le_bytes());
    req.extend_from_slice(hash);
    req.extend_from_slice(sig);
    req.extend_from_slice(&chunk_size.to_le_bytes());
    req.extend_from_slice(&(image.len() as u32).to_le_bytes());
    req.extend_from_slice(image);
    req
}

/// 子码回显响应校验（0x02/0x03/0x04 通用）。
pub fn expect_subcode_resp(resp: &[u8], subcode: u8) -> Result<(), OtaWireError> {
    check_err_prefix(resp)?;
    if resp.is_empty() {
        return Err(OtaWireError::Short(0));
    }
    if resp[0] != subcode {
        return Err(OtaWireError::UnexpectedSubcode(resp[0]));
    }
    Ok(())
}

/// 0x03 TRIAL 请求：flag ‖ version(32)。
pub fn encode_trial(flag: u8, version: &[u8]) -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_TRIAL);
    req.push(flag);
    fill_field(&mut req, version, VERSION_LEN_LONG);
    req
}

/// 0x04 COMMIT 请求。
pub fn encode_commit() -> Vec<u8> {
    let mut req = Vec::from(MAGIC);
    req.push(SUB_COMMIT);
    req
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_round_trip_both_sides_one_type() {
        // root 编码 → 服务侧解析（同一模块 = 布局不可能漂移）。
        let req = encode_persist_trial(2, "1.2.0");
        assert_eq!(req.len(), 5 + 20);
        let (remaining, version) = parse_persist_trial_req(&req).expect("parse");
        assert_eq!(remaining, 2);
        assert_eq!(version, "1.2.0");
    }

    #[test]
    fn persist_resp_gen_and_error_paths() {
        assert_eq!(parse_persist_resp(&7u64.to_le_bytes()).expect("gen"), 7);
        assert_eq!(
            parse_persist_resp(b"ERR:store-unavailable"),
            Err(OtaWireError::Rejected(String::from(
                "ERR:store-unavailable"
            )))
        );
        assert_eq!(parse_persist_resp(&[1, 2]), Err(OtaWireError::Short(2)));
    }

    #[test]
    fn load_round_trip_not_found_and_short() {
        let mut resp = vec![1u8];
        resp.extend_from_slice(&2u32.to_le_bytes());
        let mut v = vec![b'1', b'.', b'2', b'.', b'0'];
        v.resize(16, 0);
        resp.extend_from_slice(&v);
        assert_eq!(
            parse_load_resp(&resp).expect("load"),
            (2, String::from("1.2.0"))
        );
        assert_eq!(
            parse_load_resp(&[0, 0, 0, 0, 0]),
            Err(OtaWireError::NotFound)
        );
        assert_eq!(
            parse_load_resp(b"ERR:x"),
            Err(OtaWireError::Rejected(String::from("ERR:x")))
        );
    }

    #[test]
    fn download_request_carries_all_fields_in_order() {
        let hash = [0xAB; 32];
        let sig = [0xCD; 64];
        let image = [0u8; 512];
        let req = encode_download(b"0.123.0", b"qemu", 1, &image, &hash, &sig, 512);
        // 布局钉扎：magic+sub+32+16+4+8+32+64+4+4+image
        let fixed = 5 + 32 + 16 + 4 + 8 + 32 + 64 + 4 + 4;
        assert_eq!(req.len(), fixed + image.len());
        assert_eq!(&req[..5], b"OTA1\x02");
        assert_eq!(&req[53..57], &1u32.to_le_bytes()); // secver（5+32+16 后）
    }

    #[test]
    fn subcode_echo_validation() {
        assert_eq!(expect_subcode_resp(&[3], 3), Ok(()));
        assert_eq!(
            expect_subcode_resp(&[9], 3),
            Err(OtaWireError::UnexpectedSubcode(9))
        );
        assert_eq!(expect_subcode_resp(&[], 3), Err(OtaWireError::Short(0)));
    }

    #[test]
    fn trial_and_commit_requests_shape() {
        let t = encode_trial(1, b"0.123.0");
        assert_eq!(&t[..6], b"OTA1\x03\x01");
        assert_eq!(t.len(), 5 + 1 + 32);
        assert_eq!(encode_commit(), Vec::from(&b"OTA1\x04"[..]));
    }
}
