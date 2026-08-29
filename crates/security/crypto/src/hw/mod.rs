//! 国密硬件加速模块 (workspace v0.192.0).
//!
//! 提供 SM2/SM3/SM4 硬件加密引擎驱动和统一抽象层。
//! 支持飞腾 D2000 和鲲鹏 920 KAE 双平台硬件加速，
//! 并自动 fallback 到纯 Rust 软件实现。
//!
//! # 架构
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │  hw module (workspace v0.192.0)                                   │
//! │                                                         │
//! │  ┌───────────────────────────────────────────────────┐  │
//! │  │ CryptoSelector (引擎选择器)                        │  │
//! │  │  - 硬件优先 + 自动 fallback                       │  │
//! │  │  - 故障检测 + 降级策略                            │  │
//! │  └───────────────────────┬───────────────────────────┘  │
//! │                          │                              │
//! │  ┌───────────────────────▼───────────────────────────┐  │
//! │  │ CryptoEngine trait (统一接口)                      │  │
//! │  │  - sm2_sign / sm2_verify                          │  │
//! │  │  - sm3_hash                                       │  │
//! │  │  - sm4_encrypt / sm4_decrypt                      │  │
//! │  └───┬───────────────────┬───────────────────┬───────┘  │
//! │      │                   │                   │          │
//! │  ┌───▼───────┐  ┌───────▼───────┐  ┌───────▼───────┐  │
//! │  │ Phytium   │  │ Kunpeng       │  │ Software      │  │
//! │  │ D2000     │  │ 920 KAE       │  │ (Pure Rust)   │  │
//! │  │ (MMIO)    │  │ (Queue)       │  │ (Fallback)    │  │
//! │  └───────────┘  └───────────────┘  └───────────────┘  │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # 使用示例
//! ```
//! use eneros_crypto::hw::{CryptoSelector, SoftwareCryptoEngine, Sm4Mode};
//!
//! // 创建选择器并注册软件引擎（开发环境无硬件）
//! let mut selector = CryptoSelector::new();
//! selector.register(Box::new(SoftwareCryptoEngine::new()));
//!
//! // SM3 哈希
//! let hash = selector.sm3_hash(b"hello").unwrap();
//! assert_eq!(hash.len(), 32);
//!
//! // SM4-CBC 加解密
//! let key = [0x42u8; 16];
//! let iv = [0x00u8; 16];
//! let ct = selector.sm4_encrypt(&key, Some(&iv), Sm4Mode::Cbc, b"hello world12345").unwrap();
//! let pt = selector.sm4_decrypt(&key, Some(&iv), Sm4Mode::Cbc, &ct).unwrap();
//! assert_eq!(pt, b"hello world12345");
//! ```
//!
//! # 平台支持
//! | 平台 | 引擎 | 接口 | 性能 |
//! |------|------|------|------|
//! | 飞腾 D2000 | PhytiumCryptoEngine | MMIO 寄存器 | SM3 > 1Gbps |
//! | 鲲鹏 920 | KunpengKaeEngine | KAE 队列 | SM3 > 1.5Gbps |
//! | 通用 | SoftwareCryptoEngine | 纯 Rust | SM3 ~100Mbps |
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。

pub mod engine;
pub mod kunpeng;
pub mod phytium;
pub mod software;

use alloc::boxed::Box;

// Re-export primary types
pub use engine::{Algorithm, CryptoEngine, CryptoSelector, EngineStats, Sm4Mode};
pub use kunpeng::KunpengKaeEngine;
pub use phytium::{DmaBuffer, PhytiumCryptoEngine};
pub use software::SoftwareCryptoEngine;

// ============================================================
// Convenience: create a default selector with software fallback
// ============================================================

/// 创建默认加密引擎选择器（仅软件引擎）.
///
/// 适用于开发环境或无硬件加速的场景。
/// 生产环境应使用 `create_selector_with_hw` 注册硬件引擎。
pub fn create_software_selector() -> CryptoSelector {
    let mut selector = CryptoSelector::new();
    selector.register(Box::new(SoftwareCryptoEngine::new()));
    selector
}

