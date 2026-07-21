//! PKI CRL 证书吊销列表 (v0.32.0 Task 5).
//!
//! 提供 RFC 5280 简化版证书吊销列表（CRL）的数据结构、DER 编解码与查询接口。
//! 基于 v0.32.0 Task 2 的 ASN.1 DER 编解码器与 Task 3 的 X.509 可分辨名称。
//!
//! # 核心组件
//! - [`RevocationReason`]：证书吊销原因枚举（RFC 5280 §5.3.1 简化，7 变体）
//! - [`RevokedCert`]：被吊销的证书条目（序列号 + 吊销时间 + 原因）
//! - [`Crl`]：证书吊销列表（颁发者 + 吊销条目列表 + 下次更新时间）
//!
//! # 简化说明
//! 本实现遵循 Karpathy "Simplicity First" 原则，对 RFC 5280 §5.1 做以下简化：
//! - `RevocationReason` 不写入 DER（解码时默认 `Unspecified`）
//! - CRL 仅编码 TBSCertList 部分（不含签名算法与签名值，签名验证由 verify 模块负责）
//! - 省略 `thisUpdate` 字段，仅保留 `nextUpdate`
//! - 不编码 CRL 版本号与 crlExtensions
//!
//! # no_std 合规
//! no_std 由 crate 根继承，本模块通过 `extern crate alloc` 引入堆分配。
//! 使用 `alloc::string::String` / `alloc::vec::Vec`，不使用 `std::*`。
//!
//! # 参考
//! - RFC 5280 §5.1 CertificateList 与 §5.3.1 CRL Reason Code
//! - GB/T 35275 信息安全技术 SM2 密码算法加密签名消息语法规范

extern crate alloc;

use alloc::vec::Vec;

use crate::pki::asn1::{self, DerReader, DerWriter};
use crate::pki::x509::DistinguishedName;
use crate::pki::PkiError;

// ============================================================================
// 辅助函数
// ============================================================================

/// 将 [`asn1::Asn1Error`] 转换为 [`PkiError::Asn1Error`]。
fn asn1_err(e: asn1::Asn1Error) -> PkiError {
    PkiError::Asn1Error(alloc::format!("{:?}", e))
}

/// 从 DerReader 读取时间字段（UTCTime 或 GeneralizedTime），返回 Unix 时间戳。
///
/// 由于 [`DerReader`] 不支持 peek，先 `read_element` 获取 tag+content，
/// 再重构 TLV 交给对应的 read 方法解析。与 `x509::read_time` 实现一致。
fn read_time(reader: &mut DerReader) -> Result<u64, PkiError> {
    let (tag, content) = reader.read_element().map_err(asn1_err)?;
    // 时间内容始终 ≤ 15 字节，长度用短格式即可
    let mut tlv = Vec::with_capacity(2 + content.len());
    tlv.push(tag);
    tlv.push(content.len() as u8);
    tlv.extend_from_slice(content);

    let mut sub = DerReader::new(&tlv);
    match tag {
        asn1::UTC_TIME => sub.read_utctime().map_err(asn1_err),
        asn1::GENERALIZED_TIME => sub.read_generalized_time().map_err(asn1_err),
        _ => Err(PkiError::InvalidDerFormat),
    }
}

// ============================================================================
// SubTask 5.1: RevocationReason
// ============================================================================

/// 证书吊销原因（RFC 5280 §5.3.1 简化）.
///
/// 仅保留 RFC 5280 定义的 0~6 号原因码，省略 8 (removeFromCRL) 与
/// 9 (certificateHold 的特殊变体)。未知值解码为 `Unspecified`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationReason {
    /// 未指定原因（默认值，RFC 5280 reasonCode=0）
    Unspecified,
    /// 密钥泄露（reasonCode=1）
    KeyCompromise,
    /// CA 密钥泄露（reasonCode=2）
    CaCompromise,
    /// 从属关系变更（reasonCode=3）
    AffiliationChanged,
    /// 证书已被替代（reasonCode=4）
    Superseded,
    /// 操作终止（reasonCode=5）
    CessationOfOperation,
    /// 证书暂停（reasonCode=6）
    CertificateHold,
}

impl RevocationReason {
    /// 返回 RFC 5280 reasonCode 数值.
    pub fn as_u8(&self) -> u8 {
        match self {
            RevocationReason::Unspecified => 0,
            RevocationReason::KeyCompromise => 1,
            RevocationReason::CaCompromise => 2,
            RevocationReason::AffiliationChanged => 3,
            RevocationReason::Superseded => 4,
            RevocationReason::CessationOfOperation => 5,
            RevocationReason::CertificateHold => 6,
        }
    }

