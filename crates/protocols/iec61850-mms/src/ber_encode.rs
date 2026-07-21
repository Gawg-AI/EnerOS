//! BER（Basic Encoding Rules）编码器：MMS ConfirmedRequestPDU 构造.
//!
//! 构造法（D6：修复蓝图长度回填 bug）：「tag + 0x00 长度占位 + 内容 + 回填」，
//! 所有 BER 长度恒为**内容字节数**——内容长度 < 0x80 用短型单字节，
//! ≥ 0x80 用 0x82 双字节长型。蓝图 §4.5 参考代码有两处 bug：
//! `write_tag` 后无占位字节即写内容（`backfill_length` 覆盖后续 tag）、
//! listOfVariable 用元素个数冒充字节长度，均在本实现中修复。
//!
//! 报文结构（简化栈，与解码侧对称）：
//! - Read：`ConfirmedRequestPDU(0xA0)` → `invokeID(0x02 INTEGER)` → `Read(0xA4)`
//!   → `listOfVariable(0xA0)` → 每条目 `domain(VisibleString 0x1A)` + `item(0x1A)`
//! - Write：`ConfirmedRequestPDU(0xA0)` → `invokeID(0x02)` → `Write(0xA5)`
//!   → `listOfVariable(0xA0)` → 每条目 domain + item
//!   → `listOfData(0xA0)` → 每值 `Bool(0x80)` / `Int32(0x85)` /
//!   `Float32|Float64(0x87)` / `Enum(0x86)` / `StringVal(0x89)` / `Timestamp(0x8B)`

use alloc::vec::Vec;

use eneros_iec61850_model::DaValue;

use crate::mms_client::VarAccessSpec;

/// ConfirmedRequestPDU tag。
pub(crate) const TAG_CONFIRMED_REQUEST: u8 = 0xA0;
/// invokeID（INTEGER）tag。
pub(crate) const TAG_INTEGER: u8 = 0x02;
/// Read-Request tag。
pub(crate) const TAG_READ_REQUEST: u8 = 0xA4;
/// Write-Request tag。
pub(crate) const TAG_WRITE_REQUEST: u8 = 0xA5;
/// listOfVariable / listOfData（SEQUENCE OF）tag。
pub(crate) const TAG_SEQUENCE_OF: u8 = 0xA0;
/// VisibleString tag。
pub(crate) const TAG_VISIBLE_STRING: u8 = 0x1A;
/// Data：boolean tag（context 0）。
pub(crate) const TAG_DATA_BOOLEAN: u8 = 0x80;
/// Data：integer tag。
pub(crate) const TAG_DATA_INTEGER: u8 = 0x85;
/// Data：floating-point tag。
pub(crate) const TAG_DATA_FLOAT: u8 = 0x87;
/// Data：enum tag。
pub(crate) const TAG_DATA_ENUM: u8 = 0x86;
/// Data：visible-string tag。
pub(crate) const TAG_DATA_STRING: u8 = 0x89;
/// Data：utc-time/timestamp tag。
pub(crate) const TAG_DATA_TIMESTAMP: u8 = 0x8B;

/// BER 编码器（内部复用缓冲区，避免重复分配）。
pub struct BerEncoder {
    buffer: Vec<u8>,
}

