//! 物理格式 wire 常量与编解码（[07] §4.3，布局规格唯一出处）。
//!
//! # 设备布局（4KiB 块对齐）
//!
//! ```text
//! 块 0                 : superblock（32B 有效 + 填充）
//! 块 1                 : consumer offset 槽 A（双槽之一）
//! 块 2                 : consumer offset 槽 B
//! 块 3..               : segment 槽位区（每槽 segment_blocks 块）
//! ```
//!
//! # Segment（每槽 = segment_bytes，默认 4MiB）
//!
//! ```text
//! [0    ..64)   header : magic "EVSG" | ver u16 | flags u16 |
//!                        partition u32 | segment_id u32 |
//!                        first_offset u64 | created_unix_s u64 |
//!                        config_digest u32 | rsvd u32 | crc32([0..44))
//! [64   ..D)    records: 逐条 [len u32 | flags u16 | schema u16 |
//!                        offset u64 | event_id 16B | header+payload crc32 |
//!                        payload]（36B 头 + payload，紧凑可跨块）
//! [D    ..seg)  trailer 块（段尾一块，seal 时写）:
//!                        magic "EVTR" | ver u16 | flags u16 |
//!                        record_count u32 | rsvd u32 | last_offset u64 |
//!                        sha256(records) 32B | rsvd | crc32([0..60))
//! ```
//!
//! `D = segment_bytes - 4096`（trailer 块独占段尾块；record 区上限 D-64）。
//!
//! # Consumer offset 双槽（块 1/2，世代交替）
//!
//! ```text
//! [0   ..4)   magic "COFS"
//! [4   ..6)   ver u16
//! [6   ..8)   flags u16
//! [8   ..16)  generation u64（单调递增；取两槽中更高世代的有效槽）
//! [16  ..20)  count u32（≤ OFFSET_MAX_ENTRIES = 254）
//! [20  ..20+n*16) entries: {group u32 | partition u32 | offset u64}
//! [4084..4088) crc32([0 .. 20+n*16))
//! ```
//!
//! 空/擦除判定：块前 8 字节全 `0x00` 或全 `0xFF` 视为未用（erase 语义差异）。

use alloc::vec::Vec;

use crate::StoreConfig;

/// 块大小（[07] §4.3：4KiB block 对齐；`D::block_size()` 必须等于此值）。
pub const BLOCK_SIZE: usize = 4096;
/// 格式版本。
pub const FORMAT_VERSION: u16 = 1;
/// superblock magic。
pub const SUPER_MAGIC: [u8; 4] = *b"EVST";
/// segment header magic。
pub const SEGMENT_MAGIC: [u8; 4] = *b"EVSG";
/// segment trailer magic。
pub const TRAILER_MAGIC: [u8; 4] = *b"EVTR";
/// consumer offset 槽 magic。
pub const OFFSET_SLOT_MAGIC: [u8; 4] = *b"COFS";

/// 固定区块数：superblock + 2 个消费位点双槽。
pub const FIXED_BLOCKS: u64 = 3;
/// 固定区字节数。
pub const FIXED_BYTES: u64 = FIXED_BLOCKS * BLOCK_SIZE as u64;

/// segment header 长度（record 区起点）。
pub const SEGMENT_HEADER_LEN: u64 = 64;
/// record 头长度（len/flags/schema/offset/event_id/crc32）。
///
/// CRC 覆盖 **头前 32 字节 + payload**（头自身字段 event_id/offset/flags
/// 全部受保护——[07] §4.7「无静默损坏」：任何字段位翻转都必须可检出）。
pub const RECORD_HEADER_LEN: usize = 36;
/// record 最大字节数（[07] §4.3：1152B；ObservationEvent 128B、
/// DomainEvent ≤1024B 均可容纳）。
pub const RECORD_MAX: usize = 1152;
/// payload 最大字节数 = 1152 - 36。
pub const PAYLOAD_MAX: usize = RECORD_MAX - RECORD_HEADER_LEN;
/// segment trailer 长度。
pub const TRAILER_LEN: usize = 64;
/// segment header 编码长度。
pub const SEGMENT_HEADER_BYTES: usize = 64;
/// superblock 编码长度。
pub const SUPERBLOCK_BYTES: usize = 32;
/// 双槽最大条目数（4084B / 16B 向下取整）。
pub const OFFSET_MAX_ENTRIES: usize = 254;
/// 双槽条目区起点。
pub const OFFSET_ENTRIES_AT: usize = 20;
/// 双槽 CRC 存放位置（固定，覆盖 [0..20+n*16)）。
pub const OFFSET_CRC_AT: usize = OFFSET_ENTRIES_AT + OFFSET_MAX_ENTRIES * 16;

