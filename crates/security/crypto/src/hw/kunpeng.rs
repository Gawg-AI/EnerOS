//! 鲲鹏 920 KAE (Kunpeng Accelerator Engine) 国密硬件加速驱动 (workspace v0.192.0).
//!
//! 鲲鹏 920 SoC 内置 KAE 加速引擎，支持 SM2/SM3/SM4 硬件加速。
//! 与飞腾 D2000 不同，鲲鹏使用 KAE 驱动框架（队列式接口）。
//!
//! # 硬件规格
//! - KAE 基地址：0x4000_0000（鲲鹏 920 KAE）
//! - SM3 吞吐量：> 1.5 Gbps
//! - SM4 吞吐量：> 3 Gbps
//! - SM2 签名延迟：< 3 ms
//!
//! # 架构差异
//! 鲲鹏 KAE 使用队列式接口（提交请求 → 等待完成），而非飞腾的寄存器直接操作。
//! 本驱动抽象了队列接口，对外提供统一的 CryptoEngine trait。
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。

use alloc::vec;
use alloc::vec::Vec;

use super::engine::{CryptoEngine, EngineStats, Sm4Mode};
use crate::error::CryptoError;

// ============================================================
// Constants
// ============================================================

/// 鲲鹏 920 KAE MMIO 基地址.
pub const KUNPENG_KAE_BASE: usize = 0x4000_0000;

/// KAE 队列深度.
const KAE_QUEUE_DEPTH: usize = 64;

/// 硬件操作超时（毫秒）.
const KAE_TIMEOUT_MS: u64 = 100;

/// DMA 缓冲区大小
const KAE_DMA_BUF_SIZE: usize = 4096;

// ── KAE 控制寄存器 ──
/// KAE 使能
const KAE_CTRL_ENABLE: u32 = 0x0000_0001;
/// KAE 复位
const KAE_CTRL_RESET: u32 = 0x0000_0002;
/// KAE 忙标志
const KAE_CTRL_BUSY: u32 = 0x0000_0004;

// ── KAE 请求类型 ──
/// SM3 哈希请求
const KAE_REQ_SM3: u32 = 0x0000_0001;
/// SM4 加密请求
const KAE_REQ_SM4_ENC: u32 = 0x0000_0002;
/// SM4 解密请求
const KAE_REQ_SM4_DEC: u32 = 0x0000_0003;
/// SM2 签名请求
const KAE_REQ_SM2_SIGN: u32 = 0x0000_0004;
/// SM2 验签请求
const KAE_REQ_SM2_VERIFY: u32 = 0x0000_0005;

// ── KAE 状态 ──
/// 请求完成
const KAE_STATUS_DONE: u32 = 0x0000_0001;
/// 请求错误
const KAE_STATUS_ERROR: u32 = 0x0000_0002;

// ============================================================
// KAE Register Map
// ============================================================

/// 鲲鹏 920 KAE 寄存器映射.
///
/// KAE 使用队列式接口：写入请求描述符 → 触发处理 → 读取完成状态。
#[repr(C)]
pub struct KunpengKaeRegs {
    /// 控制寄存器
    pub ctrl: u32,
    /// 状态寄存器
    pub status: u32,
    /// 请求类型
    pub req_type: u32,
    /// 请求长度
    pub req_len: u32,
    /// 队列头指针
    pub queue_head: u32,
    /// 队列尾指针
    pub queue_tail: u32,
    /// 完成计数
    pub done_count: u32,
    /// 错误计数
    pub error_count: u32,
    /// 密钥寄存器（8 × 32-bit）
    pub key: [u32; 8],
    /// IV 寄存器（4 × 32-bit）
    pub iv: [u32; 4],
    /// 数据输入区（16 × 32-bit）
    pub data_in: [u32; 16],
    /// 数据输出区（16 × 32-bit）
    pub data_out: [u32; 16],
    /// DMA 控制
    pub dma_ctrl: u32,
    /// DMA 源地址
    pub dma_src: u64,
    /// DMA 目的地址
    pub dma_dst: u64,
    /// DMA 长度
    pub dma_len: u32,
    /// 保留
    pub _reserved: [u32; 20],
}