impl BerEncoder {
    /// 创建编码器（预分配 1024 字节）。
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(1024),
        }
    }

    /// 编码 MMS Read 请求（返回内部缓冲区切片，下次编码前有效）。
    pub fn encode_read_request(&mut self, invoke_id: u32, vars: &[VarAccessSpec]) -> &[u8] {
        self.buffer.clear();
        let pdu = self.begin_tlv(TAG_CONFIRMED_REQUEST);
        self.push_invoke_id(invoke_id);
        let read = self.begin_tlv(TAG_READ_REQUEST);
        self.encode_var_list(vars);
        self.end_tlv(read);
        self.end_tlv(pdu);
        &self.buffer
    }

    /// 编码 MMS Write 请求（listOfVariable + listOfData 两段）。
    pub fn encode_write_request(
        &mut self,
        invoke_id: u32,
        vars: &[(VarAccessSpec, DaValue)],
    ) -> &[u8] {
        self.buffer.clear();
        let pdu = self.begin_tlv(TAG_CONFIRMED_REQUEST);
        self.push_invoke_id(invoke_id);
        let write = self.begin_tlv(TAG_WRITE_REQUEST);
        let list = self.begin_tlv(TAG_SEQUENCE_OF);
        for (var, _) in vars {
            push_visible_string(&mut self.buffer, &var.domain);
            push_visible_string(&mut self.buffer, &var.item);
        }
        self.end_tlv(list);
        let data = self.begin_tlv(TAG_SEQUENCE_OF);
        for (_, val) in vars {
            push_da_value(&mut self.buffer, val);
        }
        self.end_tlv(data);
        self.end_tlv(write);
        self.end_tlv(pdu);
        &self.buffer
    }

    /// 编码 listOfVariable（每条目 domain + item 两个 VisibleString）。
    fn encode_var_list(&mut self, vars: &[VarAccessSpec]) {
        let list = self.begin_tlv(TAG_SEQUENCE_OF);
        for var in vars {
            push_visible_string(&mut self.buffer, &var.domain);
            push_visible_string(&mut self.buffer, &var.item);
        }
        self.end_tlv(list);
    }

    /// 写入 invokeID（INTEGER，大端最小字节数且保持正数语义）。
    fn push_invoke_id(&mut self, invoke_id: u32) {
        push_integer(&mut self.buffer, invoke_id);
    }

    /// 开始一个 TLV：写 tag + 0x00 长度占位，返回内容起始偏移（D6）。
    fn begin_tlv(&mut self, tag: u8) -> usize {
        self.buffer.push(tag);
        self.buffer.push(0x00); // 长度占位字节
        self.buffer.len()
    }

    /// 结束一个 TLV：回填长度 = 内容字节数（短型单字节 / 0x82 双字节长型）。
    fn end_tlv(&mut self, content_start: usize) {
        let len = self.buffer.len() - content_start;
        if len < 0x80 {
            self.buffer[content_start - 1] = len as u8;
        } else {
            self.buffer[content_start - 1] = 0x82;
            let be = (len as u16).to_be_bytes();
            self.buffer.insert(content_start, be[0]);
            self.buffer.insert(content_start + 1, be[1]);
        }
    }
}

impl Default for BerEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// 写入 BER 长度（独立辅助：< 0x80 短型，否则 0x82 双字节长型）。
fn write_length(buffer: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        buffer.push(len as u8);
    } else {
        buffer.push(0x82);
        buffer.extend_from_slice(&(len as u16).to_be_bytes());
    }
}

/// 写入 VisibleString TLV。
fn push_visible_string(buffer: &mut Vec<u8>, s: &str) {
    buffer.push(TAG_VISIBLE_STRING);
    write_length(buffer, s.len());
    buffer.extend_from_slice(s.as_bytes());
}

/// 写入 INTEGER TLV（大端最小字节数；最高位为 1 时前补 0x00 保持正数语义）。
fn push_integer(buffer: &mut Vec<u8>, v: u32) {
    let be = v.to_be_bytes();
    let mut start = 0usize;
    while start < 3 && be[start] == 0 && (be[start + 1] & 0x80) == 0 {
        start += 1;
    }
    let content = &be[start..];
    buffer.push(TAG_INTEGER);
    buffer.push(content.len() as u8);
    buffer.extend_from_slice(content);
}

