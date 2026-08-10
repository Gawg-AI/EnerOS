//! seL4 IPC MR（message register）帧编解码（21-A K2.2/K2.3 共享契约）。
//!
//! root-task ↔ EL0 服务经 seL4 Endpoint 通信时，ipc-wire 帧字节装入
//! MessageInfo 载荷（MR 槽 + IPC buffer）。本模块是两侧共用的唯一
//! 编解码实现（单一事实源）：
//!
//! - MR0 = 帧总字节数（u64 小端）；
//! - MR1.. = 帧字节按 8 字节小端打包（末字不足补 0）；
//! - MessageInfo：label = 帧类型（[`crate::el0::El0FrameType`] 数值），
//!   length = MR words 数。
//!
//! # no_std
//!
//! 纯 `core` + `alloc`（Vec 返回）。

use alloc::vec::Vec;

/// 帧长度槽（MR 索引 0）。
pub const MR_FRAME_LEN_SLOT: u64 = 0;

/// 单帧最大字节数。
///
/// 受 seL4 aarch64 IPC buffer 消息上限约束：`seL4_MsgMaxLength = 120`
/// words（≈960 字节载荷），其中 MR0 已被帧长度占用，故帧数据上限 =
/// `(120 - 1) × 8 = 952` 字节。ipc-wire 协议层默认 payload 1024 超出该
/// 上限——EL0 帧必须 ≤ 952 字节（root-task ↔ 服务契约，K2.2/K2.3）。
pub const MAX_FRAME_BYTES: usize = 952;

/// 30 RT-020：MR 单帧可承载的最大 EL0 payload（契约常量）——
/// `MAX_FRAME_BYTES - EL0 帧开销(10B 头 + 4B crc)`。el0 层
/// `DEFAULT_MAX_PAYLOAD_LEN=1024` 超出 MR 单帧容量（满载荷必然
/// FrameTooLong）——载荷 > 本常量必须分帧。
pub const MAX_EL0_PAYLOAD_IN_MR: usize = MAX_FRAME_BYTES - crate::el0::EL0_FRAME_HEADER_LEN - crate::el0::EL0_FRAME_CRC_LEN;

/// 30 RT-020：编译期断言——MR 契约常量与 el0 常量互相引用（防止两侧
/// 各自为政漂移）。
const _: () = assert!(
    MAX_EL0_PAYLOAD_IN_MR > 0 && MAX_EL0_PAYLOAD_IN_MR < crate::DEFAULT_MAX_PAYLOAD_LEN,
    "MR 单帧 payload 容量必须小于 el0 默认上限（超限需分帧）"
);

/// 帧 ↔ MR 编解码错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMrError {
    /// 帧超长（> [`MAX_FRAME_BYTES`]）。
    FrameTooLong,
    /// MR 列表为空（无长度槽）。
    NoLengthSlot,
    /// 长度与 MR 载荷不一致。
    BadLength,
    /// 内存分配失败（22 §2.W1.1：try_reserve 失败，不 panic）。
    OutOfMemory,
}

/// 帧字节 → MR words（[`MR_FRAME_LEN_SLOT`] 布局）。
pub fn frame_into_mrs(frame: &[u8]) -> Result<Vec<u64>, FrameMrError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(FrameMrError::FrameTooLong);
    }
    let mut mrs = Vec::with_capacity(1 + frame.len().div_ceil(8));
    mrs.push(frame.len() as u64);
    let mut i = 0;
    while i < frame.len() {
        let mut word = 0u64;
        for j in 0..8 {
            if i + j < frame.len() {
                word |= u64::from(frame[i + j]) << (8 * j);
            }
        }
        mrs.push(word);
        i += 8;
    }
    Ok(mrs)
}

/// MR words → 帧字节（[`MR_FRAME_LEN_SLOT`] 布局）。
pub fn frame_from_mrs(mrs: &[u64]) -> Result<Vec<u8>, FrameMrError> {
    let len = *mrs.first().ok_or(FrameMrError::NoLengthSlot)? as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameMrError::FrameTooLong);
    }
    let words = 1 + len.div_ceil(8);
    if mrs.len() < words {
        return Err(FrameMrError::BadLength);
    }
    // 22 §2.W1.1：IPC 热路径预分配 try_reserve（len 已有 MAX_FRAME_BYTES
    // 上限；CapacityOverflow/OOM 均映射 OutOfMemory，不 panic）。
    let mut out = Vec::new();
    out.try_reserve(len)
        .map_err(|_| FrameMrError::OutOfMemory)?;
    out.resize(len, 0u8);
    for i in 0..len {
        let word = mrs[1 + i / 8];
        out[i] = (word >> (8 * (i % 8))) as u8;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_mr_roundtrip() {
        let frame = crate::el0::encode_frame(crate::el0::El0FrameType::Report, b"hello:eneros");
        let mrs = frame_into_mrs(&frame).expect("encode");
        assert_eq!(mrs[MR_FRAME_LEN_SLOT as usize], frame.len() as u64);
        assert_eq!(frame_from_mrs(&mrs).expect("decode"), frame);
    }

    #[test]
    fn frame_mr_partial_word_padding() {
        let mrs = frame_into_mrs(&[0xAB]).expect("encode");
        assert_eq!(mrs, vec![1, 0xAB]);
        assert_eq!(frame_from_mrs(&mrs).expect("decode"), vec![0xAB]);
    }

    #[test]
    fn frame_mr_errors() {
        assert_eq!(
            frame_into_mrs(&[0u8; MAX_FRAME_BYTES + 1]),
            Err(FrameMrError::FrameTooLong)
        );
        assert_eq!(frame_from_mrs(&[]), Err(FrameMrError::NoLengthSlot));
        assert_eq!(
            frame_from_mrs(&[(MAX_FRAME_BYTES + 1) as u64]),
            Err(FrameMrError::FrameTooLong)
        );
        assert_eq!(frame_from_mrs(&[16, 0]), Err(FrameMrError::BadLength));
    }

    /// 30 RT-020 判据：payload = MAX_EL0_PAYLOAD_IN_MR 恰好成帧（MR 单帧
    /// 容量）；+1 拒绝（FrameTooLong）——旧常量各自为政（el0 1024 >
    /// MR 952），满载荷必然 FrameTooLong。
    #[test]
    fn el0_payload_at_mr_capacity_roundtrips() {
        let payload = [0x5Au8; MAX_EL0_PAYLOAD_IN_MR];
        // 构造完整 el0 帧（10B 头 + payload + 4B crc）。
        let mut frame = alloc::vec![0u8; 10 + payload.len() + 4];
        frame[0..4].copy_from_slice(b"EL0F");
        frame[6..10].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        frame[10..10 + payload.len()].copy_from_slice(&payload);
        let mrs = frame_into_mrs(&frame).expect("恰好 MR 容量必须成帧");
        let back = frame_from_mrs(&mrs).expect("解码");
        assert_eq!(back.len(), frame.len());
        assert_eq!(&back[10..10 + payload.len()], &payload[..]);
        // +1 超限拒绝。
        let mut too_long = frame.clone();
        too_long.push(0);
        assert_eq!(
            frame_into_mrs(&too_long),
            Err(FrameMrError::FrameTooLong)
        );
    }
}
