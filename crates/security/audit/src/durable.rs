//! WP-37.8：持久化完整审计记录（DurableAuditRecord）。
//!
//! 修复模型缺陷（P1-03）：`ChainEntry` 只持久化哈希摘要（112B），不存
//! 事件事实——掉电后审计链不可查询、不可恢复事件。本模块定义
//! **完整事件 + 链字段 + schema/crc** 的持久化记录：
//!
//! - `record_hash` 覆盖完整规范化事件（复用 [`compute_event_hash`] 的
//!   定长编码——含全部字段，非摘要之摘要）；
//! - 第一锚（首条 `previous_hash`）来自签名 manifest / TPM NV / 外部
//!   批准 checkpoint，**不能由待验证日志自己提供**（`verify_log` 的
//!   `anchor` 参数）；
//! - truncation（删首条 → `AnchorMismatch`、全删 → `EmptyLog`）、
//!   reorder（非递增 → `Reordered`；sequence 字段受 crc 保护并与
//!   event.seq 交叉断言）、duplicate（序号重复 → `DuplicateSequence`）、
//!   后缀重算（改 record_hash → `HashMismatch`）、位翻转（crc →
//!   `CrcMismatch`）全部可检测；
//! - schema 版本化：未知新 schema 拒绝解码（不误读），migration 时
//!   旧 schema 记录由调用方显式迁移。
//!
//! 37 只建立 runtime audit 记录模型与校验；41 接领域事件与存储接线。

use alloc::vec::Vec;

use crate::chain::{compute_combined_hash, compute_event_hash};
use crate::events::{AuditEvent, AuditEventType};

/// 当前记录 schema 版本。
pub const DURABLE_AUDIT_SCHEMA: u16 = 1;

/// BootId 长度（启动标识，16B）。
pub const DURABLE_BOOT_ID_LEN: usize = 16;

/// CRC32（IEEE 802.3，多项式 0xEDB88320）——无依赖逐位实现（记录 ≤ 约
/// 500B，性能无关；不引入 crc crate 保持 no_std 零依赖）。
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// WP-37.8：持久化审计记录违规（`verify`/`verify_log`/`from_bytes` 输出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordViolation {
    /// schema 未知（不误读新版本）。
    UnknownSchema(u16),
    /// 序号非严格递增（乱序/重放）。
    Reordered { expect_gt: u64, got: u64 },
    /// 序号重复（duplicate）。
    DuplicateSequence(u64),
    /// CRC32 不符（记录位翻转/截断）。
    CrcMismatch { sequence: u64 },
    /// record_hash 与 previous_hash ‖ canonical(event) 不符（后缀重算/
    /// 事件被改）。
    HashMismatch { sequence: u64 },
    /// previous_hash 与前条 record_hash 不符（链断裂/篡改）。
    ChainBreak { sequence: u64 },
    /// 首条 previous_hash 与外部锚不符（首条被删/替换 → truncation）。
    AnchorMismatch,
    /// 记录列表为空（全量截断/从未记录无法区分——fail-closed 拒绝）。
    EmptyLog,
    /// ckpt_flag 未知值（非 0/1）——拒绝不误读。
    InvalidCheckpointFlag,
    /// 事件类型判别值未知/UTF-8 非法（解码不误读）。
    InvalidEventType,
    /// 解码长度不足/越界。
    DecodeTruncated,
}

/// WP-37.8：持久化审计记录（完整事件 + 链字段 + schema/crc）。
/// 注：AuditEvent 未实现 Eq，记录比较用逐字段（测试）或序列化 roundtrip。
#[derive(Debug, Clone)]
pub struct DurableAuditRecord {
    /// 记录 schema 版本（未知版本拒绝解码）。
    pub schema: u16,
    /// 链内严格递增序号（与事件序号一致）。
    pub sequence: u64,
    /// 启动标识（本次启动生成的 boot_id，跨重启区分）。
    pub boot_id: [u8; DURABLE_BOOT_ID_LEN],
    /// 完整规范化事件（record_hash 覆盖它）。
    pub event: AuditEvent,
    /// 前条 record_hash（首条 = 外部锚 genesis）。
    pub previous_hash: [u8; 32],
    /// SHA-256(previous_hash ‖ canonical(event))。
    pub record_hash: [u8; 32],
    /// 周期 checkpoint 编号（None = 普通记录）。
    pub checkpoint_id: Option<u32>,
    /// CRC32(previous_hash ‖ canonical(event) ‖ boot_id) —— 位翻转检测。
    pub crc: u32,
}