/// 写入 Data 值 TLV（与解码侧 tag 对称：Bool 0x80 / Int32 0x85 / Float 0x87）。
fn push_da_value(buffer: &mut Vec<u8>, val: &DaValue) {
    match val {
        DaValue::Bool(b) => {
            buffer.push(TAG_DATA_BOOLEAN);
            buffer.push(1);
            buffer.push(u8::from(*b));
        }
        DaValue::Int32(v) => {
            buffer.push(TAG_DATA_INTEGER);
            buffer.push(4);
            buffer.extend_from_slice(&v.to_be_bytes());
        }
        DaValue::Float32(f) => {
            buffer.push(TAG_DATA_FLOAT);
            buffer.push(4);
            buffer.extend_from_slice(&f.to_be_bytes());
        }
        DaValue::Float64(f) => {
            buffer.push(TAG_DATA_FLOAT);
            buffer.push(8);
            buffer.extend_from_slice(&f.to_be_bytes());
        }
        DaValue::Enum(v) => {
            buffer.push(TAG_DATA_ENUM);
            buffer.push(2);
            buffer.extend_from_slice(&v.to_be_bytes());
        }
        DaValue::StringVal(s) => {
            buffer.push(TAG_DATA_STRING);
            write_length(buffer, s.len());
            buffer.extend_from_slice(s.as_bytes());
        }
        DaValue::Timestamp(t) => {
            buffer.push(TAG_DATA_TIMESTAMP);
            buffer.push(8);
            buffer.extend_from_slice(&t.to_be_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::string::String;
    use alloc::vec::Vec;

    use super::*;
    use crate::ber_decode::read_tag_length;

    fn spec(domain: &str, item: &str) -> VarAccessSpec {
        VarAccessSpec {
            domain: String::from(domain),
            item: String::from(item),
        }
    }

    /// 解出 Read 请求各层：(pdu_len, invoke_id, list_content_len)。
    fn parse_read_request(data: &[u8]) -> (usize, u32, usize) {
        let mut pos = 0usize;
        let (tag, pdu_len) = read_tag_length(data, &mut pos).unwrap();
        assert_eq!(tag, TAG_CONFIRMED_REQUEST);
        let (itag, ilen) = read_tag_length(data, &mut pos).unwrap();
        assert_eq!(itag, TAG_INTEGER);
        let mut id: u32 = 0;
        for i in 0..ilen {
            id = (id << 8) | u32::from(data[pos + i]);
        }
        pos += ilen;
        let (rtag, _rlen) = read_tag_length(data, &mut pos).unwrap();
        assert_eq!(rtag, TAG_READ_REQUEST);
        let (ltag, llen) = read_tag_length(data, &mut pos).unwrap();
        assert_eq!(ltag, TAG_SEQUENCE_OF);
        (pdu_len, id, llen)
    }

    // ===== BE1：Read 请求 0xA0/0xA4 tag + 顶层长度为内容字节数 =====
    #[test]
    fn test_be1_read_request_top_tags() {
        let mut enc = BerEncoder::new();
        let vars = [spec("IED1_LD0", "XCBR1.Pos.stVal")];
        let pdu = enc.encode_read_request(1, &vars);
        assert_eq!(pdu[0], 0xA0);
        let (pdu_len, _, _) = parse_read_request(pdu);
        assert_eq!(pdu_len, pdu.len() - 2); // 短型：总长 = tag(1) + len(1) + 内容
    }

    // ===== BE2：invokeID 编码（INTEGER 最小字节）=====
    #[test]
    fn test_be2_invoke_id_encoding() {
        let mut enc = BerEncoder::new();
        let vars = [spec("D", "I")];
        let pdu = enc.encode_read_request(7, &vars);
        let (_, id, _) = parse_read_request(pdu);
        assert_eq!(id, 7);
        // invoke_id = 7 → [0x02, 0x01, 0x07]
        assert_eq!(&pdu[2..5], &[0x02, 0x01, 0x07]);
        // 大 invokeID：0x1234 → [0x02, 0x02, 0x12, 0x34]
        let pdu2 = enc.encode_read_request(0x1234, &vars);
        assert_eq!(&pdu2[2..6], &[0x02, 0x02, 0x12, 0x34]);
        let (_, id2, _) = parse_read_request(pdu2);
        assert_eq!(id2, 0x1234);
    }

    // ===== BE3：domain + item VisibleString（0x1A）=====
    #[test]
    fn test_be3_domain_item_visible_strings() {
        let mut enc = BerEncoder::new();
        let vars = [spec("IED1_LD0", "XCBR1.Pos.stVal")];
        let pdu = enc.encode_read_request(1, &vars);
        let needle: &[u8] = b"\x1A\x08IED1_LD0\x1A\x0FXCBR1.Pos.stVal";
        assert!(pdu.windows(needle.len()).any(|w| w == needle));
    }

    // ===== BE4：单变量 listOfVariable 长度 == 条目字节数 =====
    #[test]
    fn test_be4_single_var_list_length_bytes() {
        let mut enc = BerEncoder::new();
        let vars = [spec("IED1_LD0", "XCBR1.Pos.stVal")];
        let pdu = enc.encode_read_request(1, &vars);
        let (_, _, llen) = parse_read_request(pdu);
        // 条目 = (1+1+8) + (1+1+15) = 27 字节
        assert_eq!(llen, 27);
    }

    // ===== BE5：多变量 listOfVariable 长度 == 字节和（非元素个数 2，D6）=====
    #[test]
    fn test_be5_multi_var_list_length_is_byte_sum() {
        let mut enc = BerEncoder::new();
        let vars = [
            spec("IED1_LD0", "XCBR1.Pos.stVal"),
            spec("IED1_LD0", "MMXU1.PhV.mag"),
        ];
        let pdu = enc.encode_read_request(1, &vars);
        let (_, _, llen) = parse_read_request(pdu);
        // 条目1 = 10 + 17 = 27；条目2 = 10 + 15 = 25；合计 52（≠ 个数 2）
        assert_eq!(llen, 27 + 25);
        assert_ne!(llen, vars.len());
    }

    // ===== BE6：内容 ≥ 0x80 → 0x82 双字节长型，声明长度 == 实际内容字节数 =====
    #[test]
    fn test_be6_long_form_length() {
        let mut enc = BerEncoder::new();
        let vars: Vec<VarAccessSpec> = (0..20)
            .map(|i| spec("IED1_LD0_DOMAIN", &alloc::format!("XCBR{}.Pos.stVal", i)))
            .collect();
        let pdu = enc.encode_read_request(1, &vars);
        // 顶层应为长型：0xA0 0x82 hi lo
        assert_eq!(pdu[0], 0xA0);
        assert_eq!(pdu[1], 0x82);
        let declared = (usize::from(pdu[2]) << 8) | usize::from(pdu[3]);
        assert_eq!(declared, pdu.len() - 4);
        // read_tag_length 可解出相同长度
        let mut pos = 0usize;
        let (tag, len) = read_tag_length(pdu, &mut pos).unwrap();
        assert_eq!(tag, 0xA0);
        assert_eq!(len, declared);
    }

    // ===== BE7：Write 请求 0xA5 tag =====
    #[test]
    fn test_be7_write_request_tag() {
        let mut enc = BerEncoder::new();
        let vars = [(spec("IED1_LD0", "XCBR1.Pos.stVal"), DaValue::Bool(true))];
        let pdu = enc.encode_write_request(1, &vars);
        assert_eq!(pdu[0], 0xA0);
        assert!(pdu.contains(&0xA5));
        // 顶层长度正确
        let mut pos = 0usize;
        let (tag, len) = read_tag_length(pdu, &mut pos).unwrap();
        assert_eq!(tag, 0xA0);
        assert_eq!(len, pdu.len() - 2);
    }

    // ===== BE8：Bool 值编码（0x80 01 01，与解码侧对称）=====
    #[test]
    fn test_be8_write_bool_value() {
        let mut enc = BerEncoder::new();
        let vars = [(spec("D", "I"), DaValue::Bool(true))];
        let pdu = enc.encode_write_request(1, &vars);
        let needle: &[u8] = &[0x80, 0x01, 0x01];
        assert!(pdu.windows(3).any(|w| w == needle));
    }

    // ===== BE9：Int32 值编码（0x85 04 + 4 字节大端）=====
    #[test]
    fn test_be9_write_int32_value() {
        let mut enc = BerEncoder::new();
        let vars = [(spec("D", "I"), DaValue::Int32(0x0102_0304))];
        let pdu = enc.encode_write_request(1, &vars);
        let needle: &[u8] = &[0x85, 0x04, 0x01, 0x02, 0x03, 0x04];
        assert!(pdu.windows(6).any(|w| w == needle));
    }

    // ===== BE10：Float64 值编码（0x87 08 + 8 字节大端）=====
    #[test]
    fn test_be10_write_float64_value() {
        let mut enc = BerEncoder::new();
        let vars = [(spec("D", "I"), DaValue::Float64(2.5))];
        let pdu = enc.encode_write_request(1, &vars);
        let mut needle: Vec<u8> = alloc::vec![0x87, 0x08];
        needle.extend_from_slice(&2.5f64.to_be_bytes());
        assert!(pdu.windows(needle.len()).any(|w| w == needle));
    }
}
