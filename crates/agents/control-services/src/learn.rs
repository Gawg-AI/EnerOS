//! SPEC-SERIES-2026-08-27/E WP-E.3：有界学习闭环（宿主 = svc-ai-worker）。
//!
//! 把既有 [`DeadbandAdaptive`]（dual-brain W2.5 死区自适应，零新算法）接成
//! **真落盘真恢复真改变边界**的生产回路：
//!
//! 1. **输入（EAQ-4 实证口径）**：生产 spawn 面 `DecisionRecord.deviation`
//!    恒为硬编码 0.0（dispatch engine.rs）——诚实可得的偏差流是受信遥测的
//!    SOC 逐 tick 步长 `|soc_k − soc_{k−1}|`（遥测噪声统计语义，与
//!    DeadbandAdaptive 文档用途一致）。3σ 自适应抑制边界抖动（chatter）。
//! 2. **下游消费（R9 主判据）**：学到的半宽经 [`soc_band`]（E.4 单源）换算
//!    为 System1 充/放电阈值——`HeuristicBackend::set_deadband`，边界判定
//!    随之真实改变（同 SOC 提案前后对照可证）。
//! 3. **持久化**：参数快照 `Versioned` 记录入 [`RegionStateStore`]
//!    （LEARN_STATE 区段，CAS 代次；先例 = 业务状态区）；boot 恢复
//!    `restore_with_catalog`（EMA/计数原样回载）。
//! 4. **安全阀四件套**：①SafetyCatalog 硬钳（越界拒绝保留旧值）；
//!    ②`rejected_updates` 计数上行（LEARN-STATUS wire + 串口）；
//!    ③`learn_enabled` 配置闸（ai-peer 键；false ⇒ 零学习零持久，
//!    阈值恒出厂值 = 旧链路 byte-equal）；④learn-reset 回出厂动词。
//!    **durable 写失败 ⇒ 本次学习整步回滚**（内存不得独走，C2 判据）。
//!
//! 传输面 `Io: PersistIo`：EL0 = agent-sdk `BlockIo`（EIPC → svc-storage）；
//! host 测试 = 共享内存区段 + 故障注入包装。

use alloc::vec::Vec;

use eneros_dual_brain::learning::DeadbandAdaptive;
use eneros_durable_state::{
    Decode, DurableStateStore, Encode, Generation, PersistIo, RegionStateStore, StoreError,
};
use eneros_safety_validator::SafetyCatalog;

use crate::ai_worker::soc_band;

/// 快照 schema 版本（不识版本拒绝解码，不误读）。
const LEARN_SNAPSHOT_SCHEMA: u16 = 1;

/// SOC 域安全目录（半宽硬界唯一权威；值域登记 [`soc_band`] 常量）。
pub fn soc_catalog() -> SafetyCatalog {
    SafetyCatalog {
        deadband_min: soc_band::LEARN_MIN_HALF_WIDTH,
        deadband_max: soc_band::LEARN_MAX_HALF_WIDTH,
        max_correction: 0.5,
        clamp_power_min_kw: -100.0,
        clamp_power_max_kw: 100.0,
    }
}

/// 学习参数快照（RegionStateStore 记录；Encode/Decode 定长线码）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LearnSnapshot {
    /// 当前生效半宽（学习器 current）。
    pub current: f64,
    /// 噪声 EMA（学习器内部态——恢复用）。
    pub noise_ema: f64,
    /// 学习样本数。
    pub samples: u64,
    /// 越界拒绝计数（安全阀②）。
    pub rejected_updates: u64,
    /// 上一 tick 受信 SOC（None = 尚未锚定基线）。
    pub last_soc: Option<f64>,
}

impl Encode for LearnSnapshot {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::with_capacity(2 + 8 * 4 + 8 + 1);
        out.extend_from_slice(&LEARN_SNAPSHOT_SCHEMA.to_le_bytes());
        out.extend_from_slice(&self.current.to_le_bytes());
        out.extend_from_slice(&self.noise_ema.to_le_bytes());
        out.extend_from_slice(&self.samples.to_le_bytes());
        out.extend_from_slice(&self.rejected_updates.to_le_bytes());
        match self.last_soc {
            Some(s) => {
                out.push(1);
                out.extend_from_slice(&s.to_le_bytes());
            }
            None => out.push(0),
        }
        Ok(out)
    }
}

