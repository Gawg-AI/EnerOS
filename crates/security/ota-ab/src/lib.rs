//! EnerOS A/B 分区 OTA 升级 (v0.123.0) — Phase 2 P2-K OTA 与 Recovery 第 1 版。
//!
//! 实现 A/B 双分区镜像级 OTA 升级机制：SM2/SM3 签名校验、原子切换、
//! 启动健康检查与失败自动回滚、分区表断电持久化（魔数 + CRC32）。
//!
//! # 交付物
//! - [`ab_partition`] — `PartitionSlot` / `PartitionState` / `Partition` / `AbPartitionOta`
//! - [`upgrade`] — `OtaError` / `UpgradeMeta` / `OtaManager` trait
//! - [`rollback`] — 回滚决策、`boot_health_check` 启动健康检查、分区表持久化
//! - [`kernel_ota`] — v0.163.0 内核 OTA 主逻辑（重启切换 + 热迁移）
//! - [`hot_migrate`] — v0.163.0 内核热迁移状态机与平台抽象层
//!
//! # no_std 合规
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；非测试代码不使用
//! `std::*` / `panic!` / `todo!` / `unimplemented!` / `unwrap()`；时间戳一律经
//! `now_ms` 参数传入。
//!
//! # 蓝图偏差声明（对应 spec D1~D5）
//! - **D1**：蓝图 `crate::current_time_ms()` 改为 `now_ms` 参数传入
//!   （`AbPartitionOta::new` / `boot_health_check`），no_std 环境无系统时钟。
//! - **D2**：蓝图 `verify_hash/verify_signature` 返回 `true` 的 stub 替换为真实
//!   SM3 增量哈希（eneros-crypto `Sm3Hasher`）+ SM2 验签（`sm2_verify`）；
//!   `AbPartitionOta::new` 相应增加设备根公钥 `Sm2PublicKey` 与后端存储参数。
//! - **D3**：crate 落位由蓝图 `crates/ota/` 调整为 `crates/security/ota-ab/`
//!   （仓库规范强制子系统归口；与 v0.113.0 Secure Boot 同链，镜像签名验证
//!   属启动安全范畴），包名 `eneros-ota-ab`。
//! - **D4**：`AbPartitionOta` 泛型化后端存储 `<D: BlockDevice>`
//!   （v0.23.0 eneros-storage），`write_chunk` 真实写盘、`verify` 真实回读；
//!   测试使用 `MockBlockDevice`。
//! - **D5**：断电恢复以分区表定长序列化 + CRC32（eneros-storage `crc32`）
//!   持久化到保留块交付（蓝图 §5.4 难点仅列"需能恢复"未给接口，此处补
//!   最小实现：`PartitionTable` + `save_to_device` / `load_from_device`）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod ab_partition;
pub mod hot_migrate;
pub mod kernel_ota;
pub mod rollback;
pub mod upgrade;

pub use ab_partition::{AbPartitionOta, Partition, PartitionSlot, PartitionState};
pub use hot_migrate::{
    MigrateContext, MigrateEngine, MigrateState, NullPlatform, PlatformAbstraction,
};
pub use kernel_ota::{HotMigrate, KernelImage, KernelOta, KernelOtaError, MigrateError};
pub use rollback::{boot_health_check, load_from_device, save_to_device, PartitionTable};
pub use upgrade::{OtaError, OtaManager, UpgradeMeta};
