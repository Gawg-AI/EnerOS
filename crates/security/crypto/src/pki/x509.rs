//! X.509 证书结构与编解码 (v0.32.0 Task 3).
//!
//! 提供 X.509 v3 证书的核心数据结构、DER 编解码与证书请求构建器，
//! 基于 v0.31.0 国密 SM2 公钥与 v0.32.0 Task 2 的 ASN.1 DER 编解码器。
//!
//! # 核心组件
//! - [`DistinguishedName`]：X.501 可分辨名称（CN/O/OU/C），RDNSequence 编解码
//! - [`SubjectPublicKey`]：SubjectPublicKeyInfo，支持 SM2 公钥
//! - [`SignatureAlgorithm`]：签名算法标识（SM2-with-SM3 / ECDSA-with-SHA256）
//! - [`Extension`]：X.509 v3 扩展（通用 OID + critical + value 结构）
//! - [`KeyUsage`]：密钥用法位图（RFC 5280 §4.2.1.3，高位优先 BIT STRING）
//! - [`ExtKeyUsage`]：扩展密钥用法（ServerAuth / ClientAuth / CodeSigning / EmailProtection）
//! - [`X509Certificate`]：完整 X.509 v1/v3 证书结构与 DER 编解码
//! - [`CertRequest`]：证书请求构建器（subject + public_key + validity + key_usage）
//!
//! # no_std 合规
//! no_std 由 crate 根继承，本模块通过 `extern crate alloc` 引入堆分配。
//! 使用 `alloc::string::String` / `alloc::vec::Vec`，不使用 `std::*`。
//!
//! # 参考
//! - RFC 5280 Internet X.509 Public Key Infrastructure Certificate and CRL Profile
//! - GB/T 35275 信息安全技术 SM2 密码算法加密签名消息语法规范

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::pki::asn1::{self, encode_oid, DerReader, DerWriter};
use crate::pki::PkiError;
use crate::sm2::Sm2PublicKey;

// ============================================================================
// 辅助函数
// ============================================================================

/// 将 [`asn1::Asn1Error`] 转换为 [`PkiError::Asn1Error`]。
fn asn1_err(e: asn1::Asn1Error) -> PkiError {
    PkiError::Asn1Error(alloc::format!("{:?}", e))
}

/// 编码单个 RDN 属性为 `SET { SEQUENCE { OID, UTF8String } }` 的完整 DER TLV。
///
/// 返回的 Vec 是一个完整的 SET TLV，可直接拼接到 RDNSequence 内容中。
fn encode_rdn_attr(oid_content: &[u8], value: &str) -> Vec<u8> {
    let mut inner = DerWriter::new();
    inner.write_oid(oid_content);
    inner.write_element(asn1::UTF8_STRING, value.as_bytes());

    let mut seq = DerWriter::new();
    seq.write_sequence(inner.as_bytes());

    let mut set = DerWriter::new();
    set.write_set(seq.as_bytes());
    set.into_bytes()
}

/// 从 DerReader 读取时间字段（UTCTime 或 GeneralizedTime），返回 Unix 时间戳。
///
/// 由于 [`DerReader`] 不支持 peek，先 `read_element` 获取 tag+content，
/// 再重构 TLV 交给对应的 read 方法解析。
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
// SubTask 3.1: DistinguishedName
// ============================================================================

/// X.501 可分辨名称（Distinguished Name）。
///
/// 表示为 RDNSequence，仅支持 CN/O/OU/C 四种常见属性。
/// 编码为 `SEQUENCE OF SET { SEQUENCE { OID, UTF8String } }`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistinguishedName {
    /// Common Name（必需）
    pub cn: String,
    /// Organization（可选）
    pub o: Option<String>,
    /// Organizational Unit（可选）
    pub ou: Option<String>,
    /// Country（可选）
    pub c: Option<String>,
}

impl DistinguishedName {
    /// 创建仅含 CN 的可分辨名称。
    pub fn new(cn: &str) -> Self {
        Self {
            cn: String::from(cn),
            o: None,
            ou: None,
            c: None,
        }
    }

    /// 设置 Organization（builder）。
    pub fn with_o(mut self, o: &str) -> Self {
        self.o = Some(String::from(o));
        self
    }

    /// 设置 Organizational Unit（builder）。
    pub fn with_ou(mut self, ou: &str) -> Self {
        self.ou = Some(String::from(ou));
        self
    }

    /// 设置 Country（builder）。
    pub fn with_c(mut self, c: &str) -> Self {
        self.c = Some(String::from(c));
        self
    }

    /// 编码为 RDNSequence 的完整 DER（SEQUENCE OF SET）。
    ///
    /// 属性顺序：CN → O → OU → C。
    pub fn encode_rdn_sequence(&self) -> Vec<u8> {
        let mut content = Vec::new();
        // CN OID: 2.5.4.3
        content.extend_from_slice(&encode_rdn_attr(&encode_oid(&[2, 5, 4, 3]), &self.cn));
        // O OID: 2.5.4.10
        if let Some(ref o) = self.o {
            content.extend_from_slice(&encode_rdn_attr(&encode_oid(&[2, 5, 4, 10]), o));
        }
        // OU OID: 2.5.4.11
        if let Some(ref ou) = self.ou {
            content.extend_from_slice(&encode_rdn_attr(&encode_oid(&[2, 5, 4, 11]), ou));
        }
        // C OID: 2.5.4.6
        if let Some(ref c) = self.c {
            content.extend_from_slice(&encode_rdn_attr(&encode_oid(&[2, 5, 4, 6]), c));
        }

        let mut result = DerWriter::new();
        result.write_sequence(&content);
        result.into_bytes()
    }

