//! 41（WP-41.3 落盘）：target 侧真实 durable 后端与显式 Unavailable 的
//! 二态封装。
//!
//! 存储能力经 EIPC 授予时使用 `AppendOutbox` / `RegionStateStore`（BlockIo
//! 真落盘）；未授予时使用 `UnavailableOutbox` / `UnavailableStateStore`
//! 显式 fail-closed，绝不退回内存假实现。

#![cfg(target_os = "none")]

use alloc::vec::Vec;

use eneros_agent_sdk::ipc_sel4::Sel4IpcClient;
use eneros_durable_state::{
    AppendOutbox, Decode, DurableOutbox, DurableStateStore, Encode, Generation, IdempotencyKey,
    OutboxId, PeerReceipt, Pending, RegionStateStore, StoreError, UnavailableOutbox,
    UnavailableStateStore, Versioned,
};

use crate::block_io::BlockIo;

/// 市场/云同步/可观测等 outbox 的二态封装。
pub enum MaybeOutbox<M> {
    Real(AppendOutbox<M, BlockIo<Sel4IpcClient>>),
    Unavailable(UnavailableOutbox<M>),
}

impl<M: Encode + Decode + Clone> DurableOutbox<M> for MaybeOutbox<M> {
    fn enqueue(&mut self, msg: &M, idempotency: IdempotencyKey) -> Result<OutboxId, StoreError> {
        match self {
            MaybeOutbox::Real(o) => o.enqueue(msg, idempotency),
            MaybeOutbox::Unavailable(u) => u.enqueue(msg, idempotency),
        }
    }
    fn mark_acked(&mut self, id: OutboxId, peer_receipt: &PeerReceipt) -> Result<(), StoreError> {
        match self {
            MaybeOutbox::Real(o) => o.mark_acked(id, peer_receipt),
            MaybeOutbox::Unavailable(u) => u.mark_acked(id, peer_receipt),
        }
    }
    fn mark_sent(&mut self, id: OutboxId, at_ms: u64) -> Result<(), StoreError> {
        match self {
            MaybeOutbox::Real(o) => o.mark_sent(id, at_ms),
            MaybeOutbox::Unavailable(u) => u.mark_sent(id, at_ms),
        }
    }
    fn recover_pending(&mut self) -> Result<Vec<Pending<M>>, StoreError> {
        match self {
            MaybeOutbox::Real(o) => o.recover_pending(),
            MaybeOutbox::Unavailable(u) => u.recover_pending(),
        }
    }
}

/// AgentBus/账本等 state store 的二态封装。
pub enum MaybeState<S> {
    Real(RegionStateStore<S, BlockIo<Sel4IpcClient>>),
    Unavailable(UnavailableStateStore<S>),
}

impl<S: Encode + Decode + Clone> DurableStateStore<S> for MaybeState<S> {
    fn load(&mut self) -> Result<Option<Versioned<S>>, StoreError> {
        match self {
            MaybeState::Real(s) => s.load(),
            MaybeState::Unavailable(u) => u.load(),
        }
    }
    fn compare_and_swap(
        &mut self,
        expected: Option<Generation>,
        next: &S,
    ) -> Result<Generation, StoreError> {
        match self {
            MaybeState::Real(s) => s.compare_and_swap(expected, next),
            MaybeState::Unavailable(u) => u.compare_and_swap(expected, next),
        }
    }
}
