//! ASN.1 DER 编解码器 (v0.32.0 Task 2).
//!
//! 提供最小化 ASN.1 DER (Distinguished Encoding Rules) 编解码能力，
//! 支持 X.509 证书所需的 tag 子集。零外部依赖，纯 Rust + no_std。
//!
//! # 核心组件
//! - [`DerReader`]：光标式 DER 读取器，按 TLV (Tag-Length-Value) 结构逐元素解析。
//! - [`DerWriter`]：DER 写入器，自动处理长度编码（短格式/长格式）。
//! - [`encode_oid`] / [`decode_oid`]：OID 弧列表与 base-128 DER 内容字节互转。
//! - [`Asn1Error`]：ASN.1 编解码错误类型（6 变体）。
//!
//! # 支持的 ASN.1 类型
//! - INTEGER（正整数，大端字节，自动前导 0x00 处理）
//! - BIT STRING（自动 unused-bits 首字节处理）
//! - OCTET STRING
//! - OBJECT IDENTIFIER（OID，base-128 编码）
//! - SEQUENCE / SET（构造类型）
//! - UTCTime（YYMMDDHHMMSSZ，2 位年份：< 50 → 20XX，≥ 50 → 19XX）
//! - GeneralizedTime（YYYYMMDDHHMMSSZ）
//! - BOOLEAN / NULL
//! - Context-specific [0]..[3] EXPLICIT（X.509 版本/扩展）
//!
//! # no_std 合规
//! no_std 由 crate 根继承，本模块通过 `extern crate alloc` 引入堆分配。
//! 使用 `alloc::vec::Vec`，不使用 `std::*`。
//!
//! # 参考
//! - ITU-T X.690 (07/2002) ASN.1 encoding rules
//! - RFC 5280 §4.1 (X.509 certificate structure)
//! - GB/T 35275 SM2 密码算法加密签名消息语法规范

extern crate alloc;

use alloc::vec::Vec;

// ============================================================================
// ASN.1 Tag 常量
// ============================================================================

/// BOOLEAN tag (0x01)
pub const BOOLEAN: u8 = 0x01;
/// INTEGER tag (0x02)
pub const INTEGER: u8 = 0x02;
/// BIT STRING tag (0x03)
pub const BIT_STRING: u8 = 0x03;
/// OCTET STRING tag (0x04)
pub const OCTET_STRING: u8 = 0x04;
/// NULL tag (0x05)
pub const NULL: u8 = 0x05;
/// OBJECT IDENTIFIER tag (0x06)
pub const OID: u8 = 0x06;
/// UTF8String tag (0x0C)
pub const UTF8_STRING: u8 = 0x0C;
/// SEQUENCE tag (0x30, constructed)
pub const SEQUENCE: u8 = 0x30;
/// SET tag (0x31, constructed)
pub const SET: u8 = 0x31;
/// UTCTime tag (0x17)
pub const UTC_TIME: u8 = 0x17;
/// GeneralizedTime tag (0x18)
pub const GENERALIZED_TIME: u8 = 0x18;
/// Context-specific [0] constructed EXPLICIT (0xA0) — X.509 version
pub const CONTEXT_0: u8 = 0xA0;
/// Context-specific [3] constructed EXPLICIT (0xA3) — X.509 extensions
pub const CONTEXT_3: u8 = 0xA3;

// ============================================================================
// 错误类型
// ============================================================================

/// ASN.1 DER 编解码错误类型（6 变体）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asn1Error {
    /// 数据意外结束（truncated input）
    Truncated,
    /// 长度字段无效（如 indefinite length、超范围）
    InvalidLength,
    /// Tag 不匹配（期望 tag 与实际 tag 不符）
    InvalidTag {
        /// 期望的 tag 值
        expected: u8,
        /// 实际读取的 tag 值
        actual: u8,
    },
    /// OID 编码无效
    InvalidOid,
    /// 时间格式无效或超出范围
    InvalidTime,
    /// 数值溢出（u64 转换失败等）
    Overflow,
}

// ============================================================================
// DerReader — 光标式 DER 读取器
// ============================================================================