    /// 从 DerReader 解析 RDNSequence（reader 应位于 SEQUENCE tag 处）。
    ///
    /// 未知 OID 的属性会被忽略（容错）。CN 必需，缺失则返回 `InvalidDerFormat`。
    pub fn decode_rdn_sequence(reader: &mut DerReader) -> Result<Self, PkiError> {
        let mut seq = reader.read_sequence().map_err(asn1_err)?;

        let mut cn = None;
        let mut o = None;
        let mut ou = None;
        let mut c = None;

        let cn_oid = encode_oid(&[2, 5, 4, 3]);
        let o_oid = encode_oid(&[2, 5, 4, 10]);
        let ou_oid = encode_oid(&[2, 5, 4, 11]);
        let c_oid = encode_oid(&[2, 5, 4, 6]);

        while !seq.is_empty() {
            let mut set = seq.read_set().map_err(asn1_err)?;
            while !set.is_empty() {
                let mut attr = set.read_sequence().map_err(asn1_err)?;
                let oid_bytes = attr.read_oid().map_err(asn1_err)?;
                let (_tag, value_bytes) = attr.read_element().map_err(asn1_err)?;
                let value = String::from_utf8(value_bytes.to_vec())
                    .map_err(|_| PkiError::InvalidDerFormat)?;

                if oid_bytes == cn_oid {
                    cn = Some(value);
                } else if oid_bytes == o_oid {
                    o = Some(value);
                } else if oid_bytes == ou_oid {
                    ou = Some(value);
                } else if oid_bytes == c_oid {
                    c = Some(value);
                }
                // 未知 OID：忽略
            }
        }

        let cn = cn.ok_or(PkiError::InvalidDerFormat)?;
        Ok(Self { cn, o, ou, c })
    }
}

// ============================================================================
// SubTask 3.2: SubjectPublicKey
// ============================================================================

/// 主体公钥（SubjectPublicKeyInfo 中携带的公钥）。
///
/// 当前仅支持 SM2 公钥编解码；RSA 变体保留枚举占位，编解码时返回
/// [`PkiError::UnsupportedAlgorithm`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubjectPublicKey {
    /// SM2 公钥（SM2 推荐曲线上的点）
    Sm2(Sm2PublicKey),
    /// RSA 公钥（保留枚举变体，编解码时返回不支持错误）
    Rsa(Vec<u8>),
}

impl SubjectPublicKey {
    /// 编码为 SubjectPublicKeyInfo DER。
    ///
    /// SM2: `SEQUENCE { SEQUENCE{OID(1.2.156.10197.1.301), NULL}, BIT STRING(04||x||y) }`
    ///
    /// RSA: 返回 `Err(UnsupportedAlgorithm)`。
    pub fn encode_spki(&self) -> Result<Vec<u8>, PkiError> {
        match self {
            SubjectPublicKey::Sm2(pk) => {
                // AlgorithmIdentifier: SEQUENCE { OID, NULL }
                let sm2_oid = encode_oid(&[1, 2, 156, 10197, 1, 301]);
                let mut alg_inner = DerWriter::new();
                alg_inner.write_oid(&sm2_oid);
                alg_inner.write_null();

                // SPKI content: AlgorithmIdentifier SEQUENCE + BIT STRING
                let mut content = DerWriter::new();
                content.write_sequence(alg_inner.as_bytes());
                content.write_bit_string(&pk.to_bytes_uncompressed());

                let mut result = DerWriter::new();
                result.write_sequence(content.as_bytes());
                Ok(result.into_bytes())
            }
            SubjectPublicKey::Rsa(_) => Err(PkiError::UnsupportedAlgorithm),
        }
    }

    /// 从 DerReader 解析 SubjectPublicKeyInfo（reader 应位于 SEQUENCE tag 处）。
    ///
    /// 仅支持 SM2 算法 OID；非 SM2 返回 `UnsupportedAlgorithm`。
    pub fn decode_spki(reader: &mut DerReader) -> Result<Self, PkiError> {
        let mut spki = reader.read_sequence().map_err(asn1_err)?;

        // AlgorithmIdentifier
        let mut alg = spki.read_sequence().map_err(asn1_err)?;
        let oid_bytes = alg.read_oid().map_err(asn1_err)?;

        let sm2_oid = encode_oid(&[1, 2, 156, 10197, 1, 301]);
        if oid_bytes != sm2_oid {
            return Err(PkiError::UnsupportedAlgorithm);
        }

        // 跳过 NULL 参数（如果存在）
        if !alg.is_empty() {
            let _ = alg.read_element();
        }

        // BIT STRING（read_bit_string 自动剥离首字节 unused bits）
        let key_bytes = spki.read_bit_string().map_err(asn1_err)?;
        let pk = Sm2PublicKey::from_bytes(&key_bytes).map_err(|_| PkiError::InvalidDerFormat)?;

        Ok(SubjectPublicKey::Sm2(pk))
    }
}

// ============================================================================
// SubTask 3.3: SignatureAlgorithm
// ============================================================================

/// 签名算法标识。
///
/// 支持 SM2-with-SM3（国密）和 ECDSA-with-SHA256 两种算法的 OID 编解码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    /// SM2-with-SM3 签名算法，OID 1.2.156.10197.1.501
    Sm2WithSm3,
    /// ECDSA-with-SHA256 签名算法，OID 1.2.840.10045.4.3.2
    EcdsaWithSha256,
}

impl SignatureAlgorithm {
    /// 返回算法 OID 的 DER 内容字节（base-128 编码）。
    pub fn oid_bytes(&self) -> Vec<u8> {
        match self {
            SignatureAlgorithm::Sm2WithSm3 => encode_oid(&[1, 2, 156, 10197, 1, 501]),
            SignatureAlgorithm::EcdsaWithSha256 => encode_oid(&[1, 2, 840, 10045, 4, 3, 2]),
        }
    }

    /// 从 OID DER 内容字节解析签名算法。
    ///
    /// 非 SM2/ECDSA 返回 `UnsupportedAlgorithm`。
    pub fn from_oid(oid_content: &[u8]) -> Result<Self, PkiError> {
        let sm2_sig = encode_oid(&[1, 2, 156, 10197, 1, 501]);
        let ecdsa_sig = encode_oid(&[1, 2, 840, 10045, 4, 3, 2]);
        if oid_content == sm2_sig.as_slice() {
            Ok(SignatureAlgorithm::Sm2WithSm3)
        } else if oid_content == ecdsa_sig.as_slice() {
            Ok(SignatureAlgorithm::EcdsaWithSha256)
        } else {
            Err(PkiError::UnsupportedAlgorithm)
        }
    }

