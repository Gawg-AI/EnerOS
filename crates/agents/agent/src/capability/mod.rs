//! 能力令牌（Capability Token）模块 (v0.40.0).
//!
//! 提供 Agent 访问控制的能力令牌系统，包含：
//! - [`CapabilityToken`] — 令牌主体（SM2 签名）
//! - [`CapabilityTokenBuilder`] — 令牌构建器（Builder 模式 + SM2 签名）
//! - [`TokenVerifier`] — 令牌验证器
//! - [`TokenStore`] — 令牌存储（双索引：主表 + by_owner）
//! - [`CapabilityManager`] — 能力管理器（签发/校验/冻结/撤销/过期清理）
//! - [`ResourceTarget`] / [`PermissionSet`] / [`ConstraintPack`] — 令牌字段类型
//!
//! # 偏差声明
//! - v0.39.0 D1~D13：详见 `token.rs` 模块文档
//! - v0.40.0 D1~D10：详见 `store.rs` 与 `manager.rs` 模块文档

pub mod builder;
pub mod manager;
pub mod store;
pub mod token;
pub mod verifier;

// Re-export key types for convenience.
pub use builder::CapabilityTokenBuilder;
pub use manager::CapabilityManager;
pub use store::TokenStore;
pub use token::{
    CapabilityToken, ConstraintPack, ConstraintType, DeviceId, PermissionSet, ResourceTarget,
    SocketAddr, SystemResource,
};
pub use verifier::TokenVerifier;
