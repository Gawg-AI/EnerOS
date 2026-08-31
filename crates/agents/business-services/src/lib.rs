//! EnerOS 蓝图 41 业务/运维 EL0 服务。
//!
//! 每个服务 = 真实领域逻辑模块（no_std，host 可测）+ `bin/svc-*` 薄壳
//! （`_start` → `control_service_main` 分块 EIPC 循环，40 模式）。
//!
//! 诚实性纪律（蓝图 41 §3）：
//! - 闭包返回 Ok / Vec push / 计数增加不再是 “real”；
//! - 外部副作用（持久介质/独立 peer/虚拟设备）未就绪时显式
//!   `Unavailable/Degraded`，不伪造成功；
//! - durable 状态经 [`eneros_durable_state`] 语义层（掉电恢复在 host
//!   负例中验证；target 存储句柄移交前显式 Unavailable）。

#![cfg_attr(not(test), no_std)]
#![allow(clippy::result_large_err)]

extern crate alloc;

pub mod agent_bus;
/// 41（WP-41.3 落盘）：durable-state PersistIo 的 EIPC 块客户端
/// （业务服务经 svc-storage 真落盘；host 可测）。
pub mod block_io;
pub mod cloud_sync;
pub mod config_service;
pub mod ledger;
pub mod market;

/// 蓝图 61 v0.314.31（[07] §3.4）：市场事实接入 adapter——经认证信封
/// （SM3 摘要 + SM2 验签）走同一 IngestionModule 合同；修订/过期纪律。
pub mod external_facts;
pub mod market_ingest;
// SPEC-C（WP-C.2）：history-query 分页执行器内核（RawIpc 泛型，host 可测）。
pub mod mgmt_history;
pub mod mgmt_rpc;
pub mod observability;
pub mod ota_service;
// WP-54.5：附接核心 host 可测（无 cfg 门）；target_durable 保持 target 门。
pub mod outbox_attach;
#[cfg(target_os = "none")]
pub mod target_durable;

pub use config_service::ConfigService;
pub use ledger::Ledger;
pub use market::MarketAdapter;
pub use ota_service::OtaService;

include!(concat!(env!("OUT_DIR"), "/config_bundle.rs"));

/// 服务侧共享：HeapAllocator（与 drv-svc 同源）。
pub use eneros_drv_svc::allocator::HeapAllocator;
/// 服务实例 epoch。
pub use eneros_drv_svc::instance_epoch;
/// 单调毫秒。
pub use eneros_drv_svc::now_ms;