    /// 编码为 AlgorithmIdentifier DER：`SEQUENCE { OID, NULL }`。
    pub fn encode_algorithm_identifier(&self) -> Vec<u8> {
        let mut inner = DerWriter::new();
        inner.write_oid(&self.oid_bytes());
        inner.write_null();

        let mut result = DerWriter::new();
        result.write_sequence(inner.as_bytes());
        result.into_bytes()
    }

    /// 从 DerReader 解析 AlgorithmIdentifier（reader 应位于 SEQUENCE tag 处）。
    ///
    /// 读取 OID 后跳过参数字段（NULL 或其他），再调用 [`from_oid`]。
    pub fn decode_algorithm_identifier(reader: &mut DerReader) -> Result<Self, PkiError> {
        let mut alg = reader.read_sequence().map_err(asn1_err)?;
        let oid_bytes = alg.read_oid().map_err(asn1_err)?;
        // 可选参数（NULL 或曲线 OID），存在则跳过
        if !alg.is_empty() {
            let _ = alg.read_element();
        }
        Self::from_oid(&oid_bytes)
    }
}

// ============================================================================
// SubTask 3.4: Extension
// ============================================================================

/// X.509 v3 扩展（通用结构）。
///
/// `oid` 存储 DER 编码的 OID 内容字节（base-128），便于直接写入。
/// `value` 存储 OCTET STRING 的内容（即扩展值的 DER 编码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    /// 扩展 OID（DER base-128 内容字节）
    pub oid: Vec<u8>,
    /// 是否为关键扩展
    pub critical: bool,
    /// 扩展值（OCTET STRING 内容）
    pub value: Vec<u8>,
}

impl Extension {
    /// 编码为 Extension DER：`SEQUENCE { OID, [BOOLEAN TRUE]?, OCTET STRING(value) }`。
    ///
    /// `critical=false` 时不写 BOOLEAN（DEFAULT FALSE 省略）。
    pub fn encode(&self) -> Vec<u8> {
        let mut inner = DerWriter::new();
        inner.write_oid(&self.oid);
        if self.critical {
            inner.write_boolean(true);
        }
        inner.write_octet_string(&self.value);

        let mut result = DerWriter::new();
        result.write_sequence(inner.as_bytes());
        result.into_bytes()
    }

    /// 从 DerReader 解析 Extension（reader 应位于 SEQUENCE tag 处）。
    pub fn decode(reader: &mut DerReader) -> Result<Self, PkiError> {
        let mut ext = reader.read_sequence().map_err(asn1_err)?;
        let oid = ext.read_oid().map_err(asn1_err)?;

        // 读取下一个元素，根据 tag 判断是 BOOLEAN(critical) 还是 OCTET STRING(value)
        let (critical, value) = if !ext.is_empty() {
            let (tag, content) = ext.read_element().map_err(asn1_err)?;
            if tag == asn1::BOOLEAN {
                let crit = !content.is_empty() && content[0] != 0x00;
                let val = ext.read_octet_string().map_err(asn1_err)?;
                (crit, val)
            } else if tag == asn1::OCTET_STRING {
                (false, content.to_vec())
            } else {
                return Err(PkiError::InvalidDerFormat);
            }
        } else {
            return Err(PkiError::InvalidDerFormat);
        };

        Ok(Extension {
            oid,
            critical,
            value,
        })
    }

    /// 是否为 KeyUsage 扩展（OID 2.5.29.15）。
    pub fn is_key_usage(&self) -> bool {
        self.oid == encode_oid(&[2, 5, 29, 15])
    }

    /// 是否为 ExtKeyUsage 扩展（OID 2.5.29.37）。
    pub fn is_ext_key_usage(&self) -> bool {
        self.oid == encode_oid(&[2, 5, 29, 37])
    }
}

// ============================================================================
// SubTask 3.5: KeyUsage
// ============================================================================

/// 密钥用法位图（RFC 5280 §4.2.1.3）。
///
/// 用 u16 表示前 9 位，按 RFC 5280 高位优先约定：
/// bit 0 = DIGITAL_SIGNATURE = 0x8000（u16 最高位）
/// bit 8 = DECIPHER_ONLY = 0x0080（u16 第二字节最高位）
///
/// DER 编码为 BIT STRING：内容 = `[0x00(无未用位), ku_high, ku_low]`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyUsage(u16);

impl KeyUsage {
    /// digitalSignature（bit 0）
    pub const DIGITAL_SIGNATURE: u16 = 0x8000;
    /// nonRepudiation（bit 1）
    pub const NON_REPUDIATION: u16 = 0x4000;
    /// keyEncipherment（bit 2）
    pub const KEY_ENCIPHERMENT: u16 = 0x2000;
    /// dataEncipherment（bit 3）
    pub const DATA_ENCIPHERMENT: u16 = 0x1000;
    /// keyAgreement（bit 4）
    pub const KEY_AGREEMENT: u16 = 0x0800;
    /// keyCertSign（bit 5）
    pub const KEY_CERT_SIGN: u16 = 0x0400;
    /// cRLSign（bit 6）
    pub const CRL_SIGN: u16 = 0x0200;
    /// encipherOnly（bit 7）
    pub const ENCIPHER_ONLY: u16 = 0x0100;
    /// decipherOnly（bit 8）
    pub const DECIPHER_ONLY: u16 = 0x0080;

    /// 从位标志创建 KeyUsage。
    pub fn new(bits: u16) -> Self {
        KeyUsage(bits)
    }

    /// 检查是否包含指定位标志。
    pub fn contains(&self, flag: u16) -> bool {
        self.0 & flag == flag
    }

    /// 累加位标志。
    pub fn add(&mut self, flag: u16) {
        self.0 |= flag;
    }