/// CRC32（HYG-001：eneros-storage → ipc-wire 唯一实现，IEEE 802.3）。
#[inline]
pub(crate) fn crc32(data: &[u8]) -> u32 {
    eneros_storage::crc32(data)
}

/// 未使用块判定（全 0 或全 0xFF——覆盖 mock erase 与真实 flash erase）。
pub fn is_blank(buf: &[u8]) -> bool {
    let first = buf.first().copied().unwrap_or(0);
    (first == 0 && buf.iter().all(|b| *b == 0)) || (first == 0xFF && buf.iter().all(|b| *b == 0xFF))
}

/// config digest（superblock 与 segment header 携带；防配置漂移）。
pub fn config_digest(cfg: &StoreConfig) -> u32 {
    let mut buf = [0u8; 16];
    buf[0..4].copy_from_slice(&cfg.segment_bytes.to_le_bytes());
    buf[4..8].copy_from_slice(&cfg.batch_records.to_le_bytes());
    buf[8..12].copy_from_slice(&cfg.q0_batch_records.to_le_bytes());
    buf[12] = u8::from(cfg.reject_q2q3_at_95_percent);
    crc32(&buf)
}

/// superblock 解码结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperInfo {
    /// segment 大小（字节）。
    pub segment_bytes: u32,
    /// format 时的 config digest。
    pub config_digest: u32,
    /// format 时间（unix 秒，时钟源由 config 提供）。
    pub created_unix_s: u64,
}

/// superblock 解码失败原因（open 错误分类用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperDecode {
    /// 全零/全 FF——从未 format。
    Blank,
    /// 非零但 magic 不符。
    Foreign,
    /// magic 命中但版本/CRC 不符。
    Corrupt,
}

/// 编码 superblock（32B；CRC 覆盖 [0..28)）。
pub fn encode_superblock(cfg: &StoreConfig, created_unix_s: u64) -> [u8; SUPERBLOCK_BYTES] {
    let mut b = [0u8; SUPERBLOCK_BYTES];
    b[0..4].copy_from_slice(&SUPER_MAGIC);
    b[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    b[6..8].copy_from_slice(&0u16.to_le_bytes()); // flags
    b[8..12].copy_from_slice(&cfg.segment_bytes.to_le_bytes());
    b[12..16].copy_from_slice(&config_digest(cfg).to_le_bytes());
    b[16..24].copy_from_slice(&created_unix_s.to_le_bytes());
    let crc = crc32(&b[..28]);
    b[28..32].copy_from_slice(&crc.to_le_bytes());
    b
}

/// 解码 superblock。
pub fn decode_superblock(block: &[u8]) -> Result<SuperInfo, SuperDecode> {
    if is_blank(&block[..8]) {
        return Err(SuperDecode::Blank);
    }
    if block[0..4] != SUPER_MAGIC {
        return Err(SuperDecode::Foreign);
    }
    let version = u16::from_le_bytes([block[4], block[5]]);
    if version != FORMAT_VERSION {
        return Err(SuperDecode::Corrupt);
    }
    let stored = u32::from_le_bytes(block[28..32].try_into().expect("super crc"));
    if crc32(&block[..28]) != stored {
        return Err(SuperDecode::Corrupt);
    }
    Ok(SuperInfo {
        segment_bytes: u32::from_le_bytes(block[8..12].try_into().expect("seg bytes")),
        config_digest: u32::from_le_bytes(block[12..16].try_into().expect("digest")),
        created_unix_s: u64::from_le_bytes(block[16..24].try_into().expect("created")),
    })
}

/// segment header 解码结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentHeader {
    /// 分区。
    pub partition: u32,
    /// 每分区单调 segment id。
    pub segment_id: u32,
    /// 段首记录 offset。
    pub first_offset: u64,
    /// 创建时间。
    pub created_unix_s: u64,
    /// config digest。
    pub config_digest: u32,
}