impl DurableAuditRecord {
    /// 构造：事件序号由调用方定值（链内递增）；`record_hash` 与 `crc`
    /// 自动计算。`previous_hash` 首条必须为外部锚（签名 manifest/TPM NV）。
    pub fn new(
        previous_hash: [u8; 32],
        event: AuditEvent,
        boot_id: [u8; DURABLE_BOOT_ID_LEN],
        checkpoint_id: Option<u32>,
    ) -> Self {
        let event_hash = compute_event_hash(&event);
        let record_hash = compute_combined_hash(&previous_hash, &event_hash);
        let crc = crc32(&Self::crc_input(
            event.seq,
            &previous_hash,
            &event,
            &boot_id,
        ));
        DurableAuditRecord {
            schema: DURABLE_AUDIT_SCHEMA,
            sequence: event.seq,
            boot_id,
            event,
            previous_hash,
            record_hash,
            checkpoint_id,
            crc,
        }
    }

    /// CRC 覆盖的字节：previous_hash ‖ canonical(event) ‖ boot_id
    /// （record_hash 错误由 HashMismatch 独立检测）。
    fn crc_input(
        sequence: u64,
        previous_hash: &[u8; 32],
        event: &AuditEvent,
        boot_id: &[u8; DURABLE_BOOT_ID_LEN],
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 32 + 512 + DURABLE_BOOT_ID_LEN);
        out.extend_from_slice(&sequence.to_be_bytes());
        out.extend_from_slice(previous_hash);
        out.extend_from_slice(&event_to_canonical(event));
        out.extend_from_slice(boot_id);
        out
    }

    /// 校验单条记录：crc → record_hash → previous_hash 链。
    pub fn verify(&self, expected_prev: [u8; 32]) -> Result<(), RecordViolation> {
        if self.schema != DURABLE_AUDIT_SCHEMA {
            return Err(RecordViolation::UnknownSchema(self.schema));
        }
        if self.crc
            != crc32(&Self::crc_input(
                self.sequence,
                &self.previous_hash,
                &self.event,
                &self.boot_id,
            ))
        {
            return Err(RecordViolation::CrcMismatch {
                sequence: self.sequence,
            });
        }
        // review R1：sequence 字段与 event.seq 必须一致（event.seq 已受
        // record_hash 保护；不一致即篡改）。
        if self.sequence != self.event.seq {
            return Err(RecordViolation::Reordered {
                expect_gt: self.event.seq,
                got: self.sequence,
            });
        }
        let event_hash = compute_event_hash(&self.event);
        let expect_hash = compute_combined_hash(&self.previous_hash, &event_hash);
        if self.record_hash != expect_hash {
            return Err(RecordViolation::HashMismatch {
                sequence: self.sequence,
            });
        }
        if self.previous_hash != expected_prev {
            return Err(RecordViolation::ChainBreak {
                sequence: self.sequence,
            });
        }
        Ok(())
    }
}

/// 定长规范化事件编码（复用 [`compute_event_hash`] 的语义：完整字段、
/// 长度前缀字符串；与哈希输入严格一致）。
pub fn event_to_canonical(event: &AuditEvent) -> Vec<u8> {
    // 与 chain.rs compute_event_hash 的编码完全一致（8+8+1+1+4+前缀字符串）。
    let mut buf = Vec::with_capacity(512);
    buf.extend_from_slice(&event.seq.to_be_bytes());
    buf.extend_from_slice(&event.timestamp_ms.to_be_bytes());
    buf.push(event.event_type as u8);
    buf.push(event.severity);
    buf.extend_from_slice(&event.source_ip);
    buf.push(event.actor.len() as u8);
    buf.extend_from_slice(event.actor.as_bytes());
    buf.push(event.target.len() as u8);
    buf.extend_from_slice(event.target.as_bytes());
    buf.extend_from_slice(event.detail.as_bytes());
    buf
}