    /// 从 RFC 5280 reasonCode 数值构造原因，未知值返回 `Unspecified`.
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => RevocationReason::Unspecified,
            1 => RevocationReason::KeyCompromise,
            2 => RevocationReason::CaCompromise,
            3 => RevocationReason::AffiliationChanged,
            4 => RevocationReason::Superseded,
            5 => RevocationReason::CessationOfOperation,
            6 => RevocationReason::CertificateHold,
            _ => RevocationReason::Unspecified,
        }
    }
}

// ============================================================================
// SubTask 5.2: RevokedCert
// ============================================================================

/// 被吊销的证书条目.
///
/// 对应 RFC 5280 §5.1 的 `revokedCertificates` 条目（简化版）：
/// `SEQUENCE { serialNumber INTEGER, revocationDate Time }`
///
/// `serial_number` 为大端 INTEGER 字节（已剥离前导 0x00 padding）。
/// `reason` 不参与 DER 编解码（简化方案），解码时默认 `Unspecified`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedCert {
    /// 证书序列号（大端 INTEGER 字节，已剥离前导 0x00）
    pub serial_number: Vec<u8>,
    /// 吊销时间（Unix 时间戳，秒）
    pub revocation_date: u64,
    /// 吊销原因（不写入 DER，仅内存中保留）
    pub reason: RevocationReason,
}

impl RevokedCert {
    /// 创建被吊销证书条目.
    pub fn new(serial: &[u8], date: u64, reason: RevocationReason) -> Self {
        Self {
            serial_number: serial.to_vec(),
            revocation_date: date,
            reason,
        }
    }

    /// 编码为 DER：`SEQUENCE { INTEGER(serial), UTCTime(date) }`.
    ///
    /// 简化方案：`reason` 不写入 DER（Simplicity First）。
    pub fn encode(&self) -> Vec<u8> {
        let mut inner = DerWriter::new();
        inner.write_integer(&self.serial_number);
        inner.write_utctime(self.revocation_date);

        let mut result = DerWriter::new();
        result.write_sequence(inner.as_bytes());
        result.into_bytes()
    }

    /// 从 DerReader 解析被吊销证书条目（reader 应位于 SEQUENCE tag 处）.
    ///
    /// 解析 `SEQUENCE { INTEGER(serial), Time(date) }`，时间支持 UTCTime/GeneralizedTime。
    /// `reason` 默认为 `Unspecified`（DER 中不编码原因）。
    pub fn decode(reader: &mut DerReader) -> Result<Self, PkiError> {
        let mut seq = reader.read_sequence().map_err(asn1_err)?;

        let serial_number = seq.read_integer().map_err(asn1_err)?;
        let revocation_date = read_time(&mut seq)?;

        Ok(RevokedCert {
            serial_number,
            revocation_date,
            reason: RevocationReason::Unspecified,
        })
    }
}

// ============================================================================
// SubTask 5.3 ~ 5.5: Crl
// ============================================================================

/// 证书吊销列表（RFC 5280 §5.1 简化版）.
///
/// 仅编码 TBSCertList 部分（不含签名），结构为：
/// `SEQUENCE { issuer Name, nextUpdate Time, [revokedCertificates] OPTIONAL }`
///
/// `revoked` 为空时不编码 revokedCertificates 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crl {
    /// 颁发者可分辨名称（CA 的 DN）
    pub issuer: DistinguishedName,
    /// 被吊销的证书条目列表
    pub revoked: Vec<RevokedCert>,
    /// 下次更新时间（Unix 时间戳，秒）
    pub next_update: u64,
}

impl Crl {
    /// 创建空的吊销列表（无吊销条目）.
    pub fn new(issuer: DistinguishedName, next_update: u64) -> Self {
        Self {
            issuer,
            revoked: Vec::new(),
            next_update,
        }
    }

    /// 检查指定序列号的证书是否已被吊销.
    ///
    /// 遍历 `revoked` 列表，比较 `serial_number` 字节（精确匹配）。
    pub fn is_revoked(&self, serial: &[u8]) -> bool {
        self.revoked.iter().any(|c| c.serial_number == serial)
    }

    /// 添加一个被吊销的证书条目.
    pub fn add_revoked(&mut self, cert: RevokedCert) {
        self.revoked.push(cert);
    }

    /// 查找指定序列号的吊销条目，返回引用.
    ///
    /// 未找到返回 `None`。
    pub fn find_revoked(&self, serial: &[u8]) -> Option<&RevokedCert> {
        self.revoked.iter().find(|c| c.serial_number == serial)
    }

