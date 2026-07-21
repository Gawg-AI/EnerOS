//! PKI 证书解析器：DER/PEM 解码与编码 (v0.32.0 Task 4).
//!
//! 提供 Base64 自实现、DER/PEM 格式 X.509 证书的解析与编码。
//! 零外部依赖，纯 Rust + no_std。
//!
//! # 核心函数
//! - [`base64_decode`] / [`base64_encode`]：Base64 编解码（标准字母表，含 padding）
//! - [`parse_der`]：解析 DER 编码的 X.509 证书
//! - [`parse_pem`]：解析 PEM 编码的 X.509 证书
//! - [`to_der`]：将证书编码为 DER 字节
//! - [`to_pem`]：将证书编码为 PEM 字符串（64 字符分行）
//!
//! # no_std 合规
//! no_std 由 crate 根继承，本模块通过 `extern crate alloc` 引入堆分配。
//! 使用 `alloc::string::String` / `alloc::vec::Vec`，不使用 `std::*`。
//!
//! # 参考
//! - RFC 4648 Base64 Encoding
//! - RFC 7468 Textual Encodings of PKI Objects (PEM)
//! - RFC 5280 Internet X.509 Public Key Infrastructure Certificate and CRL Profile

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::pki::x509::X509Certificate;
use crate::pki::PkiError;

// ============================================================================
// Base64 查找表
// ============================================================================

/// Base64 标准编码字母表（A-Z a-z 0-9 + /）.
const ENCODE_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 解码查找表（256 字节，0xFF 表示非法字符）.
///
/// 使用 const fn 在编译期构建：
/// - `A`..`Z` → 0..25
/// - `a`..`z` → 26..51
/// - `0`..`9` → 52..61
/// - `+` → 62, `/` → 63
/// - 其余（含 `=` padding）→ 0xFF
const DECODE_TABLE: [u8; 256] = {
    let mut table = [0xFFu8; 256];
    let mut i: u8 = 0;
    while i < 26 {
        table[(b'A' + i) as usize] = i;
        i += 1;
    }
    let mut i: u8 = 0;
    while i < 26 {
        table[(b'a' + i) as usize] = 26 + i;
        i += 1;
    }
    let mut i: u8 = 0;
    while i < 10 {
        table[(b'0' + i) as usize] = 52 + i;
        i += 1;
    }
    table[b'+' as usize] = 62;
    table[b'/' as usize] = 63;
    table
};

// ============================================================================
// SubTask 4.1: base64_decode
// ============================================================================

/// Base64 解码（标准字母表，忽略空白字符）.
///
/// 输入含非法字符（不在字母表且非空白且非 `=`）返回 `PkiError::InvalidPemFormat`。
/// 空白字符（空格、`\r`、`\n`、`\t`）会被跳过。`=` 仅允许出现在每组的第 3、4 位。
pub fn base64_decode(input: &[u8]) -> Result<Vec<u8>, PkiError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    // 第一遍：过滤空白，验证字符合法性
    let mut filtered: Vec<u8> = Vec::with_capacity(input.len());
    for &b in input {
        if b == b' ' || b == b'\r' || b == b'\n' || b == b'\t' {
            continue;
        }
        if b == b'=' {
            filtered.push(b'=');
            continue;
        }
        if DECODE_TABLE[b as usize] == 0xFF {
            return Err(PkiError::InvalidPemFormat);
        }
        filtered.push(b);
    }

    // 长度（含 padding）必须是 4 的倍数
    if filtered.is_empty() {
        return Ok(Vec::new());
    }
    if filtered.len() % 4 != 0 {
        return Err(PkiError::InvalidPemFormat);
    }

    // 第二遍：按 4 字符一组解码
    let mut out: Vec<u8> = Vec::with_capacity(filtered.len() / 4 * 3);
    let mut i = 0;
    while i < filtered.len() {
        let c0 = filtered[i];
        let c1 = filtered[i + 1];
        let c2 = filtered[i + 2];
        let c3 = filtered[i + 3];

        let v0 = DECODE_TABLE[c0 as usize];
        let v1 = DECODE_TABLE[c1 as usize];

        // c0、c1 不能是 padding（= 不在字母表，查表为 0xFF）
        if v0 == 0xFF || v1 == 0xFF {
            return Err(PkiError::InvalidPemFormat);
        }

        let v0 = v0 as u32;
        let v1 = v1 as u32;

        if c2 == b'=' {
            // XX== → 1 字节
            if c3 != b'=' {
                return Err(PkiError::InvalidPemFormat);
            }
            out.push(((v0 << 2) | (v1 >> 4)) as u8);
        } else if c3 == b'=' {
            // XXX= → 2 字节
            let v2 = DECODE_TABLE[c2 as usize];
            if v2 == 0xFF {
                return Err(PkiError::InvalidPemFormat);
            }
            let v2 = v2 as u32;
            out.push(((v0 << 2) | (v1 >> 4)) as u8);
            out.push((((v1 & 0x0F) << 4) | (v2 >> 2)) as u8);
        } else {
            // XXXX → 3 字节
            let v2 = DECODE_TABLE[c2 as usize];
            let v3 = DECODE_TABLE[c3 as usize];
            if v2 == 0xFF || v3 == 0xFF {
                return Err(PkiError::InvalidPemFormat);
            }
            let v2 = v2 as u32;
            let v3 = v3 as u32;
            out.push(((v0 << 2) | (v1 >> 4)) as u8);
            out.push((((v1 & 0x0F) << 4) | (v2 >> 2)) as u8);
            out.push((((v2 & 0x03) << 6) | v3) as u8);
        }

        i += 4;
    }

    Ok(out)
}

