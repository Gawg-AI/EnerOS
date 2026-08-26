//! EXECUTE 载荷中继面（agent-sdk 共享模块，蓝图 59 C4）。
//!
//! 单一事实：AI 请求分片（ai_relay）与遥测帧分片（telemetry_relay）曾各持
//! 一份逐行相同的 BEGIN/CONT/END 分片算法；svc-mqtt 内双 handler 各写一遍
//! 分发；组装器住在 protocol-services。本模块把「一个中继面 = 子码三元组 +
//! 上限 + 分片 + 分类 + 组装」收归一处——下一个中继面（如 log/config 推送）
//! 声明即得全套，wire 子码与帧格式不变（ADR-0025：同 bin Rust）。
//!
//! 域差异（END 定稿语义：AI 发布应答主题 vs 遥测帧头三重校验）留在调用方
//! ——面只管传输，不解释内容。

use alloc::vec::Vec;

/// BEGIN 首帧数据容量（drv 帧 PAYLOAD_CAP=64 − 子码 1B − total u16 2B）。
pub const DATA_BEGIN_MAX: usize = 64 - 3;
/// CONT 帧数据容量（PAYLOAD_CAP=64 − 子码 1B）。
pub const DATA_MAX: usize = 64 - 1;

/// 分片失败（空载荷/超限）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentError;

/// 组装错误（bin 据此回 ST_INVALID + 计数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssembleError {
    /// 未以 BEGIN 开始。
    NoBegin,
    /// 总长超限或追加越界。
    TooLarge,
    /// END 时长度不符（实际 < BEGIN 声明）。
    Incomplete,
}

/// 中继面声明：子码三元组 + 组装总长上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayFaceSpec {
    /// 分片首帧子码（载荷 = [begin][total u16 LE][data ≤ DATA_BEGIN_MAX]）。
    pub begin: u8,
    /// 中继帧子码（载荷 = [cont][data ≤ DATA_MAX]）。
    pub cont: u8,
    /// 收尾帧子码（载荷 = [end]）。
    pub end: u8,
    /// 组装总长上限（超限即中止 + 计数）。
    pub max_total: usize,
}

/// 一帧的解析结果（服务侧分发视图）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentStep<'a> {
    /// 首帧：声明总长 + 首片数据。
    Begin {
        /// 声明总长。
        total: u16,
        /// 首片数据。
        data: &'a [u8],
    },
    /// 中继帧数据。
    Cont {
        /// 追加数据。
        data: &'a [u8],
    },
    /// 收尾（组装定稿交调用方）。
    End,
}

impl RelayFaceSpec {
    /// 载荷 → 分片帧序列（唯一分片算法；帧不含 EXECUTE opcode——发送方
    /// drv_call 侧再拼）。发送方按序 drv_call；中途失败直接放弃（接收方
    /// BEGIN 隐式中止残片）。
    pub fn fragment(&self, payload: &[u8]) -> Result<(u16, Vec<Vec<u8>>), FragmentError> {
        if payload.is_empty() || payload.len() > self.max_total {
            return Err(FragmentError);
        }
        let total = payload.len() as u16;
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut first = Vec::with_capacity(3 + DATA_BEGIN_MAX);
        first.push(self.begin);
        first.extend_from_slice(&total.to_le_bytes());
        let head = core::cmp::min(payload.len(), DATA_BEGIN_MAX);
        first.extend_from_slice(&payload[..head]);
        frames.push(first);
        let mut off = head;
        while off < payload.len() {
            let end = core::cmp::min(off + DATA_MAX, payload.len());
            let mut f = Vec::with_capacity(1 + (end - off));
            f.push(self.cont);
            f.extend_from_slice(&payload[off..end]);
            frames.push(f);
            off = end;
        }
        frames.push(alloc::vec![self.end]);
        Ok((total, frames))
    }

