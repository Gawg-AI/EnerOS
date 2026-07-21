//! BER 解码器：MMS 响应解析.
//!
//! 长度解析支持短型（< 0x80 单字节）与 0x82 双字节长型两型；任何声明长度
//! 超出剩余缓冲区的输入判 `BerDecodeError`（截断安全）。
//!
//! Read 响应结构（简化栈，与编码侧对称）：
//! `ConfirmedResponsePDU(0xA1)` → `invokeID(0x02)` → `Read-Response(0xA5)`
//! → `listOfAccessResult(0xA0)` → 每条目 `boolean(0x80)` / `integer(0x85)` /
//! `floating-point(0x87)`；未知 tag 跳过该条内容，`value = None`。
//!
//! 浮点按 val_len 右对齐解码（D7）：4 字节 → `Float32`，8 字节 → `Float64`，
//! 其余长度右对齐/截断到 8 字节按 `Float64`（蓝图左对齐 bug 修复）。
//!
//! 解码侧无时间语义：`MmsReadResult.quality` 默认 Good，`timestamp` 默认 0，
//! 真实品质/时间戳由集成层在数据就绪时注入（与 v0.50.0 D1 参数注入先例一致）。

use alloc::format;
use alloc::vec::Vec;

use eneros_iec61850_model::{DaValue, Quality, Source, Validity};

use crate::ber_encode::{TAG_DATA_BOOLEAN, TAG_DATA_FLOAT, TAG_DATA_INTEGER, TAG_SEQUENCE_OF};
use crate::mms_client::{MmsReadResult, MmsWriteResult};
use crate::MmsError;

/// ConfirmedResponsePDU tag。
pub(crate) const TAG_CONFIRMED_RESPONSE: u8 = 0xA1;
/// ConfirmedErrorPDU tag。
pub(crate) const TAG_CONFIRMED_ERROR: u8 = 0xA2;
/// Read-Response tag。
pub(crate) const TAG_READ_RESPONSE: u8 = 0xA5;
/// Write-Response tag。
pub(crate) const TAG_WRITE_RESPONSE: u8 = 0xA6;
/// Write-Result：success（NULL）tag。
pub(crate) const TAG_WRITE_SUCCESS: u8 = 0x80;
/// Write-Result：failure（DataAccessError INTEGER）tag。
pub(crate) const TAG_WRITE_FAILURE: u8 = 0x81;

/// 读取一个 TLV 的 tag 与内容长度，`pos` 推进到内容起始。
///
/// 支持短型（< 0x80）与 0x82 双字节长型；声明长度超出剩余缓冲区、
/// 或其他长型标记 → `BerDecodeError`。
pub fn read_tag_length(data: &[u8], pos: &mut usize) -> Result<(u8, usize), MmsError> {
    let tag = *data.get(*pos).ok_or(MmsError::BerDecodeError)?;
    *pos += 1;
    let first = usize::from(*data.get(*pos).ok_or(MmsError::BerDecodeError)?);
    *pos += 1;
    let len = if first < 0x80 {
        first
    } else if first == 0x82 {
        let hi = usize::from(*data.get(*pos).ok_or(MmsError::BerDecodeError)?);
        let lo = usize::from(*data.get(*pos + 1).ok_or(MmsError::BerDecodeError)?);
        *pos += 2;
        (hi << 8) | lo
    } else {
        return Err(MmsError::BerDecodeError);
    };
    if *pos + len > data.len() {
        return Err(MmsError::BerDecodeError);
    }
    Ok((tag, len))
}

/// 解码 MMS Read 响应为结果列表（保序；未知 tag 条目 `value = None`）。
pub fn decode_read_response(data: &[u8]) -> Result<Vec<MmsReadResult>, MmsError> {
    let mut pos = 0usize;
    let (tag, _pdu_len) = read_tag_length(data, &mut pos)?;
    if tag != TAG_CONFIRMED_RESPONSE {
        return Err(MmsError::BerDecodeError);
    }
    skip_tlv(data, &mut pos)?; // invokeID
    let (rtag, _rlen) = read_tag_length(data, &mut pos)?;
    if rtag != TAG_READ_RESPONSE {
        return Err(MmsError::BerDecodeError);
    }
    let (ltag, llen) = read_tag_length(data, &mut pos)?;
    if ltag != TAG_SEQUENCE_OF {
        return Err(MmsError::BerDecodeError);
    }
    let lend = pos + llen;
    let mut results = Vec::new();
    while pos < lend {
        let (vtag, vlen) = read_tag_length(data, &mut pos)?;
        let value = match vtag {
            TAG_DATA_BOOLEAN => {
                if vlen < 1 {
                    return Err(MmsError::BerDecodeError);
                }
                let b = data[pos] != 0;
                pos += vlen;
                Some(DaValue::Bool(b))
            }
            TAG_DATA_INTEGER => {
                let mut v: i32 = 0;
                for i in 0..vlen {
                    v = (v << 8) | i32::from(data[pos + i]);
                }
                pos += vlen;
                Some(DaValue::Int32(v))
            }
            TAG_DATA_FLOAT => {
                let v = decode_float(&data[pos..pos + vlen]);
                pos += vlen;
                Some(v)
            }
            _ => {
                pos += vlen; // 未知 tag：跳过内容，该条 value = None
                None
            }
        };
        results.push(MmsReadResult {
            value,
            quality: good_quality(),
            timestamp: 0,
        });
    }
    Ok(results)
}

