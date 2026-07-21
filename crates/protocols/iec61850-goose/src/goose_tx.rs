//! GOOSE 发布者：以太网组播组帧 + BER PDU 编码 + st_num/sq_num 重传状态机（D5/D6/D7/D8）.
//!
//! 帧格式（**APPID 2 字节为蓝图省略字段**，接收侧过滤器需要，本实现补入并声明）：
//! `dst MAC(6B) + src 组播 MAC 01:0C:CD:01:00:00(6B) + EtherType 0x88B8(2B)
//! + APPID(2B) + GOOSE PDU`
//!
//! GOOSE PDU（BER TLV，长度恒为**内容字节数**：< 0x80 短型单字节，
//! ≥ 0x80 用 0x82 双字节长型，与 v0.106.0 一致）：
//! `0x61` → gocbRef(0x80 VisibleString) / timeAllowedToLive(0x81 INTEGER)
//! / datSet(0x82 VisibleString) / goID(0x83 VisibleString) / t(0x84 UtcTime 8B BE)
//! / stNum(0x85 INTEGER) / sqNum(0x86 INTEGER) / simulation(0x87 BOOLEAN，恒 false)
//! / confRef(0x88 INTEGER，恒 1) / ndsCom(0x89 BOOLEAN，恒 false)
//! / numDatSetEntries(0x8A INTEGER) / allData(0xAB **tag + 长度 + 内容**，D7)
//!
//! allData 条目 tag 与 v0.106.0 MMS 栈统一（D8）：Bool 0x80 / Int32 0x85 /
//! Float32|Float64 0x87（4B/8B）/ Enum 0x86 / StringVal 0x8A / Timestamp 0x91 8B。
//!
//! 重传策略（蓝图 §4.3）：事件后前 3 次按 min_time 间隔重发，其后按 max_time
//! 周期心跳；`update_value` 触发 st_num+1、sq_num=0、needs_retransmit=true、
//! retransmit_count=0。时间由外部注入（D6）：publish(now) / retransmit_if_needed(now)。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_iec61850_model::DaValue;

use crate::dataset::GooseDataset;
use crate::{GooseError, L2Transport};

/// GOOSE EtherType（IEC 61850-8-1）。
pub(crate) const ETHERTYPE_GOOSE: u16 = 0x88B8;
/// GOOSE 组播源 MAC（IEC 61850-8-1 保留段）。
pub(crate) const SRC_MULTICAST: [u8; 6] = [0x01, 0x0C, 0xCD, 0x01, 0x00, 0x00];

/// GOOSE PDU tag。
pub(crate) const TAG_GOOSE_PDU: u8 = 0x61;
/// gocbRef（VisibleString）tag。
pub(crate) const TAG_GOCB_REF: u8 = 0x80;
/// timeAllowedToLive（INTEGER）tag。
pub(crate) const TAG_TIME_ALLOWED: u8 = 0x81;
/// datSet（VisibleString）tag。
pub(crate) const TAG_DATSET: u8 = 0x82;
/// goID（VisibleString）tag。
pub(crate) const TAG_GOID: u8 = 0x83;
/// t（UtcTime 8B）tag。
pub(crate) const TAG_TIMESTAMP: u8 = 0x84;
/// stNum（INTEGER）tag。
pub(crate) const TAG_ST_NUM: u8 = 0x85;
/// sqNum（INTEGER）tag。
pub(crate) const TAG_SQ_NUM: u8 = 0x86;
/// simulation（BOOLEAN）tag。
pub(crate) const TAG_SIMULATION: u8 = 0x87;
/// confRef（INTEGER）tag。
pub(crate) const TAG_CONF_REF: u8 = 0x88;
/// ndsCom（BOOLEAN）tag。
pub(crate) const TAG_NDS_COM: u8 = 0x89;
/// numDatSetEntries（INTEGER）tag。
pub(crate) const TAG_NUM_ENTRIES: u8 = 0x8A;
/// allData（SEQUENCE OF Data）tag。
pub(crate) const TAG_ALL_DATA: u8 = 0xAB;

