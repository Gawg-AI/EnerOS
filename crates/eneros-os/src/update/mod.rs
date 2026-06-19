//! OTA update with A/B partition
//!
//! v0.22.0 部署与 OTA 更新模块，提供：
//! - A/B 分区管理（槽位切换、持久化、boot count、health 状态）
//! - Ed25519 签名的 OTA 更新包（manifest + signer）
//! - 完整 OTA 流程（下载→校验→写入→切换→回滚）
//! - 声明式机器配置（eneros-machine.yaml）

pub mod ab_partition;
pub mod error;
pub mod machine_config;
pub mod manifest;
pub mod ota;
pub mod signer;

pub use ab_partition::{AbPartition, Slot, SlotStatus};
pub use error::UpdateError;
pub use machine_config::{
    BootSpec, HardwareSpec, MachineConfig, NetworkSpec, PartitionLayout,
};
pub use manifest::{ImageEntry, UpdateManifest};
pub use ota::{OtaConfig, OtaManager, UpdateBundleInfo};
pub use signer::{SigningKey, VerifyingKey};
