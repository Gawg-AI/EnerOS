//! crate 内共享测试假件（cfg(test)——跨模块复用，防第二份抄本）。
//!
//! 蓝图 64 卷 64.2（WP-64.2.4）：mqtt_io 的 host 测试需要 chunked 服务端
//! mock——与 ingestion_stage 测试的 ChunkServer 同族，按消费面拆分：
//! [`FixedRespServer`] 固定应答（转发环三分支），ingestion_stage 侧
//! obs 语义 mock 留在原处（语义耦合其词形/停批断言）。

use alloc::vec::Vec;

use eneros_agent_sdk::chunked::{
    ack_bytes, is_pull_request, PagedServerV2, CHUNK_MAGIC_FINAL, CHUNK_MAGIC_HEADER,
};
use eneros_agent_sdk::{chunked::RawIpc, AgentError};

/// 固定应答 chunked 服务端 mock：任意完整传输（Header→ACK / Final→分页头 /
/// 拉取→分页数据）完成后回预置字节。`failing()` 时 Header 即拒（客户端
/// → Err 传输分支）。
pub struct FixedRespServer {
    resp: Vec<u8>,
    pages: PagedServerV2,
    fail_transport: bool,
    /// 完成传输计数（断言用）。
    pub transfers: usize,
}

impl FixedRespServer {
    /// 正常服务端：Final 后回 `resp`。
    pub fn responding(resp: Vec<u8>) -> Self {
        Self {
            resp,
            pages: PagedServerV2::new(),
            fail_transport: false,
            transfers: 0,
        }
    }

    /// 传输失败服务端：Header 应答非 ACK（客户端 chunk 状态机拒绝）。
    pub fn failing() -> Self {
        Self {
            resp: Vec::new(),
            pages: PagedServerV2::new(),
            fail_transport: true,
            transfers: 0,
        }
    }
}

impl RawIpc for FixedRespServer {
    fn request(
        &mut self,
        _target: &str,
        payload: &[u8],
        _timeout_ns: u64,
    ) -> Result<Vec<u8>, AgentError> {
        if is_pull_request(payload) || (payload.len() == 4 && payload[0..4] == *CHUNK_MAGIC_FINAL) {
            let is_final = payload.len() == 4 && payload[0..4] == *CHUNK_MAGIC_FINAL;
            if is_final {
                self.transfers += 1;
            }
            return self
                .pages
                .on_request(payload, self.resp.clone())
                .ok_or(AgentError::Timeout);
        }
        if payload.len() >= 4 && payload[0..4] == *CHUNK_MAGIC_HEADER {
            // 事务标识随 Header epoch 绑定（v2 Pull 回带校验的供页前提）。
            if let Ok(h) = eneros_agent_sdk::chunked::ChunkHeader::decode(payload) {
                self.pages.set_txn(h.epoch);
            }
            if self.fail_transport {
                return Ok(alloc::vec![0xDE, 0xAD, 0xBE, 0xEF]);
            }
            return Ok(ack_bytes().to_vec());
        }
        Err(AgentError::Timeout)
    }

    fn send(&mut self, _target: &str, _payload: &[u8]) -> Result<(), AgentError> {
        Ok(())
    }
}