/// DER 读取器，光标式按 TLV 结构逐元素解析.
///
/// 生命周期 `'a` 绑定到输入数据的引用，所有返回的内容切片均借用原始数据。
pub struct DerReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> DerReader<'a> {
    /// 创建 DER 读取器，绑定到输入数据切片.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// 是否已读取到数据末尾.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// 剩余未读取的字节数.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// 读取一个 DER 元素，返回 (tag, content_bytes).
    ///
    /// 解析 Tag（1 字节）+ Length（短格式 ≤ 127 或长格式 ≥ 128）+ Value。
    /// 返回的 content 切片借用原始数据（零拷贝）。
    pub fn read_element(&mut self) -> Result<(u8, &'a [u8]), Asn1Error> {
        if self.pos >= self.data.len() {
            return Err(Asn1Error::Truncated);
        }
        let tag = self.data[self.pos];
        self.pos += 1;
        let len = read_length(self.data, &mut self.pos)?;
        if self.pos + len > self.data.len() {
            return Err(Asn1Error::Truncated);
        }
        let content = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok((tag, content))
    }

    /// 读取 INTEGER（正整数），返回大端字节（去掉前导 0x00 padding）.
    ///
    /// DER INTEGER 用前导 0x00 保持正数符号（当 MSB 置位时），
    /// 本函数剥离该 padding，返回纯数值字节。
    pub fn read_integer(&mut self) -> Result<Vec<u8>, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != INTEGER {
            return Err(Asn1Error::InvalidTag {
                expected: INTEGER,
                actual: tag,
            });
        }
        if content.is_empty() {
            return Err(Asn1Error::Truncated);
        }
        // 去掉前导 0x00（仅在有多字节时才剥离，保留单独的 0x00 表示值 0）
        let start = if content.len() > 1 && content[0] == 0x00 {
            1
        } else {
            0
        };
        Ok(content[start..].to_vec())
    }

    /// 读取 u64 INTEGER.
    ///
    /// 将 INTEGER 的内容字节解析为 u64 大端无符号整数。
    /// 超过 8 字节（不含 padding）的整数返回 `Overflow` 错误。
    pub fn read_u64(&mut self) -> Result<u64, Asn1Error> {
        let bytes = self.read_integer()?;
        if bytes.is_empty() {
            return Ok(0);
        }
        if bytes.len() > 8 {
            return Err(Asn1Error::Overflow);
        }
        let mut result: u64 = 0;
        for &b in &bytes {
            result = (result << 8) | (b as u64);
        }
        Ok(result)
    }

    /// 读取 OID，返回 DER 内容字节（即 base-128 编码的弧列表）.
    ///
    /// 返回的是原始 DER content（不含 tag 和 length），可直接传给 [`decode_oid`]。
    pub fn read_oid(&mut self) -> Result<Vec<u8>, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != OID {
            return Err(Asn1Error::InvalidTag {
                expected: OID,
                actual: tag,
            });
        }
        Ok(content.to_vec())
    }

    /// 读取 BIT STRING，返回去掉首字节 unused-bits 后的内容.
    ///
    /// DER BIT STRING 的第一个 content 字节是 unused bits 数（通常为 0），
    /// 本函数剥离该字节，返回实际比特数据。
    pub fn read_bit_string(&mut self) -> Result<Vec<u8>, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != BIT_STRING {
            return Err(Asn1Error::InvalidTag {
                expected: BIT_STRING,
                actual: tag,
            });
        }
        if content.is_empty() {
            return Err(Asn1Error::Truncated);
        }
        // 首字节是 unused bits（通常为 0），跳过
        Ok(content[1..].to_vec())
    }

    /// 读取 OCTET STRING，返回内容字节.
    pub fn read_octet_string(&mut self) -> Result<Vec<u8>, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != OCTET_STRING {
            return Err(Asn1Error::InvalidTag {
                expected: OCTET_STRING,
                actual: tag,
            });
        }
        Ok(content.to_vec())
    }

    /// 读取 SEQUENCE，返回 content 的 DerReader（用于继续解析嵌套元素）.
    pub fn read_sequence(&mut self) -> Result<DerReader<'a>, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != SEQUENCE {
            return Err(Asn1Error::InvalidTag {
                expected: SEQUENCE,
                actual: tag,
            });
        }
        Ok(DerReader::new(content))
    }

    /// 读取 SET，返回 content 的 DerReader.
    pub fn read_set(&mut self) -> Result<DerReader<'a>, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != SET {
            return Err(Asn1Error::InvalidTag {
                expected: SET,
                actual: tag,
            });
        }
        Ok(DerReader::new(content))
    }

    /// 读取 UTCTime（YYMMDDHHMMSSZ → u64 Unix 时间戳）.
    ///
    /// 2 位年份规则：YY < 50 → 20XX，YY ≥ 50 → 19XX（RFC 5280 §4.1.2.5.1）。
    /// 1970 年以前的日期返回 `InvalidTime`（u64 无法表示负时间戳）。
    pub fn read_utctime(&mut self) -> Result<u64, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != UTC_TIME {
            return Err(Asn1Error::InvalidTag {
                expected: UTC_TIME,
                actual: tag,
            });
        }
        // 格式：YYMMDDHHMMSSZ（13 字节）
        if content.len() != 13 || content[12] != b'Z' {
            return Err(Asn1Error::InvalidTime);
        }
        let yy = parse_two_digits(&content[0..2])?;
        let month = parse_two_digits(&content[2..4])?;
        let day = parse_two_digits(&content[4..6])?;
        let hour = parse_two_digits(&content[6..8])?;
        let minute = parse_two_digits(&content[8..10])?;
        let second = parse_two_digits(&content[10..12])?;
        let year = if yy < 50 {
            2000 + yy as i64
        } else {
            1900 + yy as i64
        };
        validate_time_fields(year, month, day, hour, minute, second)?;
        date_to_unix(year, month, day, hour, minute, second).ok_or(Asn1Error::InvalidTime)
    }

    /// 读取 GeneralizedTime（YYYYMMDDHHMMSSZ → u64 Unix 时间戳）.
    pub fn read_generalized_time(&mut self) -> Result<u64, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != GENERALIZED_TIME {
            return Err(Asn1Error::InvalidTag {
                expected: GENERALIZED_TIME,
                actual: tag,
            });
        }
        // 格式：YYYYMMDDHHMMSSZ（15 字节）
        if content.len() != 15 || content[14] != b'Z' {
            return Err(Asn1Error::InvalidTime);
        }
        let year = parse_four_digits(&content[0..4])? as i64;
        let month = parse_two_digits(&content[4..6])?;
        let day = parse_two_digits(&content[6..8])?;
        let hour = parse_two_digits(&content[8..10])?;
        let minute = parse_two_digits(&content[10..12])?;
        let second = parse_two_digits(&content[12..14])?;
        validate_time_fields(year, month, day, hour, minute, second)?;
        date_to_unix(year, month, day, hour, minute, second).ok_or(Asn1Error::InvalidTime)
    }

    /// 读取 BOOLEAN.
    pub fn read_boolean(&mut self) -> Result<bool, Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != BOOLEAN {
            return Err(Asn1Error::InvalidTag {
                expected: BOOLEAN,
                actual: tag,
            });
        }
        if content.len() != 1 {
            return Err(Asn1Error::InvalidLength);
        }
        Ok(content[0] != 0x00)
    }

    /// 读取 NULL（验证 tag 后跳过）.
    pub fn read_null(&mut self) -> Result<(), Asn1Error> {
        let (tag, content) = self.read_element()?;
        if tag != NULL {
            return Err(Asn1Error::InvalidTag {
                expected: NULL,
                actual: tag,
            });
        }
        if !content.is_empty() {
            return Err(Asn1Error::InvalidLength);
        }
        Ok(())
    }

    /// 读取 context-specific [n] EXPLICIT，返回 content 的 DerReader.
    ///
    /// X.509 中 [0] 用于版本，[3] 用于扩展。tag = 0xA0 | n。
    pub fn read_context_explicit(&mut self, n: u8) -> Result<DerReader<'a>, Asn1Error> {
        let expected_tag = 0xA0 | (n & 0x1F);
        let (tag, content) = self.read_element()?;
        if tag != expected_tag {
            return Err(Asn1Error::InvalidTag {
                expected: expected_tag,
                actual: tag,
            });
        }
        Ok(DerReader::new(content))
    }
}