/// allData 条目：boolean tag（D8，与 MMS 一致）。
pub(crate) const DATA_BOOLEAN: u8 = 0x80;
/// allData 条目：integer tag。
pub(crate) const DATA_INTEGER: u8 = 0x85;
/// allData 条目：floating-point tag（4B→Float32、8B→Float64）。
pub(crate) const DATA_FLOAT: u8 = 0x87;
/// allData 条目：enum tag。
pub(crate) const DATA_ENUM: u8 = 0x86;
/// allData 条目：visible-string tag。
pub(crate) const DATA_STRING: u8 = 0x8A;
/// allData 条目：utc-time/timestamp tag（8B）。
pub(crate) const DATA_TIMESTAMP: u8 = 0x91;

/// GOOSE 控制块（蓝图 §4.1，9 字段全 pub）。
#[derive(Debug, Clone, PartialEq)]
pub struct GooseControlBlock {
    /// 控制块引用（如 "IED1LD/LLN0$GO$gocb1"）。
    pub go_cb_ref: String,
    /// GOOSE APPID（帧过滤键，0 非法）。
    pub app_id: u16,
    /// 目标组播 MAC。
    pub dst_addr: [u8; 6],
    /// 最小重发间隔 ms。
    pub min_time: u16,
    /// 最大重发间隔 ms（心跳周期，同时作为 timeAllowedToLive）。
    pub max_time: u16,
    /// 状态号（事件计数）。
    pub st_num: u32,
    /// 序号（同事件内重传计数）。
    pub sq_num: u32,
    /// 数据集引用。
    pub dataset_ref: String,
    /// 是否需要重发。
    pub needs_retransmit: bool,
}

/// GOOSE 发布者（泛型二层传输，D5；时间注入，D6）。
pub struct GoosePublisher<T: L2Transport> {
    cb: GooseControlBlock,
    dataset: GooseDataset,
    last_tx_time: u64,
    retransmit_count: u32,
    transport: T,
}

impl<T: L2Transport> GoosePublisher<T> {
    /// 创建发布者（app_id == 0 → `InvalidConfig`）。
    pub fn new(cb: GooseControlBlock, transport: T) -> Result<Self, GooseError> {
        if cb.app_id == 0 {
            return Err(GooseError::InvalidConfig);
        }
        Ok(Self {
            cb,
            dataset: GooseDataset::new(),
            last_tx_time: 0,
            retransmit_count: 0,
            transport,
        })
    }

    /// 更新数据集值并标记新事件：st_num+1、sq_num=0、needs_retransmit=true、retransmit_count=0。
    pub fn update_value(&mut self, path: &str, value: DaValue) {
        self.dataset.set(path, value);
        self.cb.st_num = self.cb.st_num.wrapping_add(1);
        self.cb.sq_num = 0;
        self.cb.needs_retransmit = true;
        self.retransmit_count = 0;
    }

    /// 组帧并发送（D6：t = now）；发送后 sq_num+1、last_tx_time = now。
    pub fn publish(&mut self, now: u64) -> Result<(), GooseError> {
        let frame = self.build_frame(now);
        self.transport.send(&frame)?;
        self.cb.sq_num = self.cb.sq_num.wrapping_add(1);
        self.last_tx_time = now;
        Ok(())
    }

    /// 按需重传（蓝图 §4.3）：前 3 次按 min_time 间隔，其后按 max_time 周期。
    ///
    /// 到达间隔则 publish 并 retransmit_count+1，返回 true；否则 false。
    pub fn retransmit_if_needed(&mut self, now: u64) -> bool {
        if !self.cb.needs_retransmit {
            return false;
        }
        let interval = if self.retransmit_count < 3 {
            u64::from(self.cb.min_time)
        } else {
            u64::from(self.cb.max_time)
        };
        if now.saturating_sub(self.last_tx_time) >= interval {
            let _ = self.publish(now);
            self.retransmit_count = self.retransmit_count.wrapping_add(1);
            true
        } else {
            false
        }
    }

    /// 读取控制块。
    pub fn cb(&self) -> &GooseControlBlock {
        &self.cb
    }

    /// 读取数据集。
    pub fn dataset(&self) -> &GooseDataset {
        &self.dataset
    }

