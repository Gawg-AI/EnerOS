//! 70.12（方向 6 WP-70.12.1/70.12.3）：板级事实账本与平台事实校验。
//!
//! # 为什么有这个 crate
//! `product/skus/*.json` 冻结记录此前只有「值 + verified 布尔 + 来源串」三列
//! （`xtask/src/catalog.rs` 的 `BoardFactFreeze` v1）。方向 6 需要的判据是
//! **可核对的**：某个子系统缺失时同族事实不得声明 `verified`、内存区不得
//! 重叠、宣称硬件实证必须带摘要、声明 `kind=product` 的组合必须覆盖硬门
//! 能力族。这些规则此前只在文档里，没有任何机器判据。
//!
//! 本 crate 是这些规则的**唯一实现**（纯逻辑、`no_std + alloc`）：
//! - xtask 在 `catalog validate/gate` 时用同一套函数裁决冻结记录；
//! - 目标侧 root 用 [`platform::vouch_selection`] 核对「构建期所选 SKU ×
//!   运行时描述摘要」是否成立，不匹配即拒绝继续装配。
//!
//! # 边界（诚实声明）
//! 本 crate 判的是**记录与描述的一致性**，不是硬件本身。真实板级事实由
//! 厂商手册与实板 bring-up 提供（`hardware_scope` 字段 + 摘要 + 器械绑定），
//! 缺失时一律 `verified=false` 并登记进 `missing`——禁止用 QEMU 地址或
//! 默认容量填空。

#![cfg_attr(not(test), no_std)]
// 测试断言允许 panic!（elf-loader/ota-ab 先例：`disallowed_macros` 是生产代码纪律）。
#![cfg_attr(test, allow(clippy::disallowed_macros))]

extern crate alloc;

pub mod capability;
pub mod digest;
pub mod fact;
pub mod ledger;
pub mod memory;
pub mod platform;
pub mod registry;

pub use capability::{CapabilityRequirement, SkuKind, Subsystem};
pub use fact::{Fact, FactError, HardwareScope, UNIT_NONE};
pub use ledger::{BoardLedger, LedgerError, LedgerReport};
pub use memory::{MemorySpan, MEM_ADDR_UNKNOWN};
pub use platform::{
    platform_role, required_device_subsystems, vouch_development, vouch_selection, BoardSelection,
    PlatformRole, PlatformVouch, PlatformVouchError, SelectionError,
};
pub use registry::{
    assembly_selection, AdapterRegistry, BoardAdapter, BoardRefusal, RegisteredAdapter,
};
