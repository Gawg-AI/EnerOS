//! 飞腾 D2000 国密硬件加密引擎驱动 (workspace v0.192.0).
//!
//! 利用飞腾 D2000 SoC 内置的 SM2/SM3/SM4 硬件加密引擎实现国密算法加速。
//! 通过 MMIO 寄存器接口控制引擎，DMA 零拷贝传输数据。
//!
//! # 硬件规格
//! - 基地址：0x2820_0000（飞腾 D2000 加密引擎）
//! - SM3 吞吐量：> 1 Gbps
//! - SM4-ECB 吞吐量：> 2 Gbps
//! - SM2 签名延迟：< 5 ms
//!
//! # 寄存器映射
//! 参见 `PhytiumCryptoRegs` 结构体定义。
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。
//! 硬件寄存器访问通过 `volatile` 读写实现。

use alloc::vec;
use alloc::vec::Vec;

use super::engine::{CryptoEngine, EngineStats, Sm4Mode};
use crate::error::CryptoError;

// ============================================================
// Constants
// ============================================================

/// 飞腾 D2000 加密引擎 MMIO 基地址.
pub const PHYTECH_CRYPTO_BASE: usize = 0x2820_0000;

/// SM3 分组大小（字节）.
const SM3_BLOCK_SIZE: usize = 64;

/// 硬件操作超时（毫秒）.
const CRYPTO_TIMEOUT_MS: u64 = 100;

/// DMA 缓冲区大小（4KB 对齐）.
const DMA_BUF_SIZE: usize = 4096;

// ── 控制寄存器位域 ──
/// 启动操作
const CTRL_START: u32 = 0x0000_0001;
/// 复位引擎
const CTRL_RESET: u32 = 0x0000_0002;
/// 引擎忙标志
const CTRL_BUSY: u32 = 0x0000_0004;

// ── 中断位域 ──
/// 操作完成中断
const IRQ_DONE: u32 = 0x0000_0001;
/// 错误中断
const IRQ_ERROR: u32 = 0x0000_0002;

// ── SM3 命令 ──
/// SM3 初始化命令
const CMD_SM3_INIT: u32 = 0x0001_0001;
/// SM3 更新命令
const CMD_SM3_UPDATE: u32 = 0x0001_0002;
/// SM3 完成命令
const CMD_SM3_FINAL: u32 = 0x0001_0003;

// ── SM4 命令 ──
/// SM4 加密命令
const CMD_SM4_ENCRYPT: u32 = 0x0002_0000;
/// SM4 解密命令
const CMD_SM4_DECRYPT: u32 = 0x0002_0001;

// ── SM2 命令 ──
/// SM2 签名命令
const CMD_SM2_SIGN: u32 = 0x0003_0000;
/// SM2 验签命令
const CMD_SM2_VERIFY: u32 = 0x0003_0001;

// ============================================================
// Register Map
// ============================================================

/// 飞腾 D2000 加密引擎寄存器映射.
///
/// 所有寄存器为 32 位宽，MMIO 访问。
/// 寄存器布局参考飞腾 D2000 加密引擎手册（NDA 文档）。
#[repr(C)]
pub struct PhytiumCryptoRegs {
    /// 控制寄存器（启动/复位/状态）
    pub ctrl: u32,
    /// 状态寄存器（引擎状态）
    pub status: u32,
    /// 中断使能寄存器
    pub irq_en: u32,
    /// 中断状态寄存器（写 1 清除）
    pub irq_status: u32,
    /// SM2 命令寄存器
    pub sm2_cmd: u32,
    /// SM3 命令寄存器
    pub sm3_cmd: u32,
    /// SM4 命令寄存器
    pub sm4_cmd: u32,
    /// SM4 模式寄存器（ECB/CBC/CTR/GCM）
    pub sm4_mode: u32,
    /// 数据输入缓冲（16 × 32-bit = 64 字节）
    pub data_in: [u32; 16],
    /// 数据输出缓冲（16 × 32-bit = 64 字节）
    pub data_out: [u32; 16],
    /// 密钥缓冲（8 × 32-bit = 32 字节）
    pub key: [u32; 8],
    /// IV 缓冲（4 × 32-bit = 16 字节）
    pub iv: [u32; 4],
    /// DMA 源地址低 32 位
    pub dma_src_lo: u32,
    /// DMA 源地址高 32 位
    pub dma_src_hi: u32,
    /// DMA 目的地址低 32 位
    pub dma_dst_lo: u32,
    /// DMA 目的地址高 32 位
    pub dma_dst_hi: u32,
    /// DMA 传输长度
    pub dma_len: u32,
    /// 保留区域
    pub _reserved: [u32; 27],
}

