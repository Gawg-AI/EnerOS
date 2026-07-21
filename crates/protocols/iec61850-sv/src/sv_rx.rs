//! SV 采样值接收器（EtherType 0x88BA 过滤 / APPID/MAC 过滤 / smpCnt 连续性检测）。
//!
//! receive 流程：帧长 < 16 → Ok(false)（不足以过滤，静默忽略）→
//! EtherType != 0x88BA → Ok(false) → dst MAC 不匹配 → Ok(false) →
//! APPID 不匹配 → Ok(false) → BER 解码 SV PDU（smpCnt 0x80 / timestamp 0x81 /
//! channels 0x82，未知 tag 跳过）→ SampleStatus 判定 → 写环形缓冲 → 回调 → Ok(true)。
//!
//! SampleStatus 判定（蓝图 §4.4，D12）：
//! - 首个采样（尚未建立基线）→ `New`
//! - smp_cnt == last + 1（u16 回绕语义）→ `New`
//! - smp_cnt == last → `Duplicate`
//! - 其余（跳变 / 乱序旧帧）→ `SmpJump`（采样丢失）
//!
//! SV PDU 布局（BER TLV，自定界，与 GOOSE crate 同解码规则）：
//! `[0x80 0x02 smpCnt:u16 BE] [0x81 0x08 timestamp:u64 BE] [0x82 len N×f32 BE]`。

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::{L2Transport, RingBuffer, SvError};

/// SV 以太网类型（IEC 61850-9-2）。
const ETHERTYPE_SV: u16 = 0x88BA;
/// smpCnt 字段 tag（u16 采样计数）。
const TAG_SMP_CNT: u8 = 0x80;
/// timestamp 字段 tag（u64 采样时间戳）。
const TAG_TIMESTAMP: u8 = 0x81;
/// channels 字段 tag（N × f32 通道采样值）。
const TAG_CHANNELS: u8 = 0x82;
/// 以太网头 + APPID 最小帧长（dst 6 + src 6 + EtherType 2 + APPID 2）。
const MIN_FRAME_LEN: usize = 16;

/// 采样状态（D12：随样本返回，区分新采样/重复/丢样）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SampleStatus {
    /// 新采样（smpCnt 连续递增 1）。
    New,
    /// 重复采样（smpCnt 与上次相同）。
    Duplicate,
    /// 采样跳变（smpCnt 跳变 > 1，存在丢样）。
    SmpJump,
}

/// 解码后的 SV 采样。
#[derive(Debug, Clone, PartialEq)]
pub struct SvSample {
    /// 采样计数器（0~65535 循环）。
    pub smp_cnt: u16,
    /// 采样时间戳。
    pub timestamp: u64,
    /// 通道采样值序列。
    pub channels: Vec<f32>,
    /// 采样状态（D12）。
    pub status: SampleStatus,
}

/// 采样回调类型（D9：去 Send+Sync bound）。
type SampleCallback = Box<dyn Fn(&SvSample)>;

/// SV 订阅者（泛型 L2 传输注入，D4/D5）。
pub struct SvSubscriber<T: L2Transport> {
    app_id: u16,
    filter_mac: [u8; 6],
    last_smp_cnt: u16,
    /// 是否已建立 smpCnt 基线（首采样不做跳变检测）。
    has_last: bool,
    callback: Option<SampleCallback>,
    transport: T,
    buffer: RingBuffer<SvSample>,
}

impl<T: L2Transport> SvSubscriber<T> {
    /// 创建订阅者；`app_id == 0` 视为无效配置。
    pub fn new(app_id: u16, mac: [u8; 6], buf_size: usize, transport: T) -> Result<Self, SvError> {
        if app_id == 0 {
            return Err(SvError::InvalidConfig);
        }
        Ok(Self {
            app_id,
            filter_mac: mac,
            last_smp_cnt: 0,
            has_last: false,
            callback: None,
            transport,
            buffer: RingBuffer::new(buf_size),
        })
    }