    /// 编码为 BIT STRING DER（完整 TLV：tag + length + 0x00 + 2 字节大端）。
    pub fn encode(&self) -> Vec<u8> {
        let bytes = self.0.to_be_bytes();
        let mut w = DerWriter::new();
        w.write_bit_string(&bytes);
        w.into_bytes()
    }

    /// 从 BIT STRING DER 解码（接受完整 BIT STRING TLV）。
    pub fn decode(bit_string_der: &[u8]) -> Result<Self, PkiError> {
        let mut r = DerReader::new(bit_string_der);
        let bytes = r.read_bit_string().map_err(asn1_err)?;
        if bytes.len() != 2 {
            return Err(PkiError::InvalidDerFormat);
        }
        let bits = ((bytes[0] as u16) << 8) | (bytes[1] as u16);
        Ok(KeyUsage(bits))
    }
}

// ============================================================================
// SubTask 3.6: ExtKeyUsage
// ============================================================================

/// 扩展密钥用法（RFC 5280 §4.2.1.12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtKeyUsage {
    /// TLS 服务器认证，OID 1.3.6.1.5.5.7.3.1
    ServerAuth,
    /// TLS 客户端认证，OID 1.3.6.1.5.5.7.3.2
    ClientAuth,
    /// 代码签名，OID 1.3.6.1.5.5.7.3.3
    CodeSigning,
    /// 电子邮件保护，OID 1.3.6.1.5.5.7.3.4
    EmailProtection,
}

impl ExtKeyUsage {
    /// 返回 OID 的 DER 内容字节。
    pub fn oid_bytes(&self) -> Vec<u8> {
        match self {
            ExtKeyUsage::ServerAuth => encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 1]),
            ExtKeyUsage::ClientAuth => encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 2]),
            ExtKeyUsage::CodeSigning => encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 3]),
            ExtKeyUsage::EmailProtection => encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 4]),
        }
    }

    /// 从 OID DER 内容字节解析 ExtKeyUsage。
    pub fn from_oid(oid_content: &[u8]) -> Result<Self, PkiError> {
        let server = encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 1]);
        let client = encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 2]);
        let code = encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 3]);
        let email = encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 4]);

        if oid_content == server.as_slice() {
            Ok(ExtKeyUsage::ServerAuth)
        } else if oid_content == client.as_slice() {
            Ok(ExtKeyUsage::ClientAuth)
        } else if oid_content == code.as_slice() {
            Ok(ExtKeyUsage::CodeSigning)
        } else if oid_content == email.as_slice() {
            Ok(ExtKeyUsage::EmailProtection)
        } else {
            Err(PkiError::UnsupportedAlgorithm)
        }
    }

    /// 编码为 ExtKeyUsageSequence DER：`SEQUENCE OF OID`。
    pub fn encode_sequence(usages: &[ExtKeyUsage]) -> Vec<u8> {
        let mut inner = DerWriter::new();
        for u in usages {
            inner.write_oid(&u.oid_bytes());
        }
        let mut result = DerWriter::new();
        result.write_sequence(inner.as_bytes());
        result.into_bytes()
    }

    /// 从 DerReader 解析 ExtKeyUsageSequence（reader 应位于 SEQUENCE tag 处）。
    pub fn decode_sequence(reader: &mut DerReader) -> Result<Vec<Self>, PkiError> {
        let mut seq = reader.read_sequence().map_err(asn1_err)?;
        let mut result = Vec::new();
        while !seq.is_empty() {
            let oid = seq.read_oid().map_err(asn1_err)?;
            result.push(ExtKeyUsage::from_oid(&oid)?);
        }
        Ok(result)
    }
}

// ============================================================================
// SubTask 3.7: X509Certificate
// ============================================================================

/// X.509 证书（支持 v1/v3）。
///
/// `version`: 0=v1, 1=v2, 2=v3。
/// `signature`: SM2 签名的 r‖s 拼接（64 字节）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X509Certificate {
    /// 版本：0=v1, 1=v2, 2=v3
    pub version: u8,
    /// 序列号（大端 INTEGER 字节，已剥离前导 0x00 padding）
    pub serial_number: Vec<u8>,
    /// TBS 中的签名算法标识
    pub signature_algorithm: SignatureAlgorithm,
    /// 颁发者可分辨名称
    pub issuer: DistinguishedName,
    /// 主体可分辨名称
    pub subject: DistinguishedName,
    /// 有效期起始（Unix 时间戳，秒）
    pub not_before: u64,
    /// 有效期截止（Unix 时间戳，秒）
    pub not_after: u64,
    /// 主体公钥
    public_key: SubjectPublicKey,
    /// v3 扩展列表
    pub extensions: Vec<Extension>,
    /// 签名值（SM2 r‖s，64 字节）
    pub signature: Vec<u8>,
}