// ============================================================
// DMA Buffer
// ============================================================

/// DMA 缓冲区（4KB 对齐）.
///
/// 用于硬件引擎与内存之间的零拷贝数据传输。
pub struct DmaBuffer {
    /// 缓冲区数据
    buf: Vec<u8>,
    /// 模拟物理地址（实际硬件由 IOMMU 映射）
    phys_addr: usize,
}

impl DmaBuffer {
    /// 分配指定大小的 DMA 缓冲区（4KB 对齐）.
    pub fn alloc(size: usize) -> Self {
        let aligned_size = (size + 4095) & !4095;
        let buf = vec![0u8; aligned_size];
        // 模拟物理地址（实际硬件通过 IOMMU 映射）
        let phys_addr = buf.as_ptr() as usize;
        Self { buf, phys_addr }
    }

    /// 写入数据到缓冲区指定偏移.
    pub fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), CryptoError> {
        if offset + data.len() > self.buf.len() {
            return Err(CryptoError::DmaError);
        }
        self.buf[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// 从缓冲区读取数据.
    pub fn read(&self, offset: usize, len: usize) -> Result<&[u8], CryptoError> {
        if offset + len > self.buf.len() {
            return Err(CryptoError::DmaError);
        }
        Ok(&self.buf[offset..offset + len])
    }

    /// 获取物理地址.
    pub fn phys_addr(&self) -> usize {
        self.phys_addr
    }

    /// 缓冲区容量.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// 清零缓冲区（安全擦除）.
    pub fn zeroize(&mut self) {
        self.buf.fill(0);
    }
}

// ============================================================
// PhytiumCryptoEngine
// ============================================================

/// 飞腾 D2000 国密硬件加密引擎.
///
/// 通过 MMIO 寄存器控制硬件加密引擎，支持 SM2/SM3/SM4 硬件加速。
/// DMA 零拷贝传输大数据，避免 CPU 拷贝开销。
///
/// # 安全
/// - 密钥写入引擎内部寄存器，操作完成后自动清零
/// - DMA 缓冲区使用后清零
pub struct PhytiumCryptoEngine {
    /// 寄存器基地址（volatile 访问）
    regs: *mut PhytiumCryptoRegs,
    /// DMA 缓冲区
    dma_buf: DmaBuffer,
    /// 引擎统计
    stats: EngineStats,
    /// 是否已初始化
    initialized: bool,
    /// 模拟模式（无实际硬件时使用）
    simulate: bool,
}

// 安全：PhytiumCryptoEngine 仅在单核环境使用，或通过外部互斥保护
unsafe impl Send for PhytiumCryptoEngine {}

impl PhytiumCryptoEngine {
    /// 创建飞腾加密引擎实例（实际硬件模式）.
    ///
    /// # Safety
    /// 调用者必须确保 `PHYTECH_CRYPTO_BASE` 地址有效且可访问。
    pub unsafe fn new() -> Self {
        let regs = PHYTECH_CRYPTO_BASE as *mut PhytiumCryptoRegs;
        let mut engine = Self {
            regs,
            dma_buf: DmaBuffer::alloc(DMA_BUF_SIZE),
            stats: EngineStats::new(),
            initialized: false,
            simulate: false,
        };
        engine.reset();
        engine.initialized = true;
        engine
    }

    /// 创建模拟模式实例（无实际硬件，用于测试/开发）.
    ///
    /// 模拟模式下，寄存器操作被模拟，算法使用软件实现。
    /// 用于验证驱动逻辑和接口正确性。
    pub fn new_simulated() -> Self {
        Self {
            regs: core::ptr::null_mut(),
            dma_buf: DmaBuffer::alloc(DMA_BUF_SIZE),
            stats: EngineStats::new(),
            initialized: true,
            simulate: true,
        }
    }

    /// 复位加密引擎.
    fn reset(&mut self) {
        if self.simulate {
            return;
        }
        unsafe {
            // 发送复位命令
            (*self.regs).ctrl = CTRL_RESET;
            // 等待复位完成
            while core::ptr::read_volatile(&(*self.regs).ctrl) & CTRL_BUSY != 0 {
                core::hint::spin_loop();
            }
            // 清除所有中断状态
            (*self.regs).irq_status = 0xFFFF_FFFF;
            // 使能完成中断和错误中断
            (*self.regs).irq_en = IRQ_DONE | IRQ_ERROR;
        }
    }

    /// 等待硬件操作完成.
    ///
    /// 轮询中断状态寄存器，等待操作完成或超时。
    fn wait_completion(&mut self) -> Result<(), CryptoError> {
        if self.simulate {
            return Ok(());
        }

        let mut elapsed: u64 = 0;
        loop {
            let status = unsafe { (*self.regs).irq_status };

            if status & IRQ_DONE != 0 {
                // 清除完成中断
                unsafe {
                    (*self.regs).irq_status = IRQ_DONE;
                }
                return Ok(());
            }

            if status & IRQ_ERROR != 0 {
                // 清除错误中断
                unsafe {
                    (*self.regs).irq_status = IRQ_ERROR;
                }
                return Err(CryptoError::EngineError);
            }

            // 简单超时计数（实际应使用硬件定时器）
            elapsed += 1;
            if elapsed > CRYPTO_TIMEOUT_MS * 1000 {
                return Err(CryptoError::EngineTimeout);
            }

            core::hint::spin_loop();
        }
    }

    /// 设置 DMA 源地址.
    fn set_dma_src(&mut self, addr: usize, len: usize) {
        if self.simulate {
            return;
        }
        unsafe {
            (*self.regs).dma_src_lo = (addr & 0xFFFF_FFFF) as u32;
            (*self.regs).dma_src_hi = (addr >> 32) as u32;
            (*self.regs).dma_len = len as u32;
        }
    }

    /// 写入密钥到引擎寄存器.
    fn write_key(&mut self, key: &[u8]) {
        if self.simulate {
            return;
        }
        unsafe {
            for (i, chunk) in key.chunks(4).enumerate() {
                if i >= 8 {
                    break;
                }
                let mut bytes = [0u8; 4];
                bytes[..chunk.len()].copy_from_slice(chunk);
                (*self.regs).key[i] = u32::from_le_bytes(bytes);
            }
        }
    }

    /// 清零引擎密钥寄存器（安全擦除）.
    fn clear_key(&mut self) {
        if self.simulate {
            return;
        }
        unsafe {
            for reg in (*self.regs).key.iter_mut() {
                *reg = 0;
            }
        }
    }

    /// 写入 IV 到引擎寄存器.
    fn write_iv(&mut self, iv: &[u8]) {
        if self.simulate {
            return;
        }
        unsafe {
            for (i, chunk) in iv.chunks(4).enumerate() {
                if i >= 4 {
                    break;
                }
                let mut bytes = [0u8; 4];
                bytes[..chunk.len()].copy_from_slice(chunk);
                (*self.regs).iv[i] = u32::from_le_bytes(bytes);
            }
        }
    }

    /// 从引擎读取哈希结果.
    fn read_hash_result(&mut self) -> [u8; 32] {
        if self.simulate {
            return [0u8; 32];
        }
        let mut hash = [0u8; 32];
        unsafe {
            for i in 0..8 {
                let val = (*self.regs).data_out[i];
                hash[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
            }
        }
        hash
    }
}

impl CryptoEngine for PhytiumCryptoEngine {
    fn sm2_sign(&mut self, private_key: &[u8], msg: &[u8]) -> Result<[u8; 64], CryptoError> {
        if !self.initialized {
            return Err(CryptoError::EngineUnavailable);
        }
        if private_key.len() != 32 {
            return Err(CryptoError::InvalidKeyLength {
                expected: 32,
                actual: private_key.len(),
            });
        }

        if self.simulate {
            // 模拟模式：使用软件 SM2 签名
            return self.sm2_sign_software(private_key, msg);
        }

        // 硬件模式：写入私钥
        self.write_key(private_key);
        // DMA 写入消息
        self.dma_buf
            .write(0, msg)
            .map_err(|_| CryptoError::DmaError)?;
        self.set_dma_src(self.dma_buf.phys_addr(), msg.len());

        unsafe {
            // 发送 SM2 签名命令
            (*self.regs).sm2_cmd = CMD_SM2_SIGN | (msg.len() as u32);
            (*self.regs).ctrl = CTRL_START;
        }

        // 等待完成
        self.wait_completion()?;

        // 读取签名结果 (r ‖ s)
        let mut sig = [0u8; 64];
        unsafe {
            for i in 0..16 {
                let val = (*self.regs).data_out[i];
                sig[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
            }
        }

        // 安全清零密钥
        self.clear_key();
        self.dma_buf.zeroize();
        self.stats.record_success(msg.len());
        Ok(sig)
    }

    fn sm2_verify(
        &mut self,
        public_key: &[u8],
        msg: &[u8],
        sig: &[u8; 64],
    ) -> Result<bool, CryptoError> {
        if !self.initialized {
            return Err(CryptoError::EngineUnavailable);
        }
        if public_key.len() != 65 {
            return Err(CryptoError::InvalidKeyLength {
                expected: 65,
                actual: public_key.len(),
            });
        }

        if self.simulate {
            return self.sm2_verify_software(public_key, msg, sig);
        }

        // 写入公钥到密钥缓冲
        self.write_key(&public_key[..32]);
        // DMA 写入消息和签名
        self.dma_buf
            .write(0, msg)
            .map_err(|_| CryptoError::DmaError)?;
        self.dma_buf
            .write(msg.len(), sig)
            .map_err(|_| CryptoError::DmaError)?;
        self.set_dma_src(self.dma_buf.phys_addr(), msg.len() + 64);

        unsafe {
            (*self.regs).sm2_cmd = CMD_SM2_VERIFY | (msg.len() as u32);
            (*self.regs).ctrl = CTRL_START;
        }

        self.wait_completion()?;

        // 读取验证结果（status 寄存器 bit 0）
        let result = unsafe { (*self.regs).status & 0x01 != 0 };
        self.dma_buf.zeroize();
        self.stats.record_success(msg.len());
        Ok(result)
    }

    fn sm3_hash(&mut self, data: &[u8]) -> Result<[u8; 32], CryptoError> {
        if !self.initialized {
            return Err(CryptoError::EngineUnavailable);
        }

        if self.simulate {
            // 模拟模式：使用软件 SM3
            let hash = crate::sm3::hash(data);
            self.stats.record_success(data.len());
            return Ok(hash);
        }

        // 硬件模式：分块处理大数据
        let mut offset = 0;

        // 初始化
        unsafe {
            (*self.regs).sm3_cmd = CMD_SM3_INIT;
            (*self.regs).ctrl = CTRL_START;
        }
        self.wait_completion()?;

        // 处理完整分组
        while offset + SM3_BLOCK_SIZE <= data.len() {
            self.dma_buf
                .write(0, &data[offset..offset + SM3_BLOCK_SIZE])
                .map_err(|_| CryptoError::DmaError)?;
            self.set_dma_src(self.dma_buf.phys_addr(), SM3_BLOCK_SIZE);

            unsafe {
                (*self.regs).sm3_cmd = CMD_SM3_UPDATE | (SM3_BLOCK_SIZE as u32);
                (*self.regs).ctrl = CTRL_START;
            }
            self.wait_completion()?;
            offset += SM3_BLOCK_SIZE;
        }

        // 处理剩余数据 + 填充
        let remaining = &data[offset..];
        self.dma_buf
            .write(0, remaining)
            .map_err(|_| CryptoError::DmaError)?;
        self.set_dma_src(self.dma_buf.phys_addr(), remaining.len());

        unsafe {
            (*self.regs).sm3_cmd = CMD_SM3_FINAL | (remaining.len() as u32);
            (*self.regs).ctrl = CTRL_START;
        }
        self.wait_completion()?;

        // 读取哈希结果
        let hash = self.read_hash_result();
        self.dma_buf.zeroize();
        self.stats.record_success(data.len());
        Ok(hash)
    }

    fn sm4_encrypt(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if !self.initialized {
            return Err(CryptoError::EngineUnavailable);
        }

        if self.simulate {
            return self.sm4_encrypt_software(key, iv, mode, data);
        }

        // 写入密钥
        self.write_key(key);

        // 写入 IV（如果需要）
        if let Some(iv_bytes) = iv {
            if iv_bytes.len() >= 16 {
                self.write_iv(&iv_bytes[..16]);
            }
        }

        // 设置模式
        unsafe {
            (*self.regs).sm4_mode = mode as u32;
        }

        // DMA 写入明文
        self.dma_buf
            .write(0, data)
            .map_err(|_| CryptoError::DmaError)?;
        self.set_dma_src(self.dma_buf.phys_addr(), data.len());

        unsafe {
            (*self.regs).sm4_cmd = CMD_SM4_ENCRYPT | (data.len() as u32);
            (*self.regs).ctrl = CTRL_START;
        }

        self.wait_completion()?;

        // 读取密文
        let mut result = vec![0u8; data.len()];
        unsafe {
            for (i, chunk) in result.chunks_mut(4).enumerate() {
                if i >= 16 {
                    break;
                }
                let val = (*self.regs).data_out[i];
                let bytes = val.to_be_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
        }

        // 安全清零
        self.clear_key();
        self.dma_buf.zeroize();
        self.stats.record_success(data.len());
        Ok(result)
    }

    fn sm4_decrypt(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if !self.initialized {
            return Err(CryptoError::EngineUnavailable);
        }

        if self.simulate {
            return self.sm4_decrypt_software(key, iv, mode, data);
        }

        // 写入密钥
        self.write_key(key);

        // 写入 IV
        if let Some(iv_bytes) = iv {
            if iv_bytes.len() >= 16 {
                self.write_iv(&iv_bytes[..16]);
            }
        }

        // 设置模式
        unsafe {
            (*self.regs).sm4_mode = mode as u32;
        }

        // DMA 写入密文
        self.dma_buf
            .write(0, data)
            .map_err(|_| CryptoError::DmaError)?;
        self.set_dma_src(self.dma_buf.phys_addr(), data.len());

        unsafe {
            (*self.regs).sm4_cmd = CMD_SM4_DECRYPT | (data.len() as u32);
            (*self.regs).ctrl = CTRL_START;
        }

        self.wait_completion()?;

        // 读取明文
        let mut result = vec![0u8; data.len()];
        unsafe {
            for (i, chunk) in result.chunks_mut(4).enumerate() {
                if i >= 16 {
                    break;
                }
                let val = (*self.regs).data_out[i];
                let bytes = val.to_be_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
        }

        // 安全清零
        self.clear_key();
        self.dma_buf.zeroize();
        self.stats.record_success(data.len());
        Ok(result)
    }

    fn name(&self) -> &'static str {
        "Phytium D2000 Crypto Engine"
    }

    fn is_hardware(&self) -> bool {
        true
    }

    fn is_available(&self) -> bool {
        if self.simulate {
            return true;
        }
        // 实际硬件：探测寄存器是否可读
        self.initialized
    }

    fn stats(&self) -> EngineStats {
        self.stats.clone()
    }
}

// ── 软件 fallback 辅助方法（模拟模式使用） ──

impl PhytiumCryptoEngine {
    /// 软件 SM2 签名（模拟模式 fallback）.
    fn sm2_sign_software(
        &mut self,
        private_key: &[u8],
        msg: &[u8],
    ) -> Result<[u8; 64], CryptoError> {
        use crate::rng::CsRng;
        use crate::sm2::{sm2_sign, Sm2KeyPair, Sm2PrivateKey};

        let mut rng = CsRng::for_deterministic_signature(
            private_key,
            msg,
            b"eneros/phytium/software-fallback/sm2-sign/v1",
        );
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(private_key);
        let sk = Sm2PrivateKey::from_bytes(&key_bytes)
            .map_err(|_| CryptoError::InvalidInput("invalid SM2 private key".into()))?;
        // 从私钥推导公钥
        let kp = Sm2KeyPair::from_private_key(&sk).map_err(|_| CryptoError::InternalError)?;
        let sig =
            sm2_sign(msg, &sk, &kp.public_key, &mut rng).map_err(|_| CryptoError::InternalError)?;
        self.stats.record_success(msg.len());
        Ok(sig.to_bytes())
    }

    /// 软件 SM2 验签（模拟模式 fallback）.
    fn sm2_verify_software(
        &mut self,
        public_key: &[u8],
        msg: &[u8],
        sig: &[u8; 64],
    ) -> Result<bool, CryptoError> {
        use crate::sm2::{sm2_verify, Sm2PublicKey, Sm2Signature};

        let pk = Sm2PublicKey::from_bytes(public_key)
            .map_err(|_| CryptoError::InvalidInput("invalid SM2 public key".into()))?;
        let signature = Sm2Signature::from_bytes(sig);
        let result = sm2_verify(msg, &signature, &pk).map_err(|_| CryptoError::InternalError)?;
        self.stats.record_success(msg.len());
        Ok(result)
    }

    /// 软件 SM4 加密（模拟模式 fallback）.
    fn sm4_encrypt_software(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        use crate::sm4::Sm4;

        let cipher = Sm4::new(key);
        let result = match mode {
            Sm4Mode::Ecb => {
                // ECB 模式：逐块加密
                let mut out = Vec::with_capacity(data.len());
                for chunk in data.chunks(16) {
                    let mut block = [0u8; 16];
                    block[..chunk.len()].copy_from_slice(chunk);
                    let enc = cipher.encrypt_block(&block);
                    out.extend_from_slice(&enc[..chunk.len()]);
                }
                out
            }
            Sm4Mode::Cbc => {
                use crate::sm4::cbc::Sm4Cbc;
                let iv_bytes = iv.ok_or(CryptoError::InvalidNonceLength {
                    expected: 16,
                    actual: 0,
                })?;
                let mut iv_arr = [0u8; 16];
                iv_arr.copy_from_slice(&iv_bytes[..16]);
                let cbc = Sm4Cbc::new(key, &iv_arr);
                cbc.encrypt(data)
            }
            _ => {
                return Err(CryptoError::Unsupported);
            }
        };
        self.stats.record_success(data.len());
        Ok(result)
    }

    /// 软件 SM4 解密（模拟模式 fallback）.
    fn sm4_decrypt_software(
        &mut self,
        key: &[u8; 16],
        iv: Option<&[u8]>,
        mode: Sm4Mode,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        use crate::sm4::Sm4;

        let cipher = Sm4::new(key);
        let result = match mode {
            Sm4Mode::Ecb => {
                let mut out = Vec::with_capacity(data.len());
                for chunk in data.chunks(16) {
                    let mut block = [0u8; 16];
                    block[..chunk.len()].copy_from_slice(chunk);
                    let dec = cipher.decrypt_block(&block);
                    out.extend_from_slice(&dec[..chunk.len()]);
                }
                out
            }
            Sm4Mode::Cbc => {
                use crate::sm4::cbc::Sm4Cbc;
                let iv_bytes = iv.ok_or(CryptoError::InvalidNonceLength {
                    expected: 16,
                    actual: 0,
                })?;
                let mut iv_arr = [0u8; 16];
                iv_arr.copy_from_slice(&iv_bytes[..16]);
                let cbc = Sm4Cbc::new(key, &iv_arr);
                cbc.decrypt(data)?
            }
            _ => {
                return Err(CryptoError::Unsupported);
            }
        };
        self.stats.record_success(data.len());
        Ok(result)
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phytium_simulated_sm3() {
        let mut engine = PhytiumCryptoEngine::new_simulated();
        assert!(engine.is_hardware());
        assert!(engine.is_available());
        assert_eq!(engine.name(), "Phytium D2000 Crypto Engine");

        // SM3 哈希应使用软件实现
        let hash = engine.sm3_hash(b"abc").unwrap();
        let expected = crate::sm3::hash(b"abc");
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_phytium_simulated_sm3_kat() {
        let mut engine = PhytiumCryptoEngine::new_simulated();

        // GB/T 32905-2016 示例 1
        let hash = engine.sm3_hash(b"abc").unwrap();
        let expected: [u8; 32] = [
            0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10,
            0xE4, 0xE2, 0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B,
            0x8F, 0x4B, 0xA8, 0xE0,
        ];
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_phytium_simulated_sm4_ecb() {
        let mut engine = PhytiumCryptoEngine::new_simulated();
        let key = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let plaintext = key; // 国标测试向量

        let ciphertext = engine
            .sm4_encrypt(&key, None, Sm4Mode::Ecb, &plaintext)
            .unwrap();
        let decrypted = engine
            .sm4_decrypt(&key, None, Sm4Mode::Ecb, &ciphertext)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_phytium_simulated_sm4_cbc() {
        let mut engine = PhytiumCryptoEngine::new_simulated();
        let key = [0x42u8; 16];
        let iv = [0x00u8; 16];
        let plaintext = b"Hello, SM4-CBC hardware simulation!";

        let ciphertext = engine
            .sm4_encrypt(&key, Some(&iv), Sm4Mode::Cbc, plaintext)
            .unwrap();
        assert_ne!(&ciphertext[..], &plaintext[..]);

        let decrypted = engine
            .sm4_decrypt(&key, Some(&iv), Sm4Mode::Cbc, &ciphertext)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_phytium_simulated_sm2_sign_verify() {
        let mut engine = PhytiumCryptoEngine::new_simulated();

        // 生成密钥对
        use crate::rng::CsRng;
        use crate::sm2::Sm2KeyPair;
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();

        let msg = b"test message for SM2 hardware simulation";
        let sk_bytes = kp.private_key.to_bytes();
        let pk_bytes = kp.public_key.to_bytes_uncompressed();

        // 签名
        let sig = engine.sm2_sign(&sk_bytes, msg).unwrap();
        assert_ne!(sig, [0u8; 64]);

        // 验签
        let valid = engine.sm2_verify(&pk_bytes, msg, &sig).unwrap();
        assert!(valid);

        // 篡改消息后验签应失败
        let invalid = engine.sm2_verify(&pk_bytes, b"tampered", &sig).unwrap();
        assert!(!invalid);
    }

    #[test]
    fn test_phytium_invalid_key_length() {
        let mut engine = PhytiumCryptoEngine::new_simulated();
        let short_key = [0u8; 16]; // 应该是 32 字节
        let result = engine.sm2_sign(&short_key, b"msg");
        assert!(matches!(
            result,
            Err(CryptoError::InvalidKeyLength {
                expected: 32,
                actual: 16
            })
        ));
    }

    #[test]
    fn test_phytium_stats() {
        let mut engine = PhytiumCryptoEngine::new_simulated();
        let _ = engine.sm3_hash(b"data1");
        let _ = engine.sm3_hash(b"data2");

        let stats = engine.stats();
        assert_eq!(stats.total_ops, 2);
        assert_eq!(stats.success_ops, 2);
        assert_eq!(stats.success_rate(), 100);
    }

    #[test]
    fn test_dma_buffer() {
        let mut buf = DmaBuffer::alloc(128);
        assert!(buf.capacity() >= 128);

        // 写入和读取
        buf.write(0, b"hello").unwrap();
        let data = buf.read(0, 5).unwrap();
        assert_eq!(data, b"hello");

        // 越界写入
        let result = buf.write(buf.capacity(), b"overflow");
        assert!(result.is_err());

        // 清零
        buf.zeroize();
        let data = buf.read(0, 5).unwrap();
        assert_eq!(data, &[0u8; 5]);
    }
}