impl Decode for LearnSnapshot {
    fn decode(bytes: &[u8]) -> Result<(Generation, Self), StoreError> {
        // RegionStateStore payload 布局：generation(8) ‖ state——generation
        // 前缀由本 impl 消费（与 ledger/St 先例同源契约）；state 段首 2B =
        // schema 版本（未知拒绝，不误读）。
        if bytes.len() < 8 + 2 + 8 * 4 + 1 {
            return Err(StoreError::Corrupt);
        }
        let gen = u64::from_le_bytes(bytes[0..8].try_into().expect("checked"));
        let b = &bytes[8..];
        let schema = u16::from_le_bytes([b[0], b[1]]);
        if schema != LEARN_SNAPSHOT_SCHEMA {
            return Err(StoreError::Corrupt);
        }
        let mut off = 2usize;
        let take8 = |off: &mut usize| -> f64 {
            let v = f64::from_le_bytes(b[*off..*off + 8].try_into().expect("checked"));
            *off += 8;
            v
        };
        let current = take8(&mut off);
        let noise_ema = take8(&mut off);
        let samples = u64::from_le_bytes(b[off..off + 8].try_into().expect("checked"));
        off += 8;
        let rejected_updates = u64::from_le_bytes(b[off..off + 8].try_into().expect("checked"));
        off += 8;
        let last_soc = match b[off] {
            0 => None,
            1 => {
                off += 1;
                if off + 8 > b.len() {
                    return Err(StoreError::Corrupt);
                }
                Some(f64::from_le_bytes(
                    b[off..off + 8].try_into().expect("checked"),
                ))
            }
            _ => return Err(StoreError::Corrupt),
        };
        Ok((
            Generation(gen),
            LearnSnapshot {
                current,
                noise_ema,
                samples,
                rejected_updates,
                last_soc,
            },
        ))
    }
}

/// 单次观测结局（串口/wire 证据面词形来源）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LearnOutcome {
    /// 学习闸关闭——零学习零持久（C2 三态之一）。
    Disabled,
    /// 本样本仅锚定基线（首样本/非有限 SOC），未学习。
    Anchored,
    /// 提议生效且已持久化（`LEARN-APPLY p=`）。
    Applied { current: f64 },
    /// 提议越 catalog 界被拒、样本态已持久化（`LEARN-REJECT-CAP p=`）。
    RejectCap { proposed: f64 },
    /// durable 写失败——**整步回滚**（行为退回上一稳定值，内存不独走）。
    PersistFailed,
}

/// 有界死区学习器（持久化包装；宿主 svc-ai-worker）。
pub struct DeadbandLearner<Io: PersistIo> {
    store: RegionStateStore<LearnSnapshot, Io>,
    adaptive: DeadbandAdaptive,
    last_soc: Option<f64>,
    enabled: bool,
    /// 本 boot 内学习生效次数（观测；LEARN-STATUS wire 字段）。
    pub applies: u64,
    /// 本 boot 内持久化失败次数（观测）。
    pub persist_failures: u64,
}

impl<Io: PersistIo> DeadbandLearner<Io> {
    /// 打开：读快照恢复（损坏 ⇒ Err，调用方 fail-closed 转出厂行为）；
    /// `enabled=false` 时不恢复任何状态（零消费），阈值恒出厂。
    pub fn open(enabled: bool, io: Io) -> Result<Self, StoreError> {
        let mut store =
            RegionStateStore::new(io, learn_state_base_offset(), learn_state_region_len());
        store.open()?;
        let (adaptive, last_soc) = if enabled {
            match DurableStateStore::<LearnSnapshot>::load(&mut store)? {
                Some(v) => {
                    let s = v.state;
                    (
                        DeadbandAdaptive::restore_with_catalog(
                            s.current,
                            s.noise_ema,
                            s.samples,
                            s.rejected_updates,
                            soc_catalog(),
                        ),
                        s.last_soc,
                    )
                }
                None => (
                    DeadbandAdaptive::with_catalog(soc_band::FACTORY_HALF_WIDTH, soc_catalog()),
                    None,
                ),
            }
        } else {
            // 闸关：不消费快照（零消费），工厂值。
            (
                DeadbandAdaptive::with_catalog(soc_band::FACTORY_HALF_WIDTH, soc_catalog()),
                None,
            )
        };
        Ok(Self {
            store,
            adaptive,
            last_soc,
            enabled,
            applies: 0,
            persist_failures: 0,
        })
    }