/// 定长编码：schema(2) ‖ sequence(8) ‖ boot_id(16) ‖ canonical_len(2)
/// ‖ canonical ‖ previous_hash(32) ‖ record_hash(32) ‖ ckpt_flag(1)
/// ‖ ckpt_id(4) ‖ crc(4)。
pub fn record_to_bytes(record: &DurableAuditRecord) -> Vec<u8> {
    let canonical = event_to_canonical(&record.event);
    let mut out = Vec::with_capacity(2 + 8 + 16 + 2 + canonical.len() + 32 + 32 + 1 + 4 + 4);
    out.extend_from_slice(&record.schema.to_be_bytes());
    out.extend_from_slice(&record.sequence.to_be_bytes());
    out.extend_from_slice(&record.boot_id);
    out.extend_from_slice(&(canonical.len() as u16).to_be_bytes());
    out.extend_from_slice(&canonical);
    out.extend_from_slice(&record.previous_hash);
    out.extend_from_slice(&record.record_hash);
    out.push(record.checkpoint_id.map(|_| 1u8).unwrap_or(0));
    if let Some(id) = record.checkpoint_id {
        out.extend_from_slice(&id.to_be_bytes());
    }
    out.extend_from_slice(&record.crc.to_be_bytes());
    out
}

/// 解码：schema 未知 / 长度不足 → Err（不误读新版本；migration 由调用方
/// 显式处理）。不做 crc/链校验（那是 `verify`/`verify_log` 的职责）。
pub fn record_from_bytes(bytes: &[u8]) -> Result<DurableAuditRecord, RecordViolation> {
    let need = |n: usize| -> Result<(), RecordViolation> {
        if bytes.len() < n {
            return Err(RecordViolation::DecodeTruncated);
        }
        Ok(())
    };
    need(2 + 8 + 16 + 2)?;
    let schema = u16::from_be_bytes([bytes[0], bytes[1]]);
    if schema != DURABLE_AUDIT_SCHEMA {
        return Err(RecordViolation::UnknownSchema(schema));
    }
    let sequence = u64::from_be_bytes(bytes[2..10].try_into().unwrap());
    let mut boot_id = [0u8; DURABLE_BOOT_ID_LEN];
    boot_id.copy_from_slice(&bytes[10..10 + DURABLE_BOOT_ID_LEN]);
    let canonical_len = u16::from_be_bytes([bytes[26], bytes[27]]) as usize;
    let canonical_start = 28;
    need(canonical_start + canonical_len + 32 + 32 + 1)?;
    let canonical = &bytes[canonical_start..canonical_start + canonical_len];
    let event = canonical_to_event(canonical)?;
    let mut previous_hash = [0u8; 32];
    previous_hash.copy_from_slice(
        &bytes[canonical_start + canonical_len..canonical_start + canonical_len + 32],
    );
    let mut record_hash = [0u8; 32];
    record_hash.copy_from_slice(
        &bytes[canonical_start + canonical_len + 32..canonical_start + canonical_len + 64],
    );
    let ckpt_flag = bytes[canonical_start + canonical_len + 64];
    if ckpt_flag != 0 && ckpt_flag != 1 {
        return Err(RecordViolation::InvalidCheckpointFlag);
    }
    let ckpt_id = if ckpt_flag == 1 {
        need(canonical_start + canonical_len + 64 + 1 + 4 + 4)?;
        Some(u32::from_be_bytes(
            bytes[canonical_start + canonical_len + 65..canonical_start + canonical_len + 69]
                .try_into()
                .unwrap(),
        ))
    } else {
        None
    };
    // crc 紧跟 ckpt 字段之后（flag(1) [+ ckpt_id(4)] 后）。
    let crc_start = canonical_start + canonical_len + 64 + 1 + if ckpt_flag == 1 { 4 } else { 0 };
    need(crc_start + 4)?;
    let crc = u32::from_be_bytes(bytes[crc_start..crc_start + 4].try_into().unwrap());
    Ok(DurableAuditRecord {
        schema,
        sequence,
        boot_id,
        event,
        previous_hash,
        record_hash,
        checkpoint_id: ckpt_id,
        crc,
    })
}