    /// 接收一帧以太网报文；`Ok(true)` 表示有效采样已写入缓冲。
    ///
    /// 过滤不匹配（非 0x88BA / MAC / APPID / 帧过短）返回 `Ok(false)` 静默丢弃；
    /// 通过过滤后 PDU 截断或畸形返回 `Err(BerDecodeError)`。
    pub fn receive(&mut self, frame: &[u8]) -> Result<bool, SvError> {
        if frame.len() < MIN_FRAME_LEN {
            return Ok(false);
        }
        if u16::from_be_bytes([frame[12], frame[13]]) != ETHERTYPE_SV {
            return Ok(false);
        }
        if frame[0..6] != self.filter_mac {
            return Ok(false);
        }
        if u16::from_be_bytes([frame[14], frame[15]]) != self.app_id {
            return Ok(false);
        }
        let (smp_cnt, timestamp, channels) = decode_pdu(&frame[16..])?;
        let status = self.classify(smp_cnt);
        self.last_smp_cnt = smp_cnt;
        self.has_last = true;
        let sample = SvSample {
            smp_cnt,
            timestamp,
            channels,
            status,
        };
        self.buffer.push(sample.clone());
        if let Some(cb) = &self.callback {
            cb(&sample);
        }
        Ok(true)
    }

    /// 取出全部已缓冲采样（旧→新顺序）并清空缓冲。
    pub fn take_samples(&mut self) -> Vec<SvSample> {
        self.buffer.drain()
    }

    /// 注册采样回调；每收到一个有效采样调用一次。
    pub fn set_callback<F: Fn(&SvSample) + 'static>(&mut self, f: F) {
        self.callback = Some(Box::new(f));
    }

    /// 最近一次接收的 smpCnt（未收到采样时为 0）。
    pub fn last_smp_cnt(&self) -> u16 {
        self.last_smp_cnt
    }

    /// 获取传输层可变引用（供集成层接线/错误注入）。
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// smpCnt 状态判定（D12，见模块文档；u16 回绕语义）。
    fn classify(&self, smp_cnt: u16) -> SampleStatus {
        if !self.has_last {
            return SampleStatus::New;
        }
        if smp_cnt == self.last_smp_cnt {
            SampleStatus::Duplicate
        } else if smp_cnt == self.last_smp_cnt.wrapping_add(1) {
            SampleStatus::New
        } else {
            SampleStatus::SmpJump
        }
    }
}

/// 读取一个 TLV 的 tag 与内容长度，`pos` 推进到内容起始（与 GOOSE crate 同规则）。
///
/// 支持 BER 短型长度（< 0x80）与长型 0x81（单字节）/ 0x82（双字节）；
/// 声明长度超出剩余缓冲区、或其他长型标记 → `BerDecodeError`。
pub(crate) fn read_tag_length(data: &[u8], pos: &mut usize) -> Result<(u8, usize), SvError> {
    let tag = *data.get(*pos).ok_or(SvError::BerDecodeError)?;
    *pos += 1;
    let first = usize::from(*data.get(*pos).ok_or(SvError::BerDecodeError)?);
    *pos += 1;
    let len = if first < 0x80 {
        first
    } else if first == 0x81 {
        let b = usize::from(*data.get(*pos).ok_or(SvError::BerDecodeError)?);
        *pos += 1;
        b
    } else if first == 0x82 {
        let hi = usize::from(*data.get(*pos).ok_or(SvError::BerDecodeError)?);
        let lo = usize::from(*data.get(*pos + 1).ok_or(SvError::BerDecodeError)?);
        *pos += 2;
        (hi << 8) | lo
    } else {
        return Err(SvError::BerDecodeError);
    };
    if *pos + len > data.len() {
        return Err(SvError::BerDecodeError);
    }
    Ok((tag, len))
}

