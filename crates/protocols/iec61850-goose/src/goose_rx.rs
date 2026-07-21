//! GOOSE 订阅者：MAC/APPID 过滤 + BER PDU 解码 + st_num 跳变检测（D12）.
//!
//! poll 流程：recv → 空帧 Ok(None) → 帧长 < 16 BerDecodeError →
//! EtherType != 0x88B8 → Ok(None) → dst MAC 不匹配 → Ok(None) →
//! APPID 不匹配 → Ok(None) → 解码 GOOSE PDU → RxStatus 判定 → 回调 → 返回。
//!
//! RxStatus 判定（蓝图 §4.4，D12）：
//! - 首帧（last_st_num == 0）→ `New`
//! - st_num == last + 1 → `New`（新事件）
//! - st_num > last + 1 → `StJump`（事件丢失）
//! - st_num == last 且 sq_num > last_sq → `New`（同事件重传）
//! - 其余（完全重复 / 旧帧乱序）→ `Duplicate`
//!
//! rx 侧无数据集路径语义：`GooseEntry.path` 置空字符串（值与保序语义完整，声明）。
//! 数据条目 tag 与编码侧对称（D8）：Bool 0x80 / Int32 0x85 / Float32|Float64 0x87 /
//! Enum 0x86 / StringVal 0x8A / Timestamp 0x91；未知 tag 跳过不报错。

use alloc::boxed::Box;
use alloc::string::String;

use eneros_iec61850_model::DaValue;

use crate::dataset::{GooseDataset, GooseEntry};
use crate::goose_tx::{
    DATA_BOOLEAN, DATA_ENUM, DATA_FLOAT, DATA_INTEGER, DATA_STRING, DATA_TIMESTAMP,
    ETHERTYPE_GOOSE, TAG_ALL_DATA, TAG_GOOSE_PDU, TAG_SQ_NUM, TAG_ST_NUM, TAG_TIMESTAMP,
};
use crate::{GooseError, L2Transport};

/// 接收缓冲区大小（GOOSE 帧远小于以太网 MTU）。
const RECV_BUF_LEN: usize = 2048;

/// 解码后的 GOOSE PDU。
#[derive(Debug, Clone, PartialEq)]
pub struct GoosePdu {
    /// 状态号（事件计数）。
    pub st_num: u32,
    /// 序号（同事件内重传计数）。
    pub sq_num: u32,
    /// 事件时间戳（t 字段，8B BE）。
    pub timestamp: u64,
    /// 数据集（rx 侧 path 置空字符串，仅值保序）。
    pub dataset: GooseDataset,
}

/// 接收状态（D12：随 PDU 返回 st_num 跳变检测结果）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RxStatus {
    /// 新事件（含首帧与同事件重传帧）。
    New,
    /// 完全重复 / 旧帧乱序。
    Duplicate,
    /// st_num 跳变 > 1（事件丢失）。
    StJump,
}

/// PDU 回调类型（D9：去 Send+Sync bound）。
type PduCallback = Box<dyn Fn(&GoosePdu)>;

/// GOOSE 订阅者（泛型二层传输，D5；回调去 Send+Sync bound，D9）。
pub struct GooseSubscriber<T: L2Transport> {
    app_id: u16,
    filter_mac: [u8; 6],
    last_st_num: u32,
    last_sq_num: u32,
    callback: Option<PduCallback>,
    transport: T,
}

impl<T: L2Transport> GooseSubscriber<T> {
    /// 创建订阅者（app_id == 0 → `InvalidConfig`；初始 last_st/last_sq = 0）。
    pub fn new(app_id: u16, mac: [u8; 6], transport: T) -> Result<Self, GooseError> {
        if app_id == 0 {
            return Err(GooseError::InvalidConfig);
        }
        Ok(Self {
            app_id,
            filter_mac: mac,
            last_st_num: 0,
            last_sq_num: 0,
            callback: None,
            transport,
        })
    }

