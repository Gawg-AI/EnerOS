//! Cryptographically Secure Random Number Generator (CSRNG).
//!
//! Based on SM3 Hash DRBG (NIST SP 800-90A style).
//!
//! # 设计概述
//! 基于 SM3 杂凑算法的确定性随机比特生成器（DRBG），状态为 256-bit `V` + 64-bit 计数器。
//! 每生成一个 32 字节块：`V = SM3(V || counter)`，输出 `V`。
//!
//! # 安全警告
//! **WARNING**: 本实现使用确定性种子，仅适用于 no_std 测试环境。
//! 生产环境必须接入硬件 TRNG 作为熵源，通过 `CsRng::from_seed` 传入硬件采集的熵。
//! 相同种子将产生相同的输出序列——切勿在未注入硬件熵的情况下用于生产密码学用途。

pub mod csrng;

pub use csrng::{fill_bytes_checked, fill_bytes_or_reseed, CsRng, DrbgError, RESEED_INTERVAL};

pub mod entropy;
pub mod entropy_health;
pub mod virtio_rng;
pub use entropy::{
    csrng_from_platform, csrng_from_platform_checked, platform_entropy_seed,
    platform_entropy_seed_policy, platform_entropy_source, EntropyError, EntropySource,
};
// 70.12（WP-70.12.4）：熵源健康判据与有界轮询（生产装配在取种前必须跑）。
pub use entropy_health::{fill_bounded, HealthPolicy, HealthTest, HealthVerdict};