/// 解码 SV PDU（字段序列：smpCnt 0x80 / timestamp 0x81 / channels 0x82；未知 tag 跳过）。
///
/// `smpCnt` 与 `channels` 缺失、字段长度不符（smpCnt != 2 / timestamp != 8 /
/// channels 非 4 的倍数）、TLV 截断 → `BerDecodeError`。
fn decode_pdu(data: &[u8]) -> Result<(u16, u64, Vec<f32>), SvError> {
    let mut pos = 0usize;
    let mut smp_cnt = None;
    let mut timestamp = 0u64;
    let mut channels = None;
    while pos < data.len() {
        let (tag, len) = read_tag_length(data, &mut pos)?;
        match tag {
            TAG_SMP_CNT => {
                if len != 2 {
                    return Err(SvError::BerDecodeError);
                }
                smp_cnt = Some(u16::from_be_bytes([data[pos], data[pos + 1]]));
                pos += len;
            }
            TAG_TIMESTAMP => {
                if len != 8 {
                    return Err(SvError::BerDecodeError);
                }
                let mut b = [0u8; 8];
                b.copy_from_slice(&data[pos..pos + 8]);
                timestamp = u64::from_be_bytes(b);
                pos += len;
            }
            TAG_CHANNELS => {
                if len % 4 != 0 {
                    return Err(SvError::BerDecodeError);
                }
                let mut ch = Vec::with_capacity(len / 4);
                for i in 0..len / 4 {
                    let off = pos + i * 4;
                    let mut b = [0u8; 4];
                    b.copy_from_slice(&data[off..off + 4]);
                    ch.push(f32::from_be_bytes(b));
                }
                channels = Some(ch);
                pos += len;
            }
            _ => {
                pos += len; // 未知字段跳过
            }
        }
    }
    let smp_cnt = smp_cnt.ok_or(SvError::BerDecodeError)?;
    let channels = channels.ok_or(SvError::BerDecodeError)?;
    Ok((smp_cnt, timestamp, channels))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::rc::Rc;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    use super::*;
    use crate::MockL2;

    const DST: [u8; 6] = [0x01, 0x0C, 0xCD, 0x04, 0x00, 0x01];
    const SRC: [u8; 6] = [0x01, 0x0C, 0xCD, 0x04, 0x00, 0x02];
    const APP_ID: u16 = 0x4001;

    fn make_sub() -> SvSubscriber<MockL2> {
        SvSubscriber::new(APP_ID, DST, 8, MockL2::new()).unwrap()
    }

    /// 构造标准 SV 帧：以太网头 + APPID + smpCnt/timestamp/channels TLV。
    fn sv_frame(
        dst_mac: [u8; 6],
        src_mac: [u8; 6],
        app_id: u16,
        smp_cnt: u16,
        timestamp: u64,
        channels: &[f32],
    ) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&dst_mac);
        f.extend_from_slice(&src_mac);
        f.extend_from_slice(&ETHERTYPE_SV.to_be_bytes());
        f.extend_from_slice(&app_id.to_be_bytes());
        f.extend_from_slice(&[TAG_SMP_CNT, 0x02]);
        f.extend_from_slice(&smp_cnt.to_be_bytes());
        f.extend_from_slice(&[TAG_TIMESTAMP, 0x08]);
        f.extend_from_slice(&timestamp.to_be_bytes());
        f.push(TAG_CHANNELS);
        let clen = channels.len() * 4;
        if clen < 0x80 {
            f.push(clen as u8);
        } else {
            f.push(0x82);
            f.extend_from_slice(&(clen as u16).to_be_bytes());
        }
        for c in channels {
            f.extend_from_slice(&c.to_be_bytes());
        }
        f
    }

    // ===== RX7：有效帧解码 → Ok(true)，样本字段正确，status == New =====
    #[test]
    fn test_rx7_valid_frame_decode() {
        let mut sub = make_sub();
        let frame = sv_frame(DST, SRC, APP_ID, 100, 12_345, &[1.5, -2.5]);
        assert_eq!(sub.receive(&frame), Ok(true));
        let samples = sub.take_samples();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].smp_cnt, 100);
        assert_eq!(samples[0].timestamp, 12_345);
        assert_eq!(samples[0].channels, vec![1.5, -2.5]);
        assert_eq!(samples[0].status, SampleStatus::New);
        assert_eq!(sub.last_smp_cnt(), 100);
    }

    // ===== RX8：APPID 不匹配 → Ok(false)，缓冲为空 =====
    #[test]
    fn test_rx8_appid_mismatch_dropped() {
        let mut sub = make_sub();
        let frame = sv_frame(DST, SRC, 0x4002, 1, 0, &[1.0]);
        assert_eq!(sub.receive(&frame), Ok(false));
        assert!(sub.take_samples().is_empty());
        assert_eq!(sub.last_smp_cnt(), 0);
    }

    // ===== RX9：dst MAC 不匹配 → Ok(false) =====
    #[test]
    fn test_rx9_dst_mac_mismatch_dropped() {
        let mut sub = make_sub();
        let frame = sv_frame(
            [0x01, 0x0C, 0xCD, 0x04, 0x00, 0x99],
            SRC,
            APP_ID,
            1,
            0,
            &[1.0],
        );
        assert_eq!(sub.receive(&frame), Ok(false));
        assert!(sub.take_samples().is_empty());
    }

    // ===== RX10：EtherType != 0x88BA → Ok(false) =====
    #[test]
    fn test_rx10_non_sv_ethertype_dropped() {
        let mut sub = make_sub();
        let mut frame = sv_frame(DST, SRC, APP_ID, 1, 0, &[1.0]);
        frame[12] = 0x08; // EtherType → 0x0800 (IPv4)
        frame[13] = 0x00;
        assert_eq!(sub.receive(&frame), Ok(false));
        assert!(sub.take_samples().is_empty());
    }

    // ===== RX11：smpCnt 跳变 → SmpJump（D12，采样丢失）=====
    #[test]
    fn test_rx11_smp_jump_detected() {
        let mut sub = make_sub();
        let f100 = sv_frame(DST, SRC, APP_ID, 100, 0, &[1.0]);
        let f103 = sv_frame(DST, SRC, APP_ID, 103, 0, &[1.0]); // 101/102 丢失
        assert_eq!(sub.receive(&f100), Ok(true));
        assert_eq!(sub.receive(&f103), Ok(true));
        let samples = sub.take_samples();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].status, SampleStatus::New);
        assert_eq!(samples[1].status, SampleStatus::SmpJump);
        assert_eq!(samples[1].smp_cnt, 103);
        assert_eq!(sub.last_smp_cnt(), 103);
    }

    // ===== RX12：重复 smpCnt → Duplicate =====
    #[test]
    fn test_rx12_duplicate_sample() {
        let mut sub = make_sub();
        let f = sv_frame(DST, SRC, APP_ID, 42, 0, &[1.0]);
        assert_eq!(sub.receive(&f), Ok(true));
        assert_eq!(sub.receive(&f), Ok(true));
        let samples = sub.take_samples();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].status, SampleStatus::New);
        assert_eq!(samples[1].status, SampleStatus::Duplicate);
        assert_eq!(sub.last_smp_cnt(), 42);
    }

    // ===== RX13：截断帧 → BerDecodeError；过短帧 → Ok(false) =====
    #[test]
    fn test_rx13_truncated_frame() {
        let mut sub = make_sub();
        // 通过过滤后 PDU 截断（channels 内容少 1 字节）→ BerDecodeError
        let frame = sv_frame(DST, SRC, APP_ID, 1, 0, &[1.0]);
        let truncated = &frame[..frame.len() - 1];
        assert_eq!(sub.receive(truncated), Err(SvError::BerDecodeError));
        // 帧长 < 16（不足以过滤）→ 静默忽略
        assert_eq!(sub.receive(&frame[..10]), Ok(false));
        assert!(sub.take_samples().is_empty());
    }

    // ===== RX14：smpCnt tag 0x80 边界值解码（0xABCD）=====
    #[test]
    fn test_rx14_smp_cnt_tag_decode() {
        let mut sub = make_sub();
        let frame = sv_frame(DST, SRC, APP_ID, 0xABCD, 0, &[1.0]);
        assert_eq!(sub.receive(&frame), Ok(true));
        let samples = sub.take_samples();
        assert_eq!(samples[0].smp_cnt, 0xABCD);
        assert_eq!(sub.last_smp_cnt(), 0xABCD);
    }

    // ===== RX15：timestamp tag 0x81 八字节解码（u64 边界）=====
    #[test]
    fn test_rx15_timestamp_tag_decode() {
        let mut sub = make_sub();
        let ts = 0x0102_0304_0506_0708u64;
        let frame = sv_frame(DST, SRC, APP_ID, 1, ts, &[1.0]);
        assert_eq!(sub.receive(&frame), Ok(true));
        let samples = sub.take_samples();
        assert_eq!(samples[0].timestamp, ts);
    }

    // ===== RX16：channels tag 0x82 长度解码；长度非 4 倍数 → BerDecodeError =====
    #[test]
    fn test_rx16_channels_tag_decode() {
        let mut sub = make_sub();
        let frame = sv_frame(DST, SRC, APP_ID, 1, 0, &[1.5, -2.5]);
        assert_eq!(sub.receive(&frame), Ok(true));
        let samples = sub.take_samples();
        assert_eq!(samples[0].channels, vec![1.5, -2.5]);

        // channels 长度 = 6（非 4 的倍数）→ BerDecodeError
        let mut raw = Vec::new();
        raw.extend_from_slice(&DST);
        raw.extend_from_slice(&SRC);
        raw.extend_from_slice(&ETHERTYPE_SV.to_be_bytes());
        raw.extend_from_slice(&APP_ID.to_be_bytes());
        raw.extend_from_slice(&[TAG_SMP_CNT, 0x02]);
        raw.extend_from_slice(&7u16.to_be_bytes());
        raw.extend_from_slice(&[TAG_CHANNELS, 0x06]);
        raw.extend_from_slice(&[0u8; 6]);
        assert_eq!(sub.receive(&raw), Err(SvError::BerDecodeError));
    }

    // ===== RX17：set_callback 被调用（每个有效采样一次）=====
    #[test]
    fn test_rx17_callback_invoked() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen2 = Rc::clone(&seen);
        let mut sub = make_sub();
        sub.set_callback(move |s: &SvSample| {
            seen2.borrow_mut().push(s.smp_cnt);
        });
        let f1 = sv_frame(DST, SRC, APP_ID, 1, 0, &[1.0]);
        let f2 = sv_frame(DST, SRC, APP_ID, 2, 0, &[2.0]);
        sub.receive(&f1).unwrap();
        sub.receive(&f2).unwrap();
        assert_eq!(*seen.borrow(), vec![1, 2]);
    }

    // ===== RX18：take_samples 返回全部并清空 =====
    #[test]
    fn test_rx18_take_samples_drains_buffer() {
        let mut sub = make_sub();
        for cnt in 1..=3u16 {
            let f = sv_frame(DST, SRC, APP_ID, cnt, 0, &[1.0]);
            assert_eq!(sub.receive(&f), Ok(true));
        }
        let samples = sub.take_samples();
        assert_eq!(samples.len(), 3);
        let cnts: Vec<u16> = samples.iter().map(|s| s.smp_cnt).collect();
        assert_eq!(cnts, vec![1, 2, 3]);
        // 第二次取出为空
        assert!(sub.take_samples().is_empty());
    }
}
