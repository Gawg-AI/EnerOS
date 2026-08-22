//! 49（WP-49.2 远程 dual-brain 传输面）：AI 请求分片组装 + 应答转发状态。
//!
//! svc-ai-worker 经 drv EXECUTE 子码（`agent-sdk::ai_relay`）把请求 JSON
//! 分片送达（单帧 PAYLOAD_CAP=64B 装不下 ~300B JSON）；本模块负责无序
//! 防御的顺序组装——BEGIN 重置、CONT 追加、END 定稿入插件 outbox。
//! **组装器不解释内容**（契约解释权在 intent-contract::peer / ai-worker）。

use alloc::vec::Vec;

use eneros_agent_sdk::ai_relay::{AI_REQUEST_MAX, AI_REQ_DATA_BEGIN_MAX, AI_REQ_DATA_MAX};

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

/// AI 请求分片组装器（有界：≤ AI_REQUEST_MAX 字节）。
#[derive(Default)]
pub struct RequestAssembler {
    buf: Vec<u8>,
    total: usize,
    active: bool,
}

impl RequestAssembler {
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
    pub fn begin(&mut self, total: u16, data: &[u8]) -> Result<(), AssembleError> {
        let total = total as usize;
        if total == 0 || total > AI_REQUEST_MAX || data.len() > AI_REQ_DATA_BEGIN_MAX {
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
        if data.len() > AI_REQ_DATA_MAX || self.buf.len() + data.len() > self.total {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_fragments_in_order() {
        let mut a = RequestAssembler::new();
        let payload = b"{\"req_id\":1".to_vec();
        let total = (payload.len() + b",\"v\":1}".len()) as u16;
        a.begin(total, &payload).expect("begin");
        a.cont(b",\"v\":1}").expect("cont");
        assert!(a.is_active());
        let out = a.end().expect("end");
        assert_eq!(out, b"{\"req_id\":1,\"v\":1}".to_vec());
        assert!(!a.is_active());
    }

    #[test]
    fn cont_without_begin_rejected() {
        let mut a = RequestAssembler::new();
        assert_eq!(a.cont(b"x"), Err(AssembleError::NoBegin));
        assert_eq!(a.end(), Err(AssembleError::NoBegin));
    }

    #[test]
    fn double_begin_restarts_discarding_stale() {
        let mut a = RequestAssembler::new();
        a.begin(10, b"abc").expect("begin");
        // 中途失败残留 → 新 BEGIN 隐式中止旧组装（新请求胜出）。
        a.begin(2, b"xy").expect("restart");
        assert_eq!(a.end().expect("end"), b"xy".to_vec());
    }

    #[test]
    fn overflow_aborts_assembly() {
        let mut a = RequestAssembler::new();
        a.begin(4, b"abc").expect("begin");
        // 越界追加 → 错误且组装被中止（后续 cont 报 NoBegin）。
        assert_eq!(a.cont(b"dxyz"), Err(AssembleError::TooLarge));
        assert!(!a.is_active());
        assert_eq!(a.cont(b"d"), Err(AssembleError::NoBegin));
    }

    #[test]
    fn oversized_total_and_zero_total_rejected() {
        let mut a = RequestAssembler::new();
        assert_eq!(
            a.begin((AI_REQUEST_MAX + 1) as u16, b"x"),
            Err(AssembleError::TooLarge)
        );
        assert_eq!(a.begin(0, b""), Err(AssembleError::TooLarge));
    }

    #[test]
    fn incomplete_end_reports_mismatch() {
        let mut a = RequestAssembler::new();
        a.begin(10, b"abc").expect("begin");
        a.end().expect_err("3 != 10 必须 Incomplete");
        // 中止后可重新开始（错误恢复面）。
        a.begin(3, b"xyz").expect("restart after abort");
        assert_eq!(a.end().expect("end"), b"xyz".to_vec());
    }
}
