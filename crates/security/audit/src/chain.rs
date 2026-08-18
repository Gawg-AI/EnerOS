//! 哈希链核心：事件编码哈希、日志器（追加/批量/恢复）与链校验器.
//!
//! 哈希方案（36 WP-36.2：SM3 → SHA-256，eneros-crypto 未批准不得进入
//! 产品闭包；仅对已编码前缀计算 SHA-256，而非 512 字节定长整体哈希）：
//!
//! - 事件编码（最大 472 字节，缓冲 512 字节有界安全）：
//!   `seq(8B BE) ‖ ts(8B BE) ‖ type(1B) ‖ severity(1B) ‖ ip(4B) ‖
//!    actor.len(1B)+actor ‖ target.len(1B)+target ‖ detail(尾部无前缀)`
//! - 事件哈希：`event_hash = SHA-256(编码前缀)`
//! - 组合哈希：`combined = SHA-256(prev_hash ‖ event_hash)`（64 字节输入）
//!
//! 注意：36 之前的 SM3 链与当前 SHA-256 链不兼容（链算法无标记字段），
//! 旧链在恢复校验时会因哈希不匹配而失败——这是有意的：旧链作废，
//! 新链从创世重新开始（见 evidence/36 验证记录）。

use alloc::vec::Vec;

use sha2::{Digest, Sha256};

use crate::events::{AuditEvent, ChainEntry, HashChain};
use crate::store::AuditStore;
use crate::{AuditError, VerifyReport};

/// 事件编码缓冲长度（最大编码 472 字节，留 40 字节余量）.
const ENCODE_BUF_LEN: usize = 512;

/// 计算事件哈希：定序编码后仅对已写入前缀做 SHA-256（偏差 D3）.
pub fn compute_event_hash(event: &AuditEvent) -> [u8; 32] {
    let mut buf = [0u8; ENCODE_BUF_LEN];
    let mut pos = 0usize;

    // seq：8 字节大端
    buf[pos..pos + 8].copy_from_slice(&event.seq.to_be_bytes());
    pos += 8;
    // timestamp_ms：8 字节大端
    buf[pos..pos + 8].copy_from_slice(&event.timestamp_ms.to_be_bytes());
    pos += 8;
    // event_type / severity：各 1 字节
    buf[pos] = event.event_type as u8;
    pos += 1;
    buf[pos] = event.severity;
    pos += 1;
    // source_ip：4 字节
    buf[pos..pos + 4].copy_from_slice(&event.source_ip);
    pos += 4;
    // actor：1 字节长度前缀（容量 64 ≤ 255，截断安全）+ 原始字节
    buf[pos] = event.actor.len() as u8;
    pos += 1;
    let actor = event.actor.as_bytes();
    buf[pos..pos + actor.len()].copy_from_slice(actor);
    pos += actor.len();
    // target：1 字节长度前缀（容量 128 ≤ 255，截断安全）+ 原始字节
    buf[pos] = event.target.len() as u8;
    pos += 1;
    let target = event.target.as_bytes();
    buf[pos..pos + target.len()].copy_from_slice(target);
    pos += target.len();
    // detail：尾部全部字节（无长度前缀）
    let detail = event.detail.as_bytes();
    buf[pos..pos + detail.len()].copy_from_slice(detail);
    pos += detail.len();

    // 仅对已编码前缀哈希（尾部零字节不参与）。
    // 36 WP-36.2：SM3 → SHA-256（eneros-crypto 未批准，不得进入产品闭包）。
    Sha256::digest(&buf[..pos]).into()
}

/// 计算组合哈希：`SHA-256(prev_hash ‖ event_hash)`，输入恰好 64 字节.
pub fn compute_combined_hash(prev_hash: &[u8; 32], event_hash: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(prev_hash);
    buf[32..].copy_from_slice(event_hash);
    Sha256::digest(buf).into()
}

/// 审计日志器（序号检查 → 哈希串联 → 存储追加 → 链状态推进）.
pub trait AuditLogger {
    /// 追加一条事件：`seq == 0` 自动赋序，非零序号必须等于 `last_seq + 1`，
    /// 否则返回 [`AuditError::SeqOutOfOrder`] 且链状态不变.
    fn log(&mut self, event: AuditEvent) -> Result<ChainEntry, AuditError>;

    /// 批量追加：逐条调用 [`AuditLogger::log`]，任一失败即中断返回错误.
    fn log_batch(&mut self, events: &[AuditEvent]) -> Result<Vec<ChainEntry>, AuditError>;

    /// 当前链头状态（只读）.
    fn chain_state(&self) -> &HashChain;
}