// ============================================================================
// DerWriter — DER 写入器
// ============================================================================

/// DER 写入器，自动处理长度编码（短格式/长格式）.
///
/// 所有写入方法追加到内部缓冲区，最终通过 [`into_bytes`](Self::into_bytes)
/// 或 [`as_bytes`](Self::as_bytes) 获取完整 DER 编码。
#[derive(Default)]
pub struct DerWriter {
    buf: Vec<u8>,
}

impl DerWriter {
    /// 创建空的 DER 写入器.
    pub fn new() -> Self {
        Self::default()
    }

    /// 消费写入器，返回内部缓冲区的所有字节.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// 返回已写入字节的切片引用.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// 写入一个 DER 元素（tag + length + content）.
    ///
    /// 这是所有其他写入方法的基础。长度自动编码为短格式（≤ 127）
    /// 或长格式（≥ 128）。
    pub fn write_element(&mut self, tag: u8, content: &[u8]) {
        self.buf.push(tag);
        write_length(&mut self.buf, content.len());
        self.buf.extend_from_slice(content);
    }

    /// 写入 INTEGER（正整数，大端字节）.
    ///
    /// 自动剥离输入中的前导 0x00，并在 MSB 置位时添加前导 0x00
    /// 以保持 DER 正数符号。
    pub fn write_integer(&mut self, bytes: &[u8]) {
        let mut content = Vec::new();
        // 剥离前导 0x00
        let mut start = 0;
        while start < bytes.len() && bytes[start] == 0 {
            start += 1;
        }
        if start == bytes.len() {
            // 全零 → 值 0
            content.push(0x00);
        } else {
            // MSB 置位时添加前导 0x00
            if bytes[start] & 0x80 != 0 {
                content.push(0x00);
            }
            content.extend_from_slice(&bytes[start..]);
        }
        self.write_element(INTEGER, &content);
    }

    /// 写入 u64 INTEGER.
    ///
    /// 将 u64 转换为大端字节后调用 [`write_integer`](Self::write_integer)。
    pub fn write_u64(&mut self, val: u64) {
        let bytes = val.to_be_bytes();
        self.write_integer(&bytes);
    }

    /// 写入 OID（输入是 DER 内容字节，即 base-128 编码）.
    ///
    /// 可通过 [`encode_oid`] 从弧列表生成 content。
    pub fn write_oid(&mut self, content: &[u8]) {
        self.write_element(OID, content);
    }

    /// 写入 BIT STRING（自动添加 unused-bits 首字节 0x00）.
    pub fn write_bit_string(&mut self, content: &[u8]) {
        let mut wrapped = Vec::with_capacity(content.len() + 1);
        wrapped.push(0x00); // unused bits = 0
        wrapped.extend_from_slice(content);
        self.write_element(BIT_STRING, &wrapped);
    }

    /// 写入 OCTET STRING.
    pub fn write_octet_string(&mut self, content: &[u8]) {
        self.write_element(OCTET_STRING, content);
    }

    /// 写入 SEQUENCE（content 为已编码的内部元素）.
    pub fn write_sequence(&mut self, content: &[u8]) {
        self.write_element(SEQUENCE, content);
    }

    /// 写入 SET（content 为已编码的内部元素）.
    pub fn write_set(&mut self, content: &[u8]) {
        self.write_element(SET, content);
    }

    /// 写入 UTCTime（u64 Unix 时间戳 → YYMMDDHHMMSSZ）.
    ///
    /// 年份取后 2 位。RFC 5280 规定 UTCTime 用于 1950-2049 年的日期。
    pub fn write_utctime(&mut self, unix: u64) {
        let (year, month, day, hour, minute, second) = unix_to_date(unix);
        let yy = (year % 100) as u32;
        let mut content = Vec::with_capacity(13);
        write_two_digits(&mut content, yy);
        write_two_digits(&mut content, month);
        write_two_digits(&mut content, day);
        write_two_digits(&mut content, hour);
        write_two_digits(&mut content, minute);
        write_two_digits(&mut content, second);
        content.push(b'Z');
        self.write_element(UTC_TIME, &content);
    }