/// 编码 segment header（64B；CRC 覆盖 [0..44)）。
pub fn encode_segment_header(
    partition: u32,
    segment_id: u32,
    first_offset: u64,
    created_unix_s: u64,
    digest: u32,
) -> [u8; SEGMENT_HEADER_BYTES] {
    let mut b = [0u8; SEGMENT_HEADER_BYTES];
    b[0..4].copy_from_slice(&SEGMENT_MAGIC);
    b[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    b[6..8].copy_from_slice(&0u16.to_le_bytes()); // flags
    b[8..12].copy_from_slice(&partition.to_le_bytes());
    b[12..16].copy_from_slice(&segment_id.to_le_bytes());
    b[16..24].copy_from_slice(&first_offset.to_le_bytes());
    b[24..32].copy_from_slice(&created_unix_s.to_le_bytes());
    b[32..36].copy_from_slice(&digest.to_le_bytes());
    let crc = crc32(&b[..40]);
    b[40..44].copy_from_slice(&crc.to_le_bytes());
    b
}

/// 解码 segment header（magic/版本/CRC 任一不符 → None）。
pub fn decode_segment_header(block: &[u8]) -> Option<SegmentHeader> {
    if block.len() < 44 || block[0..4] != SEGMENT_MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([block[4], block[5]]);
    if version != FORMAT_VERSION {
        return None;
    }
    let stored = u32::from_le_bytes(block[40..44].try_into().ok()?);
    if crc32(&block[..40]) != stored {
        return None;
    }
    Some(SegmentHeader {
        partition: u32::from_le_bytes(block[8..12].try_into().ok()?),
        segment_id: u32::from_le_bytes(block[12..16].try_into().ok()?),
        first_offset: u64::from_le_bytes(block[16..24].try_into().ok()?),
        created_unix_s: u64::from_le_bytes(block[24..32].try_into().ok()?),
        config_digest: u32::from_le_bytes(block[32..36].try_into().ok()?),
    })
}

/// 编码 record 头 + payload 到 `out`，返回写入长度（36 + payload.len()）。
///
/// `out` 长度必须 ≥ [`encoded_record_len`]。
pub fn encode_record_into(
    out: &mut [u8],
    offset: u64,
    event_id: [u8; 16],
    flags: u16,
    schema: u16,
    payload: &[u8],
) -> usize {
    debug_assert!(out.len() >= RECORD_HEADER_LEN + payload.len());
    out[0..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[4..6].copy_from_slice(&flags.to_le_bytes());
    out[6..8].copy_from_slice(&schema.to_le_bytes());
    out[8..16].copy_from_slice(&offset.to_le_bytes());
    out[16..32].copy_from_slice(&event_id);
    // CRC = 头前 32 字节（不含 CRC 自身）∥ payload——头部损坏亦可检出。
    let mut covered: Vec<u8> = Vec::with_capacity(32 + payload.len());
    covered.extend_from_slice(&out[..32]);
    covered.extend_from_slice(payload);
    out[32..36].copy_from_slice(&crc32(&covered).to_le_bytes());
    out[RECORD_HEADER_LEN..RECORD_HEADER_LEN + payload.len()].copy_from_slice(payload);
    RECORD_HEADER_LEN + payload.len()
}

/// 校验 record CRC（头前 32 字节 ∥ payload——与 [`encode_record_into`] 同式）。
pub fn verify_record_crc(raw_header: &[u8], payload: &[u8], stored: u32) -> bool {
    if raw_header.len() < 32 {
        return false;
    }
    let mut covered: Vec<u8> = Vec::with_capacity(32 + payload.len());
    covered.extend_from_slice(&raw_header[..32]);
    covered.extend_from_slice(payload);
    crc32(&covered) == stored
}

/// record 编码总长（36 + payload）。
#[inline]
pub fn encoded_record_len(payload_len: usize) -> usize {
    RECORD_HEADER_LEN + payload_len
}

/// 堆上编码完整 record（mem 参考模型与 SHA-256 增量摘要共用）。
pub fn encode_record(
    offset: u64,
    event_id: [u8; 16],
    flags: u16,
    schema: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut v = Vec::with_capacity(encoded_record_len(payload.len()));
    // 先压入 36B 头 + payload 占位（encode 全量覆盖；避免 resize 慢清零）。
    v.extend_from_slice(&[0u8; RECORD_HEADER_LEN]);
    v.extend_from_slice(payload);
    let n = encode_record_into(&mut v, offset, event_id, flags, schema, payload);
    debug_assert_eq!(n, v.len());
    v
}

/// record 头解析（不含 CRC 校验——payload 校验由调用方做）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    /// payload 长度。
    pub len: u32,
    /// flags（低 2 位 QoS）。
    pub flags: u16,
    /// schema（当前恒 0）。
    pub schema: u16,
    /// 分区内 offset。
    pub offset: u64,
    /// 16B 去重键。
    pub event_id: [u8; 16],
    /// payload CRC32。
    pub payload_crc: u32,
}

/// 解码 record 头（36B）。
pub fn decode_record_header(buf: &[u8]) -> Option<RecordHeader> {
    if buf.len() < RECORD_HEADER_LEN {
        return None;
    }
    let mut event_id = [0u8; 16];
    event_id.copy_from_slice(&buf[16..32]);
    Some(RecordHeader {
        len: u32::from_le_bytes(buf[0..4].try_into().ok()?),
        flags: u16::from_le_bytes(buf[4..6].try_into().ok()?),
        schema: u16::from_le_bytes(buf[6..8].try_into().ok()?),
        offset: u64::from_le_bytes(buf[8..16].try_into().ok()?),
        event_id,
        payload_crc: u32::from_le_bytes(buf[32..36].try_into().ok()?),
    })
}

/// trailer 解码结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrailerInfo {
    /// 记录数。
    pub record_count: u64,
    /// 段末记录 offset。
    pub last_offset: u64,
    /// 全段 record 编码字节流的 SHA-256。
    pub sha256: [u8; 32],
}