    /// 编码为 DER（简化版 TBSCertList，不含签名）.
    ///
    /// 结构：`SEQUENCE { issuer RDNSequence, nextUpdate UTCTime, [revokedCertificates] }`
    ///
    /// `revoked` 为空时不写 revokedCertificates 字段。
    pub fn encode(&self) -> Vec<u8> {
        let mut content = Vec::new();

        // issuer Name (RDNSequence)
        content.extend_from_slice(&self.issuer.encode_rdn_sequence());

        // nextUpdate UTCTime
        let mut next_upd = DerWriter::new();
        next_upd.write_utctime(self.next_update);
        content.extend_from_slice(next_upd.as_bytes());

        // revokedCertificates SEQUENCE OF SEQUENCE { serial, revocationDate }
        // 仅在非空时编码
        if !self.revoked.is_empty() {
            let mut revoked_content = Vec::new();
            for cert in &self.revoked {
                revoked_content.extend_from_slice(&cert.encode());
            }
            let mut revoked_seq = DerWriter::new();
            revoked_seq.write_sequence(&revoked_content);
            content.extend_from_slice(revoked_seq.as_bytes());
        }

        // 包装为外层 SEQUENCE
        let mut result = DerWriter::new();
        result.write_sequence(&content);
        result.into_bytes()
    }