/// 解码 MMS Write 响应为结果列表（Success / Failed）。
pub fn decode_write_response(data: &[u8]) -> Result<Vec<MmsWriteResult>, MmsError> {
    let mut pos = 0usize;
    let (tag, _pdu_len) = read_tag_length(data, &mut pos)?;
    if tag != TAG_CONFIRMED_RESPONSE {
        return Err(MmsError::BerDecodeError);
    }
    skip_tlv(data, &mut pos)?; // invokeID
    let (wtag, wlen) = read_tag_length(data, &mut pos)?;
    if wtag != TAG_WRITE_RESPONSE {
        return Err(MmsError::BerDecodeError);
    }
    let wend = pos + wlen;
    let mut results = Vec::new();
    while pos < wend {
        let (t, l) = read_tag_length(data, &mut pos)?;
        match t {
            TAG_WRITE_SUCCESS => {
                pos += l;
                results.push(MmsWriteResult::Success);
            }
            TAG_WRITE_FAILURE => {
                let mut code: u16 = 0;
                for i in 0..l.min(2) {
                    code = (code << 8) | u16::from(data[pos + i]);
                }
                pos += l;
                results.push(MmsWriteResult::Failed(format!("DataAccessError({})", code)));
            }
            _ => {
                pos += l; // 未知条目跳过
            }
        }
    }
    Ok(results)
}

/// 跳过当前 TLV（tag + 长度 + 内容）。
fn skip_tlv(data: &[u8], pos: &mut usize) -> Result<(), MmsError> {
    let (_t, l) = read_tag_length(data, pos)?;
    *pos += l;
    Ok(())
}

/// 浮点右对齐解码（D7）：4 → Float32；其余右对齐/截断到 8 字节 → Float64。
fn decode_float(bytes: &[u8]) -> DaValue {
    if bytes.len() == 4 {
        let mut b = [0u8; 4];
        b.copy_from_slice(bytes);
        return DaValue::Float32(f32::from_be_bytes(b));
    }
    let mut b = [0u8; 8];
    if bytes.len() >= 8 {
        b.copy_from_slice(&bytes[..8]);
    } else {
        b[8 - bytes.len()..].copy_from_slice(bytes);
    }
    DaValue::Float64(f64::from_be_bytes(b))
}

