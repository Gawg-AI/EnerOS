//! EnerOS 插件市场框架 (v0.166.0) — P3-J 生态开放.
//!
//! 实现插件签名验证、沙箱隔离、动态加载/卸载，第三方插件安全运行。
//!
//! # 核心模块
//!
//! - [`types`] — 插件包/清单/权限/状态等核心类型
//! - [`signature`] — SM2/SM3 签名验证（复用 eneros-crypto）
//! - [`sandbox`] — 沙箱隔离（内存/能力/IPC/CPU 配额）
//! - [`loader`] — ELF64 AArch64 动态加载器
//! - [`manager`] — 插件管理器（生命周期统一入口）
//!
//! # 安全模型（三重保护）
//!
//! 1. **签名验证**：插件必须经 SM2 签名 + SM3 哈希验证，由受信任 CA 签发
//! 2. **权限检查**：插件仅获得声明权限的子集，超额声明被拒绝
//! 3. **沙箱隔离**：独立内存区域 + 能力限制 + IPC/CPU 配额
//!
//! # 加载流程（蓝图 §4.2）
//!
//! ```text
//! 插件包(.epp) → 解析 Manifest → 验证签名 → 检查权限
//!   → 创建沙箱 → ELF 加载 → 获取入口 → 初始化 → 注册 → 运行
//! ```
//!
//! # 使用示例
//!
//! ```rust,no_run
//! use eneros_plugin::{PluginManagerImpl, PluginManager, SignatureVerifier};
//! use eneros_plugin::types::{PluginPackage, PluginManifest, PluginType, Certificate};
//! use eneros_crypto::rng::CsRng;
//! use eneros_crypto::sm2::{Sm2KeyPair, Sm2Signer};
//! use eneros_crypto::{sm3_hash, EcPoint};
//!
//! // 1. 生成密钥对（生产环境使用硬件 TRNG）
//! let mut rng = CsRng::new();
//! let kp = Sm2KeyPair::generate(&mut rng).unwrap();
//!
//! // 2. 创建管理器
//! let verifier = SignatureVerifier::new(vec![kp.public_key.clone()]);
//! let mut mgr = PluginManagerImpl::new(verifier, 0x4000_0000, 0x100_0000);
//!
//! // 3. 安装插件（需有效签名）
//! // let id = mgr.install(&package).unwrap();
//! // mgr.enable(id).unwrap();
//! ```
//!
//! # 偏差声明
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `kernel/src/plugin/` → `crates/kernel/plugin/`（eneros-plugin） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；插件管理属内核子系统 |
//! | **D2** | 蓝图 `docs/plugin/` 保持不变 | 记忆 §2.3.3：文档按方向分类，plugin 为新方向 |
//! | **D3** | 蓝图测试文件 `kernel/tests/plugin/` → src 内嵌 `#[cfg(test)]` | 项目惯例（v0.87.0~v0.165.0），不新增 tests/ 文件 |
//! | **D4** | 蓝图 `CryptoEngine` trait → 直接使用 eneros-crypto 函数 | 记忆 §5.5 禁止重复造轮子；eneros-crypto 已提供 sm3_hash/sm2_verify |
//! | **D5** | 蓝图 `Vec<PluginInfo>` → 保留（需 alloc） | no_std + alloc 模式，符合蓝图 §43.1 ②层规范 |
//! | **D6** | 蓝图 dlopen → 自研 ELF 解析器 | no_std 无 dlopen；蓝图 §5 技术交底已指明需内核 ELF loader |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 使用 `alloc::vec::Vec` / `alloc::string::String`，不使用 `std::*`。
//! 唯一依赖 eneros-crypto（workspace 内 path 依赖），零第三方依赖，
//! 零 unsafe，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod loader;
pub mod manager;
pub mod sandbox;
pub mod signature;
pub mod types;

pub use loader::{Elf64Header, LoadedPlugin, PluginLoader, ProgramHeader};
pub use manager::{PluginManager, PluginManagerImpl, PluginStats, MAX_PLUGINS};
pub use sandbox::{Capability, CpuQuota, IpcQuota, MemoryRegion, PluginSandbox};
pub use signature::{build_signed_payload, is_sdk_compatible, SignatureVerifier};
pub use types::{
    Certificate, Permission, PluginId, PluginInfo, PluginManifest, PluginPackage, PluginState,
    PluginType, SDK_VERSION,
};

/// 插件系统错误枚举.
///
/// 覆盖签名验证、权限检查、沙箱隔离、ELF 加载全部失败面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginError {
    /// SM2 签名验证失败（签名与数据不匹配）.
    SignatureInvalid,
    /// 签名格式无效（非 64 字节 r‖s）.
    InvalidSignatureFormat,
    /// 证书无效（公钥格式错误/过期）.
    InvalidCertificate,
    /// 签名者不受信任（不在受信任根列表中）.
    UntrustedSigner,
    /// 权限不足/权限声明过多.
    PermissionDenied,
    /// SDK 版本不兼容.
    IncompatibleSdk,
    /// 沙箱违规（内存越界/能力滥用）.
    SandboxViolation,
    /// 沙箱已停用.
    SandboxInactive,
    /// 资源配额耗尽（IPC/CPU）.
    QuotaExceeded,
    /// 插件崩溃.
    PluginCrashed,
    /// 插件未找到.
    NotFound,
    /// ELF 格式无效.
    InvalidElfFormat,
    /// 不支持的 CPU 架构（非 AArch64）.
    UnsupportedArchitecture,
    /// 沙箱内存不足.
    OutOfMemory,
    /// 已达最大插件数限制.
    MaxPluginsReached,
    /// 无效的状态转换.
    InvalidStateTransition,
}
