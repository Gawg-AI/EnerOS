//! 传输协议（TP）泛型重组逻辑（蓝图 48 WP-48.4）.
//!
//! `>8B` 报文走多包传输——本模块实现**重组逻辑层**：分片序号/总数收集、
//! 乱序缓冲、重复去重、丢包超时判定。**字节层 BLOCKED**（TP.CM/TP.DT
//! 帧格式、PGN、序列号回绕语义待规约文本）：宿主解出抽象
//! [`TpFrame`] 后注入；规约到位后字节映射替换本模块的输入端，
//! 重组语义与负例测试不变。
//!
//! # 负例覆盖（蓝图 DoD：丢包/乱序）
//!
//! - 丢包：缺序号 → 永不 Complete → [`TpReassembler::expired`] 判 Lost；
//! - 乱序：非顺序到达 → 集合式收集仍可 Complete；
//! - 重复：同序号二次到达 → Duplicate（不污染集合）；
//! - 总数不一致：首帧定 total，后续帧冲突 → Error。

/// 抽象 TP 分片（字节解码 BLOCKED）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TpFrame {
    /// 序号（1 起；回绕语义 BLOCKED 待规约）。
    pub seq: u8,
    /// 总分片数（取首帧为准）。
    pub total: u8,
}

/// 重组状态。
#[derive(Debug)]
pub struct TpReassembler {
    total: Option<u8>,
    received: [bool; 255],
    got_count: u8,
}

/// 推入一帧的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpStatus {
    /// 已收齐（可交付上层）。
    Complete,
    /// 尚缺分片。
    InProgress,
    /// 重复帧（已收过该序号）。
    Duplicate,
}

/// 重组错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpError {
    /// 序号为 0（非法——序号 1 起）。
    InvalidSeq,
    /// 序号超出首帧声明的总数。
    SeqOutOfRange,
    /// 与首帧声明总数冲突。
    TotalMismatch,
    /// 丢包超时：会话存在且未收齐（[`TpReassembler::expired`] 判定）。
    Lost,
}

impl TpReassembler {
    /// 新建空重组器。
    pub fn new() -> Self {
        Self {
            total: None,
            received: [false; 255],
            got_count: 0,
        }
    }

    /// 是否已收齐。
    pub fn is_complete(&self) -> bool {
        self.total == Some(self.got_count)
    }

    /// 已收分片数。
    pub fn received_count(&self) -> u8 {
        self.got_count
    }

    /// 声明的总分片数。
    pub fn total(&self) -> Option<u8> {
        self.total
    }

    /// 推入一帧。
    pub fn push(&mut self, f: TpFrame) -> Result<TpStatus, TpError> {
        if f.seq == 0 {
            return Err(TpError::InvalidSeq);
        }
        match self.total {
            None => self.total = Some(f.total),
            Some(t) if t != f.total => return Err(TpError::TotalMismatch),
            Some(_) => {}
        }
        let total = self.total.unwrap_or(f.total);
        if f.seq > total {
            return Err(TpError::SeqOutOfRange);
        }
        if self.received[(f.seq - 1) as usize] {
            return Ok(TpStatus::Duplicate);
        }
        self.received[(f.seq - 1) as usize] = true;
        self.got_count += 1;
        if self.is_complete() {
            Ok(TpStatus::Complete)
        } else {
            Ok(TpStatus::InProgress)
        }
    }

    /// 丢包超时判定（宿主定时调用）：会话存在且未收齐 → `Some(Lost)`；
    /// 无会话或已收齐 → `None`。**不自动清会话**——清理由上层在处置
    /// （Abort/重传请求）后显式 [`Self::reset`]，避免静默丢证据。
    pub fn expired(&self) -> Option<TpError> {
        if self.total.is_some() && !self.is_complete() {
            Some(TpError::Lost)
        } else {
            None
        }
    }

    /// 重置会话（丢弃半成品）。
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl Default for TpReassembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 正常顺序到达 → Complete。
    #[test]
    fn in_order_assembly_completes() {
        let mut tp = TpReassembler::new();
        assert_eq!(
            tp.push(TpFrame { seq: 1, total: 3 }),
            Ok(TpStatus::InProgress)
        );
        assert_eq!(
            tp.push(TpFrame { seq: 2, total: 3 }),
            Ok(TpStatus::InProgress)
        );
        assert_eq!(
            tp.push(TpFrame { seq: 3, total: 3 }),
            Ok(TpStatus::Complete)
        );
        assert!(tp.is_complete());
    }

    /// DoD 负例·乱序：乱序到达仍可收齐。
    #[test]
    fn out_of_order_still_completes() {
        let mut tp = TpReassembler::new();
        assert_eq!(
            tp.push(TpFrame { seq: 3, total: 3 }),
            Ok(TpStatus::InProgress)
        );
        assert_eq!(
            tp.push(TpFrame { seq: 1, total: 3 }),
            Ok(TpStatus::InProgress)
        );
        assert_eq!(
            tp.push(TpFrame { seq: 2, total: 3 }),
            Ok(TpStatus::Complete)
        );
    }

    /// DoD 负例·丢包：缺 seq=2 永不收齐，expired() 判丢失。
    #[test]
    fn lost_packet_never_completes_and_expires() {
        let mut tp = TpReassembler::new();
        let _ = tp.push(TpFrame { seq: 1, total: 3 });
        let _ = tp.push(TpFrame { seq: 3, total: 3 });
        assert_eq!(tp.received_count(), 2);
        assert!(!tp.is_complete());
        // 宿主超时路径：会话存在且未收齐 → 判丢失。
        assert!(tp.expired().is_some());
        tp.reset();
        assert_eq!(tp.expired(), None);
    }

    /// 重复帧去重（不污染计数、不误判 Complete）。
    #[test]
    fn duplicate_frames_are_deduped() {
        let mut tp = TpReassembler::new();
        let _ = tp.push(TpFrame { seq: 1, total: 2 });
        assert_eq!(
            tp.push(TpFrame { seq: 1, total: 2 }),
            Ok(TpStatus::Duplicate)
        );
        assert_eq!(tp.received_count(), 1);
        assert_eq!(
            tp.push(TpFrame { seq: 2, total: 2 }),
            Ok(TpStatus::Complete)
        );
    }

    /// 总数冲突与越界负例。
    #[test]
    fn total_mismatch_and_out_of_range_rejected() {
        let mut tp = TpReassembler::new();
        let _ = tp.push(TpFrame { seq: 1, total: 3 });
        assert_eq!(
            tp.push(TpFrame { seq: 2, total: 5 }),
            Err(TpError::TotalMismatch)
        );
        assert_eq!(
            tp.push(TpFrame { seq: 4, total: 3 }),
            Err(TpError::SeqOutOfRange)
        );
        assert_eq!(
            tp.push(TpFrame { seq: 0, total: 3 }),
            Err(TpError::InvalidSeq)
        );
    }
}