    /// 服务侧子码分类。`None` = 子码不属本面（或 BEGIN 帧短于声明头——
    /// 与原 svc-mqtt 手写分发「`if payload.len() >= 3` 否则落入不消费」
    /// 语义逐位一致）。
    pub fn classify<'a>(&self, payload: &'a [u8]) -> Option<FragmentStep<'a>> {
        let code = *payload.first()?;
        match code {
            c if c == self.begin && payload.len() >= 3 => {
                let total = u16::from_le_bytes([payload[1], payload[2]]);
                Some(FragmentStep::Begin {
                    total,
                    data: &payload[3..],
                })
            }
            c if c == self.begin => None,
            c if c == self.cont => Some(FragmentStep::Cont {
                data: &payload[1..],
            }),
            c if c == self.end => Some(FragmentStep::End),
            _ => None,
        }
    }
}

/// 顺序组装器（自 protocol-services::ai_relay 迁入，行为逐位；有界
/// ≤ 面声明上限——经 [`RelayFaceSpec::assemble`] 使用）。
#[derive(Default)]
pub struct RelayAssembler {
    buf: Vec<u8>,
    total: usize,
    active: bool,
}

impl RelayAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// BEGIN：重置并声明总长，携带首片数据。
    ///
    /// 已有在途组装 → **隐式中止后重启**（新请求胜出；发送方中途失败
    /// 留下的残片不得卡死后续请求——49 QEMU 实弹语义）。
    pub fn begin(
        &mut self,
        total: u16,
        data: &[u8],
        max_total: usize,
    ) -> Result<(), AssembleError> {
        let total = total as usize;
        if total == 0 || total > max_total || data.len() > DATA_BEGIN_MAX {
            self.abort();
            return Err(AssembleError::TooLarge);
        }
        self.buf.clear();
        self.buf.extend_from_slice(data);
        self.total = total;
        self.active = true;
        Ok(())
    }

    /// CONT：追加分片（越界即中止组装——fail-closed）。
    pub fn cont(&mut self, data: &[u8]) -> Result<(), AssembleError> {
        if !self.active {
            return Err(AssembleError::NoBegin);
        }
        if data.len() > DATA_MAX || self.buf.len() + data.len() > self.total {
            self.abort();
            return Err(AssembleError::TooLarge);
        }
        self.buf.extend_from_slice(data);
        Ok(())
    }

    /// END：定稿。长度与 BEGIN 声明不符 → 中止并报错。
    pub fn end(&mut self) -> Result<Vec<u8>, AssembleError> {
        if !self.active {
            return Err(AssembleError::NoBegin);
        }
        if self.buf.len() != self.total {
            self.abort();
            return Err(AssembleError::Incomplete);
        }
        self.active = false;
        Ok(core::mem::take(&mut self.buf))
    }

    /// 中止当前组装（错误恢复面）。
    pub fn abort(&mut self) {
        self.buf.clear();
        self.total = 0;
        self.active = false;
    }
}

