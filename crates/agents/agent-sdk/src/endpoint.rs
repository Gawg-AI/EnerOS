//! 蓝图 62 卷 62.2 WP-62.2.4（S4）：chunked 端点单源——epoch 只有一个
//! 递增点。
//!
//! 单一事实：此前 `ChunkedClient::new` 非测试调用点 ×22、bins+root
//! service.rs 的 epoch `saturating_add` 手递增 ×14（mqtt 一 bin 背 3 组
//! epoch 字段 + 绑定块 + 递增）。结构性风险 = 新消费点漏递增/错递增即
//! 触发服务端 epoch 拒绝（重放守卫失效或误伤）。本类型把
//! "client + epoch" 收拢为一个端点：epoch 私有，递增只发生在
//! [`ChunkedEndpoint::call`] 内——消费方**结构性不可能**漏递增。
//!
//! [`ChunkedEndpoint::bind`] 同源收编 EIPC 建端三行闭包
//! （`eipc_resolve` + `Sel4IpcClient::new` + `ok()`），未授予 → None
//! （fail-closed 语义不变）。
//!
//! 禁改项：chunked wire（104B/块、16 块上限）；epoch 初值 0 与回绕语义
//! （saturating）；`Sel4IpcClient` 本体接口。

use alloc::vec::Vec;

use crate::chunked::{ChunkError, ChunkedClient, RawIpc};

/// chunked Call 默认超时（原各消费方手传 5_000ms，值不变）。
pub const CHUNKED_CALL_TIMEOUT_MS: u64 = 5_000;

/// chunked 端点：`client + epoch` 单一持有者。
///
/// `C = Sel4IpcClient`（target）或 mock `RawIpc`（host 测试）。
pub struct ChunkedEndpoint<C: RawIpc> {
    client: C,
    /// 发送方 epoch（重启计数；**私有**——服务端 epoch 不匹配拒绝的
    /// 重放守卫，递增纪律由本类型单点承载）。
    epoch: u64,
}

impl<C: RawIpc> ChunkedEndpoint<C> {
    /// 包装既有客户端（epoch 从 0 起——与原字段初值语义一致）。
    pub fn wrap(client: C) -> Self {
        Self { client, epoch: 0 }
    }

    /// 包装既有客户端并指定起始 epoch（root 侧跨函数共享 epoch 计数器的
    /// 迁移面：调用前读 `*epoch` 作初值、调用后 [`Self::epoch`] 写回——
    /// 递增点仍只在 [`Self::call`]，共享语义逐字节保真）。
    pub fn wrap_at(client: C, epoch: u64) -> Self {
        Self { client, epoch }
    }

    /// 当前 epoch（只读观测；消费方无递增面——递增仅 [`Self::call`]）。
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// 一次 chunked Call（Header/Data/Final/分页拉取）。
    ///
    /// epoch 每次调用**恰好 +1**（与传输成败无关——原各消费方"建
    /// transfer 即递增"语义保真：传输失败后旧 epoch 序号不得复用，
    /// 服务端半组装缓冲按 epoch 拒迟到帧）。
    pub fn call(&mut self, target: &str, req: &[u8]) -> Result<Vec<u8>, ChunkError> {
        let mut transfer =
            ChunkedClient::new(&mut self.client, self.epoch, CHUNKED_CALL_TIMEOUT_MS);
        self.epoch = self.epoch.saturating_add(1);
        transfer.transfer(target, req)
    }

    /// 非 chunked 的单次 Call 面（config_entry / config_generation 等
    /// `Sel4IpcClient` 直呼方法）——epoch 不经过此面，无递增语义。
    pub fn client_mut(&mut self) -> &mut C {
        &mut self.client
    }

    /// 测试专用 epoch 观测（生产消费方无递增可能——编译期由私有保证）。
    #[cfg(test)]
    pub fn epoch_for_test(&self) -> u64 {
        self.epoch
    }
}