// ============================================================================
// SubTask 4.2: base64_encode
// ============================================================================

/// Base64 编码（标准字母表，带 padding）.
///
/// 每 3 字节输入 → 4 字符输出，末尾不足 3 字节用 `=` 填充。
pub fn base64_encode(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }

    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;

    // 完整的 3 字节组
    while i + 3 <= input.len() {
        let v = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(ENCODE_TABLE[((v >> 18) & 0x3F) as usize] as char);
        out.push(ENCODE_TABLE[((v >> 12) & 0x3F) as usize] as char);
        out.push(ENCODE_TABLE[((v >> 6) & 0x3F) as usize] as char);
        out.push(ENCODE_TABLE[(v & 0x3F) as usize] as char);
        i += 3;
    }

    // 剩余 1 或 2 字节
    let rem = input.len() - i;
    if rem == 1 {
        let v = (input[i] as u32) << 16;
        out.push(ENCODE_TABLE[((v >> 18) & 0x3F) as usize] as char);
        out.push(ENCODE_TABLE[((v >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let v = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(ENCODE_TABLE[((v >> 18) & 0x3F) as usize] as char);
        out.push(ENCODE_TABLE[((v >> 12) & 0x3F) as usize] as char);
        out.push(ENCODE_TABLE[((v >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }

    out
}

// ============================================================================
// SubTask 4.4: parse_der
// ============================================================================

/// 解析 DER 编码的 X.509 证书.
///
/// 直接委托给 [`X509Certificate::decode`]，错误透传。
pub fn parse_der(der: &[u8]) -> Result<X509Certificate, PkiError> {
    X509Certificate::decode(der)
}

// ============================================================================
// SubTask 4.5: parse_pem
// ============================================================================

/// PEM 格式标记常量.
const PEM_BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const PEM_END: &str = "-----END CERTIFICATE-----";

/// 解析 PEM 编码的 X.509 证书.
///
/// PEM 格式：
/// ```text
/// -----BEGIN CERTIFICATE-----
/// <base64 编码的 DER>
/// -----END CERTIFICATE-----
/// ```
///
/// 找不到 BEGIN/END 标记返回 `Err(PkiError::InvalidPemFormat)`。
/// 提取的 Base64 内容中的空白字符（换行等）会被自动忽略。
pub fn parse_pem(pem: &str) -> Result<X509Certificate, PkiError> {
    let begin_idx = pem.find(PEM_BEGIN).ok_or(PkiError::InvalidPemFormat)?;
    let content_start = begin_idx + PEM_BEGIN.len();
    let end_rel = pem[content_start..]
        .find(PEM_END)
        .ok_or(PkiError::InvalidPemFormat)?;
    let content_end = content_start + end_rel;

    let content = &pem[content_start..content_end];
    let der = base64_decode(content.as_bytes())?;
    parse_der(&der)
}

// ============================================================================
// SubTask 4.6: to_der
// ============================================================================

/// 将证书编码为 DER 字节.
///
/// 直接委托给 [`X509Certificate::encode`]，错误透传。
/// 编码 SM2 证书不会失败；RSA 公钥（未实现）会返回 `UnsupportedAlgorithm`。
pub fn to_der(cert: &X509Certificate) -> Result<Vec<u8>, PkiError> {
    cert.encode()
}

// ============================================================================
// SubTask 4.7: to_pem
// ============================================================================

/// 将证书编码为 PEM 字符串.
///
/// 内部流程：DER 编码 → Base64 编码 → 按 64 字符分行 → 包裹 BEGIN/END 标记。
pub fn to_pem(cert: &X509Certificate) -> Result<String, PkiError> {
    let der = to_der(cert)?;
    let b64 = base64_encode(&der);
    Ok(wrap_pem(&b64))
}

/// 将 Base64 字符串按 64 字符分行，包裹 PEM 标记.
fn wrap_pem(b64: &str) -> String {
    let mut result = String::from(PEM_BEGIN);
    result.push('\n');

    let mut line = String::with_capacity(64);
    for c in b64.chars() {
        line.push(c);
        if line.len() == 64 {
            result.push_str(&line);
            result.push('\n');
            line.clear();
        }
    }
    if !line.is_empty() {
        result.push_str(&line);
        result.push('\n');
    }

    result.push_str(PEM_END);
    result.push('\n');
    result
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    use crate::pki::asn1::DerWriter;
    use crate::pki::x509::{DistinguishedName, SignatureAlgorithm, SubjectPublicKey};
    use crate::rng::CsRng;
    use crate::sm2::Sm2KeyPair;

    /// 使用公开 API 手动构建一个有效的 v1 证书 DER（无 version 字段、无扩展）.
    ///
    /// 由于 `X509Certificate.public_key` 为私有字段，parser 模块无法通过
    /// 结构体字面量构造证书，因此用公开的 ASN.1/x509 构件组装 DER，
    /// 再通过 `parse_der` 解析为证书实例用于往返测试。
    fn build_v1_cert_der() -> Vec<u8> {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");

        // === TBSCertificate (v1: 无 version 字段、无 extensions) ===
        let mut tbs_content = Vec::new();

        // serialNumber INTEGER
        let mut sn = DerWriter::new();
        sn.write_integer(&[0x01]);
        tbs_content.extend_from_slice(sn.as_bytes());

        // signature AlgorithmIdentifier
        tbs_content
            .extend_from_slice(&SignatureAlgorithm::Sm2WithSm3.encode_algorithm_identifier());

        // issuer Name
        tbs_content.extend_from_slice(&DistinguishedName::new("Test CA").encode_rdn_sequence());

        // validity SEQUENCE { notBefore, notAfter }
        let mut val = DerWriter::new();
        val.write_utctime(1704067200); // 2024-01-01 00:00:00 UTC
        val.write_utctime(1735689600); // 2025-01-01 00:00:00 UTC
        let mut val_seq = DerWriter::new();
        val_seq.write_sequence(val.as_bytes());
        tbs_content.extend_from_slice(val_seq.as_bytes());

        // subject Name
        tbs_content
            .extend_from_slice(&DistinguishedName::new("Test Subject").encode_rdn_sequence());

        // subjectPublicKeyInfo
        tbs_content.extend_from_slice(&SubjectPublicKey::Sm2(kp.public_key).encode_spki().unwrap());

        // 包装 TBS 为 SEQUENCE
        let mut tbs_seq = DerWriter::new();
        tbs_seq.write_sequence(&tbs_content);
        let tbs_der = tbs_seq.into_bytes();

        // === Certificate SEQUENCE { TBS, sigAlg, sigValue } ===
        let mut cert_content = Vec::new();
        cert_content.extend_from_slice(&tbs_der);
        cert_content
            .extend_from_slice(&SignatureAlgorithm::Sm2WithSm3.encode_algorithm_identifier());

        // signatureValue BIT STRING（SM2 r‖s，64 字节）
        let mut sig = DerWriter::new();
        sig.write_bit_string(&[0xAA; 64]);
        cert_content.extend_from_slice(sig.as_bytes());

        let mut cert_seq = DerWriter::new();
        cert_seq.write_sequence(&cert_content);
        cert_seq.into_bytes()
    }

    // --- SubTask 4.1: base64_decode 测试 ---

    #[test]
    fn test_base64_decode_empty() {
        let result = base64_decode(b"").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_base64_decode_hello() {
        let result = base64_decode(b"SGVsbG8=").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn test_base64_decode_hello_world() {
        let result = base64_decode(b"SGVsbG8gV29ybGQ=").unwrap();
        assert_eq!(result, b"Hello World");
    }

    #[test]
    fn test_base64_decode_with_whitespace() {
        // 含 \n 和 \t 的输入应被忽略
        let result = base64_decode(b"SGVs\nbG8=\t").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn test_base64_decode_invalid_char() {
        // '!' 不在 Base64 字母表中
        assert_eq!(base64_decode(b"SGVsbG8!"), Err(PkiError::InvalidPemFormat));
    }

    // --- SubTask 4.2: base64_encode 测试 ---

    #[test]
    fn test_base64_encode_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn test_base64_encode_hello() {
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
    }

    #[test]
    fn test_base64_encode_hello_world() {
        assert_eq!(base64_encode(b"Hello World"), "SGVsbG8gV29ybGQ=");
    }

    #[test]
    fn test_base64_roundtrip() {
        // 任意字节往返：encode → decode == 原始
        let all_bytes: Vec<u8> = (0u8..=255u8).collect();
        let test_cases: &[&[u8]] = &[
            &[0x00],
            &[0xFF],
            &[0x01, 0x02],
            &[0x01, 0x02, 0x03],
            &[0xDE, 0xAD, 0xBE, 0xEF],
            b"The quick brown fox jumps over the lazy dog",
            all_bytes.as_slice(),
        ];
        for original in test_cases {
            let encoded = base64_encode(original);
            let decoded = base64_decode(encoded.as_bytes()).unwrap();
            assert_eq!(
                decoded.as_slice(),
                *original,
                "roundtrip failed for {:?}",
                original
            );
        }
    }

    // --- SubTask 4.4 & 4.6: parse_der / to_der 往返 ---

    #[test]
    fn test_parse_der_to_der_roundtrip() {
        let der = build_v1_cert_der();
        let cert = parse_der(&der).expect("parse_der failed");
        let der2 = to_der(&cert).expect("to_der failed");
        assert_eq!(der, der2, "DER 往返字节不一致");
    }

    // --- SubTask 4.5 & 4.7: parse_pem / to_pem 往返 ---

    #[test]
    fn test_parse_pem_to_pem_roundtrip() {
        let der = build_v1_cert_der();
        let cert = parse_der(&der).expect("parse_der failed");
        let pem = to_pem(&cert).expect("to_pem failed");
        let cert2 = parse_pem(&pem).expect("parse_pem failed");
        assert_eq!(cert, cert2, "PEM 往返证书不一致");
    }

    #[test]
    fn test_parse_pem_missing_begin() {
        let pem = "-----END CERTIFICATE-----\nSGVsbG8=\n";
        assert_eq!(parse_pem(pem), Err(PkiError::InvalidPemFormat));
    }

    #[test]
    fn test_parse_pem_missing_end() {
        let pem = "-----BEGIN CERTIFICATE-----\nSGVsbG8=\n";
        assert_eq!(parse_pem(pem), Err(PkiError::InvalidPemFormat));
    }

    #[test]
    fn test_to_pem_format() {
        let der = build_v1_cert_der();
        let cert = parse_der(&der).expect("parse_der failed");
        let pem = to_pem(&cert).expect("to_pem failed");

        // 验证 BEGIN/END 标记
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(pem.ends_with("-----END CERTIFICATE-----\n"));

        // 验证每行 ≤ 64 字符（除标记行外）
        let lines: Vec<&str> = pem.lines().collect();
        assert_eq!(lines[0], "-----BEGIN CERTIFICATE-----");
        assert_eq!(lines[lines.len() - 1], "-----END CERTIFICATE-----");
        for line in &lines[1..lines.len() - 1] {
            assert!(
                line.len() <= 64,
                "PEM 行超过 64 字符: len={}, content={}",
                line.len(),
                line
            );
        }
    }

    // --- 补充测试：padding 与边界场景 ---

    #[test]
    fn test_base64_decode_padding_variants() {
        // 1 字节 → XX==
        assert_eq!(base64_decode(b"AA==").unwrap(), vec![0x00]);
        // 2 字节 → XXX=
        assert_eq!(base64_decode(b"AAA=").unwrap(), vec![0x00, 0x00]);
        // 3 字节 → XXXX
        assert_eq!(base64_decode(b"AAAA").unwrap(), vec![0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_base64_decode_invalid_padding_position() {
        // padding 出现在第 0 或第 1 位置 → 非法
        assert_eq!(base64_decode(b"=AAA"), Err(PkiError::InvalidPemFormat));
        assert_eq!(base64_decode(b"A=AA"), Err(PkiError::InvalidPemFormat));
    }

    #[test]
    fn test_base64_decode_length_not_multiple_of_4() {
        // 过滤空白后长度不是 4 的倍数
        assert_eq!(base64_decode(b"SGVsbG8"), Err(PkiError::InvalidPemFormat));
    }
}