/// 创建带硬件加速的加密引擎选择器.
///
/// 按优先级注册：
/// 1. 飞腾 D2000 硬件引擎（如果可用）
/// 2. 鲲鹏 920 KAE 引擎（如果可用）
/// 3. 软件 fallback（始终可用）
///
/// # Safety
/// 硬件引擎初始化涉及 MMIO 访问，调用者需确保对应硬件存在。
/// 在 QEMU/开发环境中，使用 `create_software_selector` 代替。
pub unsafe fn create_selector_with_hw(try_phytium: bool, try_kunpeng: bool) -> CryptoSelector {
    let mut selector = CryptoSelector::new();

    // 尝试注册飞腾 D2000
    if try_phytium {
        let phytium = PhytiumCryptoEngine::new();
        if phytium.is_available() {
            selector.register(Box::new(phytium));
        }
    }

    // 尝试注册鲲鹏 920 KAE
    if try_kunpeng {
        let kunpeng = KunpengKaeEngine::new();
        if kunpeng.is_available() {
            selector.register(Box::new(kunpeng));
        }
    }

    // 始终注册软件 fallback
    selector.register(Box::new(SoftwareCryptoEngine::new()));
    selector
}

/// 创建模拟模式选择器（用于测试）.
///
/// 注册模拟硬件引擎 + 软件引擎，验证 fallback 逻辑。
///
/// **仅测试/显式 `simulated` feature**：release 构建不含本构造器
/// （`#[cfg(any(test, feature = "simulated"))]`，防止模拟硬件静默进入
/// 生产路径；SPEC-SERIES-2026-08-27/A · WP-A.3）。
#[cfg(any(test, feature = "simulated"))]
pub fn create_simulated_selector() -> CryptoSelector {
    let mut selector = CryptoSelector::new();
    selector.register(Box::new(PhytiumCryptoEngine::new_simulated()));
    selector.register(Box::new(SoftwareCryptoEngine::new()));
    selector
}

// ============================================================
// Tests
// ============================================================

/// 模拟硬件引擎行为测试（WP-A.3 收编：自 `tests/hw_engine_test.rs` 迁入——
/// `new_simulated`/`create_simulated_selector` 已门控，集成测试默认构建不再可见）。
#[cfg(test)]
mod tests_sim;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_software_selector() {
        let mut sel = create_software_selector();
        assert_eq!(sel.engine_count(), 1);
        assert_eq!(sel.active_engine_name(), "Software Crypto (Pure Rust)");
        assert!(!sel.is_active_hardware());

        let hash = sel.sm3_hash(b"test").unwrap();
        assert_eq!(hash, crate::sm3::hash(b"test"));
    }

    #[test]
    fn test_create_simulated_selector() {
        let mut sel = create_simulated_selector();
        assert_eq!(sel.engine_count(), 2);
        // 模拟硬件引擎优先
        assert_eq!(sel.active_engine_name(), "Phytium D2000 Crypto Engine");
        assert!(sel.is_active_hardware());

        // 功能验证
        let hash = sel.sm3_hash(b"hello").unwrap();
        assert_eq!(hash, crate::sm3::hash(b"hello"));
    }

    #[test]
    fn test_full_workflow() {
        let mut sel = create_simulated_selector();

        // SM3
        let hash = sel.sm3_hash(b"workflow test").unwrap();
        assert_eq!(hash.len(), 32);

        // SM4-ECB
        let key = [0xABu8; 16];
        let data = b"secret data 1234"; // 16 bytes
        let ct = sel.sm4_encrypt(&key, None, Sm4Mode::Ecb, data).unwrap();
        let pt = sel.sm4_decrypt(&key, None, Sm4Mode::Ecb, &ct).unwrap();
        assert_eq!(pt, data);

        // SM4-CBC
        let iv = [0x00u8; 16];
        let ct_cbc = sel
            .sm4_encrypt(&key, Some(&iv), Sm4Mode::Cbc, data)
            .unwrap();
        let pt_cbc = sel
            .sm4_decrypt(&key, Some(&iv), Sm4Mode::Cbc, &ct_cbc)
            .unwrap();
        assert_eq!(pt_cbc, data);

        // SM2
        use crate::rng::CsRng;
        use crate::sm2::Sm2KeyPair;
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let sk = kp.private_key.to_bytes();
        let pk = kp.public_key.to_bytes_uncompressed();

        let msg = b"sign this message";
        let sig = sel.sm2_sign(&sk, msg).unwrap();
        let valid = sel.sm2_verify(&pk, msg, &sig).unwrap();
        assert!(valid);
    }

    #[test]
    fn test_engine_listing() {
        let sel = create_simulated_selector();
        let engines = sel.list_engines();
        assert_eq!(engines.len(), 2);
        assert!(engines[0].1); // 第一个是硬件
        assert!(!engines[1].1); // 第二个是软件
    }
}