    /// 注册回调（D9：去 Send+Sync bound）；每收到有效 PDU 调用一次。
    pub fn set_callback<F: Fn(&GoosePdu) + 'static>(&mut self, f: F) {
        self.callback = Some(Box::new(f));
    }

    /// 轮询接收一帧：过滤不匹配 → Ok(None)；有效 PDU → Ok(Some((pdu, status)))。
    pub fn poll(&mut self) -> Result<Option<(GoosePdu, RxStatus)>, GooseError> {
        let mut buf = [0u8; RECV_BUF_LEN];
        let n = self.transport.recv(&mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        let frame = &buf[..n];
        if frame.len() < 16 {
            return Err(GooseError::BerDecodeError);
        }
        if u16::from_be_bytes([frame[12], frame[13]]) != ETHERTYPE_GOOSE {
            return Ok(None);
        }
        if frame[0..6] != self.filter_mac {
            return Ok(None);
        }
        if u16::from_be_bytes([frame[14], frame[15]]) != self.app_id {
            return Ok(None);
        }
        let pdu = decode_pdu(&frame[16..])?;
        let status = self.classify(pdu.st_num, pdu.sq_num);
        if status != RxStatus::Duplicate {
            self.last_st_num = pdu.st_num;
            self.last_sq_num = pdu.sq_num;
        }
        if let Some(cb) = &self.callback {
            cb(&pdu);
        }
        Ok(Some((pdu, status)))
    }

    /// 最近一次的 st_num（未收到帧时为 0）。
    pub fn last_st_num(&self) -> u32 {
        self.last_st_num
    }

    /// 可变读取传输层（测试脚本注入用）。
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// st_num/sq_num 状态判定（D12，见模块文档）。
    fn classify(&self, st_num: u32, sq_num: u32) -> RxStatus {
        if self.last_st_num == 0 {
            return RxStatus::New;
        }
        if st_num == self.last_st_num {
            if sq_num > self.last_sq_num {
                RxStatus::New // 同事件重传
            } else {
                RxStatus::Duplicate
            }
        } else if st_num == self.last_st_num + 1 {
            RxStatus::New
        } else if st_num > self.last_st_num + 1 {
            RxStatus::StJump
        } else {
            RxStatus::Duplicate // 旧帧乱序
        }
    }
}

/// 读取一个 TLV 的 tag 与内容长度，`pos` 推进到内容起始（与 v0.106.0 同规则）.
///
/// 支持短型（< 0x80）与 0x82 双字节长型；声明长度超出剩余缓冲区、
/// 或其他长型标记 → `BerDecodeError`。
pub(crate) fn read_tag_length(data: &[u8], pos: &mut usize) -> Result<(u8, usize), GooseError> {
    let tag = *data.get(*pos).ok_or(GooseError::BerDecodeError)?;
    *pos += 1;
    let first = usize::from(*data.get(*pos).ok_or(GooseError::BerDecodeError)?);
    *pos += 1;
    let len = if first < 0x80 {
        first
    } else if first == 0x82 {
        let hi = usize::from(*data.get(*pos).ok_or(GooseError::BerDecodeError)?);
        let lo = usize::from(*data.get(*pos + 1).ok_or(GooseError::BerDecodeError)?);
        *pos += 2;
        (hi << 8) | lo
    } else {
        return Err(GooseError::BerDecodeError);
    };
    if *pos + len > data.len() {
        return Err(GooseError::BerDecodeError);
    }
    Ok((tag, len))
}

/// 解码 GOOSE PDU（0x61 TLV → 字段序列；未知 tag 跳过）。
fn decode_pdu(data: &[u8]) -> Result<GoosePdu, GooseError> {
    let mut pos = 0usize;
    let (tag, pdu_len) = read_tag_length(data, &mut pos)?;
    if tag != TAG_GOOSE_PDU {
        return Err(GooseError::BerDecodeError);
    }
    let pdu_end = pos + pdu_len;
    let mut st_num = 0u32;
    let mut sq_num = 0u32;
    let mut timestamp = 0u64;
    let mut dataset = GooseDataset::new();
    while pos < pdu_end {
        let (t, l) = read_tag_length(data, &mut pos)?;
        match t {
            TAG_TIMESTAMP => {
                timestamp = read_be_u64(&data[pos..pos + l]);
                pos += l;
            }
            TAG_ST_NUM => {
                st_num = read_be_u32(&data[pos..pos + l])?;
                pos += l;
            }
            TAG_SQ_NUM => {
                sq_num = read_be_u32(&data[pos..pos + l])?;
                pos += l;
            }
            TAG_ALL_DATA => {
                let ad_end = pos + l;
                while pos < ad_end {
                    let (dt, dl) = read_tag_length(data, &mut pos)?;
                    if let Some(value) = decode_data_entry(dt, &data[pos..pos + dl])? {
                        dataset.entries.push(GooseEntry {
                            path: String::new(), // rx 侧无路径语义（声明）
                            value,
                        });
                    }
                    pos += dl;
                }
            }
            _ => {
                pos += l; // 未知字段跳过
            }
        }
    }
    Ok(GoosePdu {
        st_num,
        sq_num,
        timestamp,
        dataset,
    })
}

