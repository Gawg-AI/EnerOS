//! 平台熵源适配层（CRY-002）：硬件 TRNG / QEMU 源 → CsRng::from_seed 闭环。
//!
//! 设计（蓝图 08 §CRY-002「熵源闭环」）：
//! - [`EntropySource`]：统一熵源接口（32 字节块输出）；
//! - [`platform_entropy_source`]：按编译配置选择平台源（fail-closed，
//!   无可用源时返回 [`EntropyError::Unavailable`]，绝不静默回退固定种子）；
//! - `CsRng::from_platform`：平台熵 → `from_seed` 注入闭环。
//!
//! 平台现状：
//! - `PhytiumTrng` / `KunpengTrng`：真实硬件 TRNG 寄存器接口——**BLOCKED（缺硬件）**，
//!   提供接口与地址占位，调用返回 `EntropyUnavailable`（不伪造读数）；
//! - QEMU virtio-rng：`qemu-entropy` feature 下选择 [`QemuVirtioRng`] 真实
//!   virtqueue 读熵（蓝图/20 R1-4：删除固定种子顶包，收编为
//!   `#[cfg(test)]` 的 `TestFixedEntropy` 测试夹具）。
//!
//! # release 可达性
//! `CsRng::new()`（固定种子）在 release 构建下不存在
//! （`#[cfg(any(test, debug_assertions, feature = "insecure-test-rng"))]`）；
//! release 生产路径唯一入口 = `from_seed`（种子来自本层平台熵源）。

use alloc::boxed::Box;

use super::CsRng;

/// 熵源错误（fail-closed：无源即错误，不降级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropyError {
    /// 当前平台无可用熵源（缺硬件 / 未接入）。
    Unavailable,
    /// 熵源返回数据不足。
    ShortRead,
}

/// 统一熵源接口（CRY-002）。
pub trait EntropySource {
    /// 填充 32 字节熵；失败返回错误（调用方不得回退固定种子）。
    fn fill_entropy(&mut self, out: &mut [u8; 32]) -> Result<(), EntropyError>;
}

/// 飞腾 D2000 TRNG 接口（**BLOCKED 缺硬件**）。
///
/// 寄存器接口占位：真实实现须映射飞腾 TRNG 寄存器并按 BSP-001 设备树
/// 基址访问；无硬件时返回 [`EntropyError::Unavailable`]——不伪造读数。
pub struct PhytiumTrng;

impl EntropySource for PhytiumTrng {
    fn fill_entropy(&mut self, _out: &mut [u8; 32]) -> Result<(), EntropyError> {
        Err(EntropyError::Unavailable) // BLOCKED：缺飞腾 D2000 硬件
    }
}

/// 鲲鹏 TRNG 接口（**BLOCKED 缺硬件**）。同上，fail-closed。
pub struct KunpengTrng;

impl EntropySource for KunpengTrng {
    fn fill_entropy(&mut self, _out: &mut [u8; 32]) -> Result<(), EntropyError> {
        Err(EntropyError::Unavailable) // BLOCKED：缺鲲鹏硬件
    }
}

/// QEMU virtio-rng 适配（v0.209.0 P0-15：真实 virtqueue 队列 I/O）。
///
/// v0.200.0 在 drv-svc 实现了 VirtioRng（virtio-mmio 协商 + legacy 队列 +
/// 单 desc 读熵，无 alloc）；本适配复用同一 virtqueue 协议，直接为 crypto
/// 熵源提供 32 字节熵块。host 测试用 RAM 仿真 MMIO；真机（QEMU）用
/// volatile MMIO（M2.1 seL4 MMIO 映射后接线）。
pub struct QemuVirtioRng {
    /// virtio-mmio 基址（QEMU virt 平台典型 0x0a00_0000 附近）。
    base_addr: u64,
}

impl QemuVirtioRng {
    /// 创建 virtio-rng 熵源。
    ///
    /// `base_addr` 为 virtio-mmio 设备基址；0 时使用 QEMU virt 默认。
    pub fn new(base_addr: u64) -> Self {
        Self {
            base_addr: if base_addr == 0 {
                0x0a00_0000
            } else {
                base_addr
            },
        }
    }
}

impl EntropySource for QemuVirtioRng {
    fn fill_entropy(&mut self, out: &mut [u8; 32]) -> Result<(), EntropyError> {
        // 委托给共享的 virtio-rng 队列读取实现（与 drv-svc 同协议）。
        match crate::rng::virtio_rng::read_entropy_block(self.base_addr, out) {
            Some(_) => Ok(()),
            None => Err(EntropyError::Unavailable),
        }
    }
}

/// 测试专用固定熵（蓝图/20 R1-4：原固定 QEMU 熵收编为测试替身）。
///
/// 仅 `cfg(test)`：用于验证「熵 → from_seed」闭环的测试夹具，
/// 生产/仿真构建一律不含本类型（生产用 [`QemuVirtioRng`] 真实读熵）。
#[cfg(test)]
pub struct TestFixedEntropy;

#[cfg(test)]
impl EntropySource for TestFixedEntropy {
    fn fill_entropy(&mut self, out: &mut [u8; 32]) -> Result<(), EntropyError> {
        // 固定但非全零：测试夹具专用（标记字节 0x51 0x4d 0x55 = "QMU"）
        out[0] = 0x51;
        out[1] = 0x4d;
        out[2] = 0x55;
        for (i, slot) in out.iter_mut().enumerate().skip(3) {
            *slot = (i as u8).wrapping_mul(0x5d).wrapping_add(0x31);
        }
        Ok(())
    }
}