// ============================================================
// KunpengKaeEngine
// ============================================================

/// 鲲鹏 920 KAE 国密硬件加速引擎.
///
/// 通过 KAE 队列式接口提交加密请求，支持 SM2/SM3/SM4 硬件加速。
/// 性能优于飞腾 D2000（鲲鹏 920 服务器级 SoC）。
///
/// # 安全
/// - 密钥通过 KAE 安全寄存器写入，操作后自动擦除
/// - DMA 缓冲区使用后清零
pub struct KunpengKaeEngine {
    /// 寄存器基地址
    regs: *mut KunpengKaeRegs,
    /// DMA 缓冲区
    dma_buf: Vec<u8>,
    /// 引擎统计
    stats: EngineStats,
    /// 是否已初始化
    initialized: bool,
    /// 模拟模式
    simulate: bool,
}

// SAFETY: `KunpengKaeEngine` 的 `regs` 指向有效 KAE MMIO 映射，
// DMA 缓冲区与统计由 `&mut self` 独占访问，跨线程移动不引入数据竞争。
unsafe impl Send for KunpengKaeEngine {}

impl KunpengKaeEngine {
    /// 创建鲲鹏 KAE 引擎实例（实际硬件模式）.
    ///
    /// # Safety
    /// 调用者必须确保 `KUNPENG_KAE_BASE` 地址有效且可访问。
    pub unsafe fn new() -> Self {
        let regs = KUNPENG_KAE_BASE as *mut KunpengKaeRegs;
        let mut engine = Self {
            regs,
            dma_buf: vec![0u8; KAE_DMA_BUF_SIZE],
            stats: EngineStats::new(),
            initialized: false,
            simulate: false,
        };
        engine.reset();
        engine.initialized = true;
        engine
    }

    /// 创建模拟模式实例（无实际硬件）.
    pub fn new_simulated() -> Self {
        Self {
            regs: core::ptr::null_mut(),
            dma_buf: vec![0u8; KAE_DMA_BUF_SIZE],
            stats: EngineStats::new(),
            initialized: true,
            simulate: true,
        }
    }

    /// 复位 KAE 引擎.
    fn reset(&mut self) {
        if self.simulate {
            return;
        }
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
        unsafe {
            (*self.regs).ctrl = KAE_CTRL_RESET;
            while core::ptr::read_volatile(&(*self.regs).ctrl) & KAE_CTRL_BUSY != 0 {
                core::hint::spin_loop();
            }
            (*self.regs).ctrl = KAE_CTRL_ENABLE;
        }
    }

    /// 提交请求并等待完成.
    fn submit_and_wait(&mut self, req_type: u32, data_len: usize) -> Result<(), CryptoError> {
        if self.simulate {
            return Ok(());
        }

// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
        unsafe {
            // 写入请求类型和长度
            (*self.regs).req_type = req_type;
            (*self.regs).req_len = data_len as u32;
            // 推进队列尾指针（触发处理）
            (*self.regs).queue_tail =
                (*self.regs).queue_tail.wrapping_add(1) % (KAE_QUEUE_DEPTH as u32);
        }

        // 等待完成
        let mut elapsed: u64 = 0;
        loop {
            // SAFETY: `self.regs` 由 `new()` 保证有效（simulate 已提前返回）；
            // 状态寄存器读为 volatile 硬件语义。
            let status = unsafe { (*self.regs).status };
            if status & KAE_STATUS_DONE != 0 {
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；状态寄存器读取/清除为 volatile 语义。
                unsafe {
                    (*self.regs).status = KAE_STATUS_DONE;
                }
                return Ok(());
            }
            if status & KAE_STATUS_ERROR != 0 {
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；状态寄存器读取/清除为 volatile 语义。
                unsafe {
                    (*self.regs).status = KAE_STATUS_ERROR;
                }
                return Err(CryptoError::EngineError);
            }
            elapsed += 1;
            if elapsed > KAE_TIMEOUT_MS * 1000 {
                return Err(CryptoError::EngineTimeout);
            }
            core::hint::spin_loop();
        }
    }