/// 日志器默认实现：内存链头 + 泛型存储后端.
#[derive(Debug)]
pub struct AuditLoggerImpl<S: AuditStore> {
    /// 链头状态（链尾视图）.
    chain: HashChain,
    /// 存储后端.
    store: S,
}

impl<S: AuditStore> AuditLoggerImpl<S> {
    /// 新建日志器：空链，`last_hash = genesis`，`last_seq = 0`，`entry_count = 0`.
    pub fn new(store: S, genesis: [u8; 32]) -> Self {
        AuditLoggerImpl {
            chain: HashChain {
                last_hash: genesis,
                last_seq: 0,
                entry_count: 0,
                genesis_hash: genesis,
            },
            store,
        }
    }

    /// 从既有存储恢复链头（断电重启后续链）：
    /// 存储为空等价 [`AuditLoggerImpl::new`]；否则读取末条恢复
    /// `last_hash = combined_hash`、`last_seq = seq`、`entry_count = count`.
    pub fn recover(store: S, genesis: [u8; 32]) -> Result<Self, AuditError> {
        let count = store.count()?;
        if count == 0 {
            return Ok(Self::new(store, genesis));
        }
        let last = store.read_last_n(1)?;
        // count > 0 时末条必然存在；缺失视为存储读异常
        let last = match last.last() {
            Some(e) => e,
            None => return Err(AuditError::StorageReadFailed),
        };
        Ok(AuditLoggerImpl {
            chain: HashChain {
                last_hash: last.combined_hash,
                last_seq: last.seq,
                entry_count: count,
                genesis_hash: genesis,
            },
            store,
        })
    }

    /// 只读访问存储后端（供测试/校验读取已落盘条目）.
    pub fn store(&self) -> &S {
        &self.store
    }
}

impl<S: AuditStore> AuditLogger for AuditLoggerImpl<S> {
    /// 追加事件：序号非法时返回错误且不触碰链状态与存储.
    fn log(&mut self, event: AuditEvent) -> Result<ChainEntry, AuditError> {
        let expect = self.chain.last_seq + 1;
        if event.seq != 0 && event.seq != expect {
            return Err(AuditError::SeqOutOfOrder {
                expect,
                got: event.seq,
            });
        }
        // seq 定值（0 → 自动赋序），克隆事件赋 seq 后编码哈希
        let mut ev = event;
        ev.seq = expect;
        let event_hash = compute_event_hash(&ev);
        let combined_hash = compute_combined_hash(&self.chain.last_hash, &event_hash);
        let entry = ChainEntry {
            seq: ev.seq,
            prev_hash: self.chain.last_hash,
            event_hash,
            combined_hash,
            timestamp_ms: ev.timestamp_ms,
        };
        // 先落盘再推进链头：落盘失败则链状态不变
        self.store.append(&entry)?;
        self.chain.last_hash = combined_hash;
        self.chain.last_seq = ev.seq;
        self.chain.entry_count += 1;
        Ok(entry)
    }

    /// 批量追加：逐条 `log`，成功条目已入链，失败即中断.
    fn log_batch(&mut self, events: &[AuditEvent]) -> Result<Vec<ChainEntry>, AuditError> {
        let mut out = Vec::with_capacity(events.len());
        for ev in events {
            let entry = self.log(ev.clone())?;
            out.push(entry);
        }
        Ok(out)
    }

    /// 链头状态只读视图.
    fn chain_state(&self) -> &HashChain {
        &self.chain
    }
}

/// 链校验器：离线重放链接关系，定位首个断点.
pub trait ChainVerifier {
    /// 校验一段链上条目（锚点取 `entries[0].prev_hash`）：
    /// 逐条检查 `prev_hash` 衔接与 `combined_hash` 重算一致性，
    /// 首个不一致处记入 [`VerifyReport::broken_at`].
    fn verify_chain(&self, entries: &[ChainEntry]) -> Result<VerifyReport, AuditError>;

    /// 校验单条链接：`entry.prev_hash` 是否等于给定前链哈希，且组合哈希自洽.
    fn verify_link(&self, entry: &ChainEntry, prev_hash: &[u8; 32]) -> bool;
}

/// 链校验器无状态实现.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChainVerifierImpl;

