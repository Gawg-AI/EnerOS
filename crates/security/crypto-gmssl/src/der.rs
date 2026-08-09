//! DER 编解码：SM2 签名 `r||s`（各 32 字节）↔ ASN.1 ECDSA-Sig-Value。
//!
//! OpenSSL `EVP_PKEY_sign` 输出 DER 编码的 `SEQUENCE { INTEGER r, INTEGER s }`；
//! 本仓库手卷 `Sm2Signature` 使用 64 字节 `r||s`。互操作（C2 KAT）需要双向转换。
//! 本模块为纯格式操作（RFC 5915 / X9.62 语法），仅处理本仓库用到的子集，
//! 非法输入一律 `Err`（fail-closed，拒绝部分解析）。

/// 编码错误：输入/输出不满足 DER 结构约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerError {
    /// 整数过长（DER 单字节长度编码上限 127）。
    IntegerTooLong,
    /// 解码时结构非法（标签/长度/内容不匹配）。
    Malformed,
}

fn encode_len(buf: &mut Vec<u8>, len: usize) -> Result<(), DerError> {
    if len < 0x80 {
        buf.push(len as u8);
    } else if len <= 0xff {
        buf.push(0x81);
        buf.push(len as u8);
    } else {
        return Err(DerError::IntegerTooLong);
    }
    Ok(())
}

/// 大端整数 → DER INTEGER 内容字节（去前导零；最高位为 1 时前置 0x00）。
fn integer_content(bytes: &[u8]) -> Vec<u8> {
    let mut trimmed = bytes.to_vec();
    while trimmed.len() > 1 && trimmed[0] == 0 {
        trimmed.remove(0);
    }
    if trimmed.is_empty() {
        trimmed.push(0);
    }
    if trimmed[0] & 0x80 != 0 {
        trimmed.insert(0, 0);
    }
    trimmed
}

/// `r||s`（64 字节）→ DER ECDSA-Sig-Value。
pub fn rs_to_der(r: &[u8; 32], s: &[u8; 32]) -> Result<Vec<u8>, DerError> {
    let r_content = integer_content(r);
    let s_content = integer_content(s);
    let inner_len = 2 + r_content.len() + 2 + s_content.len();
    let mut out = Vec::with_capacity(4 + inner_len);
    out.push(0x30);
    encode_len(&mut out, inner_len)?;
    out.push(0x02);
    encode_len(&mut out, r_content.len())?;
    out.extend_from_slice(&r_content);
    out.push(0x02);
    encode_len(&mut out, s_content.len())?;
    out.extend_from_slice(&s_content);
    Ok(out)
}

struct DerReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> DerReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], DerError> {
        let end = self.pos.checked_add(n).ok_or(DerError::Malformed)?;
        if end > self.buf.len() {
            return Err(DerError::Malformed);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    fn read_len(&mut self) -> Result<usize, DerError> {
        let first = self.take(1)?[0];
        match first {
            0x81 => Ok(usize::from(self.take(1)?[0])),
            len if len < 0x80 => Ok(usize::from(len)),
            _ => Err(DerError::Malformed),
        }
    }
}

/// DER ECDSA-Sig-Value → `r||s`（64 字节，左补零右对齐）。
pub fn der_to_rs(der: &[u8]) -> Result<([u8; 32], [u8; 32]), DerError> {
    let mut reader = DerReader::new(der);
    if reader.take(1)?[0] != 0x30 {
        return Err(DerError::Malformed);
    }
    let seq_len = reader.read_len()?;
    if seq_len != der.len() - reader.pos {
        return Err(DerError::Malformed);
    }
    let mut fields = [None; 2];
    for slot in &mut fields {
        if reader.take(1)?[0] != 0x02 {
            return Err(DerError::Malformed);
        }
        let int_len = reader.read_len()?;
        if int_len > 33 {
            return Err(DerError::Malformed);
        }
        let content = reader.take(int_len)?;
        *slot = Some(right_align_32(content)?);
    }
    if reader.pos != der.len() || fields[0].is_none() || fields[1].is_none() {
        return Err(DerError::Malformed);
    }
    // Reason: 上一行已检查 fields[0]/fields[1] 非 None（结构不变量），
    // unwrap 不可达（21-B D1 豁免）。
    Ok((fields[0].unwrap(), fields[1].unwrap()))
}

/// 大端整数内容 → 32 字节（左补零；拒绝 33 字节且首字节非 0 的越界值）。
fn right_align_32(content: &[u8]) -> Result<[u8; 32], DerError> {
    if content.len() > 33 || (content.len() == 33 && content[0] != 0) {
        return Err(DerError::Malformed);
    }
    let mut out = [0u8; 32];
    let offset = 32usize.saturating_sub(content.len());
    out[offset..].copy_from_slice(&content[content.len() - (32 - offset)..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_rs_der() {
        let r = [0x12; 32];
        let s = [0x34; 32];
        let der = rs_to_der(&r, &s).unwrap();
        assert_eq!(der[0], 0x30);
        let (r2, s2) = der_to_rs(&der).unwrap();
        assert_eq!(r2, r);
        assert_eq!(s2, s);
    }

    #[test]
    fn high_bit_prefix_zero() {
        // r 最高位为 1 → INTEGER 内容前置 0x00，解码回 32 字节不变。
        let mut r = [0u8; 32];
        r[0] = 0x80;
        let s = [0u8; 32];
        let der = rs_to_der(&r, &s).unwrap();
        assert!(der.contains(&0x00));
        let (r2, _) = der_to_rs(&der).unwrap();
        assert_eq!(r2, r);
    }

    #[test]
    fn leading_zero_trim() {
        // r 高字节为 0 → DER 省略前导零，解码时左补零恢复。
        let mut r = [0u8; 32];
        r[31] = 0x01;
        let s = [0u8; 32];
        let der = rs_to_der(&r, &s).unwrap();
        let (r2, _) = der_to_rs(&der).unwrap();
        assert_eq!(r2, r);
    }

    #[test]
    fn malformed_rejected() {
        assert_eq!(
            der_to_rs(&[0x30, 0x02, 0x02, 0x01]),
            Err(DerError::Malformed)
        );
        assert_eq!(der_to_rs(&[0x31, 0x00]), Err(DerError::Malformed));
        // 整数长度 33 且首字节非零 → 拒绝
        let mut der = vec![0x30, 0x27, 0x02, 0x21];
        der.extend_from_slice(&[0x01; 33]);
        der.extend_from_slice(&[0x02, 0x01, 0x00]);
        assert_eq!(der_to_rs(&der), Err(DerError::Malformed));
    }
}