    /// 写入 GeneralizedTime（u64 → YYYYMMDDHHMMSSZ）.
    pub fn write_generalized_time(&mut self, unix: u64) {
        let (year, month, day, hour, minute, second) = unix_to_date(unix);
        let mut content = Vec::with_capacity(15);
        write_four_digits(&mut content, year as u32);
        write_two_digits(&mut content, month);
        write_two_digits(&mut content, day);
        write_two_digits(&mut content, hour);
        write_two_digits(&mut content, minute);
        write_two_digits(&mut content, second);
        content.push(b'Z');
        self.write_element(GENERALIZED_TIME, &content);
    }

    /// 写入 BOOLEAN.
    pub fn write_boolean(&mut self, val: bool) {
        let content = if val { [0xFF] } else { [0x00] };
        self.write_element(BOOLEAN, &content);
    }

    /// 写入 NULL.
    pub fn write_null(&mut self) {
        self.write_element(NULL, &[]);
    }

    /// 写入 context-specific [n] EXPLICIT.
    ///
    /// tag = 0xA0 | n。content 为已编码的内部元素。
    pub fn write_context_explicit(&mut self, n: u8, content: &[u8]) {
        let tag = 0xA0 | (n & 0x1F);
        self.write_element(tag, content);
    }
}

// ============================================================================
// OID 编解码辅助函数
// ============================================================================

/// 将 OID 弧列表编码为 base-128 DER 内容字节.
///
/// 示例：`[1, 2, 156, 10197, 1, 301]`（SM2 OID）→
/// `[0x2A, 0x81, 0x1C, 0xCF, 0x55, 0x01, 0x82, 0x2D]`
///
/// 前两个弧合并为 `40 * arc[0] + arc[1]`，后续弧独立 base-128 编码。
pub fn encode_oid(arcs: &[u64]) -> Vec<u8> {
    let mut result = Vec::new();
    if arcs.len() < 2 {
        return result;
    }
    // 前两个弧合并编码
    let first = arcs[0].wrapping_mul(40).wrapping_add(arcs[1]);
    encode_base128(first, &mut result);
    for arc in &arcs[2..] {
        encode_base128(*arc, &mut result);
    }
    result
}

/// 将 base-128 DER 内容字节解码为 OID 弧列表.
///
/// 示例：`[0x2A, 0x81, 0x1C, 0xCF, 0x55, 0x01, 0x82, 0x2D]` →
/// `[1, 2, 156, 10197, 1, 301]`
pub fn decode_oid(content: &[u8]) -> Result<Vec<u64>, Asn1Error> {
    if content.is_empty() {
        return Err(Asn1Error::InvalidOid);
    }
    let mut arcs = Vec::new();
    // 解码第一个数（合并的前两个弧）
    let (first, consumed) = decode_base128(content)?;

    // 拆分为前两个弧
    if first < 40 {
        arcs.push(0);
        arcs.push(first);
    } else if first < 80 {
        arcs.push(1);
        arcs.push(first - 40);
    } else {
        arcs.push(2);
        arcs.push(first - 80);
    }

    // 解码剩余弧
    let mut pos = consumed;
    while pos < content.len() {
        let (val, consumed) = decode_base128(&content[pos..])?;
        arcs.push(val);
        pos += consumed;
    }

    Ok(arcs)
}

// ============================================================================
// 内部辅助函数
// ============================================================================

/// 将 u64 编码为 base-128 并追加到输出缓冲区.
fn encode_base128(val: u64, out: &mut Vec<u8>) {
    if val == 0 {
        out.push(0x00);
        return;
    }
    // 收集 7-bit 分组（LSB 优先）
    let mut tmp = val;
    let mut bytes = Vec::new();
    while tmp > 0 {
        bytes.push((tmp & 0x7F) as u8);
        tmp >>= 7;
    }
    // 反向输出（MSB 优先），除最后一字节外均设置高位
    for i in (0..bytes.len()).rev() {
        let mut b = bytes[i];
        if i != 0 {
            b |= 0x80;
        }
        out.push(b);
    }
}

/// 从数据起始处解码一个 base-128 数值，返回 (值, 消费字节数).
fn decode_base128(data: &[u8]) -> Result<(u64, usize), Asn1Error> {
    let mut result: u64 = 0;
    for (i, &b) in data.iter().enumerate() {
        // 溢出检查：result 已使用 7*i 位，若继续移位会超出 u64
        if result >> 57 != 0 {
            return Err(Asn1Error::Overflow);
        }
        result = (result << 7) | ((b & 0x7F) as u64);
        if b & 0x80 == 0 {
            return Ok((result, i + 1));
        }
    }
    Err(Asn1Error::Truncated) // 未遇到终止字节
}

/// 写入 DER 长度字段（短格式 ≤ 127 或长格式 ≥ 128）.
fn write_length(buf: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        buf.push(len as u8);
    } else {
        // 长格式：计算需要的字节数
        let mut temp = len;
        let mut num_bytes = 0;
        while temp > 0 {
            num_bytes += 1;
            temp >>= 8;
        }
        buf.push(0x80 | (num_bytes as u8));
        // 大端写入长度字节
        for i in (0..num_bytes).rev() {
            buf.push((len >> (i * 8)) as u8);
        }
    }
}

