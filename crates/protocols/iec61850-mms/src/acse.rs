//! ACSE 关联（AARQ/AARE）与 COTP 握手（CR/CC）辅助.
//!
//! 简化栈结构（D9：蓝图文件清单无 cotp.rs，COTP 定长结构并入本文件，
//! 真实 COTP 选项协商在集成层）：
//! - AARQ：`0x60 <len> 0x1A <len> <ap_title>`（AP-title VisibleString）
//! - AARE：`0x61 <len> 0x02 0x01 <result>`（result = 0 接受；非 0 拒绝）
//! - COTP CR：定长 10 字节 `[LI=0x09, 0xE0, dst-ref(2), src-ref(2), class, 0xC0, 0x01, tpdu-size]`
//! - COTP CC：第 2 字节为 0xD0 即确认

use alloc::vec::Vec;

use crate::ber_decode::read_tag_length;
use crate::ber_encode::TAG_VISIBLE_STRING;
use crate::mms_client::MmsErrorCode;
use crate::MmsError;

/// AARQ（Associate Request）tag。
pub(crate) const TAG_AARQ: u8 = 0x60;
/// AARE（Associate Response）tag。
pub(crate) const TAG_AARE: u8 = 0x61;
/// COTP CR（Connect Request）TPDU code。
pub(crate) const COTP_CR: u8 = 0xE0;
/// COTP CC（Connect Confirm）TPDU code。
pub(crate) const COTP_CC: u8 = 0xD0;
/// COTP DT（Data）TPDU 头长度（LI + code + TPDU-NR/eot）。
pub(crate) const COTP_DT_HEADER_LEN: usize = 3;
/// 关联结果：接受。
const AARE_RESULT_ACCEPTED: u8 = 0;

/// 编码 ACSE AARQ（AP-title 以 VisibleString 携带）。
pub fn encode_aarq(ap_title: &str) -> Vec<u8> {
    let mut pdu = Vec::with_capacity(ap_title.len() + 4);
    pdu.push(TAG_AARQ);
    let content_len = ap_title.len() + 2; // 0x1A + len + bytes
    pdu.push(content_len as u8);
    pdu.push(TAG_VISIBLE_STRING);
    pdu.push(ap_title.len() as u8);
    pdu.extend_from_slice(ap_title.as_bytes());
    pdu
}

/// 解码 ACSE AARE：接受 → `Ok(())`；拒绝 → `IedError(Refused)`；畸形 → `BerDecodeError`。
pub fn decode_aare(data: &[u8]) -> Result<(), MmsError> {
    let mut pos = 0usize;
    let (tag, _len) = read_tag_length(data, &mut pos)?;
    if tag != TAG_AARE {
        return Err(MmsError::BerDecodeError);
    }
    let (rtag, rlen) = read_tag_length(data, &mut pos)?;
    if rtag != 0x02 || rlen != 1 {
        return Err(MmsError::BerDecodeError);
    }
    if data[pos] == AARE_RESULT_ACCEPTED {
        Ok(())
    } else {
        Err(MmsError::IedError(MmsErrorCode::Refused))
    }
}

/// 编码 COTP CR（定长简化结构，D9）：10 字节，TPDU size 参数 0x0A（1024）。
pub fn encode_cotp_cr() -> Vec<u8> {
    alloc::vec![
        0x09,    // LI = 后续 9 字节
        COTP_CR, // 0xE0 Connect Request
        0x00, 0x00, // dst-ref
        0x00, 0x01, // src-ref
        0x00, // class 0
        0xC0, 0x01, 0x0A, // TPDU size 参数 = 1024
    ]
}

/// 解码 COTP CC：第 2 字节为 0xD0 → `Ok(())`，否则 `BerDecodeError`。
pub fn decode_cotp_cc(data: &[u8]) -> Result<(), MmsError> {
    if data.len() >= 2 && data[1] == COTP_CC {
        Ok(())
    } else {
        Err(MmsError::BerDecodeError)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    // ===== AC21：AARQ 含 0x60 + ap_title（VisibleString）=====
    #[test]
    fn test_ac21_aarq_encoding() {
        let pdu = encode_aarq("1.1.1.999");
        assert_eq!(pdu[0], 0x60);
        assert_eq!(pdu[2], 0x1A);
        assert!(pdu.windows(9).any(|w| w == b"1.1.1.999"));
        // 顶层长度 == 内容字节数
        assert_eq!(usize::from(pdu[1]), pdu.len() - 2);
    }

    // ===== AC22：AARE 接受 → Ok =====
    #[test]
    fn test_ac22_aare_accepted() {
        let aare = [0x61, 0x03, 0x02, 0x01, 0x00];
        assert_eq!(decode_aare(&aare), Ok(()));
    }

    // ===== AC23：AARE 拒绝 → IedError(Refused) =====
    #[test]
    fn test_ac23_aare_rejected() {
        let aare = [0x61, 0x03, 0x02, 0x01, 0x01];
        assert_eq!(
            decode_aare(&aare),
            Err(MmsError::IedError(MmsErrorCode::Refused))
        );
    }

    // ===== AC24：AARE 畸形 → BerDecodeError =====
    #[test]
    fn test_ac24_aare_malformed() {
        // 顶层 tag 非 0x61
        assert_eq!(
            decode_aare(&[0x60, 0x03, 0x02, 0x01, 0x00]),
            Err(MmsError::BerDecodeError)
        );
        // 截断
        assert_eq!(decode_aare(&[0x61]), Err(MmsError::BerDecodeError));
        // 结果字段非 INTEGER
        assert_eq!(
            decode_aare(&[0x61, 0x03, 0x1A, 0x01, 0x00]),
            Err(MmsError::BerDecodeError)
        );
    }

    // ===== AC25：COTP CR 定长结构 =====
    #[test]
    fn test_ac25_cotp_cr_structure() {
        let cr = encode_cotp_cr();
        assert_eq!(cr.len(), 10);
        assert_eq!(cr[0], 0x09); // LI
        assert_eq!(cr[1], 0xE0); // CR
        assert_eq!(cr[7], 0xC0); // TPDU size 参数
        assert_eq!(cr[9], 0x0A); // 1024
    }

    // ===== AC26：COTP CC 解析 =====
    #[test]
    fn test_ac26_cotp_cc_parse() {
        let cc = [0x09, 0xD0, 0x00, 0x01, 0x00, 0x01, 0x00, 0xC0, 0x01, 0x0A];
        assert_eq!(decode_cotp_cc(&cc), Ok(()));
        // 非 CC
        assert_eq!(
            decode_cotp_cc(&[0x09, 0xE0, 0x00]),
            Err(MmsError::BerDecodeError)
        );
        // 过短
        assert_eq!(decode_cotp_cc(&[0x09]), Err(MmsError::BerDecodeError));
    }
}