/// 从规范化字节解码事件（与 [`event_to_canonical`] 对称）。
fn canonical_to_event(canonical: &[u8]) -> Result<AuditEvent, RecordViolation> {
    let need = |n: usize| -> Result<(), RecordViolation> {
        if canonical.len() < n {
            return Err(RecordViolation::DecodeTruncated);
        }
        Ok(())
    };
    need(8 + 8 + 1 + 1 + 4 + 1 + 1)?;
    let mut pos = 0usize;
    let seq = u64::from_be_bytes(canonical[pos..pos + 8].try_into().unwrap());
    pos += 8;
    let timestamp_ms = u64::from_be_bytes(canonical[pos..pos + 8].try_into().unwrap());
    pos += 8;
    let event_type = canonical[pos];
    pos += 1;
    let severity = canonical[pos];
    pos += 1;
    let mut source_ip = [0u8; 4];
    source_ip.copy_from_slice(&canonical[pos..pos + 4]);
    pos += 4;
    let actor_len = canonical[pos] as usize;
    pos += 1;
    need(pos + actor_len + 1)?;
    let actor = core::str::from_utf8(&canonical[pos..pos + actor_len])
        .map_err(|_| RecordViolation::InvalidEventType)?;
    pos += actor_len;
    let target_len = canonical[pos] as usize;
    pos += 1;
    need(pos + target_len)?;
    let target = core::str::from_utf8(&canonical[pos..pos + target_len])
        .map_err(|_| RecordViolation::InvalidEventType)?;
    pos += target_len;
    let detail =
        core::str::from_utf8(&canonical[pos..]).map_err(|_| RecordViolation::InvalidEventType)?;
    let event_type =
        AuditEventType::try_from(event_type).map_err(|_| RecordViolation::InvalidEventType)?;
    let mut actor_s = heapless::String::<64>::new();
    let _ = actor_s.push_str(actor);
    let mut target_s = heapless::String::<128>::new();
    let _ = target_s.push_str(target);
    let mut detail_s = heapless::String::<256>::new();
    let _ = detail_s.push_str(detail);
    Ok(AuditEvent {
        seq,
        timestamp_ms,
        event_type,
        actor: actor_s,
        target: target_s,
        detail: detail_s,
        severity,
        source_ip,
    })
}

/// WP-37.8：全日志校验——给定外部锚（签名 manifest/TPM NV/批准 checkpoint，
/// 不能来自待验证日志），按序校验每条记录并检测：
/// 未知 schema / 乱序 / 重复序号 / crc / hash / 链断 / 首条锚不符。
/// 返回第一条违规（fail-closed：任一违规即整链不可信）。
pub fn verify_log(records: &[DurableAuditRecord], anchor: [u8; 32]) -> Result<(), RecordViolation> {
    // review R3：空列表 = 全量截断或从未记录，无法区分 → fail-closed。
    if records.is_empty() {
        return Err(RecordViolation::EmptyLog);
    }
    let mut expected_prev = anchor;
    let mut last_seq: Option<u64> = None;
    for (idx, record) in records.iter().enumerate() {
        if record.schema != DURABLE_AUDIT_SCHEMA {
            return Err(RecordViolation::UnknownSchema(record.schema));
        }
        if let Some(last) = last_seq {
            if record.sequence <= last {
                return Err(if record.sequence == last {
                    RecordViolation::DuplicateSequence(record.sequence)
                } else {
                    RecordViolation::Reordered {
                        expect_gt: last,
                        got: record.sequence,
                    }
                });
            }
        }
        last_seq = Some(record.sequence);
        match record.verify(expected_prev) {
            Ok(()) => {}
            // 首条锚不符 → AnchorMismatch（truncation 语义，优于泛化 ChainBreak）。
            Err(RecordViolation::ChainBreak { sequence: _ }) if idx == 0 => {
                return Err(RecordViolation::AnchorMismatch);
            }
            Err(e) => return Err(e),
        }
        expected_prev = record.record_hash;
    }
    Ok(())
}

