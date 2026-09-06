//! 50（WP-50.1 exporter 真通道）：遥测批量帧传输线码（单一事实源）。
//!
//! 目标侧 svc-observability 的 exporter 把遥测/指标/日志批量帧**固定二进制
//! 格式**（magic `OBSF` + schema 版本 + batch 序号 + 条目）经 svc-mqtt
//! EXECUTE 分片子码送达 MQTT broker；**禁自由 JSON 进 target**（schema
//! 版本化，向后兼容规则登记于本模块文档头）。
//!
//! # Schema 版本化与向后兼容规则
//!
//! - 帧头固定：`OBSF(4) ‖ version u8 ‖ batch_seq u64 LE ‖ count u16 LE`。
//! - version 字节位于固定偏移 4——解码器先查 magic 再查 version；
//!   **未知 version（> CURRENT 或非登记集合）→ 拒绝解析**（负例 DoD）。
//! - 向后兼容规则：新版本**不得**改动既有字段的偏移/长度/字节序；只允许
//!   (a) 在帧尾追加字段，(b) 扩展 count/item 编码（条目自身带长度前缀）。
//!   version 仅在（且仅在）既有字段语义破坏性变更时 +1；解码器应接受
//!   `version <= CURRENT` 且属于登记集合的帧（当前集合 = {1}）。
//! - 本模块只定义**线码与帧头**，条目内容编码在
//!   `business-services::observability`（TelemetryItem::encode/decode）。

/// 帧 magic（4B）。解码器以它识别 EnerOS 遥测帧。
pub const TEL_FRAME_MAGIC: [u8; 4] = *b"OBSF";
/// 当前帧 schema 版本（第 5 字节；向后兼容规则见模块头）。
pub const TEL_FRAME_SCHEMA_VERSION: u8 = 1;
/// 帧头固定长：magic(4) + version(1) + batch_seq(8) + count(2)。
pub const TEL_FRAME_HEADER_LEN: usize = 15;
/// 单帧总长上限（对齐 durable outbox slot 1024——超限入不了 outbox）。
pub const TEL_FRAME_MAX: usize = 1024;

/// EXECUTE 载荷子码：遥测帧分片首帧（同 49 AI_REQ_* 分片语义，独立码面）。
/// 载荷 = `[TEL_PUB_BEGIN][total u16 LE][data ≤61B]`。
pub const TEL_PUB_BEGIN: u8 = 0x3A;
/// EXECUTE 载荷子码：遥测帧分片中继帧。载荷 = `[TEL_PUB_CONT][data ≤63B]`。
pub const TEL_PUB_CONT: u8 = 0x3B;
/// EXECUTE 载荷子码：遥测帧收尾帧（组装完成 → svc-mqtt 校验帧头 → 入
/// 插件 app outbox）。载荷 = `[TEL_PUB_END]`。
pub const TEL_PUB_END: u8 = 0x3C;

/// 分片数据容量（本层分片常量 64B − 子码 1B；BEGIN 再减 total u16 2B——
/// 68.3 起 drv 信封 PAYLOAD_CAP=461，本层刻意不随动）。
pub const TEL_REQ_DATA_BEGIN_MAX: usize = 64 - 3;
/// CONT/END 数据容量。
pub const TEL_REQ_DATA_MAX: usize = 64 - 1;

/// 帧头校验错误（svc-mqtt 边界 fail-closed：坏帧 → 拒绝解析 + 计数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// 前 4 字节非 `OBSF`。
    BadMagic,
    /// schema 版本未知/不兼容（DoD 负例：版本不匹配 → 拒绝解析）。
    UnsupportedVersion,
    /// 帧被截断（不足帧头长）。
    Truncated,
    /// 帧超长（> TEL_FRAME_MAX）。
    Oversize,
}

/// 分片失败（蓝图 59 起单源在 relay_face，re-export 保持导入路径稳定）。
pub use crate::relay_face::FragmentError;

/// 校验帧头（magic + schema 版本 + 长度）。返回 (batch_seq, count)。
///
/// 版本规则：`version == TEL_FRAME_SCHEMA_VERSION` 才接受；其他一律
/// `UnsupportedVersion`（向后兼容集合当前仅含 {1}，规则见模块头）。
pub fn check_frame_header(frame: &[u8]) -> Result<(u64, u16), FrameError> {
    if frame.len() > TEL_FRAME_MAX {
        return Err(FrameError::Oversize);
    }
    if frame.len() < TEL_FRAME_HEADER_LEN {
        return Err(FrameError::Truncated);
    }
    if frame[..4] != TEL_FRAME_MAGIC {
        return Err(FrameError::BadMagic);
    }
    if frame[4] != TEL_FRAME_SCHEMA_VERSION {
        return Err(FrameError::UnsupportedVersion);
    }
    let batch_seq = u64::from_le_bytes(frame[5..13].try_into().expect("5..13"));
    let count = u16::from_le_bytes(frame[13..15].try_into().expect("13..15"));
    Ok((batch_seq, count))
}

