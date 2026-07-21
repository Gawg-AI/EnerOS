//! TCG 事件日志（v0.114.0，D7/D8）.
//!
//! 度量即存证（蓝图 §5.2）：[`TcgEventLog::measure`] = `sm3(data)` 摘要 +
//! `tpm.pcr_extend` + 事件追挂，三步原子完成（extend 失败显式传播不追挂，
//! D10② 禁吞错）。验证方凭日志 [`replay`](TcgEventLog::replay) 从零值链式
//! 重放重算 PCR，与 Quote 中的 PCR 值比对判定启动链完整性（蓝图 §4.4）。
//!
//! 蓝图 `load_event_log()` 未定义全局函数已移除（D8）：日志由调用方显式
//! 持有传递，no_std 无全局状态。

use alloc::vec::Vec;

use eneros_crypto::sm3_hash;

use crate::tpm::{pcr_extend_value, TpmBackend, PCR_COUNT};
use crate::TpmError;

/// TCG 事件（一条度量存证记录）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcgEvent {
    /// 度量目标 PCR 索引（0~23）.
    pub pcr_index: u8,
    /// 事件类型（集成层自定义，如 0x8001=BL / 0x8002=Kernel / 0x8003=Runtime）.
    pub event_type: u32,
    /// 事件数据摘要（`sm3(event_data)`，度量时写入 PCR 的值）.
    pub digest: [u8; 32],
    /// 事件原始数据（如镜像内容或镜像标识，审计用，蓝图 §9 可维护）.
    pub event_data: Vec<u8>,
}

/// TCG 事件日志（有序事件序列 + PCR 重放）.
///
/// 字段私有（D8）：经 `new` / `measure` 构造，`events` 访问器只读。
pub struct TcgEventLog {
    /// 有序事件序列（度量顺序即启动顺序）.
    events: Vec<TcgEvent>,
}

impl TcgEventLog {
    /// 构造空日志.
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// crate 内部构造：从既有事件序列组装（验证方自一致性检查重放
    /// 证明方提交的日志用，D11）.
    pub(crate) fn from_events(events: Vec<TcgEvent>) -> Self {
        Self { events }
    }

    /// 度量即存证（蓝图 §5.2）：
    ///
    /// 1. `digest = sm3_hash(data)`
    /// 2. `tpm.pcr_extend(pcr_index, &digest)?`（错误显式传播，不追挂事件）
    /// 3. 追挂 [`TcgEvent`]（event_data 克隆存证）
    pub fn measure<T: TpmBackend>(
        &mut self,
        tpm: &mut T,
        pcr_index: u8,
        event_type: u32,
        data: &[u8],
    ) -> Result<(), TpmError> {
        let digest = sm3_hash(data);
        tpm.pcr_extend(pcr_index, &digest)?;
        self.events.push(TcgEvent {
            pcr_index,
            event_type,
            digest,
            event_data: data.to_vec(),
        });
        Ok(())
    }

    /// 重放重算全部 24 个 PCR（D7）：
    ///
    /// 从零值起对每个事件按 [`pcr_extend_value`] 链式重放；索引 ≥ 24 的
    /// 事件无法经 `measure` 产生（extend 已拦截），防御性跳过。
    pub fn replay(&self) -> [[u8; 32]; PCR_COUNT] {
        let mut pcrs = [[0u8; 32]; PCR_COUNT];
        for event in &self.events {
            let idx = event.pcr_index as usize;
            if idx < PCR_COUNT {
                pcrs[idx] = pcr_extend_value(&pcrs[idx], &event.digest);
            }
        }
        pcrs
    }

    /// 事件序列只读访问器.
    pub fn events(&self) -> &[TcgEvent] {
        &self.events
    }

    /// 事件数量.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 是否为空日志.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl Default for TcgEventLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use eneros_crypto::CsRng;

    use super::*;
    use crate::tpm::SoftTpm;

    /// 构造 SoftTpm + 空日志 + CsRng.
    fn make_env() -> (SoftTpm, TcgEventLog, CsRng) {
        let mut rng = CsRng::new();
        let tpm = SoftTpm::new(&mut rng);
        (tpm, TcgEventLog::new(), rng)
    }

    // ============================================================
    // LOG7：measure 追加 + extend（蓝图 §5.2 度量即存证）
    // ============================================================

    /// LOG7 measure 后日志 1 事件（字段正确）且 TPM bank 同步更新；
    /// extend 失败（越界）时不追挂事件.
    #[test]
    fn log7_measure_appends_and_extends() {
        let (mut tpm, mut log, _rng) = make_env();
        let data = b"bl-image";
        assert_eq!(log.measure(&mut tpm, 0, 0x8001, data), Ok(()));
        // 日志 1 事件，字段正确
        assert_eq!(log.len(), 1);
        assert!(!log.is_empty());
        let event = &log.events()[0];
        assert_eq!(event.pcr_index, 0);
        assert_eq!(event.event_type, 0x8001);
        assert_eq!(event.digest, sm3_hash(data));
        assert_eq!(event.event_data, data);
        // TPM bank 同步更新
        assert_eq!(
            tpm.pcr_read(0),
            Ok(pcr_extend_value(&[0u8; 32], &sm3_hash(data)))
        );
        // extend 失败（越界）→ 显式 Err 且不追挂
        assert_eq!(
            log.measure(&mut tpm, 24, 0x8001, data),
            Err(TpmError::InvalidPcrIndex)
        );
        assert_eq!(log.len(), 1, "extend 失败不应追挂事件");
    }

    // ============================================================
    // LOG8：空日志 replay 全零
    // ============================================================

    /// LOG8 空日志 replay == [[0u8;32];24]；len/is_empty 语义正确.
    #[test]
    fn log8_empty_log_replay_all_zero() {
        let log = TcgEventLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert_eq!(log.replay(), [[0u8; 32]; PCR_COUNT]);
    }

    // ============================================================
    // LOG9：replay == SoftTpm bank（蓝图 §6.4 PCR 值一致性回归）
    // ============================================================

    /// LOG9 同一串 measure（含同 PCR 多次度量）后，replay 与 SoftTpm
    /// pcr_read 全部 24 个 PCR 逐值相等.
    #[test]
    fn log9_replay_matches_softtpm_bank() {
        let (mut tpm, mut log, _rng) = make_env();
        // 三级镜像分别度量到 PCR0/1/2
        assert_eq!(log.measure(&mut tpm, 0, 0x8001, b"bl-image"), Ok(()));
        assert_eq!(log.measure(&mut tpm, 1, 0x8002, b"kernel-image"), Ok(()));
        assert_eq!(log.measure(&mut tpm, 2, 0x8003, b"runtime-image"), Ok(()));
        // 同一 PCR 二次度量（链式）
        assert_eq!(log.measure(&mut tpm, 0, 0x8004, b"bl-config"), Ok(()));
        let replayed = log.replay();
        for idx in 0..PCR_COUNT as u8 {
            assert_eq!(
                tpm.pcr_read(idx),
                Ok(replayed[idx as usize]),
                "PCR{} replay 应与 TPM bank 一致",
                idx
            );
        }
    }
}