/// 解码 allData 单条目（未知 tag → Ok(None) 跳过不报错）。
fn decode_data_entry(tag: u8, data: &[u8]) -> Result<Option<DaValue>, GooseError> {
    match tag {
        DATA_BOOLEAN => {
            if data.is_empty() {
                return Err(GooseError::BerDecodeError);
            }
            Ok(Some(DaValue::Bool(data[0] != 0)))
        }
        DATA_INTEGER => {
            if data.is_empty() {
                return Err(GooseError::BerDecodeError);
            }
            let mut v: i32 = 0;
            for &b in data {
                v = (v << 8) | i32::from(b);
            }
            Ok(Some(DaValue::Int32(v)))
        }
        DATA_FLOAT => Ok(Some(decode_float(data))),
        DATA_ENUM => {
            if data.len() < 2 {
                return Err(GooseError::BerDecodeError);
            }
            let v = (u16::from(data[0]) << 8) | u16::from(data[1]);
            Ok(Some(DaValue::Enum(v)))
        }
        DATA_STRING => Ok(Some(DaValue::StringVal(
            String::from_utf8_lossy(data).into_owned(),
        ))),
        DATA_TIMESTAMP => Ok(Some(DaValue::Timestamp(read_be_u64(data)))),
        _ => Ok(None),
    }
}

/// 大端 u64（右对齐：≥ 8 字节取前 8，不足右对齐补零）。
fn read_be_u64(bytes: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    if bytes.len() >= 8 {
        b.copy_from_slice(&bytes[..8]);
    } else {
        b[8 - bytes.len()..].copy_from_slice(bytes);
    }
    u64::from_be_bytes(b)
}

/// 大端 u32（INTEGER 字段，长度 > 4 → BerDecodeError）。
fn read_be_u32(bytes: &[u8]) -> Result<u32, GooseError> {
    if bytes.len() > 4 {
        return Err(GooseError::BerDecodeError);
    }
    let mut v: u32 = 0;
    for &b in bytes {
        v = (v << 8) | u32::from(b);
    }
    Ok(v)
}