    /// 学习闸状态（LEARN-STATUS wire `enabled` 位）。
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 当前下游阈值 `(charge_soc, discharge_soc)`——闸关 ⇒ 恒出厂值
    /// （C5 底线：关闭态行为 byte-equal 旧链路）。
    pub fn thresholds(&self) -> (f64, f64) {
        if !self.enabled {
            return (
                soc_band::charge_soc(soc_band::FACTORY_HALF_WIDTH),
                soc_band::discharge_soc(soc_band::FACTORY_HALF_WIDTH),
            );
        }
        (
            soc_band::charge_soc(self.adaptive.current),
            soc_band::discharge_soc(self.adaptive.current),
        )
    }

    /// 半宽硬界 `(min, max)`（catalog 透传——StatusQuery 观测面）。
    pub fn bounds(&self) -> (f64, f64) {
        (
            soc_band::LEARN_MIN_HALF_WIDTH,
            soc_band::LEARN_MAX_HALF_WIDTH,
        )
    }

    /// 样本/拒绝计数（StatusQuery 观测面）。
    pub fn counters(&self) -> (u64, u64) {
        (self.adaptive.samples, self.adaptive.rejected_updates)
    }

    /// 观测一帧受信 SOC：锚定 → 噪声学习 → catalog 钳 → CAS 持久化。
    ///
    /// 任一持久化失败 ⇒ 学习器整步回滚（EMA/计数/last_soc 原样），
    /// 阈值退回上一稳定值。
    pub fn observe(&mut self, soc: f64) -> LearnOutcome {
        if !self.enabled {
            return LearnOutcome::Disabled;
        }
        // 基线锚定：首帧/非有限输入不学习（fail-closed 输入门）。
        let prev = match self.last_soc {
            Some(p) if soc.is_finite() => p,
            _ => {
                self.last_soc = if soc.is_finite() { Some(soc) } else { None };
                if self.persist_snapshot(soc).is_err() {
                    self.persist_failures = self.persist_failures.saturating_add(1);
                }
                return LearnOutcome::Anchored;
            }
        };
        // 学习一步（EMA → 3σ 提议 → catalog 钳）。整步先在副本上走，
        // 持久化成功才提交——durable 失败 ⇒ 副本丢弃（内存不独走）。
        let mut candidate = self.adaptive.clone();
        let rejected_before = candidate.rejected_updates;
        let new_current = candidate.learn((soc - prev).abs());
        let proposed = 3.0 * candidate.noise_estimate();
        let cap_rejected = candidate.rejected_updates > rejected_before;
        let snapshot = LearnSnapshot {
            current: new_current,
            noise_ema: candidate.noise_estimate(),
            samples: candidate.samples,
            rejected_updates: candidate.rejected_updates,
            last_soc: Some(soc),
        };
        match self.cas_write(snapshot) {
            Ok(()) => {
                self.adaptive = candidate;
                self.last_soc = Some(soc);
                if cap_rejected {
                    LearnOutcome::RejectCap { proposed }
                } else {
                    self.applies = self.applies.saturating_add(1);
                    LearnOutcome::Applied {
                        current: new_current,
                    }
                }
            }
            Err(()) => {
                self.persist_failures = self.persist_failures.saturating_add(1);
                LearnOutcome::PersistFailed
            }
        }
    }

    /// learn-reset（EAQ-3）：参数回出厂 catalog 初值并持久化；
    /// 持久化失败 ⇒ 内存同样不回滚生效（fail-closed 保持旧态）。
    pub fn reset(&mut self) -> Result<(), StoreError> {
        let fresh = LearnSnapshot {
            current: soc_band::FACTORY_HALF_WIDTH,
            noise_ema: soc_band::FACTORY_HALF_WIDTH / 3.0,
            samples: 0,
            rejected_updates: 0,
            last_soc: self.last_soc,
        };
        self.cas_write_snapshot(&fresh).map(|_| {
            self.adaptive =
                DeadbandAdaptive::with_catalog(soc_band::FACTORY_HALF_WIDTH, soc_catalog());
            self.applies = 0;
        })
    }

    /// 学习器内部态快照（StatusQuery 观测面组装）。
    pub fn snapshot_view(&self) -> LearnSnapshot {
        LearnSnapshot {
            current: self.adaptive.current,
            noise_ema: self.adaptive.noise_estimate(),
            samples: self.adaptive.samples,
            rejected_updates: self.adaptive.rejected_updates,
            last_soc: self.last_soc,
        }
    }