/// 组装一帧帧头字节（magic + version + batch_seq + count）。
pub fn encode_frame_header(batch_seq: u64, count: u16) -> [u8; TEL_FRAME_HEADER_LEN] {
    let mut out = [0u8; TEL_FRAME_HEADER_LEN];
    out[..4].copy_from_slice(&TEL_FRAME_MAGIC);
    out[4] = TEL_FRAME_SCHEMA_VERSION;
    out[5..13].copy_from_slice(&batch_seq.to_le_bytes());
    out[13..15].copy_from_slice(&count.to_le_bytes());
    out
}

/// 遥测中继面声明（蓝图 59：分片算法单源在 relay_face）。
pub const FACE: crate::relay_face::RelayFaceSpec = crate::relay_face::RelayFaceSpec {
    begin: TEL_PUB_BEGIN,
    cont: TEL_PUB_CONT,
    end: TEL_PUB_END,
    max_total: TEL_FRAME_MAX,
};

/// 批量帧 → 分片帧序列（BEGIN/CONT/END 子码；不含 EXECUTE opcode——
/// 发送方 drv_call 侧再拼）。帧超限/空 → 失败。
/// 蓝图 59：委托 [`FACE::fragment`]（原手写算法已收敛至 relay_face 单源）。
pub fn fragment_telemetry_frame(
    frame: &[u8],
) -> Result<(u16, alloc::vec::Vec<alloc::vec::Vec<u8>>), FragmentError> {
    FACE.fragment(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 50：子码与既有 EXECUTE 码面互斥（0x11=OP_MASTER_WRITE、0x2A/2B/2C=AI）。
    #[test]
    fn wire_codes_disjoint() {
        const OP_MASTER_WRITE: u8 = 0x11;
        const AI: [u8; 3] = [0x2A, 0x2B, 0x2C];
        for c in [TEL_PUB_BEGIN, TEL_PUB_CONT, TEL_PUB_END] {
            assert_ne!(c, OP_MASTER_WRITE);
            assert!(!AI.contains(&c));
        }
        assert_ne!(TEL_PUB_BEGIN, TEL_PUB_CONT);
        assert_ne!(TEL_PUB_CONT, TEL_PUB_END);
    }

    /// 50：正例——帧头往返 + 分片重组还原。
    #[test]
    fn frame_header_and_fragment_roundtrip() {
        let mut frame = encode_frame_header(7, 2).to_vec();
        frame.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        let (batch_seq, count) = check_frame_header(&frame).expect("valid");
        assert_eq!(batch_seq, 7);
        assert_eq!(count, 2);
        let (_total, frames) = fragment_telemetry_frame(&frame).expect("fragment");
        // 首帧 BEGIN + total + head。
        assert_eq!(frames[0][0], TEL_PUB_BEGIN);
        let mut out = Vec::new();
        out.extend_from_slice(&frames[0][3..]);
        for f in &frames[1..frames.len() - 1] {
            assert_eq!(f[0], TEL_PUB_CONT);
            out.extend_from_slice(&f[1..]);
        }
        assert_eq!(frames.last().expect("end"), &vec![TEL_PUB_END]);
        assert_eq!(out, frame);
    }

    /// 50：负例（DoD）——schema 版本不匹配 → 拒绝解析。
    #[test]
    fn negative_unsupported_version_rejected() {
        let mut frame = encode_frame_header(1, 0).to_vec();
        frame[4] = TEL_FRAME_SCHEMA_VERSION + 1; // 未来版本
        assert_eq!(
            check_frame_header(&frame),
            Err(FrameError::UnsupportedVersion)
        );
        frame[4] = 0; // 旧/未知版本
        assert_eq!(
            check_frame_header(&frame),
            Err(FrameError::UnsupportedVersion)
        );
    }

    /// 50：负例——坏 magic / 截断 / 超限均拒绝。
    #[test]
    fn negative_bad_magic_truncated_oversize() {
        let frame = encode_frame_header(1, 0).to_vec();
        assert_eq!(check_frame_header(&frame[..4]), Err(FrameError::Truncated));
        let mut bad = frame.clone();
        bad[0] = b'X';
        assert_eq!(check_frame_header(&bad), Err(FrameError::BadMagic));
        let mut big = vec![0u8; TEL_FRAME_MAX + 1];
        big[..4].copy_from_slice(&TEL_FRAME_MAGIC);
        assert_eq!(check_frame_header(&big), Err(FrameError::Oversize));
        assert!(fragment_telemetry_frame(&[]).is_err());
        assert!(fragment_telemetry_frame(&[0u8; TEL_FRAME_MAX + 1]).is_err());
    }

    /// 50：容量自洽（单帧不越本层分片常量 64B）。
    #[test]
    fn capacity_within_payload_cap() {
        const PAYLOAD_CAP: usize = 64;
        const _: () = assert!(1 + 2 + TEL_REQ_DATA_BEGIN_MAX <= PAYLOAD_CAP);
        const _: () = assert!(TEL_REQ_DATA_MAX < PAYLOAD_CAP);
        assert_eq!(TEL_FRAME_HEADER_LEN, 15);
    }
}