/// 浮点右对齐解码（与 v0.106.0 同规则）：4 → Float32；其余右对齐/截断到 8 字节 → Float64。
fn decode_float(bytes: &[u8]) -> DaValue {
    if bytes.len() == 4 {
        let mut b = [0u8; 4];
        b.copy_from_slice(bytes);
        return DaValue::Float32(f32::from_be_bytes(b));
    }
    DaValue::Float64(f64::from_be_bytes({
        let mut b = [0u8; 8];
        if bytes.len() >= 8 {
            b.copy_from_slice(&bytes[..8]);
        } else {
            b[8 - bytes.len()..].copy_from_slice(bytes);
        }
        b
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::rc::Rc;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    use super::*;
    use crate::goose_tx::{GooseControlBlock, GoosePublisher};
    use crate::MockL2;

    const DST: [u8; 6] = [0x01, 0x0C, 0xCD, 0x01, 0x00, 0x01];
    const APP_ID: u16 = 0x0001;

    fn make_sub() -> GooseSubscriber<MockL2> {
        GooseSubscriber::new(APP_ID, DST, MockL2::new()).unwrap()
    }

    /// 用发布者生成一帧（init_st 为初始 st_num；每次 update_value st+1）。
    fn publish_frame(init_st: u32, vals: &[(&str, DaValue)], now: u64) -> Vec<u8> {
        let cb = GooseControlBlock {
            go_cb_ref: String::from("IED1LD/LLN0$GO$gocb1"),
            app_id: APP_ID,
            dst_addr: DST,
            min_time: 10,
            max_time: 5000,
            st_num: init_st,
            sq_num: 0,
            dataset_ref: String::from("IED1LD/LLN0$ds1"),
            needs_retransmit: false,
        };
        let mut p = GoosePublisher::new(cb, MockL2::new()).unwrap();
        for (path, v) in vals {
            p.update_value(path, v.clone());
        }
        p.publish(now).unwrap();
        p.transport().tx_frames()[0].clone()
    }

    fn push_len(buf: &mut Vec<u8>, len: usize) {
        if len < 0x80 {
            buf.push(len as u8);
        } else {
            buf.push(0x82);
            buf.extend_from_slice(&(len as u16).to_be_bytes());
        }
    }

    /// 手工构造最小 GOOSE 帧（t/st/sq + allData 原始条目）。
    fn build_raw_frame(st: u8, sq: u8, entries: &[&[u8]]) -> Vec<u8> {
        let mut content = Vec::new();
        content.push(TAG_TIMESTAMP);
        content.push(8);
        content.extend_from_slice(&0u64.to_be_bytes());
        content.extend_from_slice(&[TAG_ST_NUM, 0x01, st]);
        content.extend_from_slice(&[TAG_SQ_NUM, 0x01, sq]);
        let mut all = Vec::new();
        for e in entries {
            all.extend_from_slice(e);
        }
        content.push(TAG_ALL_DATA);
        push_len(&mut content, all.len());
        content.extend_from_slice(&all);
        let mut frame = Vec::new();
        frame.extend_from_slice(&DST);
        frame.extend_from_slice(&[0x01, 0x0C, 0xCD, 0x01, 0x00, 0x00]);
        frame.extend_from_slice(&ETHERTYPE_GOOSE.to_be_bytes());
        frame.extend_from_slice(&APP_ID.to_be_bytes());
        frame.push(TAG_GOOSE_PDU);
        push_len(&mut frame, content.len());
        frame.extend_from_slice(&content);
        frame
    }

    // ===== RX19：有效帧解码 =====
    #[test]
    fn test_rx19_valid_frame_decode() {
        let frame = publish_frame(2, &[("A", DaValue::Bool(true))], 12_345);
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&frame);
        let (pdu, status) = sub.poll().unwrap().unwrap();
        assert_eq!(status, RxStatus::New);
        assert_eq!(pdu.st_num, 3);
        assert_eq!(pdu.sq_num, 0);
        assert_eq!(pdu.timestamp, 12_345);
        assert_eq!(pdu.dataset.entries.len(), 1);
        assert_eq!(pdu.dataset.entries[0].value, DaValue::Bool(true));
        assert_eq!(sub.last_st_num(), 3);
    }

    // ===== RX20：APPID 不匹配丢弃 → Ok(None) =====
    #[test]
    fn test_rx20_appid_mismatch_dropped() {
        let frame = publish_frame(0, &[("A", DaValue::Bool(true))], 0);
        let mut sub = GooseSubscriber::new(0x2222, DST, MockL2::new()).unwrap();
        sub.transport_mut().push_rx_frame(&frame);
        assert_eq!(sub.poll().unwrap(), None);
        assert_eq!(sub.last_st_num(), 0);
    }

    // ===== RX21：dst MAC 不匹配丢弃 → Ok(None) =====
    #[test]
    fn test_rx21_dst_mac_mismatch_dropped() {
        let frame = publish_frame(0, &[("A", DaValue::Bool(true))], 0);
        let mut sub =
            GooseSubscriber::new(APP_ID, [0x01, 0x0C, 0xCD, 0x01, 0x00, 0x99], MockL2::new())
                .unwrap();
        sub.transport_mut().push_rx_frame(&frame);
        assert_eq!(sub.poll().unwrap(), None);
    }

    // ===== RX22：st_num 跳变 → StJump（D12，蓝图 §6.5 故障注入）=====
    #[test]
    fn test_rx22_st_jump_detected() {
        let f5 = publish_frame(4, &[("A", DaValue::Int32(5))], 0); // st=5
        let f7 = publish_frame(6, &[("A", DaValue::Int32(7))], 0); // st=7（6 丢失）
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&f5);
        sub.transport_mut().push_rx_frame(&f7);
        let (pdu1, s1) = sub.poll().unwrap().unwrap();
        assert_eq!((pdu1.st_num, s1), (5, RxStatus::New));
        let (pdu2, s2) = sub.poll().unwrap().unwrap();
        assert_eq!(s2, RxStatus::StJump);
        assert_eq!(pdu2.st_num, 7);
        assert_eq!(sub.last_st_num(), 7);
    }

    // ===== RX23：完全重复帧 → Duplicate =====
    #[test]
    fn test_rx23_duplicate_frame() {
        let frame = publish_frame(0, &[("A", DaValue::Bool(true))], 0); // st=1
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&frame);
        sub.transport_mut().push_rx_frame(&frame);
        assert_eq!(sub.poll().unwrap().unwrap().1, RxStatus::New);
        assert_eq!(sub.poll().unwrap().unwrap().1, RxStatus::Duplicate);
        assert_eq!(sub.last_st_num(), 1);
    }

    // ===== RX24：非 0x88B8 → Ok(None)；空队列 → Ok(None) =====
    #[test]
    fn test_rx24_non_goose_ethertype() {
        let mut frame = publish_frame(0, &[("A", DaValue::Bool(true))], 0);
        frame[12] = 0x08; // EtherType → 0x0800 (IPv4)
        frame[13] = 0x00;
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&frame);
        assert_eq!(sub.poll().unwrap(), None);
        // 空队列
        assert_eq!(sub.poll().unwrap(), None);
    }

    // ===== RX25：截断帧 → BerDecodeError =====
    #[test]
    fn test_rx25_truncated_frame() {
        let frame = publish_frame(0, &[("A", DaValue::Bool(true))], 0);
        let truncated = &frame[..frame.len() - 1];
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(truncated);
        assert_eq!(sub.poll(), Err(GooseError::BerDecodeError));
        // 帧长 < 16（以太网头不全）
        sub.transport_mut().push_rx_frame(&frame[..10]);
        assert_eq!(sub.poll(), Err(GooseError::BerDecodeError));
    }

    // ===== RX26：Bool 0x80 值解码（true/false）=====
    #[test]
    fn test_rx26_bool_decode() {
        let frame = publish_frame(
            0,
            &[("t", DaValue::Bool(true)), ("f", DaValue::Bool(false))],
            0,
        );
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&frame);
        let (pdu, _) = sub.poll().unwrap().unwrap();
        assert_eq!(pdu.dataset.entries[0].value, DaValue::Bool(true));
        assert_eq!(pdu.dataset.entries[1].value, DaValue::Bool(false));
    }

    // ===== RX27：Int32 0x85 解码（含负数）=====
    #[test]
    fn test_rx27_int32_decode() {
        let frame = publish_frame(
            0,
            &[
                ("a", DaValue::Int32(-1)),
                ("b", DaValue::Int32(0x0102_0304)),
            ],
            0,
        );
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&frame);
        let (pdu, _) = sub.poll().unwrap().unwrap();
        assert_eq!(pdu.dataset.entries[0].value, DaValue::Int32(-1));
        assert_eq!(pdu.dataset.entries[1].value, DaValue::Int32(0x0102_0304));
    }

    // ===== RX28：Float32 4B + Float64 8B 解码 =====
    #[test]
    fn test_rx28_float_decode() {
        let frame = publish_frame(
            0,
            &[
                ("f32", DaValue::Float32(1.5)),
                ("f64", DaValue::Float64(2.5)),
            ],
            0,
        );
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&frame);
        let (pdu, _) = sub.poll().unwrap().unwrap();
        assert_eq!(pdu.dataset.entries[0].value, DaValue::Float32(1.5));
        assert_eq!(pdu.dataset.entries[1].value, DaValue::Float64(2.5));
    }

    // ===== RX29：时间戳提取（t 0x84 8B BE）=====
    #[test]
    fn test_rx29_timestamp_extract() {
        let now = 0x0102_0304_0506_0708u64;
        let frame = publish_frame(0, &[("A", DaValue::Bool(true))], now);
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&frame);
        let (pdu, _) = sub.poll().unwrap().unwrap();
        assert_eq!(pdu.timestamp, now);
    }

    // ===== RX30：未知 tag 跳过不报错 =====
    #[test]
    fn test_rx30_unknown_tag_skipped() {
        let frame = build_raw_frame(
            9,
            0,
            &[
                &[0x99, 0x02, 0xAA, 0xBB], // 未知 tag → 跳过
                &[DATA_INTEGER, 0x01, 0x2A],
            ],
        );
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&frame);
        let (pdu, status) = sub.poll().unwrap().unwrap();
        assert_eq!(status, RxStatus::New);
        assert_eq!(pdu.st_num, 9);
        assert_eq!(pdu.dataset.entries.len(), 1); // 未知条目不产生 entry
        assert_eq!(pdu.dataset.entries[0].value, DaValue::Int32(42));
        assert_eq!(pdu.dataset.entries[0].path, ""); // rx 无路径语义
    }

    // ===== LB31：publish→poll loopback 值一致 =====
    #[test]
    fn test_lb31_loopback_value_consistent() {
        let cb = GooseControlBlock {
            go_cb_ref: String::from("IED1LD/LLN0$GO$gocb1"),
            app_id: APP_ID,
            dst_addr: DST,
            min_time: 10,
            max_time: 5000,
            st_num: 0,
            sq_num: 0,
            dataset_ref: String::from("IED1LD/LLN0$ds1"),
            needs_retransmit: false,
        };
        let mut l2 = MockL2::new();
        l2.set_loopback(true);
        let mut p = GoosePublisher::new(cb, l2).unwrap();
        p.update_value("Pos.stVal", DaValue::Bool(true));
        p.publish(777).unwrap();
        let l2 = core::mem::take(p.transport_mut());
        let mut sub = GooseSubscriber::new(APP_ID, DST, l2).unwrap();
        let (pdu, status) = sub.poll().unwrap().unwrap();
        assert_eq!(status, RxStatus::New);
        assert_eq!(pdu.st_num, 1);
        assert_eq!(pdu.timestamp, 777);
        assert_eq!(pdu.dataset.entries.len(), 1);
        assert_eq!(pdu.dataset.entries[0].value, DaValue::Bool(true));
    }

    // ===== LB32：多条目保序 =====
    #[test]
    fn test_lb32_multi_entry_order_preserved() {
        let cb = GooseControlBlock {
            go_cb_ref: String::from("g"),
            app_id: APP_ID,
            dst_addr: DST,
            min_time: 10,
            max_time: 5000,
            st_num: 0,
            sq_num: 0,
            dataset_ref: String::from("d"),
            needs_retransmit: false,
        };
        let mut l2 = MockL2::new();
        l2.set_loopback(true);
        let mut p = GoosePublisher::new(cb, l2).unwrap();
        p.update_value("i", DaValue::Int32(1));
        p.update_value("f", DaValue::Float32(2.5));
        p.update_value("b", DaValue::Bool(true));
        p.update_value("s", DaValue::StringVal(String::from("go")));
        p.publish(0).unwrap();
        let l2 = core::mem::take(p.transport_mut());
        let mut sub = GooseSubscriber::new(APP_ID, DST, l2).unwrap();
        let (pdu, _) = sub.poll().unwrap().unwrap();
        let values: Vec<&DaValue> = pdu.dataset.entries.iter().map(|e| &e.value).collect();
        assert_eq!(
            values,
            vec![
                &DaValue::Int32(1),
                &DaValue::Float32(2.5),
                &DaValue::Bool(true),
                &DaValue::StringVal(String::from("go")),
            ]
        );
    }

    // ===== LB33：set_callback 被调用（每个有效 PDU 一次）=====
    #[test]
    fn test_lb33_callback_invoked() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen2 = Rc::clone(&seen);
        let mut sub = make_sub();
        sub.set_callback(move |pdu: &GoosePdu| {
            seen2.borrow_mut().push(pdu.st_num);
        });
        let f1 = publish_frame(0, &[("A", DaValue::Bool(true))], 0); // st=1
        let f2 = publish_frame(1, &[("A", DaValue::Bool(false))], 0); // st=2
        sub.transport_mut().push_rx_frame(&f1);
        sub.transport_mut().push_rx_frame(&f2);
        sub.poll().unwrap();
        sub.poll().unwrap();
        assert_eq!(*seen.borrow(), vec![1, 2]);
    }

    // ===== LB34：丢帧注入 → 下一帧 StJump =====
    #[test]
    fn test_lb34_frame_loss_injection() {
        let f1 = publish_frame(0, &[("A", DaValue::Int32(1))], 0); // st=1
        let _lost = publish_frame(1, &[("A", DaValue::Int32(2))], 0); // st=2（不投递）
        let f3 = publish_frame(2, &[("A", DaValue::Int32(3))], 0); // st=3
        let mut sub = make_sub();
        sub.transport_mut().push_rx_frame(&f1);
        sub.transport_mut().push_rx_frame(&f3);
        assert_eq!(sub.poll().unwrap().unwrap().1, RxStatus::New);
        let (pdu, status) = sub.poll().unwrap().unwrap();
        assert_eq!(status, RxStatus::StJump);
        assert_eq!(pdu.st_num, 3);
    }

    // ===== LB35：MockL2 回路全链路 < 4ms + 值一致保序（D11，cfg(test) Instant）=====
    #[test]
    fn test_lb35_loopback_under_4ms() {
        let cb = GooseControlBlock {
            go_cb_ref: String::from("IED1LD/LLN0$GO$gocb1"),
            app_id: APP_ID,
            dst_addr: DST,
            min_time: 2,
            max_time: 5000,
            st_num: 0,
            sq_num: 0,
            dataset_ref: String::from("IED1LD/LLN0$ds1"),
            needs_retransmit: false,
        };
        let mut l2 = MockL2::new();
        l2.set_loopback(true);
        let mut p = GoosePublisher::new(cb, l2).unwrap();
        p.update_value("Pos.stVal", DaValue::Bool(true));
        p.update_value("Hz.mag", DaValue::Float32(50.01));
        p.update_value("W.mag", DaValue::Float64(1234.5));
        p.update_value("A.phsA", DaValue::Int32(1200));
        let start = std::time::Instant::now();
        p.publish(88).unwrap();
        let l2 = core::mem::take(p.transport_mut());
        let mut sub = GooseSubscriber::new(APP_ID, DST, l2).unwrap();
        let (pdu, status) = sub.poll().unwrap().unwrap();
        let elapsed = start.elapsed();
        assert_eq!(status, RxStatus::New);
        assert_eq!(pdu.st_num, 4);
        assert_eq!(pdu.timestamp, 88);
        // 值一致 + 保序
        let values: Vec<&DaValue> = pdu.dataset.entries.iter().map(|e| &e.value).collect();
        assert_eq!(
            values,
            vec![
                &DaValue::Bool(true),
                &DaValue::Float32(50.01),
                &DaValue::Float64(1234.5),
                &DaValue::Int32(1200),
            ]
        );
        assert!(
            elapsed.as_millis() < 4,
            "goose loopback too slow: {:?}",
            elapsed
        );
    }

    // ===== LB36：事件后重传帧 sq_num 递增接收（st 不变均为 New）=====
    #[test]
    fn test_lb36_retransmit_sq_increasing() {
        let cb = GooseControlBlock {
            go_cb_ref: String::from("g"),
            app_id: APP_ID,
            dst_addr: DST,
            min_time: 10,
            max_time: 5000,
            st_num: 0,
            sq_num: 0,
            dataset_ref: String::from("d"),
            needs_retransmit: false,
        };
        let mut l2 = MockL2::new();
        l2.set_loopback(true);
        let mut p = GoosePublisher::new(cb, l2).unwrap();
        p.update_value("A", DaValue::Bool(true)); // st=1, sq=0
        p.publish(1000).unwrap();
        assert!(p.retransmit_if_needed(1010)); // sq=1
        assert!(p.retransmit_if_needed(1020)); // sq=2
        let l2 = core::mem::take(p.transport_mut());
        let mut sub = GooseSubscriber::new(APP_ID, DST, l2).unwrap();
        let mut received = Vec::new();
        for _ in 0..3 {
            let (pdu, status) = sub.poll().unwrap().unwrap();
            received.push((pdu.st_num, pdu.sq_num, status));
        }
        assert_eq!(
            received,
            vec![
                (1, 0, RxStatus::New),
                (1, 1, RxStatus::New),
                (1, 2, RxStatus::New),
            ]
        );
        assert_eq!(sub.last_st_num(), 1);
    }
}