    fn persist_snapshot(&mut self, soc: f64) -> Result<Generation, ()> {
        let snap = LearnSnapshot {
            last_soc: soc.is_finite().then_some(soc),
            ..self.snapshot_view()
        };
        self.cas_write_snapshot(&snap).map_err(|_| ())
    }

    fn cas_write(&mut self, snap: LearnSnapshot) -> Result<(), ()> {
        self.cas_write_snapshot(&snap).map(|_| ()).map_err(|_| ())
    }

    fn cas_write_snapshot(&mut self, snap: &LearnSnapshot) -> Result<Generation, StoreError> {
        let expected =
            DurableStateStore::<LearnSnapshot>::load(&mut self.store)?.map(|v| v.generation);
        self.store.compare_and_swap(expected, snap)
    }
}

/// LEARN_STATE 区段字节基址（ota-ab 单一事实源；×512 换算唯一 accessor）。
fn learn_state_base_offset() -> u64 {
    eneros_ota_ab::ab_partition::DataRegion::LearnState.base_byte()
}

/// LEARN_STATE 区段字节长度。
fn learn_state_region_len() -> u64 {
    eneros_ota_ab::ab_partition::DataRegion::LearnState.len_bytes()
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use core::cell::Cell;

    use super::*;

    /// 共享内存持久介质（断电重启模拟：同介质跨实例）。
    ///
    /// 偏移语义 = svc-storage 整盘视图：学习器以盘级绝对偏移读写，
    /// 本介质只承载 LEARN_STATE 区段窗口——访问按 `offset − base` 平移，
    /// 越出窗口即错（RegionScope 钳制语义的测试版）。
    struct MemPersist {
        media: Rc<core::cell::RefCell<Vec<u8>>>,
        base: u64,
        len: u64,
    }

    impl MemPersist {
        fn shared(len: u64) -> (Self, Rc<core::cell::RefCell<Vec<u8>>>) {
            let media = Rc::new(core::cell::RefCell::new(vec![0u8; len as usize]));
            (
                Self {
                    media: Rc::clone(&media),
                    base: learn_state_base_offset(),
                    len,
                },
                media,
            )
        }
    }

    impl PersistIo for MemPersist {
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
            let m = self.media.borrow();
            let start = offset
                .checked_sub(self.base)
                .filter(|off| off + buf.len() as u64 <= self.len)
                .ok_or(StoreError::Unavailable)? as usize;
            buf.copy_from_slice(&m[start..start + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
            let mut m = self.media.borrow_mut();
            let start = offset
                .checked_sub(self.base)
                .filter(|off| off + data.len() as u64 <= self.len)
                .ok_or(StoreError::WriteFailed)? as usize;
            m[start..start + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), StoreError> {
            Ok(())
        }
        fn capacity(&self) -> u64 {
            self.len
        }
    }

    /// 故障注入：第 `fail_at` 次写起永久失败（C2 durable 写失败注入）。
    struct FailAfterN {
        inner: MemPersist,
        writes: Cell<u64>,
        fail_at: u64,
    }

    impl FailAfterN {
        fn new(inner: MemPersist, fail_at: u64) -> Self {
            Self {
                inner,
                writes: Cell::new(0),
                fail_at,
            }
        }
    }

    impl PersistIo for FailAfterN {
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
            self.inner.read(offset, buf)
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
            let n = self.writes.get();
            self.writes.set(n + 1);
            if n + 1 >= self.fail_at {
                return Err(StoreError::WriteFailed);
            }
            self.inner.write(offset, data)
        }
        fn flush(&mut self) -> Result<(), StoreError> {
            Ok(())
        }
        fn capacity(&self) -> u64 {
            self.inner.capacity()
        }
    }

    const REGION: u64 = 4 * 1024;

    /// C1：持续正向噪声 ⇒ 半宽沿 EMA 单调逼近上界；反向序列对称回落。
    #[test]
    fn c1_noise_series_moves_half_width_monotonically() {
        let (io, _media) = MemPersist::shared(REGION);
        let mut l = DeadbandLearner::open(true, io).expect("open");
        assert_eq!(
            l.thresholds().0,
            soc_band::charge_soc(soc_band::FACTORY_HALF_WIDTH)
        );
        // 锚定基线。
        assert_eq!(l.observe(0.4), LearnOutcome::Anchored);
        // 正向：步长 0.06 的噪声序列（高于初始噪声估计 0.0333 = 初值/3，
        // EMA 上行 ⇒ 3σ 提议逐拍推高半宽，触 0.14 上界后 catalog 拒收）。
        let mut soc = 0.4;
        let mut prev_half = l.snapshot_view().current;
        for _ in 0..30 {
            soc += 0.06;
            let out = l.observe(soc);
            assert!(matches!(
                out,
                LearnOutcome::Applied { .. } | LearnOutcome::RejectCap { .. }
            ));
            let half = l.snapshot_view().current;
            assert!(
                half >= prev_half,
                "正向序列半宽必须单调不减: {half} < {prev_half}"
            );
            prev_half = half;
        }
        assert!(
            l.snapshot_view().current > soc_band::FACTORY_HALF_WIDTH,
            "持续噪声必须推高半宽"
        );
        // 反向：安静序列 ⇒ 半宽回落（EMA 衰减；下界 = catalog min 0.06）。
        let mut soc2 = soc;
        for _ in 0..400 {
            soc2 += 0.0001;
            let _ = l.observe(soc2);
        }
        assert!(
            l.snapshot_view().current < prev_half,
            "反向序列半宽必须回落"
        );
        assert!(
            l.snapshot_view().current >= soc_band::LEARN_MIN_HALF_WIDTH,
            "半宽绝不越 catalog 下界"
        );
    }

    /// C1：超界提议被 catalog 拒（旧值保持 + 计数），行为边界不变。
    #[test]
    fn c1_over_bound_proposal_rejected_with_count() {
        let (io, _) = MemPersist::shared(REGION);
        let mut l = DeadbandLearner::open(true, io).expect("open");
        let _ = l.observe(0.4);
        // 巨步噪声 ⇒ 3σ 提议越过 0.14 上界 ⇒ RejectCap。
        let mut soc = 0.4;
        for _ in 0..5 {
            soc += 0.5;
            let out = l.observe(soc);
            assert!(matches!(out, LearnOutcome::RejectCap { .. }), "got {out:?}");
        }
        assert_eq!(l.snapshot_view().rejected_updates, 5, "越界必须计数");
        assert_eq!(
            l.thresholds(),
            (
                soc_band::charge_soc(soc_band::FACTORY_HALF_WIDTH),
                soc_band::discharge_soc(soc_band::FACTORY_HALF_WIDTH)
            ),
            "被拒后边界必须保持出厂值"
        );
    }

    /// C2-a：learn_enabled=false ⇒ 零学习零持久，阈值恒出厂（旧链路等价）。
    #[test]
    fn c2_disabled_is_zero_write_zero_consume() {
        let (io, media) = MemPersist::shared(REGION);
        let mut l = DeadbandLearner::open(false, io).expect("open");
        let before = l.snapshot_view();
        assert_eq!(l.observe(0.4), LearnOutcome::Disabled);
        assert_eq!(l.observe(0.9), LearnOutcome::Disabled);
        assert_eq!(l.snapshot_view(), before, "闸关态观测不得改变任何状态");
        assert_eq!(
            l.thresholds(),
            (
                soc_band::charge_soc(soc_band::FACTORY_HALF_WIDTH),
                soc_band::discharge_soc(soc_band::FACTORY_HALF_WIDTH)
            ),
            "闸关阈值必须恒出厂值"
        );
        // 介质必须零写入（全零区 = 从未 CAS）。
        assert!(media.borrow().iter().all(|&b| b == 0), "闸关态零持久");
    }

    /// C2-b：durable 写失败 ⇒ 学习跳过 + 行为退回上一稳定值（内存不独走）。
    #[test]
    fn c2_persist_failure_rolls_back_to_stable_value() {
        let (io, media) = MemPersist::shared(REGION);
        {
            let mut l = DeadbandLearner::open(true, io).expect("open");
            let _ = l.observe(0.4); // Anchored：建立基线（1 次持久写）
            let mut soc = 0.4;
            for _ in 0..3 {
                soc += 0.02;
                let _ = l.observe(soc); // 稳定学习态（多次持久写）
            }
        }
        // 重开（读入稳定态），随后注入永久写失败。
        let read_ok = MemPersist {
            media: Rc::clone(&media),
            base: learn_state_base_offset(),
            len: REGION,
        };
        let l = DeadbandLearner::open(true, read_ok).expect("reopen");
        let stable_thresholds = l.thresholds();
        let stable_snapshot = l.snapshot_view();
        // fail_at = 1：本实例的第一次写即失败。
        let failing = FailAfterN::new(
            MemPersist {
                media: Rc::clone(&media),
                base: learn_state_base_offset(),
                len: REGION,
            },
            1,
        );
        let mut broken = DeadbandLearner::open(true, failing).expect("open (reads ok)");
        let out = broken.observe(0.9);
        assert_eq!(out, LearnOutcome::PersistFailed, "写失败必须显式报告");
        assert_eq!(
            broken.thresholds(),
            stable_thresholds,
            "写失败后阈值必须退回上一稳定值（内存不得独走）"
        );
        assert_eq!(broken.snapshot_view().samples, stable_snapshot.samples);
        assert_eq!(broken.persist_failures, 1, "持久化失败必须计数");
    }

    /// 跨重启：快照恢复——current/EMA/计数逐字段连续，阈值随之连续。
    #[test]
    fn restart_restores_state_and_thresholds() {
        let (io, media) = MemPersist::shared(REGION);
        let mut l = DeadbandLearner::open(true, io).expect("open");
        let _ = l.observe(0.4);
        let mut soc = 0.4;
        for _ in 0..10 {
            soc += 0.02;
            let _ = l.observe(soc);
        }
        let before = l.snapshot_view();
        let thresholds_before = l.thresholds();
        drop(l);
        // 重启：同介质新实例。
        let io2 = MemPersist {
            media: Rc::clone(&media),
            base: learn_state_base_offset(),
            len: REGION,
        };
        let l2 = DeadbandLearner::open(true, io2).expect("reopen");
        let after = l2.snapshot_view();
        assert_eq!(before.current, after.current, "半宽必须逐字节恢复");
        assert_eq!(before.noise_ema, after.noise_ema, "EMA 必须原样回载");
        assert_eq!(before.samples, after.samples);
        assert_eq!(before.rejected_updates, after.rejected_updates);
        assert_eq!(before.last_soc, after.last_soc);
        assert_eq!(thresholds_before, l2.thresholds(), "阈值必须随快照连续");
    }

    /// C4：learn-reset ⇒ 回出厂 catalog 初值并持久化；重启后仍为出厂值。
    #[test]
    fn c4_reset_restores_factory_and_persists() {
        let (io, media) = MemPersist::shared(REGION);
        let mut l = DeadbandLearner::open(true, io).expect("open");
        let _ = l.observe(0.4);
        let mut soc = 0.4;
        for _ in 0..20 {
            soc += 0.04;
            let _ = l.observe(soc);
        }
        assert!(l.snapshot_view().current != soc_band::FACTORY_HALF_WIDTH);
        l.reset().expect("reset");
        assert_eq!(
            l.thresholds(),
            (
                soc_band::charge_soc(soc_band::FACTORY_HALF_WIDTH),
                soc_band::discharge_soc(soc_band::FACTORY_HALF_WIDTH)
            )
        );
        assert_eq!(l.snapshot_view().samples, 0);
        drop(l);
        let io2 = MemPersist {
            media: Rc::clone(&media),
            base: learn_state_base_offset(),
            len: REGION,
        };
        let l2 = DeadbandLearner::open(true, io2).expect("reopen");
        assert_eq!(
            l2.snapshot_view().current,
            soc_band::FACTORY_HALF_WIDTH,
            "重置必须持久（重启后仍出厂值）"
        );
    }

    /// 快照线码往返 + 未知 schema 拒绝（KAT 同步面；decode 契约含
    /// generation(8B) 前缀——与 RegionStateStore payload 布局一致）。
    #[test]
    fn snapshot_wire_roundtrip_and_schema_guard() {
        let s = LearnSnapshot {
            current: 0.113,
            noise_ema: 0.037,
            samples: 42,
            rejected_updates: 7,
            last_soc: Some(0.61),
        };
        // 模拟 RegionStateStore payload：generation ‖ state。
        let mut payload = 7u64.to_le_bytes().to_vec();
        payload.extend_from_slice(&s.encode().expect("encode"));
        let (gen, back) = LearnSnapshot::decode(&payload).expect("decode");
        assert_eq!(gen, Generation(7), "generation 必须由 decode 消费回载");
        assert_eq!(back, s);
        // 未知 schema（state 段首 2B）拒绝。
        let mut bad = payload.clone();
        bad[8] = 9;
        assert!(LearnSnapshot::decode(&bad).is_err(), "未知 schema 必须拒绝");
        // 截断拒绝。
        assert!(LearnSnapshot::decode(&payload[..payload.len() - 1]).is_err());
        // 短于 generation 前缀拒绝。
        assert!(LearnSnapshot::decode(&payload[..6]).is_err());
    }
}