impl RelayFaceSpec {
    /// 服务侧一步组装：分类 + 喂入组装器。`Ok(None)` = 子码不属本面；
    /// `Ok(Some(Ok(())))` = 已消费（BEGIN/CONT）；`Ok(Some(Err))` = 组装
    /// 错误（已消费，调用方计数回 ST_INVALID）；END 由调用方经
    /// [`RelayAssembler::end`] 定稿（域语义不同——蓝图 59 A4）。
    pub fn assemble(
        &self,
        payload: &[u8],
        assembler: &mut RelayAssembler,
    ) -> Option<Result<(), AssembleError>> {
        match self.classify(payload)? {
            FragmentStep::Begin { total, data } => {
                Some(assembler.begin(total, data, self.max_total))
            }
            FragmentStep::Cont { data } => Some(assembler.cont(data)),
            // END 定稿语义属调用方域；这里仅表示「已消费，属本面」。
            FragmentStep::End => Some(Ok(())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACE: RelayFaceSpec = RelayFaceSpec {
        begin: 0x2A,
        cont: 0x2B,
        end: 0x2C,
        max_total: 1024,
    };
    const OTHER: RelayFaceSpec = RelayFaceSpec {
        begin: 0x3A,
        cont: 0x3B,
        end: 0x3C,
        max_total: 1024,
    };

    #[allow(dead_code)] // 面矩阵断言经 assembler 路径重组；保留供后续
                        // 面扩展测试直接复用（fragment 帧序不变量对照）。
    fn reassemble(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&frames[0][3..]);
        for f in &frames[1..frames.len() - 1] {
            out.extend_from_slice(&f[1..]);
        }
        out
    }

    #[test]
    fn fragment_classify_assemble_round_trip_both_faces() {
        // 蓝图 59 核心：任一面声明即得同一算法——发送侧 fragment 与
        // 接收侧 classify+assemble 互逆（原双份算法收敛后的不变量）。
        for face in [FACE, OTHER] {
            let payload: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8 + 1).collect();
            let (total, frames) = face.fragment(&payload).expect("fragment");
            assert_eq!(total as usize, payload.len());
            let mut asm = RelayAssembler::new();
            for f in &frames {
                assert!(face.assemble(f, &mut asm).expect("owned").is_ok());
            }
            assert_eq!(asm.end().expect("end"), payload);
        }
    }

    #[test]
    fn fragment_rejects_empty_and_oversize() {
        assert_eq!(FACE.fragment(&[]), Err(FragmentError));
        assert_eq!(FACE.fragment(&[0u8; 1025]), Err(FragmentError));
    }

    #[test]
    fn classify_skips_foreign_codes_and_short_begin() {
        // 他面子码不消费；BEGIN 帧短于声明头不消费（原手写分发语义）。
        assert_eq!(FACE.classify(&[OTHER.begin, 1, 2, 3]), None);
        assert_eq!(FACE.classify(&[FACE.begin, 1]), None);
        assert_eq!(FACE.classify(&[0x99]), None);
        assert_eq!(FACE.classify(&[]), None);
        assert!(matches!(
            FACE.classify(&[FACE.begin, 4, 0, b'a']),
            Some(FragmentStep::Begin { total: 4, .. })
        ));
    }

    #[test]
    fn double_begin_restarts_discarding_stale() {
        let mut a = RelayAssembler::new();
        assert!(FACE
            .assemble(&[FACE.begin, 10, 0, b'a'], &mut a)
            .expect("owned")
            .is_ok());
        assert!(FACE
            .assemble(&[FACE.begin, 1, 0, b'x'], &mut a)
            .expect("owned")
            .is_ok());
        assert_eq!(a.end().expect("end"), b"x".to_vec());
    }

    #[test]
    fn overflow_aborts_and_cont_without_begin_rejected() {
        let mut a = RelayAssembler::new();
        assert_eq!(a.cont(b"x"), Err(AssembleError::NoBegin));
        assert_eq!(a.end(), Err(AssembleError::NoBegin));
        assert!(a.begin(4, b"abc", 1024).is_ok());
        assert_eq!(a.cont(b"dxyz"), Err(AssembleError::TooLarge));
        assert!(!a.is_active());
        assert_eq!(a.cont(b"d"), Err(AssembleError::NoBegin));
    }

    #[test]
    fn oversized_total_zero_total_and_incomplete_end() {
        let mut a = RelayAssembler::new();
        assert_eq!(a.begin(1025, b"x", 1024), Err(AssembleError::TooLarge));
        assert_eq!(a.begin(0, b"", 1024), Err(AssembleError::TooLarge));
        assert!(a.begin(10, b"abc", 1024).is_ok());
        assert_eq!(a.end(), Err(AssembleError::Incomplete));
        // 中止后可重新开始。
        assert!(a.begin(3, b"xyz", 1024).is_ok());
        assert_eq!(a.end().expect("end"), b"xyz".to_vec());
    }

    #[test]
    fn frame_capacity_within_payload_cap() {
        const PAYLOAD_CAP: usize = 64;
        const _: () = assert!(1 + 2 + DATA_BEGIN_MAX <= PAYLOAD_CAP);
        const _: () = assert!(DATA_MAX < PAYLOAD_CAP);
    }
}