/// 编码 segment trailer（64B；CRC 覆盖 [0..60)）——seal/commit marker。
pub fn encode_trailer(record_count: u64, last_offset: u64, sha256: &[u8; 32]) -> [u8; TRAILER_LEN] {
    let mut b = [0u8; TRAILER_LEN];
    b[0..4].copy_from_slice(&TRAILER_MAGIC);
    b[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    b[6..8].copy_from_slice(&0u16.to_le_bytes()); // flags
    b[8..12].copy_from_slice(&(record_count.min(u64::from(u32::MAX)) as u32).to_le_bytes());
    b[12..16].copy_from_slice(&0u32.to_le_bytes()); // rsvd
    b[16..24].copy_from_slice(&last_offset.to_le_bytes());
    b[24..56].copy_from_slice(sha256);
    let crc = crc32(&b[..60]);
    b[60..64].copy_from_slice(&crc.to_le_bytes());
    b
}

/// 解码 segment trailer。
pub fn decode_trailer(block: &[u8]) -> Option<TrailerInfo> {
    if block.len() < TRAILER_LEN || block[0..4] != TRAILER_MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([block[4], block[5]]);
    if version != FORMAT_VERSION {
        return None;
    }
    let stored = u32::from_le_bytes(block[60..64].try_into().ok()?);
    if crc32(&block[..60]) != stored {
        return None;
    }
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&block[24..56]);
    Some(TrailerInfo {
        record_count: u64::from(u32::from_le_bytes(block[8..12].try_into().ok()?)),
        last_offset: u64::from_le_bytes(block[16..24].try_into().ok()?),
        sha256,
    })
}

/// 编码消费位点槽块（4096B）。
pub fn encode_offset_slot(generation: u64, entries: &[(u32, u32, u64)]) -> [u8; BLOCK_SIZE] {
    debug_assert!(entries.len() <= OFFSET_MAX_ENTRIES);
    let mut b = [0u8; BLOCK_SIZE];
    b[0..4].copy_from_slice(&OFFSET_SLOT_MAGIC);
    b[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    b[6..8].copy_from_slice(&0u16.to_le_bytes()); // flags
    b[8..16].copy_from_slice(&generation.to_le_bytes());
    b[16..20].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    for (i, (group, partition, offset)) in entries.iter().enumerate() {
        let at = OFFSET_ENTRIES_AT + i * 16;
        b[at..at + 4].copy_from_slice(&group.to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&partition.to_le_bytes());
        b[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
    }
    let covered = OFFSET_ENTRIES_AT + entries.len() * 16;
    let crc = crc32(&b[..covered]);
    b[OFFSET_CRC_AT..OFFSET_CRC_AT + 4].copy_from_slice(&crc.to_le_bytes());
    b
}

/// 解码消费位点槽块（返回 (generation, entries)；magic/版本/count/CRC
/// 任一不符 → None，等价"槽无效"，双槽协议取另一槽）。
pub fn decode_offset_slot(block: &[u8]) -> Option<crate::OffsetSlotImage> {
    if block.len() < OFFSET_CRC_AT + 4 || block[0..4] != OFFSET_SLOT_MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([block[4], block[5]]);
    if version != FORMAT_VERSION {
        return None;
    }
    let count = u32::from_le_bytes(block[16..20].try_into().ok()?) as usize;
    if count > OFFSET_MAX_ENTRIES {
        return None;
    }
    let covered = OFFSET_ENTRIES_AT + count * 16;
    let stored = u32::from_le_bytes(block[OFFSET_CRC_AT..OFFSET_CRC_AT + 4].try_into().ok()?);
    if crc32(&block[..covered]) != stored {
        return None;
    }
    let generation = u64::from_le_bytes(block[8..16].try_into().ok()?);
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let at = OFFSET_ENTRIES_AT + i * 16;
        let group = u32::from_le_bytes(block[at..at + 4].try_into().ok()?);
        let partition = u32::from_le_bytes(block[at + 4..at + 8].try_into().ok()?);
        let offset = u64::from_le_bytes(block[at + 8..at + 16].try_into().ok()?);
        entries.push((group, partition, offset));
    }
    Some((generation, entries))
}

/// 段几何（两实现共用的尺寸判据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geo {
    /// segment 大小（字节）。
    pub segment_bytes: u64,
    /// 每段块数。
    pub segment_blocks: u64,
    /// 槽位总数。
    pub num_slots: u32,
}

