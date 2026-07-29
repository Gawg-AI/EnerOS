//! EnerOS TPM 2.0 驱动 (v0.159.0).
//!
//! 提供 TPM 2.0 芯片驱动、PCR 度量、密钥管理和平台完整性验证。
//! 为 v0.160.0 Secure Boot 提供可信根。
//!
//! # 架构
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │  eneros-tpm (v0.159.0)                                  │
//! │                                                         │
//! │  ┌───────────────────────────────────────────────────┐  │
//! │  │ TpmMeasurement (度量服务)                          │  │
//! │  │  - measure_boot_chain                             │  │
//! │  │  - verify_integrity                               │  │
//! │  │  - generate_quote (远程证明)                       │  │
//! │  └───────────────────────┬───────────────────────────┘  │
//! │                          │                              │
//! │  ┌───────────────────────▼───────────────────────────┐  │
//! │  │ PcrManager + KeyManager                           │  │
//! │  │  - PCR read/extend/verify                         │  │
//! │  │  - Key create/sign/seal/unseal                    │  │
//! │  └───────────────────────┬───────────────────────────┘  │
//! │                          │                              │
//! │  ┌───────────────────────▼───────────────────────────┐  │
//! │  │ TpmDriver trait                                   │  │
//! │  │  ┌─────────────┐  ┌─────────────┐                 │  │
//! │  │  │ SpiTpmDriver│  │ SimulatedTpm│                 │  │
//! │  │  │ (Hardware)  │  │ (Testing)   │                 │  │
//! │  │  └─────────────┘  └─────────────┘                 │  │
//! │  └───────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # 使用示例
//! ```
//! use eneros_tpm::{SimulatedTpm, TpmDriver, TpmMeasurement, BootStage};
//!
//! // 创建模拟 TPM（开发/测试环境）
//! let mut tpm = SimulatedTpm::new();
//! tpm.init().unwrap();
//!
//! // 度量启动链
//! let mut measurement = TpmMeasurement::new(&mut tpm);
//! measurement.measure(BootStage::Bootloader, b"boot code", "firmware").unwrap();
//! measurement.measure(BootStage::Kernel, b"kernel image", "linux").unwrap();
//!
//! // 获取度量日志
//! assert_eq!(measurement.log_count(), 2);
//! ```
//!
//! # PCR 分配策略
//! | PCR | 用途 | 度量内容 |
//! |-----|------|---------|
//! | 0 | 启动代码 | Bootloader/Firmware |
//! | 1 | 内核配置 | 设备树/启动参数 |
//! | 2 | 用户态 | 运行时/驱动 |
//! | 4 | 内核镜像 | Kernel binary |
//! | 7 | Secure Boot | 安全启动策略 |
//! | 8+ | 应用 | Agent 组件 |
//!
//! # no_std 合规
//! 所有代码 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod commands;
pub mod driver;
pub mod error;
pub mod key_management;
pub mod measurement;
pub mod pcr;

// Re-export primary types
pub use commands::{
    TpmCommandBuilder, TpmCommandHeader, TpmResponseHeader, PCR_COUNT, PCR_DIGEST_SIZE,
    TPM_ALG_SM2, TPM_ALG_SM3_256, TPM_CC_PCR_EXTEND, TPM_CC_PCR_READ, TPM_RC_SUCCESS,
};
pub use driver::{KeyHandle, SimulatedTpm, SpiTpmDriver, TpmDriver};
pub use error::TpmError;
pub use key_management::{KeyHierarchy, KeyManager};
pub use measurement::{BootStage, MeasurementEntry, QuoteReport, TpmMeasurement};
pub use pcr::{PcrManager, PcrStrategy, PcrVerifyResult, PCR_BOOT, PCR_SECURE_BOOT};

/// Crate version string.
pub const VERSION: &str = "0.159.0";