impl X509Certificate {
    /// 构造新的 X.509 证书（用于 builder 模块组装已签名的证书）.
    ///
    /// # 参数
    /// - `version`: 0=v1, 1=v2, 2=v3
    /// - `serial_number`: 序列号（大端 INTEGER 字节）
    /// - `signature_algorithm`: 签名算法
    /// - `issuer`: 颁发者 DN
    /// - `subject`: 主体 DN
    /// - `not_before` / `not_after`: 有效期起止 Unix 时间戳（秒）
    /// - `public_key`: 主体公钥
    /// - `extensions`: v3 扩展列表（v1/v2 传空 Vec）
    /// - `signature`: 签名值（SM2 r‖s，64 字节）
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        version: u8,
        serial_number: Vec<u8>,
        signature_algorithm: SignatureAlgorithm,
        issuer: DistinguishedName,
        subject: DistinguishedName,
        not_before: u64,
        not_after: u64,
        public_key: SubjectPublicKey,
        extensions: Vec<Extension>,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            version,
            serial_number,
            signature_algorithm,
            issuer,
            subject,
            not_before,
            not_after,
            public_key,
            extensions,
            signature,
        }
    }

    /// 获取主体公钥的引用。
    pub fn public_key(&self) -> &SubjectPublicKey {
        &self.public_key
    }

    /// 编码 TBSCertificate 为 DER。
    ///
    /// 结构：
    /// ```text
    /// TBSCertificate ::= SEQUENCE {
    ///     version         [0] EXPLICIT INTEGER (v1 省略，v2/v3 显式)
    ///     serialNumber    INTEGER,
    ///     signature       AlgorithmIdentifier,
    ///     issuer          Name,
    ///     validity        SEQUENCE { notBefore Time, notAfter Time },
    ///     subject         Name,
    ///     subjectPKInfo   SubjectPublicKeyInfo,
    ///     extensions      [3] EXPLICIT SEQUENCE OF Extension (v3, 可选)
    /// }
    /// ```
    pub fn encode_tbs(&self) -> Result<Vec<u8>, PkiError> {
        let mut content = Vec::new();

        // version [0] EXPLICIT（仅 v2/v3 显式编码）
        if self.version >= 1 {
            let mut ver_inner = DerWriter::new();
            ver_inner.write_u64(self.version as u64);
            let mut ctx = DerWriter::new();
            ctx.write_context_explicit(0, ver_inner.as_bytes());
            content.extend_from_slice(ctx.as_bytes());
        }

        // serialNumber INTEGER
        let mut sn = DerWriter::new();
        sn.write_integer(&self.serial_number);
        content.extend_from_slice(sn.as_bytes());

        // signature AlgorithmIdentifier
        content.extend_from_slice(&self.signature_algorithm.encode_algorithm_identifier());

        // issuer Name
        content.extend_from_slice(&self.issuer.encode_rdn_sequence());

        // validity SEQUENCE { notBefore, notAfter }
        let mut val = DerWriter::new();
        val.write_utctime(self.not_before);
        val.write_utctime(self.not_after);
        let mut val_seq = DerWriter::new();
        val_seq.write_sequence(val.as_bytes());
        content.extend_from_slice(val_seq.as_bytes());

        // subject Name
        content.extend_from_slice(&self.subject.encode_rdn_sequence());

        // subjectPublicKeyInfo
        content.extend_from_slice(&self.public_key.encode_spki()?);

        // extensions [3] EXPLICIT SEQUENCE OF Extension（v3 且非空时）
        if self.version >= 2 && !self.extensions.is_empty() {
            let mut ext_content = Vec::new();
            for ext in &self.extensions {
                ext_content.extend_from_slice(&ext.encode());
            }
            let mut ext_seq = DerWriter::new();
            ext_seq.write_sequence(&ext_content);
            let mut ctx = DerWriter::new();
            ctx.write_context_explicit(3, ext_seq.as_bytes());
            content.extend_from_slice(ctx.as_bytes());
        }

        // 包装为 SEQUENCE
        let mut result = DerWriter::new();
        result.write_sequence(&content);
        Ok(result.into_bytes())
    }

    /// 编码完整 Certificate DER。
    ///
    /// 结构：
    /// ```text
    /// Certificate ::= SEQUENCE {
    ///     tbsCertificate     TBSCertificate,
    ///     signatureAlgorithm AlgorithmIdentifier,
    ///     signatureValue     BIT STRING
    /// }
    /// ```
    pub fn encode(&self) -> Result<Vec<u8>, PkiError> {
        let mut content = Vec::new();

        // TBS
        content.extend_from_slice(&self.encode_tbs()?);

        // signatureAlgorithm
        content.extend_from_slice(&self.signature_algorithm.encode_algorithm_identifier());

        // signatureValue BIT STRING
        let mut sig = DerWriter::new();
        sig.write_bit_string(&self.signature);
        content.extend_from_slice(sig.as_bytes());

        // 包装为 SEQUENCE
        let mut result = DerWriter::new();
        result.write_sequence(&content);
        Ok(result.into_bytes())
    }

    /// 从 DER 字节解析完整 Certificate。
    ///
    /// 支持 v1（无 version 显式标签）和 v3（含 extensions）。
    /// ASN.1 错误转为 `PkiError::Asn1Error`。
    pub fn decode(der: &[u8]) -> Result<Self, PkiError> {
        let mut r = DerReader::new(der);
        let mut cert = r.read_sequence().map_err(asn1_err)?;

        // 1. TBSCertificate
        let mut tbs = cert.read_sequence().map_err(asn1_err)?;

        // version [0] EXPLICIT（可选，默认 v1=0）
        let version;
        let serial_number;

        let (tag, content) = tbs.read_element().map_err(asn1_err)?;
        if tag == asn1::CONTEXT_0 {
            // version 字段存在
            let mut ver_reader = DerReader::new(content);
            version = ver_reader.read_u64().map_err(asn1_err)? as u8;
            // 接下来读取 serialNumber
            serial_number = tbs.read_integer().map_err(asn1_err)?;
        } else if tag == asn1::INTEGER {
            // 无 version 字段（v1 证书），当前元素是 serialNumber
            version = 0;
            serial_number = if content.is_empty() {
                Vec::new()
            } else if content.len() > 1 && content[0] == 0x00 {
                content[1..].to_vec()
            } else {
                content.to_vec()
            };
        } else {
            return Err(PkiError::InvalidDerFormat);
        }

        // signature AlgorithmIdentifier（TBS 内）
        let signature_algorithm = SignatureAlgorithm::decode_algorithm_identifier(&mut tbs)?;

        // issuer Name
        let issuer = DistinguishedName::decode_rdn_sequence(&mut tbs)?;

        // validity SEQUENCE { notBefore, notAfter }
        let mut validity = tbs.read_sequence().map_err(asn1_err)?;
        let not_before = read_time(&mut validity)?;
        let not_after = read_time(&mut validity)?;

        // subject Name
        let subject = DistinguishedName::decode_rdn_sequence(&mut tbs)?;

        // subjectPublicKeyInfo
        let public_key = SubjectPublicKey::decode_spki(&mut tbs)?;

        // extensions [3] EXPLICIT SEQUENCE OF Extension（可选，v3）
        // [3] EXPLICIT 内容为 SEQUENCE OF Extension，需先读外层 SEQUENCE
        let mut extensions = Vec::new();
        if !tbs.is_empty() {
            let mut ext_ctx = tbs.read_context_explicit(3).map_err(asn1_err)?;
            let mut ext_seq = ext_ctx.read_sequence().map_err(asn1_err)?;
            while !ext_seq.is_empty() {
                extensions.push(Extension::decode(&mut ext_seq)?);
            }
        }

        // 2. signatureAlgorithm（Certificate 级，应与 TBS 内一致）
        let _ = SignatureAlgorithm::decode_algorithm_identifier(&mut cert)?;

        // 3. signatureValue BIT STRING
        let signature = cert.read_bit_string().map_err(asn1_err)?;

        Ok(X509Certificate {
            version,
            serial_number,
            signature_algorithm,
            issuer,
            subject,
            not_before,
            not_after,
            public_key,
            extensions,
            signature,
        })
    }
}