    /// 读取传输层（测试断言 / 集成层接线用）。
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// 可变读取传输层（测试脚本注入用）。
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// 组帧：以太网头 + APPID + GOOSE PDU（BER TLV）。
    fn build_frame(&self, now: u64) -> Vec<u8> {
        let mut content = Vec::with_capacity(256);
        push_visible_string(&mut content, TAG_GOCB_REF, &self.cb.go_cb_ref);
        push_integer(&mut content, TAG_TIME_ALLOWED, u64::from(self.cb.max_time));
        push_visible_string(&mut content, TAG_DATSET, &self.cb.dataset_ref);
        push_visible_string(&mut content, TAG_GOID, &self.cb.go_cb_ref);
        // t（UtcTime 8B BE，D6 时间注入）
        content.push(TAG_TIMESTAMP);
        content.push(8);
        content.extend_from_slice(&now.to_be_bytes());
        push_integer(&mut content, TAG_ST_NUM, u64::from(self.cb.st_num));
        push_integer(&mut content, TAG_SQ_NUM, u64::from(self.cb.sq_num));
        push_boolean(&mut content, TAG_SIMULATION, false);
        push_integer(&mut content, TAG_CONF_REF, 1);
        push_boolean(&mut content, TAG_NDS_COM, false);
        push_integer(
            &mut content,
            TAG_NUM_ENTRIES,
            self.dataset.entries.len() as u64,
        );
        // allData（D7：tag + 长度 + 内容 完整 TLV）
        let mut all_data = Vec::new();
        for entry in &self.dataset.entries {
            push_da_value(&mut all_data, &entry.value);
        }
        push_tlv(&mut content, TAG_ALL_DATA, &all_data);

        let mut frame = Vec::with_capacity(content.len() + 20);
        frame.extend_from_slice(&self.cb.dst_addr);
        frame.extend_from_slice(&SRC_MULTICAST);
        frame.extend_from_slice(&ETHERTYPE_GOOSE.to_be_bytes());
        frame.extend_from_slice(&self.cb.app_id.to_be_bytes());
        push_tlv(&mut frame, TAG_GOOSE_PDU, &content);
        frame
    }
}

/// 写入 BER 长度（< 0x80 短型，否则 0x82 双字节长型，与 v0.106.0 一致）。
fn write_length(buf: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        buf.push(len as u8);
    } else {
        buf.push(0x82);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    }
}

/// 写入完整 TLV（tag + 长度 + 内容）。
fn push_tlv(buf: &mut Vec<u8>, tag: u8, content: &[u8]) {
    buf.push(tag);
    write_length(buf, content.len());
    buf.extend_from_slice(content);
}

/// 写入 VisibleString TLV。
fn push_visible_string(buf: &mut Vec<u8>, tag: u8, s: &str) {
    buf.push(tag);
    write_length(buf, s.len());
    buf.extend_from_slice(s.as_bytes());
}

/// 写入 INTEGER TLV（大端最小字节数且保持正数语义）。
fn push_integer(buf: &mut Vec<u8>, tag: u8, v: u64) {
    let be = v.to_be_bytes();
    let mut start = 0usize;
    while start < 7 && be[start] == 0 && (be[start + 1] & 0x80) == 0 {
        start += 1;
    }
    push_tlv(buf, tag, &be[start..]);
}

/// 写入 BOOLEAN TLV（1 字节）。
fn push_boolean(buf: &mut Vec<u8>, tag: u8, b: bool) {
    push_tlv(buf, tag, &[u8::from(b)]);
}