    /// 从 DER 字节解析吊销列表（简化版 TBSCertList）.
    ///
    /// 解析 `SEQUENCE { issuer Name, nextUpdate Time, [revokedCertificates] }`。
    /// revokedCertificates 字段可选（不存在时 revoked 为空）。
    pub fn decode(der: &[u8]) -> Result<Self, PkiError> {
        let mut r = DerReader::new(der);
        let mut seq = r.read_sequence().map_err(asn1_err)?;

        // issuer Name (RDNSequence)
        let issuer = DistinguishedName::decode_rdn_sequence(&mut seq)?;

        // nextUpdate Time
        let next_update = read_time(&mut seq)?;

        // revokedCertificates (可选)
        let mut revoked = Vec::new();
        if !seq.is_empty() {
            let mut revoked_seq = seq.read_sequence().map_err(asn1_err)?;
            while !revoked_seq.is_empty() {
                let cert = RevokedCert::decode(&mut revoked_seq)?;
                revoked.push(cert);
            }
        }

        Ok(Crl {
            issuer,
            revoked,
            next_update,
        })
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ===== SubTask 5.1: RevocationReason 测试 =====

    #[test]
    fn test_revocation_reason_as_u8_roundtrip() {
        // 所有 7 个变体的 as_u8 / from_u8 往返
        let reasons = [
            RevocationReason::Unspecified,
            RevocationReason::KeyCompromise,
            RevocationReason::CaCompromise,
            RevocationReason::AffiliationChanged,
            RevocationReason::Superseded,
            RevocationReason::CessationOfOperation,
            RevocationReason::CertificateHold,
        ];

        for reason in reasons {
            let val = reason.as_u8();
            let restored = RevocationReason::from_u8(val);
            assert_eq!(
                restored, reason,
                "往返失败: {:?} -> {} -> {:?}",
                reason, val, restored
            );
        }
    }

    #[test]
    fn test_revocation_reason_as_u8_values() {
        // 验证 reasonCode 数值与 RFC 5280 §5.3.1 一致
        assert_eq!(RevocationReason::Unspecified.as_u8(), 0);
        assert_eq!(RevocationReason::KeyCompromise.as_u8(), 1);
        assert_eq!(RevocationReason::CaCompromise.as_u8(), 2);
        assert_eq!(RevocationReason::AffiliationChanged.as_u8(), 3);
        assert_eq!(RevocationReason::Superseded.as_u8(), 4);
        assert_eq!(RevocationReason::CessationOfOperation.as_u8(), 5);
        assert_eq!(RevocationReason::CertificateHold.as_u8(), 6);
    }

    #[test]
    fn test_revocation_reason_from_u8_unknown() {
        // 未知值返回 Unspecified
        assert_eq!(RevocationReason::from_u8(7), RevocationReason::Unspecified);
        assert_eq!(RevocationReason::from_u8(8), RevocationReason::Unspecified);
        assert_eq!(
            RevocationReason::from_u8(255),
            RevocationReason::Unspecified
        );
    }

    // ===== SubTask 5.2: RevokedCert 测试 =====

    #[test]
    fn test_revoked_cert_new() {
        let cert = RevokedCert::new(
            &[0x01, 0x02, 0x03],
            1700000000,
            RevocationReason::KeyCompromise,
        );
        assert_eq!(cert.serial_number, vec![0x01, 0x02, 0x03]);
        assert_eq!(cert.revocation_date, 1700000000);
        assert_eq!(cert.reason, RevocationReason::KeyCompromise);
    }

    #[test]
    fn test_revoked_cert_encode_decode_roundtrip() {
        let cert = RevokedCert::new(
            &[0x01, 0x02, 0x03],
            1700000000,
            RevocationReason::KeyCompromise,
        );
        let der = cert.encode();

        let mut reader = DerReader::new(&der);
        let decoded = RevokedCert::decode(&mut reader).unwrap();

        // reason 不写入 DER，解码后默认 Unspecified
        assert_eq!(decoded.serial_number, cert.serial_number);
        assert_eq!(decoded.revocation_date, cert.revocation_date);
        assert_eq!(decoded.reason, RevocationReason::Unspecified);
    }

    #[test]
    fn test_revoked_cert_encode_valid_der() {
        let cert = RevokedCert::new(&[0x80], 1704067200, RevocationReason::Unspecified);
        let der = cert.encode();

        // 验证是有效的 SEQUENCE
        let mut r = DerReader::new(&der);
        let mut seq = r.read_sequence().unwrap();
        assert!(r.is_empty());

        // 验证内部结构：INTEGER + UTCTime
        let serial = seq.read_integer().unwrap();
        assert_eq!(serial, vec![0x80]);
        let date = read_time(&mut seq).unwrap();
        assert_eq!(date, 1704067200);
        assert!(seq.is_empty());
    }

    // ===== SubTask 5.3 ~ 5.5: Crl 测试 =====

    #[test]
    fn test_crl_new_empty() {
        let issuer = DistinguishedName::new("Test CA");
        let crl = Crl::new(issuer.clone(), 1800000000);

        assert_eq!(crl.issuer, issuer);
        assert!(crl.revoked.is_empty());
        assert_eq!(crl.next_update, 1800000000);
    }

    #[test]
    fn test_crl_add_revoked_and_is_revoked() {
        let mut crl = Crl::new(DistinguishedName::new("Test CA"), 1800000000);

        // 初始状态：无吊销
        assert!(!crl.is_revoked(&[0x01, 0x02, 0x03]));

        // 添加吊销条目
        crl.add_revoked(RevokedCert::new(
            &[0x01, 0x02, 0x03],
            1700000000,
            RevocationReason::KeyCompromise,
        ));

        // 添加后能查到
        assert!(crl.is_revoked(&[0x01, 0x02, 0x03]));
        assert_eq!(crl.revoked.len(), 1);
    }

    #[test]
    fn test_crl_is_revoked_not_present() {
        let mut crl = Crl::new(DistinguishedName::new("Test CA"), 1800000000);
        crl.add_revoked(RevokedCert::new(
            &[0x01, 0x02, 0x03],
            1700000000,
            RevocationReason::KeyCompromise,
        ));

        // 未吊销的 serial 返回 false
        assert!(!crl.is_revoked(&[0x04, 0x05, 0x06]));
        assert!(!crl.is_revoked(&[]));
        assert!(!crl.is_revoked(&[0x01])); // 前缀不匹配
    }

    #[test]
    fn test_crl_find_revoked() {
        let mut crl = Crl::new(DistinguishedName::new("Test CA"), 1800000000);
        let cert1 = RevokedCert::new(
            &[0x01, 0x02, 0x03],
            1700000000,
            RevocationReason::KeyCompromise,
        );
        let cert2 = RevokedCert::new(&[0xAA, 0xBB], 1700001000, RevocationReason::Superseded);
        crl.add_revoked(cert1.clone());
        crl.add_revoked(cert2);

        // 找到第一个吊销条目
        let found = crl.find_revoked(&[0x01, 0x02, 0x03]);
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.serial_number, cert1.serial_number);
        assert_eq!(found.revocation_date, cert1.revocation_date);

        // 找到第二个吊销条目
        let found = crl.find_revoked(&[0xAA, 0xBB]);
        assert!(found.is_some());
        assert_eq!(found.unwrap().revocation_date, 1700001000);

        // 未找到返回 None
        assert!(crl.find_revoked(&[0xFF]).is_none());
    }

    // ===== SubTask 5.6: Crl encode/decode 测试 =====

    #[test]
    fn test_crl_encode_decode_empty_roundtrip() {
        let issuer = DistinguishedName::new("Test CA")
            .with_o("Test Org")
            .with_c("CN");
        let crl = Crl::new(issuer.clone(), 1800000000);

        let der = crl.encode();
        let decoded = Crl::decode(&der).unwrap();

        assert_eq!(decoded.issuer, issuer);
        assert!(decoded.revoked.is_empty());
        assert_eq!(decoded.next_update, 1800000000);
    }