#[cfg(feature = "sel4")]
impl ChunkedEndpoint<crate::ipc_sel4::Sel4IpcClient> {
    /// EIPC 端点绑定（原各消费方
    /// `eipc_resolve(service, target).and_then(|(_, slot)| Sel4IpcClient::
    /// new(label, slot, now_ns).ok())` 三行闭包单源）。未授予/建端失败
    /// → `None`（fail-closed，语义同原闭包）。
    pub fn bind(service: &str, target: &str) -> Option<Self> {
        let (_, slot) = crate::eipc_resolve(service, target)?;
        let client =
            crate::ipc_sel4::Sel4IpcClient::new(target, slot, eneros_drv_svc::now_ns).ok()?;
        Some(Self::wrap(client))
    }
}

// ============================================================================
// host 红测→定义锁：epoch 每次调用恰好 +1（wire 抓包断言）
// ============================================================================

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;
    use crate::chunked::{ChunkHeader, CHUNK_MAGIC_HEADER};

    #[test]
    fn call_increments_epoch_exactly_once_per_call() {
        let mut ep = ChunkedEndpoint::wrap(SpyIpc::new());
        let _ = ep.call("svc-x", &[1, 2, 3]);
        let _ = ep.call("svc-x", &[4, 5]);
        let _ = ep.call("svc-x", &[6]);
        assert_eq!(ep.epoch_for_test(), 3, "三次 call → epoch 恰好 +3");
        // wire 抓包：三次 Header 帧的 epoch 字段 = 0, 1, 2。
        assert_eq!(ep.client.header_epochs, alloc::vec![0, 1, 2]);
    }

    #[test]
    fn wrap_at_preserves_shared_counter_and_epoch_is_read_only_view() {
        let mut ep = ChunkedEndpoint::wrap(SpyIpc::new());
        let _ = ep.call("svc-x", &[1]);
        assert_eq!(ep.epoch(), 1);
        // 共享计数器写回面：wrap_at 以写回值为初值，继续递增序列。
        let mut ep2 = ChunkedEndpoint::wrap_at(SpyIpc::new(), ep.epoch());
        let _ = ep2.call("svc-x", &[2]);
        assert_eq!(ep2.epoch(), 2, "共享计数序列 0→1→2 连续不重置");
    }

    #[test]
    fn epoch_not_reused_after_transport_error() {
        let mut ep = ChunkedEndpoint::wrap(SpyIpc::rejecting());
        assert!(ep.call("svc-x", &[1]).is_err(), "mock 拒绝 → Err");
        assert!(ep.call("svc-x", &[2]).is_err());
        assert_eq!(
            ep.epoch_for_test(),
            2,
            "失败调用同样消耗 epoch（原语义：建 transfer 即递增）"
        );
    }

    // ---- 简化 mock（本文件专用；完整 chunked 服务端语义见
    //      protocol-services ingestion_stage 测试）----
    struct SpyIpc {
        reject: bool,
        header_epochs: Vec<u64>,
    }
    impl SpyIpc {
        fn new() -> Self {
            Self {
                reject: false,
                header_epochs: Vec::new(),
            }
        }
        fn rejecting() -> Self {
            Self {
                reject: true,
                header_epochs: Vec::new(),
            }
        }
    }
    impl RawIpc for SpyIpc {
        fn request(
            &mut self,
            _target: &str,
            payload: &[u8],
            _timeout_ns: u64,
        ) -> Result<Vec<u8>, crate::AgentError> {
            if payload.len() >= 4 && payload[0..4] == *CHUNK_MAGIC_HEADER {
                if let Ok(h) = ChunkHeader::decode(payload) {
                    self.header_epochs.push(h.epoch);
                }
            }
            if self.reject {
                // Header ack 非法 → ServerRejected。
                return Ok(alloc::vec![0xDE, 0xAD, 0xBE, 0xEF]);
            }
            Ok(alloc::vec![b'E', b'N', b'V', b'K'])
        }
        fn send(&mut self, _target: &str, _payload: &[u8]) -> Result<(), crate::AgentError> {
            Ok(())
        }
    }
}