/// 解码侧默认品质（无时间语义，Good/Process；见模块文档）。
fn good_quality() -> Quality {
    Quality {
        validity: Validity::Good,
        source: Source::Process,
        test: false,
        operator_blocked: false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    use crate::mms_client::MmsWriteResult;

    /// 写入 BER 长度（测试辅助，与编码侧同规则）。
    fn push_len(buf: &mut Vec<u8>, len: usize) {
        if len < 0x80 {
            buf.push(len as u8);
        } else {
            buf.push(0x82);
            buf.extend_from_slice(&(len as u16).to_be_bytes());
        }
    }

    /// 构造 Read 响应：0xA1 → invokeID → 0xA5 → 0xA0 → 各条目 TLV。
    fn build_read_response(entries: &[&[u8]]) -> Vec<u8> {
        let mut content = Vec::new();
        for e in entries {
            content.extend_from_slice(e);
        }
        let mut list = vec![0xA0];
        push_len(&mut list, content.len());
        list.extend_from_slice(&content);
        let mut rr = vec![0xA5];
        push_len(&mut rr, list.len());
        rr.extend_from_slice(&list);
        let mut inner = vec![0x02, 0x01, 0x01]; // invokeID = 1
        inner.extend_from_slice(&rr);
        let mut pdu = vec![0xA1];
        push_len(&mut pdu, inner.len());
        pdu.extend_from_slice(&inner);
        pdu
    }

    /// 构造 Write 响应：0xA1 → invokeID → 0xA6 → 各条目 TLV。
    fn build_write_response(entries: &[&[u8]]) -> Vec<u8> {
        let mut content = Vec::new();
        for e in entries {
            content.extend_from_slice(e);
        }
        let mut wr = vec![0xA6];
        push_len(&mut wr, content.len());
        wr.extend_from_slice(&content);
        let mut inner = vec![0x02, 0x01, 0x01];
        inner.extend_from_slice(&wr);
        let mut pdu = vec![0xA1];
        push_len(&mut pdu, inner.len());
        pdu.extend_from_slice(&inner);
        pdu
    }

    // ===== BD11：boolean 0x80 解码 =====
    #[test]
    fn test_bd11_boolean_decode() {
        let pdu = build_read_response(&[&[0x80, 0x01, 0x01], &[0x80, 0x01, 0x00]]);
        let results = decode_read_response(&pdu).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].value, Some(DaValue::Bool(true)));
        assert_eq!(results[1].value, Some(DaValue::Bool(false)));
        assert_eq!(results[0].quality.validity, Validity::Good);
        assert_eq!(results[0].timestamp, 0);
    }

    // ===== BD12：integer 0x85 多字节大端 =====
    #[test]
    fn test_bd12_integer_multi_byte() {
        let pdu = build_read_response(&[
            &[0x85, 0x01, 0x2A],                   // 42
            &[0x85, 0x02, 0x01, 0x00],             // 256
            &[0x85, 0x04, 0xFF, 0xFF, 0xFF, 0xFF], // -1
        ]);
        let results = decode_read_response(&pdu).unwrap();
        assert_eq!(results[0].value, Some(DaValue::Int32(42)));
        assert_eq!(results[1].value, Some(DaValue::Int32(256)));
        assert_eq!(results[2].value, Some(DaValue::Int32(-1)));
    }

    // ===== BD13：float 4 字节 → Float32（D7）=====
    #[test]
    fn test_bd13_float32_decode() {
        let mut entry = vec![0x87, 0x04];
        entry.extend_from_slice(&1.5f32.to_be_bytes());
        let pdu = build_read_response(&[&entry]);
        let results = decode_read_response(&pdu).unwrap();
        assert_eq!(results[0].value, Some(DaValue::Float32(1.5)));
    }

    // ===== BD14：float 8 字节 → Float64 =====
    #[test]
    fn test_bd14_float64_decode() {
        let mut entry = vec![0x87, 0x08];
        entry.extend_from_slice(&2.5f64.to_be_bytes());
        let pdu = build_read_response(&[&entry]);
        let results = decode_read_response(&pdu).unwrap();
        assert_eq!(results[0].value, Some(DaValue::Float64(2.5)));
    }

    // ===== BD15：未知 tag → 跳过，value = None（保序）=====
    #[test]
    fn test_bd15_unknown_tag_yields_none() {
        let pdu = build_read_response(&[
            &[0x80, 0x01, 0x01],       // Bool(true)
            &[0x99, 0x02, 0xAA, 0xBB], // 未知 tag
            &[0x85, 0x01, 0x07],       // Int32(7)
        ]);
        let results = decode_read_response(&pdu).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].value, Some(DaValue::Bool(true)));
        assert_eq!(results[1].value, None);
        assert_eq!(results[2].value, Some(DaValue::Int32(7)));
    }

    // ===== BD16：截断 → BerDecodeError =====
    #[test]
    fn test_bd16_truncated_input() {
        let pdu = build_read_response(&[&[0x85, 0x02, 0x01, 0x00]]);
        // 砍掉最后 1 字节：声明长度超出剩余 → Err
        let truncated = &pdu[..pdu.len() - 1];
        assert_eq!(
            decode_read_response(truncated),
            Err(MmsError::BerDecodeError)
        );
        // 空输入 → Err
        assert_eq!(decode_read_response(&[]), Err(MmsError::BerDecodeError));
    }

    // ===== BD17：0x82 双字节长型长度解析 =====
    #[test]
    fn test_bd17_long_form_length_parse() {
        let mut tlv = vec![0x30, 0x82, 0x01, 0x00];
        tlv.extend_from_slice(&[0u8; 256]);
        let mut pos = 0usize;
        let (tag, len) = read_tag_length(&tlv, &mut pos).unwrap();
        assert_eq!(tag, 0x30);
        assert_eq!(len, 256);
        assert_eq!(pos, 4);
        // 长型长度声明超出缓冲 → Err
        let bad = [0x30, 0x82, 0x01, 0x00, 0x00];
        let mut p2 = 0usize;
        assert_eq!(
            read_tag_length(&bad, &mut p2),
            Err(MmsError::BerDecodeError)
        );
    }

    // ===== BD18：write 响应 Success =====
    #[test]
    fn test_bd18_write_success() {
        let pdu = build_write_response(&[&[0x80, 0x00], &[0x80, 0x00]]);
        let results = decode_write_response(&pdu).unwrap();
        assert_eq!(
            results,
            vec![MmsWriteResult::Success, MmsWriteResult::Success]
        );
    }

    // ===== BD19：write 响应 Failed(String) =====
    #[test]
    fn test_bd19_write_failed() {
        let pdu = build_write_response(&[&[0x80, 0x00], &[0x81, 0x01, 0x0A]]);
        let results = decode_write_response(&pdu).unwrap();
        assert_eq!(results[0], MmsWriteResult::Success);
        match &results[1] {
            MmsWriteResult::Failed(msg) => assert!(msg.contains("DataAccessError(10)")),
            other => panic!("expect Failed, got {:?}", other),
        }
    }

    // ===== BD20：顶层 tag 非法 → Err =====
    #[test]
    fn test_bd20_illegal_top_tag() {
        let bad = [0x55, 0x00];
        assert_eq!(decode_read_response(&bad), Err(MmsError::BerDecodeError));
        assert_eq!(decode_write_response(&bad), Err(MmsError::BerDecodeError));
    }
}