    #[test]
    fn test_crl_encode_decode_with_revoked_roundtrip() {
        let issuer = DistinguishedName::new("Test CA").with_o("Test Org");
        let mut crl = Crl::new(issuer.clone(), 1800000000);
        crl.add_revoked(RevokedCert::new(
            &[0x01, 0x02, 0x03],
            1700000000,
            RevocationReason::KeyCompromise,
        ));
        crl.add_revoked(RevokedCert::new(
            &[0xAA, 0xBB, 0xCC],
            1700001000,
            RevocationReason::Superseded,
        ));

        let der = crl.encode();
        let decoded = Crl::decode(&der).unwrap();

        assert_eq!(decoded.issuer, issuer);
        assert_eq!(decoded.next_update, 1800000000);
        assert_eq!(decoded.revoked.len(), 2);

        // 验证第一个吊销条目（reason 不写入 DER，解码为 Unspecified）
        assert_eq!(decoded.revoked[0].serial_number, vec![0x01, 0x02, 0x03]);
        assert_eq!(decoded.revoked[0].revocation_date, 1700000000);
        assert_eq!(decoded.revoked[0].reason, RevocationReason::Unspecified);

        // 验证第二个吊销条目
        assert_eq!(decoded.revoked[1].serial_number, vec![0xAA, 0xBB, 0xCC]);
        assert_eq!(decoded.revoked[1].revocation_date, 1700001000);

        // 验证 is_revoked 仍可正常工作
        assert!(decoded.is_revoked(&[0x01, 0x02, 0x03]));
        assert!(decoded.is_revoked(&[0xAA, 0xBB, 0xCC]));
        assert!(!decoded.is_revoked(&[0xFF]));
    }

    #[test]
    fn test_crl_encode_valid_der_structure() {
        let mut crl = Crl::new(DistinguishedName::new("Test CA"), 1800000000);
        crl.add_revoked(RevokedCert::new(
            &[0x01],
            1700000000,
            RevocationReason::Unspecified,
        ));

        let der = crl.encode();

        // 验证外层是 SEQUENCE
        let mut r = DerReader::new(&der);
        let mut seq = r.read_sequence().unwrap();
        assert!(r.is_empty());

        // 第一个元素：issuer RDNSequence (SEQUENCE)
        let _issuer = DistinguishedName::decode_rdn_sequence(&mut seq).unwrap();

        // 第二个元素：nextUpdate UTCTime
        let next_upd = read_time(&mut seq).unwrap();
        assert_eq!(next_upd, 1800000000);

        // 第三个元素：revokedCertificates SEQUENCE OF
        let mut revoked_seq = seq.read_sequence().unwrap();
        let _cert = RevokedCert::decode(&mut revoked_seq).unwrap();
        assert!(revoked_seq.is_empty());
        assert!(seq.is_empty());
    }

    #[test]
    fn test_crl_decode_invalid_der() {
        // 空 DER
        let result = Crl::decode(&[]);
        assert!(result.is_err());

        // 非 SEQUENCE 起始
        let result = Crl::decode(&[0x01, 0x01, 0x00]); // BOOLEAN
        assert!(result.is_err());

        // 截断的 SEQUENCE
        let result = Crl::decode(&[0x30, 0x10, 0x00]); // SEQUENCE len=16 but only 1 byte
        assert!(result.is_err());
    }

    #[test]
    fn test_crl_decode_empty_revolked_field() {
        // 构造一个含空 revokedCertificates 的 CRL（虽然 encode 不会产生这种，
        // 但 decode 应能正确处理）
        let issuer = DistinguishedName::new("Test CA");
        let mut content = Vec::new();
        content.extend_from_slice(&issuer.encode_rdn_sequence());

        // nextUpdate
        let mut next_upd = DerWriter::new();
        next_upd.write_utctime(1800000000);
        content.extend_from_slice(next_upd.as_bytes());

        // 空 revokedCertificates SEQUENCE
        let mut empty_revoked = DerWriter::new();
        empty_revoked.write_sequence(&[]);
        content.extend_from_slice(empty_revoked.as_bytes());

        let mut wrapper = DerWriter::new();
        wrapper.write_sequence(&content);
        let der = wrapper.into_bytes();

        let decoded = Crl::decode(&der).unwrap();
        assert_eq!(decoded.issuer, issuer);
        assert!(decoded.revoked.is_empty());
        assert_eq!(decoded.next_update, 1800000000);
    }
}
