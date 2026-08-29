//! 蓝图 40 WP-40.6/40.7/40.8：唯一 `ControlDispatch`——从可信采集到
//! AI/算法决策、唯一安全授权、设备执行、ACK、物理读回和审计的正式
//! EL0 产品闭环唯一出口。
//!
//! 固定不变量（蓝图 40 §1）：
//!
//! 1. 所有决策路径只产生 [`ProposedControlPlan`]（无副作用），本 crate 的
//!    `DispatchEngine` 是唯一把计划变成设备写的入口（唯一 AuthorizedCommand
//!    issuer，经 [`eneros_control_gateway::ControlChain`]）；
//! 2. 任何前置失败零投递（校验/新鲜度/授权/预算/持久化失败 → `Err`）；
//! 3. 投递后超时/重启不能猜成功——进入 `Indeterminate` 并执行对账，
//!    不自动重放非幂等命令；
//! 4. `Ok` 只表示定义中的阶段成功（`DispatchReceipt.final_state` 是唯一
//!    成功判据：`Committed`）；
//! 5. transport ACK ≠ 物理成功——按命令语义读回（device-agent 独立
//!    readback），读回不符 → `Failed`；
//! 6. 任何拒绝/超时/不确定结果均审计；learning 只消费 `Committed`
//!    （调用方职责，见 receipt 状态）。
//!
//! 12 步链（WP-40.6 顺序）：proposal 校验 → telemetry 新鲜度 → actor/
//! capability/lease/fencing 验证 → safety catalog（chain.execute）→
//! rate/energy 预算 reserve → durable intent 写入 → DeliveryChannel 投递
//! （ControlEnvelope）→ device-agent AuthorizedWriter 二次验证 → transport
//! ACK → 独立 readback → commit 预算 + audit → 反馈给 Agent/twin/learning。

#![cfg_attr(not(test), no_std)]
#![allow(clippy::result_large_err)]

extern crate alloc;

pub mod channel;
pub mod dispatch;
pub mod durable_intent;
pub mod engine;
pub mod feedback;
pub mod intent;
pub mod journal;

#[cfg(feature = "sel4")]
pub use channel::Sel4EipcChannel;
pub use channel::{ChannelError, DeliveryChannel};
pub use dispatch::{
    AuthenticatedActor, ControlDispatch, DispatchError, DispatchReceipt, DispatchState,
};
pub use engine::{DispatchConfig, DispatchEngine};
pub use feedback::{FeedbackPointMapping, TrustedFeedbackBuilder, PROD_FEEDBACK_MAPPING};
pub use intent::{InMemoryIntentStore, IntentRecord, IntentStore, IntentStoreError};
pub use journal::{ExecutionJournal, ExecutionStatus, IdempotencyRecord, JournalError, JournalHit};