/// 从数据中读取 DER 长度字段.
fn read_length(data: &[u8], pos: &mut usize) -> Result<usize, Asn1Error> {
    if *pos >= data.len() {
        return Err(Asn1Error::Truncated);
    }
    let first = data[*pos];
    *pos += 1;
    if first < 0x80 {
        // 短格式
        Ok(first as usize)
    } else if first == 0x80 {
        // Indefinite length — DER 不允许
        Err(Asn1Error::InvalidLength)
    } else {
        // 长格式
        let num_bytes = (first & 0x7F) as usize;
        if num_bytes == 0 || num_bytes > 8 {
            return Err(Asn1Error::InvalidLength);
        }
        if *pos + num_bytes > data.len() {
            return Err(Asn1Error::Truncated);
        }
        let mut len: u64 = 0;
        for i in 0..num_bytes {
            len = (len << 8) | (data[*pos + i] as u64);
        }
        *pos += num_bytes;
        len.try_into().map_err(|_| Asn1Error::Overflow)
    }
}

// ============================================================================
// 时间转换辅助（no_std，不依赖 chrono）
// ============================================================================

/// 将 (year, month, day, hour, min, sec) 转换为 Unix 时间戳.
///
/// 使用 Howard Hinnant 的 civil_from_days 算法。
/// 1970 年以前的日期返回 `None`（u64 无法表示负时间戳）。
fn date_to_unix(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<u64> {
    // 3 月起为当年的第 1 个月（便于闰年计算）
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32; // [0, 399]
    let m_adj = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
    let doy = (153 * m_adj + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146097 + doe as i64 - 719468;
    let secs = days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64;
    if secs < 0 {
        None
    } else {
        Some(secs as u64)
    }
}

/// 将 Unix 时间戳转换为 (year, month, day, hour, min, sec).
///
/// 使用 Howard Hinnant 的 days_from_civil 算法的逆运算。
fn unix_to_date(unix: u64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = unix % 86400;
    let days = unix / 86400;

    let hour = (secs / 3600) as u32;
    let minute = ((secs % 3600) / 60) as u32;
    let second = (secs % 60) as u32;

    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };

    (year, m as u32, d as u32, hour, minute, second)
}

/// 验证时间字段范围.
fn validate_time_fields(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Result<(), Asn1Error> {
    if year < 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(Asn1Error::InvalidTime);
    }
    if hour > 23 || minute > 59 || second > 59 {
        return Err(Asn1Error::InvalidTime);
    }
    Ok(())
}

/// 将 2 位数字写入缓冲区（ASCII）.
fn write_two_digits(buf: &mut Vec<u8>, val: u32) {
    buf.push(b'0' + ((val / 10) as u8));
    buf.push(b'0' + ((val % 10) as u8));
}

/// 将 4 位数字写入缓冲区（ASCII）.
fn write_four_digits(buf: &mut Vec<u8>, val: u32) {
    buf.push(b'0' + ((val / 1000) as u8));
    buf.push(b'0' + (((val / 100) % 10) as u8));
    buf.push(b'0' + (((val / 10) % 10) as u8));
    buf.push(b'0' + ((val % 10) as u8));
}

/// 从 2 字节 ASCII 解析 2 位数字.
fn parse_two_digits(data: &[u8]) -> Result<u32, Asn1Error> {
    if data.len() < 2 {
        return Err(Asn1Error::Truncated);
    }
    let d0 = data[0].wrapping_sub(b'0');
    let d1 = data[1].wrapping_sub(b'0');
    if d0 > 9 || d1 > 9 {
        return Err(Asn1Error::InvalidTime);
    }
    Ok(d0 as u32 * 10 + d1 as u32)
}

/// 从 4 字节 ASCII 解析 4 位数字.
fn parse_four_digits(data: &[u8]) -> Result<u32, Asn1Error> {
    if data.len() < 4 {
        return Err(Asn1Error::Truncated);
    }
    let d0 = data[0].wrapping_sub(b'0');
    let d1 = data[1].wrapping_sub(b'0');
    let d2 = data[2].wrapping_sub(b'0');
    let d3 = data[3].wrapping_sub(b'0');
    if d0 > 9 || d1 > 9 || d2 > 9 || d3 > 9 {
        return Err(Asn1Error::InvalidTime);
    }
    Ok(d0 as u32 * 1000 + d1 as u32 * 100 + d2 as u32 * 10 + d3 as u32)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    // --- INTEGER 编解码 ---

    #[test]
    fn test_integer_small_roundtrip() {
        let mut w = DerWriter::new();
        w.write_integer(&[0x42]);
        let bytes = w.into_bytes();
        // 0x42 MSB 未置位，无需前导 0x00
        assert_eq!(bytes, vec![INTEGER, 0x01, 0x42]);

        let mut r = DerReader::new(&bytes);
        let val = r.read_integer().unwrap();
        assert_eq!(val, vec![0x42]);
        assert!(r.is_empty());
    }

    #[test]
    fn test_integer_large_roundtrip() {
        let mut w = DerWriter::new();
        w.write_integer(&[0xFF, 0xFF, 0xFF, 0xFF]);
        let bytes = w.into_bytes();
        // MSB 置位，需添加前导 0x00
        assert_eq!(bytes, vec![INTEGER, 0x05, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]);

        let mut r = DerReader::new(&bytes);
        let val = r.read_integer().unwrap();
        assert_eq!(val, vec![0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn test_integer_leading_zero_padding() {
        // 值 0x80 应编码为 [0x00, 0x80]（前导 0x00 padding）
        let mut w = DerWriter::new();
        w.write_integer(&[0x80]);
        let bytes = w.into_bytes();
        assert_eq!(bytes, vec![INTEGER, 0x02, 0x00, 0x80]);

        let mut r = DerReader::new(&bytes);
        let val = r.read_integer().unwrap();
        assert_eq!(val, vec![0x80]);
    }

    #[test]
    fn test_integer_zero() {
        let mut w = DerWriter::new();
        w.write_integer(&[0x00]);
        let bytes = w.into_bytes();
        assert_eq!(bytes, vec![INTEGER, 0x01, 0x00]);

        let mut r = DerReader::new(&bytes);
        let val = r.read_integer().unwrap();
        assert_eq!(val, vec![0x00]);
    }

    #[test]
    fn test_integer_strips_leading_zeros() {
        // 输入多个前导 0x00 应被剥离
        let mut w = DerWriter::new();
        w.write_integer(&[0x00, 0x00, 0x01, 0x02]);
        let bytes = w.into_bytes();
        assert_eq!(bytes, vec![INTEGER, 0x02, 0x01, 0x02]);

        let mut r = DerReader::new(&bytes);
        let val = r.read_integer().unwrap();
        assert_eq!(val, vec![0x01, 0x02]);
    }

    // --- u64 编解码 ---

    #[test]
    fn test_u64_roundtrip() {
        let values: [u64; 5] = [0, 1, 127, 128, 123456789012345678];
        for val in values {
            let mut w = DerWriter::new();
            w.write_u64(val);
            let bytes = w.into_bytes();
            let mut r = DerReader::new(&bytes);
            let decoded = r.read_u64().unwrap();
            assert_eq!(decoded, val, "u64 roundtrip failed for {}", val);
        }
    }

    #[test]
    fn test_u64_max() {
        let mut w = DerWriter::new();
        w.write_u64(u64::MAX);
        let bytes = w.into_bytes();
        let mut r = DerReader::new(&bytes);
        let decoded = r.read_u64().unwrap();
        assert_eq!(decoded, u64::MAX);
    }

    // --- SEQUENCE 编解码 ---

    #[test]
    fn test_sequence_roundtrip_nested() {
        // 内部 SEQUENCE 包含一个 u64
        let mut inner = DerWriter::new();
        inner.write_u64(42);
        let inner_bytes = inner.into_bytes();

        // 外部 SEQUENCE 包含内部 SEQUENCE + 一个 INTEGER
        let mut outer_content = DerWriter::new();
        outer_content.write_sequence(inner_bytes.as_slice());
        outer_content.write_integer(&[0x07]);

        let mut w = DerWriter::new();
        w.write_sequence(outer_content.as_bytes());
        let bytes = w.into_bytes();

        let mut r = DerReader::new(&bytes);
        let mut outer = r.read_sequence().unwrap();
        let mut inner_reader = outer.read_sequence().unwrap();
        let val = inner_reader.read_u64().unwrap();
        assert_eq!(val, 42);
        let int_val = outer.read_integer().unwrap();
        assert_eq!(int_val, vec![0x07]);
    }

    #[test]
    fn test_empty_sequence() {
        let mut w = DerWriter::new();
        w.write_sequence(&[]);
        let bytes = w.into_bytes();
        assert_eq!(bytes, vec![SEQUENCE, 0x00]);

        let mut r = DerReader::new(&bytes);
        let seq = r.read_sequence().unwrap();
        assert!(seq.is_empty());
        assert_eq!(seq.remaining(), 0);
    }

    // --- OID 编解码 ---

    #[test]
    fn test_oid_sm2_roundtrip() {
        let arcs = [1u64, 2, 156, 10197, 1, 301]; // SM2 OID
        let content = encode_oid(&arcs);
        // SM2 OID DER: 2A 81 1C CF 55 01 82 2D
        assert_eq!(
            content,
            vec![0x2A, 0x81, 0x1C, 0xCF, 0x55, 0x01, 0x82, 0x2D]
        );

        let decoded = decode_oid(&content).unwrap();
        assert_eq!(decoded, arcs);
    }

    #[test]
    fn test_oid_sm3_roundtrip() {
        let arcs = [1u64, 2, 156, 10197, 1, 401]; // SM3 OID
        let content = encode_oid(&arcs);
        let decoded = decode_oid(&content).unwrap();
        assert_eq!(decoded, arcs);
    }

    #[test]
    fn test_oid_writer_reader_roundtrip() {
        let arcs = [1u64, 2, 840, 113549, 1, 1, 11]; // RSA SHA-256
        let content = encode_oid(&arcs);

        let mut w = DerWriter::new();
        w.write_oid(&content);
        let bytes = w.into_bytes();

        let mut r = DerReader::new(&bytes);
        let read_content = r.read_oid().unwrap();
        assert_eq!(read_content, content);

        let decoded = decode_oid(&read_content).unwrap();
        assert_eq!(decoded, arcs);
    }

    #[test]
    fn test_oid_empty_decode_error() {
        assert_eq!(decode_oid(&[]), Err(Asn1Error::InvalidOid));
    }

    #[test]
    fn test_oid_truncated_decode_error() {
        // 缺少终止字节（所有字节高位都置位）
        assert_eq!(decode_oid(&[0x2A, 0x81]), Err(Asn1Error::Truncated));
    }

    // --- BIT STRING 编解码 ---

    #[test]
    fn test_bit_string_roundtrip() {
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let mut w = DerWriter::new();
        w.write_bit_string(&data);
        let bytes = w.into_bytes();
        // BIT_STRING tag + len 5 + unused 0x00 + data
        assert_eq!(bytes[0], BIT_STRING);
        assert_eq!(bytes[1], 5);
        assert_eq!(bytes[2], 0x00); // unused bits

        let mut r = DerReader::new(&bytes);
        let val = r.read_bit_string().unwrap();
        assert_eq!(val, data);
    }

    // --- OCTET STRING 编解码 ---

    #[test]
    fn test_octet_string_roundtrip() {
        let data = vec![0x01, 0x02, 0x03, 0x04];
        let mut w = DerWriter::new();
        w.write_octet_string(&data);
        let bytes = w.into_bytes();

        let mut r = DerReader::new(&bytes);
        let val = r.read_octet_string().unwrap();
        assert_eq!(val, data);
    }

    // --- BOOLEAN 编解码 ---

    #[test]
    fn test_boolean_roundtrip() {
        let mut w = DerWriter::new();
        w.write_boolean(true);
        w.write_boolean(false);
        let bytes = w.into_bytes();

        let mut r = DerReader::new(&bytes);
        assert!(r.read_boolean().unwrap());
        assert!(!r.read_boolean().unwrap());
        assert!(r.is_empty());
    }

    // --- NULL 编解码 ---

    #[test]
    fn test_null_roundtrip() {
        let mut w = DerWriter::new();
        w.write_null();
        let bytes = w.into_bytes();
        assert_eq!(bytes, vec![NULL, 0x00]);

        let mut r = DerReader::new(&bytes);
        r.read_null().unwrap();
        assert!(r.is_empty());
    }

    // --- UTCTime 编解码 ---

    #[test]
    fn test_utctime_roundtrip() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        let ts = 1704067200u64;
        let mut w = DerWriter::new();
        w.write_utctime(ts);
        let bytes = w.into_bytes();

        // 验证 DER 编码: 17 0D 3234303130313030303030305A
        assert_eq!(bytes[0], UTC_TIME);
        assert_eq!(bytes[1], 13); // length
        assert_eq!(&bytes[2..], b"240101000000Z");

        let mut r = DerReader::new(&bytes);
        let decoded = r.read_utctime().unwrap();
        assert_eq!(decoded, ts);
    }

    #[test]
    fn test_utctime_1999() {
        // 1999-06-15 12:30:45 UTC
        // YY=99 → >= 50 → 1999
        let ts = 929449845u64; // 1999-06-15 12:30:45
        let mut w = DerWriter::new();
        w.write_utctime(ts);
        let bytes = w.into_bytes();

        let mut r = DerReader::new(&bytes);
        let decoded = r.read_utctime().unwrap();
        assert_eq!(decoded, ts);
    }

    // --- GeneralizedTime 编解码 ---

    #[test]
    fn test_generalized_time_roundtrip() {
        // 2050-01-01 00:00:00 UTC = 2524608000
        let ts = 2524608000u64;
        let mut w = DerWriter::new();
        w.write_generalized_time(ts);
        let bytes = w.into_bytes();

        // 验证 DER 编码: 18 0F 32303530303130313030303030305A
        assert_eq!(bytes[0], GENERALIZED_TIME);
        assert_eq!(bytes[1], 15); // length
        assert_eq!(&bytes[2..], b"20500101000000Z");

        let mut r = DerReader::new(&bytes);
        let decoded = r.read_generalized_time().unwrap();
        assert_eq!(decoded, ts);
    }

    // --- Context-specific 编解码 ---

    #[test]
    fn test_context_0_roundtrip() {
        let mut inner = DerWriter::new();
        inner.write_u64(5); // version v3 = 2
        let inner_bytes = inner.into_bytes();

        let mut w = DerWriter::new();
        w.write_context_explicit(0, inner_bytes.as_slice());
        let bytes = w.into_bytes();
        assert_eq!(bytes[0], CONTEXT_0);

        let mut r = DerReader::new(&bytes);
        let mut ctx = r.read_context_explicit(0).unwrap();
        let val = ctx.read_u64().unwrap();
        assert_eq!(val, 5);
    }

    #[test]
    fn test_context_3_roundtrip() {
        let inner = vec![SEQUENCE, 0x00]; // empty SEQUENCE as extension content
        let mut w = DerWriter::new();
        w.write_context_explicit(3, &inner);
        let bytes = w.into_bytes();
        assert_eq!(bytes[0], CONTEXT_3);

        let mut r = DerReader::new(&bytes);
        let mut ctx = r.read_context_explicit(3).unwrap();
        let seq = ctx.read_sequence().unwrap();
        assert!(seq.is_empty());
    }

    // --- 长格式长度 ---

    #[test]
    fn test_long_form_length_128() {
        let data = vec![0xAA; 128];
        let mut w = DerWriter::new();
        w.write_octet_string(&data);
        let bytes = w.into_bytes();

        // 长度 128: 0x81 0x80
        assert_eq!(bytes[0], OCTET_STRING);
        assert_eq!(bytes[1], 0x81);
        assert_eq!(bytes[2], 0x80);

        let mut r = DerReader::new(&bytes);
        let val = r.read_octet_string().unwrap();
        assert_eq!(val, data);
    }

    #[test]
    fn test_long_form_length_256() {
        let data = vec![0xBB; 256];
        let mut w = DerWriter::new();
        w.write_octet_string(&data);
        let bytes = w.into_bytes();

        // 长度 256: 0x82 0x01 0x00
        assert_eq!(bytes[0], OCTET_STRING);
        assert_eq!(bytes[1], 0x82);
        assert_eq!(bytes[2], 0x01);
        assert_eq!(bytes[3], 0x00);

        let mut r = DerReader::new(&bytes);
        let val = r.read_octet_string().unwrap();
        assert_eq!(val, data);
    }

    #[test]
    fn test_long_form_length_1000() {
        let data = vec![0xCC; 1000];
        let mut w = DerWriter::new();
        w.write_octet_string(&data);
        let bytes = w.into_bytes();

        // 长度 1000 = 0x03E8: 0x82 0x03 0xE8
        assert_eq!(bytes[0], OCTET_STRING);
        assert_eq!(bytes[1], 0x82);
        assert_eq!(bytes[2], 0x03);
        assert_eq!(bytes[3], 0xE8);

        let mut r = DerReader::new(&bytes);
        let val = r.read_octet_string().unwrap();
        assert_eq!(val, data);
    }

    #[test]
    fn test_long_form_length_65536() {
        let data = vec![0xDD; 65536];
        let mut w = DerWriter::new();
        w.write_octet_string(&data);
        let bytes = w.into_bytes();

        // 长度 65536 = 0x010000: 0x83 0x01 0x00 0x00
        assert_eq!(bytes[0], OCTET_STRING);
        assert_eq!(bytes[1], 0x83);
        assert_eq!(bytes[2], 0x01);
        assert_eq!(bytes[3], 0x00);
        assert_eq!(bytes[4], 0x00);

        let mut r = DerReader::new(&bytes);
        let val = r.read_octet_string().unwrap();
        assert_eq!(val.len(), 65536);
        assert_eq!(val, data);
    }

    // --- 错误处理 ---

    #[test]
    fn test_truncated_data_error() {
        // INTEGER tag + length 5, but only 1 byte of content
        let data = [INTEGER, 0x05, 0x00];
        let mut r = DerReader::new(&data);
        assert_eq!(r.read_integer(), Err(Asn1Error::Truncated));
    }

    #[test]
    fn test_truncated_tag_error() {
        // Empty data
        let data = [];
        let mut r = DerReader::new(&data);
        assert_eq!(r.read_element(), Err(Asn1Error::Truncated));
    }

    #[test]
    fn test_invalid_tag_error() {
        // OCTET_STRING but trying to read as INTEGER
        let data = [OCTET_STRING, 0x01, 0x42];
        let mut r = DerReader::new(&data);
        assert_eq!(
            r.read_integer(),
            Err(Asn1Error::InvalidTag {
                expected: INTEGER,
                actual: OCTET_STRING
            })
        );
    }

    #[test]
    fn test_indefinite_length_error() {
        // 0x80 = indefinite length, not allowed in DER
        let data = [OCTET_STRING, 0x80, 0x00, 0x00];
        let mut r = DerReader::new(&data);
        assert_eq!(r.read_octet_string(), Err(Asn1Error::InvalidLength));
    }

    #[test]
    fn test_invalid_utctime_no_z() {
        // Missing 'Z' suffix
        let content = b"240101000000X";
        let mut bytes = vec![UTC_TIME, content.len() as u8];
        bytes.extend_from_slice(content);
        let mut r = DerReader::new(&bytes);
        assert_eq!(r.read_utctime(), Err(Asn1Error::InvalidTime));
    }

    #[test]
    fn test_invalid_utctime_wrong_length() {
        // Wrong length (12 bytes instead of 13)
        let content = b"240101000000Z"[..12].to_vec();
        let mut bytes = vec![UTC_TIME, content.len() as u8];
        bytes.extend_from_slice(&content);
        let mut r = DerReader::new(&bytes);
        assert_eq!(r.read_utctime(), Err(Asn1Error::InvalidTime));
    }

    // --- 综合测试 ---

    #[test]
    fn test_full_tlv_roundtrip() {
        // 构造一个类似 X.509 的结构
        let mut w = DerWriter::new();

        // SEQUENCE {
        //   INTEGER 3  (version)
        //   INTEGER 0x1234 (serial)
        //   SEQUENCE { OID }  (algorithm)
        //   NULL
        // }
        let mut inner = DerWriter::new();
        inner.write_u64(3);
        inner.write_integer(&[0x12, 0x34]);

        let oid_content = encode_oid(&[1u64, 2, 840, 113549, 1, 1, 11]);
        let mut algo_seq = DerWriter::new();
        algo_seq.write_oid(&oid_content);
        inner.write_sequence(algo_seq.as_bytes());
        inner.write_null();

        w.write_sequence(inner.as_bytes());
        let bytes = w.into_bytes();

        // 解码验证
        let mut r = DerReader::new(&bytes);
        let mut outer = r.read_sequence().unwrap();
        assert_eq!(outer.read_u64().unwrap(), 3);

        let serial = outer.read_integer().unwrap();
        assert_eq!(serial, vec![0x12, 0x34]);

        let mut algo = outer.read_sequence().unwrap();
        let oid_bytes = algo.read_oid().unwrap();
        let arcs = decode_oid(&oid_bytes).unwrap();
        assert_eq!(arcs, vec![1u64, 2, 840, 113549, 1, 1, 11]);

        outer.read_null().unwrap();
        assert!(outer.is_empty());
    }

    #[test]
    fn test_reader_remaining_and_is_empty() {
        let mut w = DerWriter::new();
        w.write_u64(42);
        w.write_null();
        let bytes = w.into_bytes();

        let mut r = DerReader::new(&bytes);
        assert!(!r.is_empty());
        let total = r.remaining();
        assert!(total > 0);

        r.read_u64().unwrap();
        let mid = r.remaining();
        assert!(mid < total);

        r.read_null().unwrap();
        assert!(r.is_empty());
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn test_set_roundtrip() {
        let mut inner = DerWriter::new();
        inner.write_u64(1);
        inner.write_u64(2);

        let mut w = DerWriter::new();
        w.write_set(inner.as_bytes());
        let bytes = w.into_bytes();
        assert_eq!(bytes[0], SET);

        let mut r = DerReader::new(&bytes);
        let mut set_reader = r.read_set().unwrap();
        assert_eq!(set_reader.read_u64().unwrap(), 1);
        assert_eq!(set_reader.read_u64().unwrap(), 2);
        assert!(set_reader.is_empty());
    }
}