/// WP-37.8（复盘）：掉电截断恢复——返回**最后一个完整有效记录**之后的下标
/// （即恢复点：`records[..n]` 为自锚起验证通过的最长前缀，调用方以 `n`
/// 截断后即可续链）。语义：
/// - 全链有效 → `records.len()`；
/// - 尾部损坏/被截断重算 → 违规位置（恢复到最后完整 record）；
/// - 首条锚不符（删头）→ 0（无法恢复，须重新初始化）；
/// - 空列表 → 0（等价从未记录）。
///
/// 与 [`verify_log`]（严格全链校验，任一违规即失败）互补：恢复路径
/// 用本函数定位截断点，不因单条损坏拒绝整链。
pub fn last_valid_prefix(records: &[DurableAuditRecord], anchor: [u8; 32]) -> usize {
    let mut expected_prev = anchor;
    let mut last_seq: Option<u64> = None;
    for (idx, record) in records.iter().enumerate() {
        // 与 verify_log 相同的检查序列；任一违规 → 恢复点 = 违规位置。
        if record.schema != DURABLE_AUDIT_SCHEMA {
            return idx;
        }
        if let Some(last) = last_seq {
            if record.sequence <= last {
                return idx;
            }
        }
        last_seq = Some(record.sequence);
        if record.verify(expected_prev).is_err() {
            return idx;
        }
        expected_prev = record.record_hash;
    }
    records.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuditEvent;

    fn event(seq: u64, ty: AuditEventType, detail: &str) -> AuditEvent {
        let mut e = AuditEvent::new(ty, "svc-ctrl", "point-1", detail);
        e.seq = seq;
        e.timestamp_ms = 1700000000 + seq;
        e
    }

    fn chain(n: u64, anchor: [u8; 32], boot: [u8; 16]) -> Vec<DurableAuditRecord> {
        let mut prev = anchor;
        let mut out = Vec::new();
        for seq in 1..=n {
            let r = DurableAuditRecord::new(
                prev,
                event(seq, AuditEventType::ControlCommand, "cmd"),
                boot,
                None,
            );
            prev = r.record_hash;
            out.push(r);
        }
        out
    }

    const ANCHOR: [u8; 32] = [0xAB; 32];
    const BOOT: [u8; 16] = [0x11; 16];

    #[test]
    fn crc32_known_vector() {
        // IEEE 802.3 已知向量：crc32(b"123456789") = 0xCBF43926。
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn verify_log_accepts_valid_chain() {
        let records = chain(5, ANCHOR, BOOT);
        assert_eq!(verify_log(&records, ANCHOR), Ok(()));
    }

    #[test]
    fn verify_log_rejects_anchor_mismatch_on_truncation() {
        // 删首条 → 新首条 previous_hash != anchor → AnchorMismatch。
        let records = chain(5, ANCHOR, BOOT);
        assert_eq!(
            verify_log(&records[1..], ANCHOR),
            Err(RecordViolation::AnchorMismatch)
        );
    }

    #[test]
    fn verify_log_rejects_suffix_recomputation() {
        // 攻击者重算后缀（改 record_hash）→ HashMismatch。
        let mut records = chain(3, ANCHOR, BOOT);
        records[1].record_hash[0] ^= 0xFF;
        assert_eq!(
            verify_log(&records, ANCHOR),
            Err(RecordViolation::HashMismatch { sequence: 2 })
        );
    }

    #[test]
    fn verify_log_rejects_reorder() {
        // 攻击者篡改 sequence 并同步重算 crc（record_hash 不覆盖 sequence，
        // event.seq 未变 → hash 仍对；序号整体仍递增 → verify_log 层顺序
        // 检查无法抓）→ verify 的 sequence==event.seq 交叉断言必须抓
        // （review R1 修复，mutation 验证目标）。
        let mut records = chain(3, ANCHOR, BOOT);
        records[0].sequence = 0; // event.seq=1，篡改为 0（后续 2,3 仍递增）
        records[0].crc = crc32(&DurableAuditRecord::crc_input(
            records[0].sequence,
            &records[0].previous_hash,
            &records[0].event,
            &records[0].boot_id,
        ));
        assert!(matches!(
            verify_log(&records, ANCHOR),
            Err(RecordViolation::Reordered { .. })
        ));
        // 篡改 sequence 但不重算 crc → CrcMismatch（crc 覆盖 sequence）。
        let mut records2 = chain(3, ANCHOR, BOOT);
        records2[1].sequence = 99;
        assert!(matches!(
            verify_log(&records2, ANCHOR),
            Err(RecordViolation::CrcMismatch { .. })
        ));
        // swap 完整记录（含链字段）→ ChainBreak（链式完整性优先）。
        let records3 = chain(3, ANCHOR, BOOT);
        let swapped = vec![
            records3[0].clone(),
            records3[2].clone(),
            records3[1].clone(),
        ];
        assert!(matches!(
            verify_log(&swapped, ANCHOR),
            Err(RecordViolation::ChainBreak { .. })
        ));
    }

    /// review R3：空记录列表（全量截断/从未记录）→ fail-closed EmptyLog。
    #[test]
    fn verify_log_rejects_empty_log() {
        assert_eq!(verify_log(&[], ANCHOR), Err(RecordViolation::EmptyLog));
    }

    /// review R2：ckpt_flag 未知值（≠0/1）→ 拒绝不误读。
    #[test]
    fn record_from_bytes_rejects_unknown_ckpt_flag() {
        let record = chain(1, ANCHOR, BOOT)[0].clone();
        let mut bytes = record_to_bytes(&record);
        let flag_pos = 28 + event_to_canonical(&record.event).len() + 64;
        bytes[flag_pos] = 7;
        assert!(matches!(
            record_from_bytes(&bytes),
            Err(RecordViolation::InvalidCheckpointFlag)
        ));
    }

    /// review R1：sequence 字段受 crc 保护（单独篡改 → CrcMismatch）。
    #[test]
    fn verify_rejects_sequence_tamper_without_crc_update() {
        let mut record = chain(1, ANCHOR, BOOT)[0].clone();
        record.sequence = 42;
        assert!(matches!(
            record.verify(ANCHOR),
            Err(RecordViolation::CrcMismatch { .. })
        ));
    }

    /// WP-37.8（复盘）：掉电截断恢复——尾部损坏时恢复到最后完整 record。
    #[test]
    fn last_valid_prefix_recovers_after_tail_corruption() {
        // 全链有效 → len。
        let records = chain(5, ANCHOR, BOOT);
        assert_eq!(last_valid_prefix(&records, ANCHOR), 5);
        // 尾部最后一条损坏（后缀重算）→ 恢复点 = 4（保留前 4 条完整记录）。
        let mut corrupt = chain(5, ANCHOR, BOOT);
        corrupt[4].record_hash[0] ^= 0xFF;
        assert_eq!(last_valid_prefix(&corrupt, ANCHOR), 4);
        // 中间损坏 → 恢复点 = 损坏位置。
        let mut mid = chain(5, ANCHOR, BOOT);
        mid[2].crc ^= 0x01;
        assert_eq!(last_valid_prefix(&mid, ANCHOR), 2);
        // 首条锚不符（删头场景）→ 0（无法恢复）。
        let truncated = chain(5, ANCHOR, BOOT);
        assert_eq!(last_valid_prefix(&truncated[1..], ANCHOR), 0);
        // 空列表 → 0。
        assert_eq!(last_valid_prefix(&[], ANCHOR), 0);
    }

    /// review nit：未知事件类型判别值解码 → InvalidEventType（不误读）。
    #[test]
    fn record_from_bytes_rejects_unknown_event_type() {
        let record = chain(1, ANCHOR, BOOT)[0].clone();
        let mut bytes = record_to_bytes(&record);
        // canonical 布局：seq(8)+ts(8)+type(1) @ offset 16。
        bytes[28 + 16] = 99; // 未知判别值
        assert!(matches!(
            record_from_bytes(&bytes),
            Err(RecordViolation::InvalidEventType)
        ));
    }

    #[test]
    fn verify_log_rejects_duplicate_sequence() {
        let mut records = chain(3, ANCHOR, BOOT);
        records[2].sequence = records[1].sequence;
        assert_eq!(
            verify_log(&records, ANCHOR),
            Err(RecordViolation::DuplicateSequence(2))
        );
    }

    #[test]
    fn verify_log_rejects_crc_flip() {
        let mut records = chain(3, ANCHOR, BOOT);
        records[1].crc ^= 0x01;
        assert_eq!(
            verify_log(&records, ANCHOR),
            Err(RecordViolation::CrcMismatch { sequence: 2 })
        );
    }

    #[test]
    fn verify_log_rejects_unknown_schema() {
        let mut records = chain(1, ANCHOR, BOOT);
        records[0].schema = 999;
        assert_eq!(
            verify_log(&records, ANCHOR),
            Err(RecordViolation::UnknownSchema(999))
        );
    }

    #[test]
    fn record_roundtrip_preserves_all_fields() {
        let records = chain(2, ANCHOR, BOOT);
        for r in &records {
            let bytes = record_to_bytes(r);
            let back = record_from_bytes(&bytes).expect("decode");
            assert_eq!(back.schema, r.schema, "schema");
            assert_eq!(back.sequence, r.sequence, "sequence");
            assert_eq!(back.boot_id, r.boot_id, "boot_id");
            assert_eq!(back.previous_hash, r.previous_hash, "previous_hash");
            assert_eq!(back.record_hash, r.record_hash, "record_hash");
            assert_eq!(back.checkpoint_id, r.checkpoint_id, "checkpoint_id");
            assert_eq!(back.crc, r.crc, "crc");
            assert_eq!(back.event.seq, r.event.seq, "event.seq");
            assert_eq!(back.event.timestamp_ms, r.event.timestamp_ms, "event.ts");
            assert_eq!(back.event.event_type, r.event.event_type, "event.type");
            assert_eq!(back.event.severity, r.event.severity, "event.severity");
            assert_eq!(back.event.source_ip, r.event.source_ip, "event.ip");
            assert_eq!(back.event.actor.as_str(), r.event.actor.as_str(), "actor");
            assert_eq!(
                back.event.target.as_str(),
                r.event.target.as_str(),
                "target"
            );
            assert_eq!(
                back.event.detail.as_str(),
                r.event.detail.as_str(),
                "detail"
            );
        }
    }

    #[test]
    fn record_from_bytes_rejects_unknown_schema_without_misread() {
        let mut bytes = record_to_bytes(&chain(1, ANCHOR, BOOT)[0]);
        bytes[0..2].copy_from_slice(&999u16.to_be_bytes());
        assert!(matches!(
            record_from_bytes(&bytes),
            Err(RecordViolation::UnknownSchema(999))
        ));
    }

    #[test]
    fn record_from_bytes_rejects_truncated() {
        let bytes = record_to_bytes(&chain(1, ANCHOR, BOOT)[0]);
        assert!(matches!(
            record_from_bytes(&bytes[..bytes.len() - 5]),
            Err(RecordViolation::DecodeTruncated)
        ));
    }
}