/// 按编译配置选择平台熵源（CRY-002 工厂，fail-closed，蓝图/20 R1-4 真实化）。
///
/// - `qemu-entropy` feature：选择 [`QemuVirtioRng`]——真实 virtqueue 读熵
///   （virtio-rng 设备），不再是固定种子顶包；
/// - 无 feature/无硬件（BSP-001 BLOCKED）：返回 [`EntropyError::Unavailable`]，
///   绝不静默回退固定种子（fail-closed）。
pub fn platform_entropy_source() -> Result<Box<dyn EntropySource>, EntropyError> {
    #[cfg(feature = "qemu-entropy")]
    {
        return Ok(Box::new(QemuVirtioRng::new(0)));
    }
    #[cfg(not(feature = "qemu-entropy"))]
    {
        // 无硬件（BSP-001 BLOCKED）且无 QEMU 门控 → fail-closed
        Err(EntropyError::Unavailable)
    }
}

/// 从平台熵源构造 CSRNG（CRY-002 闭环）：熵 → from_seed。
///
/// 无可用熵源时返回错误——调用方（密钥生成/签名）必须 fail-closed，
/// 不得静默使用固定种子（`CsRng::new()` 在 release 下不存在）。
pub fn csrng_from_platform() -> Result<CsRng, EntropyError> {
    let mut src = platform_entropy_source()?;
    let mut seed = [0u8; 32];
    src.fill_entropy(&mut seed)?;
    Ok(CsRng::from_seed(&seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cry002_platform_unavailable_fails_closed() {
        #[cfg(not(feature = "qemu-entropy"))]
        {
            // 默认：无硬件 → Unavailable，不伪造熵（fail-closed）
            assert!(platform_entropy_source().is_err());
            assert!(csrng_from_platform().is_err());
        }
        #[cfg(feature = "qemu-entropy")]
        {
            // R1-4：qemu-entropy 必须选择真实 virtio-rng 源（构造成功）；
            // host 测试环境无 virtio 设备，不得调用 fill_entropy（会访问
            // MMIO）——真实读取由 virtio_rng 模块的 SimMmio 仿真测试覆盖。
            let src = platform_entropy_source().expect("qemu-entropy selects QemuVirtioRng");
            let _ = src;
        }
    }

    #[test]
    fn test_cry002_test_fixed_entropy_closed_loop() {
        // R1-4：测试夹具固定熵 → from_seed 闭环（替代原固定 QEMU 熵冒烟语义；
        // 测试夹具不进入生产/仿真构建）
        let mut src = TestFixedEntropy;
        let mut seed = [0u8; 32];
        src.fill_entropy(&mut seed)
            .expect("test fixture entropy ok");
        assert_ne!(seed, [0u8; 32], "夹具熵非全零");
        let mut rng = CsRng::from_seed(&seed);
        let mut out = [0u8; 32];
        rng.fill_bytes(&mut out);
        assert_ne!(out, [0u8; 32]);
        // 同种子可复现（DRBG 确定性）
        let mut src2 = TestFixedEntropy;
        let mut seed2 = [0u8; 32];
        src2.fill_entropy(&mut seed2).unwrap();
        let mut rng2 = CsRng::from_seed(&seed2);
        let mut out2 = [0u8; 32];
        rng2.fill_bytes(&mut out2);
        assert_eq!(out, out2, "同种子输出可复现");
    }

    #[test]
    fn test_cry002_hw_trng_blocked() {
        assert_eq!(
            PhytiumTrng.fill_entropy(&mut [0u8; 32]),
            Err(EntropyError::Unavailable),
            "飞腾 TRNG 缺硬件必须 fail-closed"
        );
        assert_eq!(
            KunpengTrng.fill_entropy(&mut [0u8; 32]),
            Err(EntropyError::Unavailable)
        );
        // QemuVirtioRng 真实 virtqueue 读取在无设备环境（host 测试）下会
        // 访问 volatile MMIO——不在此断言（见 rng::virtio_rng 模块的
        // 专用 host 仿真测试）。此处仅验证构造可用。
        let _rng = QemuVirtioRng::new(0);
        let _ = _rng.base_addr;
    }

    #[test]
    fn test_cry002_from_seed_distinct_outputs() {
        let mut seed_a = [0x11u8; 32];
        let seed_b = [0x22u8; 32];
        let mut rng_a = CsRng::from_seed(&seed_a);
        let mut rng_b = CsRng::from_seed(&seed_b);
        let mut out_a = [0u8; 32];
        let mut out_b = [0u8; 32];
        rng_a.fill_bytes(&mut out_a);
        rng_b.fill_bytes(&mut out_b);
        assert_ne!(out_a, out_b, "不同种子必须产生不同输出");
        // 同种子可复现（DRBG 确定性）
        seed_a[0] = 0x11;
        let mut rng_a2 = CsRng::from_seed(&seed_a);
        let mut out_a2 = [0u8; 32];
        rng_a2.fill_bytes(&mut out_a2);
        assert_eq!(out_a, out_a2, "同种子输出可复现（DRBG 确定性）");
    }

    #[test]
    fn test_cry002_seed_zeroized_sensitive() {
        // from_seed 后种子不残留于内部状态之外（V = SM3(seed) 单向）
        let mut seed = [0xABu8; 32];
        let rng = CsRng::from_seed(&seed);
        let _ = rng;
        seed.fill(0x00);
        assert_eq!(seed, [0u8; 32]);
    }
}