impl ChainVerifier for ChainVerifierImpl {
    /// 空切片视为合法（total = 0）；锚点取首条 `prev_hash`（信任调用方给定
    /// 的链段起点，创世锚校验由调用方另行比对 `genesis_hash`）.
    fn verify_chain(&self, entries: &[ChainEntry]) -> Result<VerifyReport, AuditError> {
        let total = entries.len() as u64;
        if entries.is_empty() {
            return Ok(VerifyReport {
                total: 0,
                broken_at: None,
                valid: true,
            });
        }
        let mut prev = entries[0].prev_hash;
        let mut broken_at = None;
        for entry in entries {
            // 链接检查：本条 prev_hash 必须等于前条 combined_hash
            if entry.prev_hash != prev {
                broken_at = Some(entry.seq);
                break;
            }
            // 自洽检查：重算组合哈希必须与存证一致（检出事件哈希篡改）
            let recomputed = compute_combined_hash(&entry.prev_hash, &entry.event_hash);
            if recomputed != entry.combined_hash {
                broken_at = Some(entry.seq);
                break;
            }
            prev = entry.combined_hash;
        }
        Ok(VerifyReport {
            total,
            broken_at,
            valid: broken_at.is_none(),
        })
    }

    /// 单条链接校验：锚点匹配 + 组合哈希自洽.
    fn verify_link(&self, entry: &ChainEntry, prev_hash: &[u8; 32]) -> bool {
        if entry.prev_hash != *prev_hash {
            return false;
        }
        compute_combined_hash(&entry.prev_hash, &entry.event_hash) == entry.combined_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AuditEventType;
    use crate::store::MemStore;

    /// 测试创世哈希.
    const GENESIS: [u8; 32] = [0x42u8; 32];

    /// 构造默认测试事件（seq = 0 自动赋序）.
    fn make_event() -> AuditEvent {
        AuditEvent::new(AuditEventType::AuthLogin, "root", "console", "login ok")
    }

    /// 建 logger 并追加 n 条事件，返回 (logger, 条目列表).
    fn build_chain(n: usize) -> (AuditLoggerImpl<MemStore>, Vec<ChainEntry>) {
        let mut logger = AuditLoggerImpl::new(MemStore::new(), GENESIS);
        let mut entries = Vec::new();
        for _ in 0..n {
            let e = logger.log(make_event()).expect("追加应成功");
            entries.push(e);
        }
        (logger, entries)
    }

    /// CHN6：首条 entry.prev_hash == genesis，且链头 last_hash == combined_hash.
    #[test]
    fn chn6_first_entry_anchors_to_genesis() {
        let (logger, entries) = build_chain(1);
        assert_eq!(entries[0].prev_hash, GENESIS);
        assert_eq!(logger.chain_state().last_hash, entries[0].combined_hash);
        assert_eq!(logger.chain_state().genesis_hash, GENESIS);
    }

    /// CHN7：连续 5 条逐条链接，entry_count == 5.
    #[test]
    fn chn7_five_entries_link_sequentially() {
        let (logger, entries) = build_chain(5);
        assert_eq!(entries[0].prev_hash, GENESIS);
        for i in 1..entries.len() {
            assert_eq!(entries[i].prev_hash, entries[i - 1].combined_hash);
        }
        assert_eq!(logger.chain_state().entry_count, 5);
        assert_eq!(logger.chain_state().last_seq, 5);
    }

    /// CHN8：seq = 0 自动赋值 1,2,3 递增.
    #[test]
    fn chn8_zero_seq_auto_assigned_incrementally() {
        let (_logger, entries) = build_chain(3);
        assert_eq!(entries[0].seq, 1);
        assert_eq!(entries[1].seq, 2);
        assert_eq!(entries[2].seq, 3);
    }

    /// CHN9：last_seq = 3 时提交 seq = 9 → SeqOutOfOrder{4, 9}，链状态不变.
    #[test]
    fn chn9_out_of_order_seq_rejected_state_intact() {
        let (mut logger, _entries) = build_chain(3);
        let mut ev = make_event();
        ev.seq = 9;
        let err = logger.log(ev).expect_err("乱序应被拒绝");
        assert_eq!(err, AuditError::SeqOutOfOrder { expect: 4, got: 9 });
        // 链状态保持 last_seq == 3、entry_count == 3
        assert_eq!(logger.chain_state().last_seq, 3);
        assert_eq!(logger.chain_state().entry_count, 3);
        // 存储未被写入第 4 条
        assert_eq!(logger.store().len(), 3);
    }

    /// CHN10：log_batch 3 条全序成功，seq 1~3.
    #[test]
    fn chn10_log_batch_appends_in_order() {
        let mut logger = AuditLoggerImpl::new(MemStore::new(), GENESIS);
        let events = [make_event(), make_event(), make_event()];
        let entries = logger.log_batch(&events).expect("批量追加应成功");
        assert_eq!(entries.len(), 3);
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(e.seq, (i as u64) + 1);
        }
        assert_eq!(logger.chain_state().entry_count, 3);
    }