/// 写入 allData 数据条目 TLV（D8：tag 与 v0.106.0 MMS 栈统一）。
fn push_da_value(buf: &mut Vec<u8>, val: &DaValue) {
    match val {
        DaValue::Bool(b) => push_tlv(buf, DATA_BOOLEAN, &[u8::from(*b)]),
        DaValue::Int32(v) => push_tlv(buf, DATA_INTEGER, &v.to_be_bytes()),
        DaValue::Float32(f) => push_tlv(buf, DATA_FLOAT, &f.to_be_bytes()),
        DaValue::Float64(f) => push_tlv(buf, DATA_FLOAT, &f.to_be_bytes()),
        DaValue::Enum(v) => push_tlv(buf, DATA_ENUM, &v.to_be_bytes()),
        DaValue::StringVal(s) => {
            buf.push(DATA_STRING);
            write_length(buf, s.len());
            buf.extend_from_slice(s.as_bytes());
        }
        DaValue::Timestamp(t) => push_tlv(buf, DATA_TIMESTAMP, &t.to_be_bytes()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::string::String;
    use alloc::vec;

    use super::*;
    use crate::goose_rx::read_tag_length;
    use crate::MockL2;

    const DST: [u8; 6] = [0x01, 0x0C, 0xCD, 0x01, 0x00, 0x01];

    fn make_cb() -> GooseControlBlock {
        GooseControlBlock {
            go_cb_ref: String::from("IED1LD/LLN0$GO$gocb1"),
            app_id: 0x0001,
            dst_addr: DST,
            min_time: 10,
            max_time: 5000,
            st_num: 0,
            sq_num: 0,
            dataset_ref: String::from("IED1LD/LLN0$ds1"),
            needs_retransmit: false,
        }
    }

    fn make_publisher() -> GoosePublisher<MockL2> {
        GoosePublisher::new(make_cb(), MockL2::new()).unwrap()
    }

    /// 在帧中查找子串。
    fn contains(frame: &[u8], needle: &[u8]) -> bool {
        frame.windows(needle.len()).any(|w| w == needle)
    }

    // ===== TX7：以太网头 dst/src 组播 MAC + EtherType 0x88B8 + APPID =====
    #[test]
    fn test_tx7_ethernet_header() {
        let mut p = make_publisher();
        p.publish(1000).unwrap();
        let frame = &p.transport().tx_frames()[0];
        assert_eq!(&frame[0..6], &DST);
        assert_eq!(&frame[6..12], &[0x01, 0x0C, 0xCD, 0x01, 0x00, 0x00]);
        assert_eq!(&frame[12..14], &0x88B8u16.to_be_bytes());
        assert_eq!(&frame[14..16], &0x0001u16.to_be_bytes());
        assert_eq!(frame[16], TAG_GOOSE_PDU);
    }

    // ===== TX8：gocbRef 0x80 VisibleString 编码 =====
    #[test]
    fn test_tx8_gocb_ref_encoding() {
        let mut p = make_publisher();
        p.publish(1000).unwrap();
        let frame = &p.transport().tx_frames()[0];
        let mut needle = vec![TAG_GOCB_REF, 20];
        needle.extend_from_slice(b"IED1LD/LLN0$GO$gocb1");
        assert!(contains(frame, &needle));
    }

    // ===== TX9：timeAllowedToLive 0x81 INTEGER =====
    #[test]
    fn test_tx9_time_allowed_to_live() {
        let mut p = make_publisher();
        p.publish(1000).unwrap();
        let frame = &p.transport().tx_frames()[0];
        // max_time = 5000 = 0x1388 → INTEGER 2 字节
        assert!(contains(frame, &[TAG_TIME_ALLOWED, 0x02, 0x13, 0x88]));
    }

    // ===== TX10：t 0x84 UtcTime 8 字节 BE（D6 时间注入）=====
    #[test]
    fn test_tx10_timestamp_8_bytes() {
        let mut p = make_publisher();
        let now = 0x0102_0304_0506_0708u64;
        p.publish(now).unwrap();
        let frame = &p.transport().tx_frames()[0];
        let mut needle = vec![TAG_TIMESTAMP, 0x08];
        needle.extend_from_slice(&now.to_be_bytes());
        assert!(contains(frame, &needle));
    }

    // ===== TX11：stNum 0x85 INTEGER =====
    #[test]
    fn test_tx11_st_num_encoding() {
        let mut p = make_publisher();
        p.update_value("A", DaValue::Bool(true)); // st_num: 0 → 1
        p.publish(1000).unwrap();
        let frame = &p.transport().tx_frames()[0];
        assert!(contains(frame, &[TAG_ST_NUM, 0x01, 0x01]));
        // sqNum 0x86 在 stNum 之后
        let st_pos = frame
            .windows(3)
            .position(|w| w == [TAG_ST_NUM, 0x01, 0x01])
            .unwrap();
        assert_eq!(frame[st_pos + 3], TAG_SQ_NUM);
    }

    // ===== TX12：sqNum 0x86 INTEGER =====
    #[test]
    fn test_tx12_sq_num_encoding() {
        let mut p = make_publisher();
        p.update_value("A", DaValue::Bool(true));
        p.publish(1000).unwrap(); // 发送时 sq=0
        let frame0 = &p.transport().tx_frames()[0];
        assert!(contains(frame0, &[TAG_SQ_NUM, 0x01, 0x00]));
        p.publish(1010).unwrap(); // 发送时 sq=1
        let frame1 = &p.transport().tx_frames()[1];
        assert!(contains(frame1, &[TAG_SQ_NUM, 0x01, 0x01]));
    }

    // ===== TX13：allData 0xAB 含长度（D7：tag + 长度 + 内容）=====
    #[test]
    fn test_tx13_all_data_has_length() {
        let mut p = make_publisher();
        p.update_value("A", DaValue::Bool(true));
        p.publish(1000).unwrap();
        let frame = &p.transport().tx_frames()[0];
        // allData 内容 = [0x80, 0x01, 0x01]（3 字节）→ [0xAB, 0x03, 0x80, 0x01, 0x01]
        assert!(contains(frame, &[TAG_ALL_DATA, 0x03, 0x80, 0x01, 0x01]));
        // 用 read_tag_length 解出声明长度 == 实际内容长度
        let ab_pos = frame
            .windows(2)
            .position(|w| w == [TAG_ALL_DATA, 0x03])
            .unwrap();
        let mut pos = ab_pos;
        let (tag, len) = read_tag_length(frame, &mut pos).unwrap();
        assert_eq!(tag, TAG_ALL_DATA);
        assert_eq!(len, 3);
        assert_eq!(pos + len, frame.len()); // allData 为 PDU 末字段
    }

    // ===== TX14：数据 tag 统一 0x80/0x85/0x87（D8）=====
    #[test]
    fn test_tx14_unified_data_tags() {
        let mut p = make_publisher();
        p.update_value("b", DaValue::Bool(true));
        p.update_value("i", DaValue::Int32(0x0102_0304));
        p.update_value("f", DaValue::Float32(1.5));
        p.publish(1000).unwrap();
        let frame = &p.transport().tx_frames()[0];
        assert!(contains(frame, &[DATA_BOOLEAN, 0x01, 0x01])); // Bool 0x80
        assert!(contains(
            frame,
            &[DATA_INTEGER, 0x04, 0x01, 0x02, 0x03, 0x04]
        )); // Int32 0x85
        let mut f32_needle = vec![DATA_FLOAT, 0x04];
        f32_needle.extend_from_slice(&1.5f32.to_be_bytes());
        assert!(contains(frame, &f32_needle)); // Float32 0x87 4B
                                               // 蓝图 bug tag（0x01/0x03）不得作为 allData 条目 tag：TLV 遍历校验 tag 序列
        let ab_pos = frame.windows(2).position(|w| w[0] == TAG_ALL_DATA).unwrap();
        let mut pos = ab_pos;
        let (tag, len) = read_tag_length(frame, &mut pos).unwrap();
        assert_eq!(tag, TAG_ALL_DATA);
        let ad_end = pos + len;
        let mut entry_tags = alloc::vec::Vec::new();
        while pos < ad_end {
            let (et, el) = read_tag_length(frame, &mut pos).unwrap();
            entry_tags.push(et);
            pos += el;
        }
        assert_eq!(pos, ad_end);
        assert_eq!(entry_tags, &[DATA_BOOLEAN, DATA_INTEGER, DATA_FLOAT]);
        assert!(!entry_tags.contains(&0x01));
        assert!(!entry_tags.contains(&0x03));
    }

    // ===== TX15：update_value 后 st+1 / sq=0 / needs_retransmit=true / retransmit_count=0 =====
    #[test]
    fn test_tx15_update_value_state() {
        let mut p = make_publisher();
        p.publish(1000).unwrap(); // sq → 1
        p.update_value("A", DaValue::Int32(42));
        assert_eq!(p.cb().st_num, 1);
        assert_eq!(p.cb().sq_num, 0);
        assert!(p.cb().needs_retransmit);
        // 再更新覆盖同路径仍 st+1
        p.update_value("A", DaValue::Int32(43));
        assert_eq!(p.cb().st_num, 2);
        assert_eq!(p.dataset().entries.len(), 1);
        assert_eq!(p.dataset().entries[0].value, DaValue::Int32(43));
    }

    // ===== TX16：publish 后 sq_num+1、last_tx_time 更新 =====
    #[test]
    fn test_tx16_publish_increments_sq_num() {
        let mut p = make_publisher();
        assert_eq!(p.cb().sq_num, 0);
        p.publish(1000).unwrap();
        assert_eq!(p.cb().sq_num, 1);
        p.publish(1010).unwrap();
        assert_eq!(p.cb().sq_num, 2);
        // st_num 不因 publish 改变
        assert_eq!(p.cb().st_num, 0);
    }

    // ===== TX17：retransmit 前 3 次 min_time、其后 max_time（蓝图 §4.3）=====
    #[test]
    fn test_tx17_retransmit_timing() {
        let mut p = make_publisher();
        p.update_value("A", DaValue::Bool(true)); // st=1
        p.publish(1000).unwrap(); // last_tx = 1000
                                  // 未到 min_time：不重发
        assert!(!p.retransmit_if_needed(1009));
        // 第 1 次：+min_time
        assert!(p.retransmit_if_needed(1010));
        // 第 2 次：+2×min_time
        assert!(p.retransmit_if_needed(1020));
        // 第 3 次：+3×min_time
        assert!(p.retransmit_if_needed(1030));
        // 第 4 次起按 max_time：+3×min_time+max_time-1 不到
        assert!(!p.retransmit_if_needed(1030 + 4999));
        // 到达 max_time → 重发
        assert!(p.retransmit_if_needed(1030 + 5000));
        // 共 1 初始 + 4 重传 = 5 帧；st 恒 1，sq 递增
        assert_eq!(p.transport().tx_frames().len(), 5);
        assert_eq!(p.cb().st_num, 1);
        assert_eq!(p.cb().sq_num, 5);
        // needs_retransmit=false 时不再重发
        let mut q = make_publisher();
        assert!(!q.retransmit_if_needed(999_999));
    }

    // ===== TX18：整帧 TLV 可被 read_tag_length 逐层解析 =====
    #[test]
    fn test_tx18_full_frame_tlv_walk() {
        let mut p = make_publisher();
        p.update_value("b", DaValue::Bool(true));
        p.update_value("i", DaValue::Int32(-1));
        p.update_value("f", DaValue::Float64(2.5));
        p.publish(0xAABB_CCDDu64).unwrap();
        let frame = p.transport().tx_frames()[0].clone();

        // 以太网头 16 字节（6+6+2+2）后为 GOOSE PDU
        let mut pos = 16usize;
        let (tag, pdu_len) = read_tag_length(&frame, &mut pos).unwrap();
        assert_eq!(tag, TAG_GOOSE_PDU);
        assert_eq!(pos + pdu_len, frame.len());
        let pdu_end = pos + pdu_len;

        // 逐字段遍历，tag 序列必须与编码顺序一致
        let expected_tags = [
            TAG_GOCB_REF,
            TAG_TIME_ALLOWED,
            TAG_DATSET,
            TAG_GOID,
            TAG_TIMESTAMP,
            TAG_ST_NUM,
            TAG_SQ_NUM,
            TAG_SIMULATION,
            TAG_CONF_REF,
            TAG_NDS_COM,
            TAG_NUM_ENTRIES,
            TAG_ALL_DATA,
        ];
        for (i, expect) in expected_tags.iter().enumerate() {
            let (t, l) = read_tag_length(&frame, &mut pos).unwrap();
            assert_eq!(&t, expect, "field {} tag mismatch", i);
            if t == TAG_ALL_DATA {
                let ad_end = pos + l;
                // 条目：Bool(0x80) / Int32(0x85) / Float64(0x87)
                let entry_tags = [DATA_BOOLEAN, DATA_INTEGER, DATA_FLOAT];
                for et in entry_tags {
                    let (etag, elen) = read_tag_length(&frame, &mut pos).unwrap();
                    assert_eq!(etag, et);
                    pos += elen;
                }
                assert_eq!(pos, ad_end);
            } else {
                pos += l;
            }
        }
        assert_eq!(pos, pdu_end);
    }
}