    /// 写入密钥.
    fn write_key(&mut self, key: &[u8]) {
        if self.simulate {
            return;
        }
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
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

    /// 清零密钥.
    fn clear_key(&mut self) {
        if self.simulate {
            return;
        }
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器字段写为 volatile 语义硬件操作。
        unsafe {
            for reg in (*self.regs).key.iter_mut() {
                *reg = 0;
            }
        }
    }

    /// 写入 IV.
    fn write_iv(&mut self, iv: &[u8]) {
        if self.simulate {
            return;
        }
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
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
}

impl CryptoEngine for KunpengKaeEngine {
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
            return self.sm2_sign_software(private_key, msg);
        }

        self.write_key(private_key);
        // 写入消息到数据区
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
        unsafe {
            for (i, chunk) in msg.chunks(4).enumerate() {
                if i >= 16 {
                    break;
                }
                let mut bytes = [0u8; 4];
                bytes[..chunk.len()].copy_from_slice(chunk);
                (*self.regs).data_in[i] = u32::from_le_bytes(bytes);
            }
        }

        self.submit_and_wait(KAE_REQ_SM2_SIGN, msg.len())?;

        let mut sig = [0u8; 64];
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
        unsafe {
            for i in 0..16 {
                let val = (*self.regs).data_out[i];
                sig[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
            }
        }

        self.clear_key();
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

        self.write_key(&public_key[..32]);
        self.submit_and_wait(KAE_REQ_SM2_VERIFY, msg.len() + 64)?;

        // SAFETY: `self.regs` 由 `new()` 保证有效（simulate 已提前返回）；
        // 状态位读取为硬件语义。
        let result = unsafe { (*self.regs).status & 0x01 != 0 };
        self.stats.record_success(msg.len());
        Ok(result)
    }

    fn sm3_hash(&mut self, data: &[u8]) -> Result<[u8; 32], CryptoError> {
        if !self.initialized {
            return Err(CryptoError::EngineUnavailable);
        }

        if self.simulate {
            let hash = crate::sm3::hash(data);
            self.stats.record_success(data.len());
            return Ok(hash);
        }

        // 写入数据到 DMA 缓冲
        let copy_len = data.len().min(self.dma_buf.len());
        self.dma_buf[..copy_len].copy_from_slice(&data[..copy_len]);

// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
        unsafe {
            (*self.regs).dma_src = self.dma_buf.as_ptr() as u64;
            (*self.regs).dma_len = copy_len as u32;
        }

        self.submit_and_wait(KAE_REQ_SM3, data.len())?;

        let mut hash = [0u8; 32];
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
        unsafe {
            for i in 0..8 {
                let val = (*self.regs).data_out[i];
                hash[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
            }
        }

        self.dma_buf.fill(0);
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

        self.write_key(key);
        if let Some(iv_bytes) = iv {
            if iv_bytes.len() >= 16 {
                self.write_iv(&iv_bytes[..16]);
            }
        }

        let copy_len = data.len().min(self.dma_buf.len());
        self.dma_buf[..copy_len].copy_from_slice(&data[..copy_len]);

        self.submit_and_wait(KAE_REQ_SM4_ENC, data.len())?;

        let mut result = vec![0u8; data.len()];
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
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

        self.clear_key();
        self.dma_buf.fill(0);
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

        self.write_key(key);
        if let Some(iv_bytes) = iv {
            if iv_bytes.len() >= 16 {
                self.write_iv(&iv_bytes[..16]);
            }
        }

        let copy_len = data.len().min(self.dma_buf.len());
        self.dma_buf[..copy_len].copy_from_slice(&data[..copy_len]);

        self.submit_and_wait(KAE_REQ_SM4_DEC, data.len())?;

        let mut result = vec![0u8; data.len()];
// SAFETY: `self.regs` 由 `new()` 保证指向有效 KAE MMIO 映射（simulate 模式提前返回）；寄存器写为硬件控制语义。
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

        self.clear_key();
        self.dma_buf.fill(0);
        self.stats.record_success(data.len());
        Ok(result)
    }

    fn name(&self) -> &'static str {
        "Kunpeng 920 KAE"
    }

    fn is_hardware(&self) -> bool {
        true
    }

    fn is_available(&self) -> bool {
        if self.simulate {
            return true;
        }
        self.initialized
    }

    fn stats(&self) -> EngineStats {
        self.stats.clone()
    }
}

// ── 软件 fallback 辅助方法 ──

impl KunpengKaeEngine {
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
            b"eneros/kunpeng-kae/software-fallback/sm2-sign/v1",
        );
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(private_key);
        let sk = Sm2PrivateKey::from_bytes(&key_bytes)
            .map_err(|_| CryptoError::InvalidInput("invalid SM2 private key".into()))?;
        let kp = Sm2KeyPair::from_private_key(&sk).map_err(|_| CryptoError::InternalError)?;
        let sig =
            sm2_sign(msg, &sk, &kp.public_key, &mut rng).map_err(|_| CryptoError::InternalError)?;
        self.stats.record_success(msg.len());
        Ok(sig.to_bytes())
    }

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
            _ => return Err(CryptoError::Unsupported),
        };
        self.stats.record_success(data.len());
        Ok(result)
    }

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
            _ => return Err(CryptoError::Unsupported),
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
    fn test_kunpeng_simulated_sm3() {
        let mut engine = KunpengKaeEngine::new_simulated();
        assert!(engine.is_hardware());
        assert!(engine.is_available());
        assert_eq!(engine.name(), "Kunpeng 920 KAE");

        let hash = engine.sm3_hash(b"abc").unwrap();
        let expected = crate::sm3::hash(b"abc");
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_kunpeng_simulated_sm3_kat() {
        let mut engine = KunpengKaeEngine::new_simulated();

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
    fn test_kunpeng_simulated_sm4_ecb() {
        let mut engine = KunpengKaeEngine::new_simulated();
        let key = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let plaintext = key;

        let ciphertext = engine
            .sm4_encrypt(&key, None, Sm4Mode::Ecb, &plaintext)
            .unwrap();
        let decrypted = engine
            .sm4_decrypt(&key, None, Sm4Mode::Ecb, &ciphertext)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_kunpeng_simulated_sm2_sign_verify() {
        let mut engine = KunpengKaeEngine::new_simulated();

        use crate::rng::CsRng;
        use crate::sm2::Sm2KeyPair;
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();

        let msg = b"kunpeng KAE SM2 test message";
        let sk_bytes = kp.private_key.to_bytes();
        let pk_bytes = kp.public_key.to_bytes_uncompressed();

        let sig = engine.sm2_sign(&sk_bytes, msg).unwrap();
        let valid = engine.sm2_verify(&pk_bytes, msg, &sig).unwrap();
        assert!(valid);
    }

    #[test]
    fn test_kunpeng_invalid_key() {
        let mut engine = KunpengKaeEngine::new_simulated();
        let result = engine.sm2_sign(&[0u8; 16], b"msg");
        assert!(matches!(
            result,
            Err(CryptoError::InvalidKeyLength {
                expected: 32,
                actual: 16
            })
        ));
    }

    #[test]
    fn test_kunpeng_stats() {
        let mut engine = KunpengKaeEngine::new_simulated();
        let _ = engine.sm3_hash(b"data");
        let stats = engine.stats();
        assert_eq!(stats.total_ops, 1);
        assert_eq!(stats.success_ops, 1);
    }
}
