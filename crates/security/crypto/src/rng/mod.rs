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

pub use csrng::CsRng;