// ============================================================================
// SubTask 3.8: CertRequest
// ============================================================================

/// 证书请求构建器。
///
/// 封装证书签发所需的主体信息、公钥、有效期与密钥用法，
/// 供 CA 签发流程使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertRequest {
    /// 主体可分辨名称
    pub subject: DistinguishedName,
    /// 主体公钥
    pub public_key: SubjectPublicKey,
    /// 有效期（天）
    pub validity_days: u32,
    /// 密钥用法
    pub key_usage: KeyUsage,
    /// 扩展密钥用法列表
    pub ext_key_usage: Vec<ExtKeyUsage>,
}

impl CertRequest {
    /// 创建证书请求（默认 validity_days=365, key_usage=DIGITAL_SIGNATURE, ext_key_usage=空）。
    pub fn new(subject: DistinguishedName, public_key: SubjectPublicKey) -> Self {
        Self {
            subject,
            public_key,
            validity_days: 365,
            key_usage: KeyUsage::new(KeyUsage::DIGITAL_SIGNATURE),
            ext_key_usage: Vec::new(),
        }
    }

    /// 设置有效期天数（builder）。
    pub fn with_validity_days(mut self, days: u32) -> Self {
        self.validity_days = days;
        self
    }

    /// 设置密钥用法（builder）。
    pub fn with_key_usage(mut self, ku: KeyUsage) -> Self {
        self.key_usage = ku;
        self
    }