    /// VER11：10 条净链 verify_chain → valid, broken_at = None, total == 10.
    #[test]
    fn ver11_clean_chain_verifies_valid() {
        let (logger, _entries) = build_chain(10);
        let stored = logger.store().read_last_n(10).expect("读取应成功");
        let verifier = ChainVerifierImpl;
        let report = verifier.verify_chain(&stored).expect("校验应成功");
        assert!(report.valid);
        assert_eq!(report.broken_at, None);
        assert_eq!(report.total, 10);
    }

    /// VER12：篡改第 5 条 event_hash（翻转一字节）→ broken_at == Some(5).
    #[test]
    fn ver12_tampered_event_hash_detected() {
        let (logger, _entries) = build_chain(10);
        let mut stored = logger.store().read_last_n(10).expect("读取应成功");
        // 篡改第 5 条（下标 4）事件哈希
        stored[4].event_hash[0] ^= 0xFF;
        let verifier = ChainVerifierImpl;
        let report = verifier.verify_chain(&stored).expect("校验应成功");
        assert!(!report.valid);
        assert_eq!(report.broken_at, Some(5));
    }

    /// VER13：删除第 5 条后，第 6 条 prev_hash 衔接失败 → broken_at == Some(6).
    #[test]
    fn ver13_deleted_entry_breaks_link_at_next() {
        let (logger, _entries) = build_chain(10);
        let stored = logger.store().read_last_n(10).expect("读取应成功");
        // 去掉第 5 条（下标 4）
        let mut partial: Vec<ChainEntry> = Vec::new();
        for (i, e) in stored.into_iter().enumerate() {
            if i != 4 {
                partial.push(e);
            }
        }
        let verifier = ChainVerifierImpl;
        let report = verifier.verify_chain(&partial).expect("校验应成功");
        assert!(!report.valid);
        assert_eq!(report.broken_at, Some(6));
    }

    /// VER14：verify_link 正确锚点为 true，错误 prev_hash 为 false.
    #[test]
    fn ver14_verify_link_anchor_check() {
        let (_logger, entries) = build_chain(3);
        let verifier = ChainVerifierImpl;
        // 首条正确锚点 = genesis
        assert!(verifier.verify_link(&entries[0], &GENESIS));
        // 首条错误锚点
        assert!(!verifier.verify_link(&entries[0], &[0xFFu8; 32]));
        // 后续条目正确锚点 = 前条 combined_hash
        assert!(verifier.verify_link(&entries[1], &entries[0].combined_hash));
        assert!(!verifier.verify_link(&entries[1], &GENESIS));
    }

    /// VER15：空切片 verify_chain → valid, total == 0.
    #[test]
    fn ver15_empty_slice_is_valid() {
        let verifier = ChainVerifierImpl;
        let report = verifier.verify_chain(&[]).expect("校验应成功");
        assert!(report.valid);
        assert_eq!(report.broken_at, None);
        assert_eq!(report.total, 0);
    }

    /// REC16：log 7 条后 recover 同一存储，链头三字段与恢复前一致.
    #[test]
    fn rec16_recover_restores_chain_state() {
        let (logger, _entries) = build_chain(7);
        let before = logger.chain_state().clone();
        // 追加第 8 条前的状态已确认；克隆存储交给 recover 消费
        let store_clone = logger.store().clone();
        let recovered = AuditLoggerImpl::recover(store_clone, GENESIS).expect("恢复应成功");
        assert_eq!(recovered.chain_state().last_hash, before.last_hash);
        assert_eq!(recovered.chain_state().last_seq, before.last_seq);
        assert_eq!(recovered.chain_state().entry_count, before.entry_count);
        assert_eq!(recovered.chain_state().genesis_hash, before.genesis_hash);
        // 恢复后续链正常：第 8 条 prev_hash 衔接
        let mut logger2 = recovered;
        let e8 = logger2.log(make_event()).expect("恢复后追加应成功");
        assert_eq!(e8.seq, 8);
        assert_eq!(e8.prev_hash, before.last_hash);
        // 原 logger 仍可只读（未被 move）
        let _ = logger.chain_state();
    }

    /// REC17：空存储 recover → last_hash == genesis、last_seq == 0、count == 0；
    /// 再 log 一条 prev_hash == genesis.
    #[test]
    fn rec17_recover_empty_store_equals_new() {
        let mut logger = AuditLoggerImpl::recover(MemStore::new(), GENESIS).expect("恢复应成功");
        assert_eq!(logger.chain_state().last_hash, GENESIS);
        assert_eq!(logger.chain_state().last_seq, 0);
        assert_eq!(logger.chain_state().entry_count, 0);
        let e1 = logger.log(make_event()).expect("追加应成功");
        assert_eq!(e1.prev_hash, GENESIS);
        assert_eq!(e1.seq, 1);
    }
}