impl Geo {
    /// 由配置与设备块数推导；设备装不下一个段 → None。
    pub fn new(cfg: &StoreConfig, device_blocks: u64) -> Option<Self> {
        let segment_blocks = u64::from(cfg.segment_bytes) / BLOCK_SIZE as u64;
        if device_blocks < FIXED_BLOCKS + segment_blocks {
            return None;
        }
        Some(Geo {
            segment_bytes: u64::from(cfg.segment_bytes),
            segment_blocks,
            num_slots: ((device_blocks - FIXED_BLOCKS) / segment_blocks) as u32,
        })
    }

    /// segment 槽位 `slot` 的起始设备块号。
    #[inline]
    pub fn slot_base_block(&self, slot: u32) -> u64 {
        FIXED_BLOCKS + u64::from(slot) * self.segment_blocks
    }

    /// record 数据区终点（段内相对字节；trailer 块独占段尾）。
    #[inline]
    pub fn data_end(&self) -> u64 {
        self.segment_bytes - BLOCK_SIZE as u64
    }

    /// 单段 record 区容量（字节，不含 64B header）。
    #[inline]
    pub fn segment_record_capacity(&self) -> u64 {
        self.data_end() - SEGMENT_HEADER_LEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> StoreConfig {
        StoreConfig {
            segment_bytes: 8192,
            batch_records: 8,
            q0_batch_records: 2,
            reject_q2q3_at_95_percent: true,
            clock_unix_s: || 42,
        }
    }

    /// CRC32 KAT（IEEE 802.3，"123456789" → 0xCBF43926）。
    #[test]
    fn crc32_known_answer() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn superblock_roundtrip_and_error_kinds() {
        let cfg = test_config();
        let block = encode_superblock(&cfg, 1234);
        assert_eq!(block.len(), SUPERBLOCK_BYTES);
        let info = decode_superblock(&block).expect("decode");
        assert_eq!(info.segment_bytes, 8192);
        assert_eq!(info.config_digest, config_digest(&cfg));
        assert_eq!(info.created_unix_s, 1234);
        assert_eq!(decode_superblock(&[0u8; 4096]), Err(SuperDecode::Blank));
        assert_eq!(decode_superblock(&[0xFFu8; 4096]), Err(SuperDecode::Blank));
        let mut foreign = block;
        foreign[0] = b'X';
        assert_eq!(decode_superblock(&foreign), Err(SuperDecode::Foreign));
        let mut corrupt = block;
        corrupt[9] ^= 0xFF;
        assert_eq!(decode_superblock(&corrupt), Err(SuperDecode::Corrupt));
    }

    #[test]
    fn segment_header_roundtrip() {
        let cfg = test_config();
        let hdr = encode_segment_header(7, 3, 900, (cfg.clock_unix_s)(), config_digest(&cfg));
        assert_eq!(hdr.len(), SEGMENT_HEADER_BYTES);
        let dec = decode_segment_header(&hdr).expect("decode");
        assert_eq!(
            dec,
            SegmentHeader {
                partition: 7,
                segment_id: 3,
                first_offset: 900,
                created_unix_s: 42,
                config_digest: config_digest(&cfg),
            }
        );
        let mut bad = hdr;
        bad[20] ^= 0x01;
        assert!(decode_segment_header(&bad).is_none());
        assert!(decode_segment_header(&[0u8; 64]).is_none());
    }

    /// record 编解码边界：1116B 恰好 = RECORD_MAX(1152) - 头(36)；1117B
    /// 超限（调用方拒绝），编码层保证 1152B 硬上限。
    #[test]
    fn record_boundary_and_roundtrip() {
        assert_eq!(PAYLOAD_MAX, 1116);
        assert_eq!(RECORD_MAX, 1152);
        let payload = [0xA5u8; PAYLOAD_MAX];
        let rec = encode_record(7, [9; 16], 0b10, 0, &payload);
        assert_eq!(rec.len(), RECORD_MAX);
        let hdr = decode_record_header(&rec).expect("decode");
        assert_eq!(hdr.len as usize, PAYLOAD_MAX);
        assert_eq!(hdr.offset, 7);
        assert_eq!(hdr.flags, 0b10);
        assert_eq!(hdr.event_id, [9; 16]);
        // CRC 覆盖头前 32 字节 + payload（头部字段也受保护）。
        let mut covered = alloc::vec::Vec::new();
        covered.extend_from_slice(&rec[..32]);
        covered.extend_from_slice(&payload);
        assert_eq!(hdr.payload_crc, crc32(&covered));
        // 位翻转（event_id 区）→ verify_record_crc 必须检出。
        let mut flipped = rec;
        flipped[20] ^= 0x01;
        assert!(!verify_record_crc(&flipped, &payload, hdr.payload_crc));
        assert_eq!(hdr.schema, 0);
        // 空 payload 合法（len=0，record 头 36B）。
        let empty = encode_record(0, [0; 16], 0, 0, &[]);
        assert_eq!(empty.len(), RECORD_HEADER_LEN);
    }

    #[test]
    fn trailer_roundtrip() {
        let mut sha = [0u8; 32];
        for (i, b) in sha.iter_mut().enumerate() {
            *b = i as u8;
        }
        let t = encode_trailer(100, 999, &sha);
        assert_eq!(t.len(), TRAILER_LEN);
        assert_eq!(
            decode_trailer(&t),
            Some(TrailerInfo {
                record_count: 100,
                last_offset: 999,
                sha256: sha,
            })
        );
        let mut bad = t;
        bad[30] ^= 0x80;
        assert!(decode_trailer(&bad).is_none());
        assert!(decode_trailer(&[0u8; 64]).is_none());
    }

    #[test]
    fn offset_slot_roundtrip_and_torn() {
        let entries: Vec<(u32, u32, u64)> = (0..OFFSET_MAX_ENTRIES)
            .map(|i| ((i / 3) as u32, i as u32, i as u64 * 2))
            .collect();
        let slot = encode_offset_slot(17, &entries);
        let (gen, dec) = decode_offset_slot(&slot).expect("decode");
        assert_eq!(gen, 17);
        assert_eq!(dec.len(), OFFSET_MAX_ENTRIES);
        assert_eq!(dec[123], (41, 123, 246));
        // 超容量编码在 debug_assert 下不可达；此处验证 count 超限拒绝。
        let mut over = slot;
        over[16..20].copy_from_slice(&(OFFSET_MAX_ENTRIES as u32 + 1).to_le_bytes());
        assert!(decode_offset_slot(&over).is_none());
        // CRC 撕裂（覆盖域内字节）→ 槽无效。
        let mut torn = encode_offset_slot(18, &[(1, 2, 3)]);
        torn[20] ^= 0xFF;
        assert!(decode_offset_slot(&torn).is_none());
        // 全零块 → 无效。
        assert!(decode_offset_slot(&[0u8; BLOCK_SIZE]).is_none());
    }

    #[test]
    fn blank_detection_and_geo() {
        assert!(is_blank(&[0u8; 16]));
        assert!(is_blank(&[0xFFu8; 16]));
        assert!(!is_blank(&[0, 0, 0, 1]));
        let cfg = StoreConfig {
            segment_bytes: 8192,
            ..StoreConfig::default()
        };
        let geo = Geo::new(&cfg, 64).expect("geo");
        assert_eq!(geo.num_slots, 30); // (64-3)/2
        assert_eq!(geo.slot_base_block(0), 3);
        assert_eq!(geo.slot_base_block(1), 5);
        assert_eq!(geo.data_end(), 4096);
        assert_eq!(geo.segment_record_capacity(), 4032);
        assert!(Geo::new(&cfg, 4).is_none(), "装不下一个段");
    }
}