    /// 添加扩展密钥用法（builder）。
    pub fn add_ext_key_usage(mut self, eku: ExtKeyUsage) -> Self {
        self.ext_key_usage.push(eku);
        self
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::CsRng;
    use crate::sm2::Sm2KeyPair;

    /// 生成测试用 SM2 密钥对。
    fn gen_keypair() -> Sm2KeyPair {
        let mut rng = CsRng::new();
        Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败")
    }

    // ===== SubTask 3.1: DistinguishedName 测试 =====

    #[test]
    fn test_dn_full_roundtrip() {
        let dn = DistinguishedName::new("Test CN")
            .with_o("Test Org")
            .with_ou("Test OU")
            .with_c("CN");

        let encoded = dn.encode_rdn_sequence();
        let mut reader = DerReader::new(&encoded);
        let decoded = DistinguishedName::decode_rdn_sequence(&mut reader).unwrap();

        assert_eq!(decoded, dn);
        assert_eq!(decoded.cn, "Test CN");
        assert_eq!(decoded.o.as_deref(), Some("Test Org"));
        assert_eq!(decoded.ou.as_deref(), Some("Test OU"));
        assert_eq!(decoded.c.as_deref(), Some("CN"));
    }

    #[test]
    fn test_dn_cn_only_roundtrip() {
        let dn = DistinguishedName::new("Simple CN");
        let encoded = dn.encode_rdn_sequence();

        let mut reader = DerReader::new(&encoded);
        let decoded = DistinguishedName::decode_rdn_sequence(&mut reader).unwrap();

        assert_eq!(decoded, dn);
        assert_eq!(decoded.cn, "Simple CN");
        assert!(decoded.o.is_none());
        assert!(decoded.ou.is_none());
        assert!(decoded.c.is_none());
    }

    #[test]
    fn test_dn_ignores_unknown_oid() {
        // 手动构造包含未知 OID 的 RDNSequence
        let mut content = Vec::new();
        // CN 属性
        content.extend_from_slice(&encode_rdn_attr(&encode_oid(&[2, 5, 4, 3]), "Known CN"));
        // 未知 OID 属性 (1.2.3.4.5)
        content.extend_from_slice(&encode_rdn_attr(
            &encode_oid(&[1, 2, 3, 4, 5]),
            "Unknown Value",
        ));

        let mut wrapper = DerWriter::new();
        wrapper.write_sequence(&content);
        let der = wrapper.into_bytes();

        let mut reader = DerReader::new(&der);
        let dn = DistinguishedName::decode_rdn_sequence(&mut reader).unwrap();

        // 未知 OID 被忽略，CN 正常解析
        assert_eq!(dn.cn, "Known CN");
        assert!(dn.o.is_none());
    }

    // ===== SubTask 3.2: SubjectPublicKey 测试 =====

    #[test]
    fn test_spki_sm2_roundtrip() {
        let kp = gen_keypair();
        let pk = SubjectPublicKey::Sm2(kp.public_key);

        let encoded = pk.encode_spki().unwrap();

        let mut reader = DerReader::new(&encoded);
        let decoded = SubjectPublicKey::decode_spki(&mut reader).unwrap();

        assert_eq!(decoded, pk);
    }

    #[test]
    fn test_spki_rsa_unsupported() {
        let rsa_pk = SubjectPublicKey::Rsa(vec![0x01, 0x02, 0x03]);
        let result = rsa_pk.encode_spki();
        assert_eq!(result, Err(PkiError::UnsupportedAlgorithm));
    }

    // ===== SubTask 3.3: SignatureAlgorithm 测试 =====

    #[test]
    fn test_sig_alg_sm2_roundtrip() {
        let alg = SignatureAlgorithm::Sm2WithSm3;
        let encoded = alg.encode_algorithm_identifier();

        let mut reader = DerReader::new(&encoded);
        let decoded = SignatureAlgorithm::decode_algorithm_identifier(&mut reader).unwrap();

        assert_eq!(decoded, alg);
    }

    #[test]
    fn test_sig_alg_ecdsa_roundtrip() {
        let alg = SignatureAlgorithm::EcdsaWithSha256;
        let encoded = alg.encode_algorithm_identifier();

        let mut reader = DerReader::new(&encoded);
        let decoded = SignatureAlgorithm::decode_algorithm_identifier(&mut reader).unwrap();

        assert_eq!(decoded, alg);
    }

    #[test]
    fn test_sig_alg_from_oid_unsupported() {
        let unknown_oid = encode_oid(&[1, 2, 3, 4, 5]);
        let result = SignatureAlgorithm::from_oid(&unknown_oid);
        assert_eq!(result, Err(PkiError::UnsupportedAlgorithm));
    }

    // ===== SubTask 3.4: Extension 测试 =====

    #[test]
    fn test_extension_non_critical_no_boolean() {
        let ext = Extension {
            oid: encode_oid(&[2, 5, 29, 15]),
            critical: false,
            value: vec![0x03, 0x03, 0x00, 0x80, 0x00],
        };
        let encoded = ext.encode();

        // 解码验证不含 BOOLEAN
        let mut reader = DerReader::new(&encoded);
        let mut seq = reader.read_sequence().unwrap();
        let _oid = seq.read_oid().unwrap();
        let (tag, _) = seq.read_element().unwrap();
        // critical=false 时下一个元素应该是 OCTET_STRING，不是 BOOLEAN
        assert_eq!(tag, asn1::OCTET_STRING);
    }

    #[test]
    fn test_extension_critical_has_boolean_true() {
        let ext = Extension {
            oid: encode_oid(&[2, 5, 29, 15]),
            critical: true,
            value: vec![0x03, 0x03, 0x00, 0x80, 0x00],
        };
        let encoded = ext.encode();

        // 解码验证含 BOOLEAN TRUE
        let mut reader = DerReader::new(&encoded);
        let mut seq = reader.read_sequence().unwrap();
        let _oid = seq.read_oid().unwrap();
        let (tag, content) = seq.read_element().unwrap();
        assert_eq!(tag, asn1::BOOLEAN);
        assert!(!content.is_empty());
        assert_ne!(content[0], 0x00); // TRUE
    }

    #[test]
    fn test_extension_roundtrip() {
        let ext = Extension {
            oid: encode_oid(&[2, 5, 29, 37]),
            critical: true,
            value: vec![0x30, 0x06, 0x06, 0x04, 0x2B, 0x06, 0x01, 0x05],
        };

        let encoded = ext.encode();
        let mut reader = DerReader::new(&encoded);
        let decoded = Extension::decode(&mut reader).unwrap();

        assert_eq!(decoded, ext);
    }

    #[test]
    fn test_extension_is_key_usage_and_ext_key_usage() {
        let ku_ext = Extension {
            oid: encode_oid(&[2, 5, 29, 15]),
            critical: true,
            value: vec![],
        };
        assert!(ku_ext.is_key_usage());
        assert!(!ku_ext.is_ext_key_usage());

        let eku_ext = Extension {
            oid: encode_oid(&[2, 5, 29, 37]),
            critical: false,
            value: vec![],
        };
        assert!(!eku_ext.is_key_usage());
        assert!(eku_ext.is_ext_key_usage());
    }

    // ===== SubTask 3.5: KeyUsage 测试 =====

    #[test]
    fn test_key_usage_contains() {
        let ku = KeyUsage::new(KeyUsage::DIGITAL_SIGNATURE | KeyUsage::KEY_CERT_SIGN);

        assert!(ku.contains(KeyUsage::DIGITAL_SIGNATURE));
        assert!(ku.contains(KeyUsage::KEY_CERT_SIGN));
        assert!(!ku.contains(KeyUsage::CRL_SIGN));
        assert!(!ku.contains(KeyUsage::NON_REPUDIATION));
    }

    #[test]
    fn test_key_usage_add() {
        let mut ku = KeyUsage::new(KeyUsage::DIGITAL_SIGNATURE);
        assert!(!ku.contains(KeyUsage::KEY_CERT_SIGN));

        ku.add(KeyUsage::KEY_CERT_SIGN);
        assert!(ku.contains(KeyUsage::DIGITAL_SIGNATURE));
        assert!(ku.contains(KeyUsage::KEY_CERT_SIGN));

        ku.add(KeyUsage::CRL_SIGN);
        assert!(ku.contains(KeyUsage::CRL_SIGN));
    }

    #[test]
    fn test_key_usage_encode_decode_roundtrip() {
        let ku = KeyUsage::new(
            KeyUsage::DIGITAL_SIGNATURE | KeyUsage::KEY_CERT_SIGN | KeyUsage::CRL_SIGN,
        );
        let encoded = ku.encode();
        let decoded = KeyUsage::decode(&encoded).unwrap();

        assert_eq!(decoded, ku);
        assert!(decoded.contains(KeyUsage::DIGITAL_SIGNATURE));
        assert!(decoded.contains(KeyUsage::KEY_CERT_SIGN));
        assert!(decoded.contains(KeyUsage::CRL_SIGN));
    }

    // ===== SubTask 3.6: ExtKeyUsage 测试 =====

    #[test]
    fn test_ext_key_usage_sequence_roundtrip() {
        let usages = vec![
            ExtKeyUsage::ServerAuth,
            ExtKeyUsage::ClientAuth,
            ExtKeyUsage::CodeSigning,
        ];

        let encoded = ExtKeyUsage::encode_sequence(&usages);
        let mut reader = DerReader::new(&encoded);
        let decoded = ExtKeyUsage::decode_sequence(&mut reader).unwrap();

        assert_eq!(decoded, usages);
    }

    #[test]
    fn test_ext_key_usage_from_oid() {
        assert_eq!(
            ExtKeyUsage::from_oid(&encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 1])),
            Ok(ExtKeyUsage::ServerAuth)
        );
        assert_eq!(
            ExtKeyUsage::from_oid(&encode_oid(&[1, 3, 6, 1, 5, 5, 7, 3, 4])),
            Ok(ExtKeyUsage::EmailProtection)
        );
    }

    // ===== SubTask 3.7: X509Certificate 测试 =====

    /// 构造测试用 v3 证书。
    fn make_test_v3_cert() -> X509Certificate {
        let kp = gen_keypair();
        X509Certificate {
            version: 2, // v3
            serial_number: vec![0x01, 0x02, 0x03],
            signature_algorithm: SignatureAlgorithm::Sm2WithSm3,
            issuer: DistinguishedName::new("Test CA")
                .with_o("Test Org")
                .with_c("CN"),
            subject: DistinguishedName::new("Test Subject")
                .with_o("Test Org")
                .with_ou("Test OU"),
            not_before: 1704067200, // 2024-01-01 00:00:00 UTC
            not_after: 1735689600,  // 2025-01-01 00:00:00 UTC
            public_key: SubjectPublicKey::Sm2(kp.public_key),
            extensions: vec![Extension {
                oid: encode_oid(&[2, 5, 29, 15]),
                critical: true,
                value: KeyUsage::new(KeyUsage::DIGITAL_SIGNATURE | KeyUsage::KEY_CERT_SIGN)
                    .encode(),
            }],
            signature: vec![0xAA; 64],
        }
    }

    #[test]
    fn test_x509_encode_tbs_valid_der() {
        let cert = make_test_v3_cert();
        let tbs_der = cert.encode_tbs().unwrap();

        // 验证 TBS 是有效的 SEQUENCE
        let mut r = DerReader::new(&tbs_der);
        let mut tbs = r.read_sequence().unwrap();
        assert!(r.is_empty());

        // v3: 第一个元素应为 [0] EXPLICIT
        let (tag, _) = tbs.read_element().unwrap();
        assert_eq!(tag, asn1::CONTEXT_0);
    }

    #[test]
    fn test_x509_full_roundtrip_v3() {
        let cert = make_test_v3_cert();
        let der = cert.encode().unwrap();
        let decoded = X509Certificate::decode(&der).unwrap();

        assert_eq!(decoded, cert);
    }

    #[test]
    fn test_x509_v1_no_version_field() {
        let kp = gen_keypair();
        let cert = X509Certificate {
            version: 0, // v1
            serial_number: vec![0x42],
            signature_algorithm: SignatureAlgorithm::Sm2WithSm3,
            issuer: DistinguishedName::new("CA"),
            subject: DistinguishedName::new("Subject"),
            not_before: 1704067200,
            not_after: 1735689600,
            public_key: SubjectPublicKey::Sm2(kp.public_key),
            extensions: Vec::new(),
            signature: vec![0xBB; 64],
        };

        let der = cert.encode().unwrap();
        let decoded = X509Certificate::decode(&der).unwrap();

        assert_eq!(decoded.version, 0);
        assert_eq!(decoded, cert);
    }

    #[test]
    fn test_x509_v3_with_extensions_roundtrip() {
        let kp = gen_keypair();
        let ku_ext = Extension {
            oid: encode_oid(&[2, 5, 29, 15]),
            critical: true,
            value: KeyUsage::new(KeyUsage::DIGITAL_SIGNATURE | KeyUsage::KEY_CERT_SIGN).encode(),
        };
        let eku_ext = Extension {
            oid: encode_oid(&[2, 5, 29, 37]),
            critical: false,
            value: ExtKeyUsage::encode_sequence(&[
                ExtKeyUsage::ServerAuth,
                ExtKeyUsage::ClientAuth,
            ]),
        };

        let cert = X509Certificate {
            version: 2,
            serial_number: vec![0x01],
            signature_algorithm: SignatureAlgorithm::Sm2WithSm3,
            issuer: DistinguishedName::new("Root CA"),
            subject: DistinguishedName::new("Intermediate CA"),
            not_before: 1704067200,
            not_after: 1768492800, // 2026-01-01
            public_key: SubjectPublicKey::Sm2(kp.public_key),
            extensions: vec![ku_ext, eku_ext],
            signature: vec![0xCC; 64],
        };

        let der = cert.encode().unwrap();
        let decoded = X509Certificate::decode(&der).unwrap();

        assert_eq!(decoded, cert);
        assert_eq!(decoded.extensions.len(), 2);
        assert!(decoded.extensions[0].is_key_usage());
        assert!(decoded.extensions[1].is_ext_key_usage());
    }

    // ===== SubTask 3.8: CertRequest 测试 =====

    #[test]
    fn test_cert_request_builder_chain() {
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Test Subject").with_o("Test Org"),
            SubjectPublicKey::Sm2(kp.public_key),
        )
        .with_validity_days(730)
        .with_key_usage(KeyUsage::new(
            KeyUsage::DIGITAL_SIGNATURE | KeyUsage::KEY_ENCIPHERMENT,
        ))
        .add_ext_key_usage(ExtKeyUsage::ServerAuth)
        .add_ext_key_usage(ExtKeyUsage::ClientAuth);

        assert_eq!(req.subject.cn, "Test Subject");
        assert_eq!(req.subject.o.as_deref(), Some("Test Org"));
        assert_eq!(req.validity_days, 730);
        assert!(req.key_usage.contains(KeyUsage::DIGITAL_SIGNATURE));
        assert!(req.key_usage.contains(KeyUsage::KEY_ENCIPHERMENT));
        assert_eq!(req.ext_key_usage.len(), 2);
        assert_eq!(req.ext_key_usage[0], ExtKeyUsage::ServerAuth);
        assert_eq!(req.ext_key_usage[1], ExtKeyUsage::ClientAuth);
    }

    #[test]
    fn test_cert_request_default_values() {
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Default Subject"),
            SubjectPublicKey::Sm2(kp.public_key),
        );

        assert_eq!(req.validity_days, 365);
        assert!(req.key_usage.contains(KeyUsage::DIGITAL_SIGNATURE));
        assert!(req.ext_key_usage.is_empty());
    }
}
