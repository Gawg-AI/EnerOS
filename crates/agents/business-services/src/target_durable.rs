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

/// 54（WP-54.5）：durable outbox 附接唯一入口——grant 探测 → 设备打开 →
/// `AppendOutbox::open` 成败分流 → 显式 `Unavailable` 兜底与串口日志，
/// 四件事单实现（核心在 [`crate::outbox_attach::attach_in`]，host 三态可测）。
///
/// `storage_slot` 为该客户端对 svc-storage 的授予槽（多授予服务经
/// `eipc_resolve` 按名取，单授予服务即 [`eneros_agent_sdk::EIPC_EP_BASE_SLOT`]；
/// `granted=false` 时该值不被触达）。日志文案与收编前 bin 原文逐字一致。
#[allow(clippy::too_many_arguments)]
pub fn attach<M>(
    granted: bool,
    label: &str,
    storage_slot: u64,
    offset: u64,
    slot_len: usize,
    max_slots: usize,
) -> MaybeOutbox<M>
where
    M: Encode + Decode + Clone,
{
    let mut log = |line: &str| {
        eneros_runtime::println!("{}", line);
    };
    let opened = crate::outbox_attach::attach_in(
        granted,
        label,
        move || crate::block_io::eipc_block_io(storage_slot, 0, 5_000),
        offset,
        slot_len,
        max_slots,
        &mut log,
    );
    match opened {
        crate::outbox_attach::MaybeOutbox::Real(o) => MaybeOutbox::Real(o),
        crate::outbox_attach::MaybeOutbox::Unavailable(u) => MaybeOutbox::Unavailable(u),
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
