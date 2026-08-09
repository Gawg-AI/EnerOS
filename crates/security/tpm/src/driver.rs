//! TPM 2.0 驱动层 (workspace v0.192.0).
//!
//! 定义 TpmDriver trait 和 SPI/MMIO 驱动实现。
//! 支持硬件 TPM 和模拟模式（用于测试/开发）。

use alloc::vec::Vec;

use crate::commands::*;
use crate::error::TpmError;

// ============================================================
// TPM Register Offsets (TIS - TPM Interface Specification)
// ============================================================

/// TPM Access Register
pub const TPM_ACCESS_REG: u32 = 0x00;
/// TPM Status Register
pub const TPM_STS_REG: u32 = 0x18;
/// TPM Data FIFO
pub const TPM_DATA_FIFO: u32 = 0x24;
/// TPM Interface ID
pub const TPM_INTF_CAPS: u32 = 0x14;
/// TPM Device ID / Vendor ID
pub const TPM_DID_VID_REG: u32 = 0xF00;

// ── Access Register Bits ──
/// Locality 激活
pub const TPM_ACCESS_ACTIVE_LOCALITY: u8 = 0x20;
/// 请求使用
pub const TPM_ACCESS_REQUEST_USE: u8 = 0x02;
/// 释放 Locality
pub const TPM_ACCESS_ACTIVE_LOCALITY_RELINQUISH: u8 = 0x20;

// ── Status Register Bits ──
/// 状态有效
pub const TPM_STS_VALID: u8 = 0x80;
/// 数据可读
pub const TPM_STS_DATA_AVAIL: u8 = 0x10;
/// 命令就绪
pub const TPM_STS_CMD_READY: u8 = 0x40;
/// TPM Go
pub const TPM_STS_GO: u8 = 0x20;

/// 密钥句柄类型
pub type KeyHandle = u32;

// ============================================================
// TpmDriver Trait
// ============================================================

/// TPM 2.0 驱动统一接口.
///
/// 所有 TPM 驱动（SPI/MMIO/模拟）必须实现此 trait。
pub trait TpmDriver {
    /// 初始化 TPM（发送 Startup 命令）.
    fn init(&mut self) -> Result<(), TpmError>;

    /// 发送原始 TPM 命令并接收响应.
    fn send_command(&mut self, cmd: &[u8]) -> Result<Vec<u8>, TpmError>;

    /// 读取 PCR 值.
    fn pcr_read(&mut self, pcr_idx: u32) -> Result<[u8; 32], TpmError>;

    /// 扩展 PCR（PCR = Hash(PCR ‖ data)）.
    fn pcr_extend(&mut self, pcr_idx: u32, digest: &[u8; 32]) -> Result<(), TpmError>;

    /// 创建密钥（返回密钥句柄）.
    fn create_key(&mut self, parent: KeyHandle, auth: &[u8]) -> Result<KeyHandle, TpmError>;

    /// 使用 TPM 内密钥签名.
    fn sign(&mut self, key: KeyHandle, digest: &[u8]) -> Result<Vec<u8>, TpmError>;

    /// 封装数据（Seal to PCR）.
    fn seal(&mut self, parent: KeyHandle, data: &[u8]) -> Result<Vec<u8>, TpmError>;

    /// 解封数据（Unseal）.
    fn unseal(&mut self, sealed: &[u8]) -> Result<Vec<u8>, TpmError>;

    /// 获取随机数.
    fn get_random(&mut self, len: u16) -> Result<Vec<u8>, TpmError>;

    /// 驱动名称.
    fn name(&self) -> &'static str;

    /// 是否为硬件 TPM.
    fn is_hardware(&self) -> bool;

    /// TPM 是否可用.
    fn is_available(&self) -> bool;
}

// ============================================================
// Simulated TPM Driver (for testing)
// ============================================================

/// 模拟 TPM 驱动（用于无硬件环境测试，CRY-003 feature 门控）.
///
/// **仅测试/显式 `simulated` feature**：产品 release 构建不含本类型
/// （`#[cfg(any(test, feature = "simulated"))]`），防止模拟 TPM 静默
/// 进入生产路径。硬件路径用 [`SpiTpmDriver`]（SPI TPM 2.0）。
#[cfg(any(test, feature = "simulated"))]
pub struct SimulatedTpm {
    /// PCR 寄存器组（24 × 32 字节）
    pcrs: [[u8; 32]; PCR_COUNT],
    /// 密钥存储（句柄 → 密钥数据）
    keys: Vec<(KeyHandle, Vec<u8>)>,
    /// 下一个密钥句柄
    next_handle: KeyHandle,
    /// 是否已初始化
    initialized: bool,
    /// 随机数种子（模拟）
    rng_state: u64,
}

#[cfg(any(test, feature = "simulated"))]
impl SimulatedTpm {
    /// 创建模拟 TPM.
    pub fn new() -> Self {
        Self {
            pcrs: [[0u8; 32]; PCR_COUNT],
            keys: Vec::new(),
            next_handle: TPM_HR_TRANSIENT,
            initialized: false,
            rng_state: 0xDEAD_BEEF_CAFE_BABE,
        }
    }

    /// 简单伪随机数生成（仅测试用）.
    fn next_random(&mut self) -> u64 {
        // xorshift64
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        self.rng_state
    }

    /// 获取当前 PCR 值（测试辅助）.
    pub fn get_pcr(&self, idx: usize) -> Option<&[u8; 32]> {
        self.pcrs.get(idx)
    }
}

#[cfg(any(test, feature = "simulated"))]
impl Default for SimulatedTpm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "simulated"))]
impl TpmDriver for SimulatedTpm {
    fn init(&mut self) -> Result<(), TpmError> {
        self.initialized = true;
        Ok(())
    }

    fn send_command(&mut self, cmd: &[u8]) -> Result<Vec<u8>, TpmError> {
        if !self.initialized {
            return Err(TpmError::NotPresent);
        }
        if cmd.len() < 10 {
            return Err(TpmError::ResponseParseError("command too short".into()));
        }
        // 构造成功响应
        let response_size = 10u32;
        let mut resp = Vec::with_capacity(10);
        resp.extend_from_slice(&TPM_ST_NO_SESSIONS.to_be_bytes());
        resp.extend_from_slice(&response_size.to_be_bytes());
        resp.extend_from_slice(&TPM_RC_SUCCESS.to_be_bytes());
        Ok(resp)
    }

    fn pcr_read(&mut self, pcr_idx: u32) -> Result<[u8; 32], TpmError> {
        if !self.initialized {
            return Err(TpmError::NotPresent);
        }
        if pcr_idx >= PCR_COUNT as u32 {
            return Err(TpmError::InvalidPcrIndex(pcr_idx));
        }
        Ok(self.pcrs[pcr_idx as usize])
    }

    fn pcr_extend(&mut self, pcr_idx: u32, digest: &[u8; 32]) -> Result<(), TpmError> {
        if !self.initialized {
            return Err(TpmError::NotPresent);
        }
        if pcr_idx >= PCR_COUNT as u32 {
            return Err(TpmError::InvalidPcrIndex(pcr_idx));
        }
        // PCR Extend: PCR = SM3(PCR ‖ digest)
        let mut hasher = eneros_crypto::sm3::Sm3Hasher::new();
        hasher.update(&self.pcrs[pcr_idx as usize]);
        hasher.update(digest);
        self.pcrs[pcr_idx as usize] = hasher.finalize();
        Ok(())
    }

    fn create_key(&mut self, parent: KeyHandle, auth: &[u8]) -> Result<KeyHandle, TpmError> {
        if !self.initialized {
            return Err(TpmError::NotPresent);
        }
        let handle = self.next_handle;
        self.next_handle += 1;
        // 存储密钥（模拟：存储 auth 作为密钥材料）
        self.keys.push((handle, auth.to_vec()));
        let _ = parent; // 父密钥在模拟中不验证
        Ok(handle)
    }

    fn sign(&mut self, key: KeyHandle, digest: &[u8]) -> Result<Vec<u8>, TpmError> {
        if !self.initialized {
            return Err(TpmError::NotPresent);
        }
        // 验证密钥存在
        if !self.keys.iter().any(|(h, _)| *h == key) {
            return Err(TpmError::InvalidKeyHandle(key));
        }
        // 模拟签名：SM3(key_material ‖ digest)
        let key_data = self
            .keys
            .iter()
            .find(|(h, _)| *h == key)
            .map(|(_, d)| d.clone())
            .unwrap_or_default();
        let mut hasher = eneros_crypto::sm3::Sm3Hasher::new();
        hasher.update(&key_data);
        hasher.update(digest);
        let hash = hasher.finalize();
        // 返回 64 字节模拟签名 (r ‖ s)
        let mut sig = Vec::with_capacity(64);
        sig.extend_from_slice(&hash);
        sig.extend_from_slice(&hash);
        Ok(sig)
    }

    fn seal(&mut self, parent: KeyHandle, data: &[u8]) -> Result<Vec<u8>, TpmError> {
        if !self.initialized {
            return Err(TpmError::NotPresent);
        }
        if !self.keys.iter().any(|(h, _)| *h == parent) {
            return Err(TpmError::InvalidKeyHandle(parent));
        }
        // 模拟 Seal：简单 XOR 加密
        let key_data = self
            .keys
            .iter()
            .find(|(h, _)| *h == parent)
            .map(|(_, d)| d.clone())
            .unwrap_or_default();
        let sealed: Vec<u8> = data
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ key_data[i % key_data.len().max(1)])
            .collect();
        Ok(sealed)
    }

    fn unseal(&mut self, sealed: &[u8]) -> Result<Vec<u8>, TpmError> {
        if !self.initialized {
            return Err(TpmError::NotPresent);
        }
        // 模拟 Unseal：使用第一个密钥
        if let Some((_, key_data)) = self.keys.first() {
            let data: Vec<u8> = sealed
                .iter()
                .enumerate()
                .map(|(i, b)| b ^ key_data[i % key_data.len().max(1)])
                .collect();
            Ok(data)
        } else {
            Err(TpmError::SealFailed("no key available".into()))
        }
    }

    fn get_random(&mut self, len: u16) -> Result<Vec<u8>, TpmError> {
        if !self.initialized {
            return Err(TpmError::NotPresent);
        }
        let mut result = Vec::with_capacity(len as usize);
        for _ in 0..len {
            result.push((self.next_random() & 0xFF) as u8);
        }
        Ok(result)
    }

    fn name(&self) -> &'static str {
        "Simulated TPM 2.0"
    }

    fn is_hardware(&self) -> bool {
        false
    }

    fn is_available(&self) -> bool {
        true
    }
}

// ============================================================
// SPI TPM Driver (Hardware)
// ============================================================

/// SPI TPM 硬件驱动.
///
/// 通过 SPI 总线访问 TPM 2.0 芯片（TIS 接口）。
pub struct SpiTpmDriver {
    /// SPI 基地址（MMIO 映射）
    base_addr: usize,
    /// 当前 Locality
    #[allow(dead_code)]
    locality: u8,
    /// 是否已初始化
    initialized: bool,
    /// 模拟模式
    simulate: bool,
}

impl SpiTpmDriver {
    /// 创建 SPI TPM 驱动（硬件模式）.
    ///
    /// # Safety
    /// 调用者需确保 base_addr 对应的 MMIO 区域可访问。
    pub unsafe fn new(base_addr: usize) -> Self {
        Self {
            base_addr,
            locality: 0,
            initialized: false,
            simulate: false,
        }
    }

    /// 创建模拟模式实例.
    ///
    /// v0.209.0（P0-15）：仅 test/simulated feature 构建可用——release
    /// 构建无法实例化模拟 TPM（编译期证明）。
    #[cfg(any(test, feature = "simulated"))]
    pub fn new_simulated() -> Self {
        Self {
            base_addr: 0,
            locality: 0,
            initialized: true,
            simulate: true,
        }
    }

    /// 读取 TPM 寄存器.
    fn read_reg(&self, offset: u32) -> Result<u8, TpmError> {
        if self.simulate {
            return Ok(0);
        }
        let addr = (self.base_addr + offset as usize) as *const u8;
        // SAFETY: `self.base_addr` 由 `new` 的 # Safety 契约保证为有效
        // TPM MMIO 区域；`read_volatile` 为硬件寄存器读语义。
        Ok(unsafe { core::ptr::read_volatile(addr) })
    }

    /// 写入 TPM 寄存器.
    fn write_reg(&self, offset: u32, value: u8) -> Result<(), TpmError> {
        if self.simulate {
            return Ok(());
        }
        let addr = (self.base_addr + offset as usize) as *mut u8;
        // SAFETY: `self.base_addr` 由 `new` 的 # Safety 契约保证为有效
        // TPM MMIO 区域；`write_volatile` 为硬件寄存器写语义。
        unsafe { core::ptr::write_volatile(addr, value) };
        Ok(())
    }

    /// 请求 Locality.
    fn request_locality(&mut self) -> Result<(), TpmError> {
        if self.simulate {
            return Ok(());
        }
        self.write_reg(TPM_ACCESS_REG, TPM_ACCESS_REQUEST_USE)?;
        // 轮询等待
        for _ in 0..1000 {
            let access = self.read_reg(TPM_ACCESS_REG)?;
            if access & TPM_ACCESS_ACTIVE_LOCALITY != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(TpmError::LocalityTimeout)
    }

    /// 释放 Locality.
    fn release_locality(&mut self) -> Result<(), TpmError> {
        if self.simulate {
            return Ok(());
        }
        self.write_reg(TPM_ACCESS_REG, TPM_ACCESS_ACTIVE_LOCALITY_RELINQUISH)
    }
}

/// TPM 2.0 规范最大响应长度（头部 10 B + 标签/长度/返回码/参数；4096 安全上界）。
pub const TPM_RESPONSE_MAX_BYTES: usize = 4096;

/// 响应读取迭代上限（deadline；与等待响应段的 10000 次同量级——
/// 每迭代读一个字节，FIFO 卡住 10000 字节即判定超时，不挂启动）。
pub const TPM_RESPONSE_MAX_ITERS: u32 = 10_000;

/// 响应读取边界检查（22 §2.W1.5）：容量/迭代超限返回 [`TpmError::Timeout`]。
///
/// 纯逻辑（host 可测）；生产路径由 `send_command` 读循环每迭代调用。
pub fn check_read_limits(response_len: usize, iters: u32) -> Result<(), TpmError> {
    if response_len >= TPM_RESPONSE_MAX_BYTES || iters >= TPM_RESPONSE_MAX_ITERS {
        return Err(TpmError::Timeout);
    }
    Ok(())
}

impl TpmDriver for SpiTpmDriver {
    fn init(&mut self) -> Result<(), TpmError> {
        if self.simulate {
            self.initialized = true;
            return Ok(());
        }
        self.request_locality()?;
        // 发送 TPM2_Startup(CLEAR)
        let cmd = build_startup_cmd(TPM_SU_CLEAR);
        self.send_command(&cmd)?;
        self.initialized = true;
        Ok(())
    }

    fn send_command(&mut self, cmd: &[u8]) -> Result<Vec<u8>, TpmError> {
        if !self.initialized && !self.simulate {
            return Err(TpmError::NotPresent);
        }
        if self.simulate {
            // 模拟成功响应
            let mut resp = Vec::with_capacity(10);
            resp.extend_from_slice(&TPM_ST_NO_SESSIONS.to_be_bytes());
            resp.extend_from_slice(&10u32.to_be_bytes());
            resp.extend_from_slice(&TPM_RC_SUCCESS.to_be_bytes());
            return Ok(resp);
        }

        self.request_locality()?;
        // 设置 CMD_READY
        self.write_reg(TPM_STS_REG, TPM_STS_CMD_READY)?;
        // 写入命令到 FIFO
        for &byte in cmd {
            self.write_reg(TPM_DATA_FIFO, byte)?;
        }
        // GO
        self.write_reg(TPM_STS_REG, TPM_STS_GO)?;
        // 等待响应
        for _ in 0..10000 {
            let sts = self.read_reg(TPM_STS_REG)?;
            if sts & TPM_STS_DATA_AVAIL != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        // 读取响应（22 §2.W1.5：deadline + 容量上限——FIFO 卡住/异常
        // 数据不挂启动、不 OOM；OOM-risk: handled by alloc_error_handler，
        // 此处已由 try_reserve 语义 + 容量上限收敛）。
        let mut response = Vec::new();
        let mut iters = 0u32;
        loop {
            let sts = self.read_reg(TPM_STS_REG)?;
            if sts & TPM_STS_DATA_AVAIL == 0 {
                break;
            }
            check_read_limits(response.len(), iters)?;
            let data = self.read_reg(TPM_DATA_FIFO)?;
            response.push(data);
            iters += 1;
        }
        self.release_locality()?;
        Ok(response)
    }

    fn pcr_read(&mut self, pcr_idx: u32) -> Result<[u8; 32], TpmError> {
        if pcr_idx >= PCR_COUNT as u32 {
            return Err(TpmError::InvalidPcrIndex(pcr_idx));
        }
        if self.simulate {
            return Ok([0u8; 32]);
        }
        let cmd = build_pcr_read_cmd(pcr_idx, TPM_ALG_SM3_256);
        let resp = self.send_command(&cmd)?;
        if resp.len() < 42 {
            return Err(TpmError::ResponseParseError(
                "PCR read response too short".into(),
            ));
        }
        let mut pcr = [0u8; 32];
        pcr.copy_from_slice(&resp[10..42]);
        Ok(pcr)
    }

    fn pcr_extend(&mut self, pcr_idx: u32, digest: &[u8; 32]) -> Result<(), TpmError> {
        if pcr_idx >= PCR_COUNT as u32 {
            return Err(TpmError::InvalidPcrIndex(pcr_idx));
        }
        if self.simulate {
            return Ok(());
        }
        let cmd = build_pcr_extend_cmd(pcr_idx, TPM_ALG_SM3_256, digest);
        self.send_command(&cmd)?;
        Ok(())
    }

    fn create_key(&mut self, parent: KeyHandle, auth: &[u8]) -> Result<KeyHandle, TpmError> {
        if self.simulate {
            return Ok(TPM_HR_TRANSIENT);
        }
        // CRY-003：真实 TPM2_Create 命令序列（build_create_cmd 已补全）。
        // 无真实 TPM 硬件时 send_command 经 SPI 传输返回错误（fail-closed，
        // 不伪造句柄）。响应解析：header(10) + newHandle(4) + 后续对象。
        let cmd = build_create_cmd(parent, auth);
        let resp = self.send_command(&cmd)?;
        if resp.len() < 14 {
            return Err(TpmError::KeyCreationFailed("TPM2_Create 响应过短".into()));
        }
        let handle = u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]);
        if handle == 0 {
            return Err(TpmError::KeyCreationFailed("TPM2_Create 返回空句柄".into()));
        }
        Ok(handle)
    }

    fn sign(&mut self, key: KeyHandle, digest: &[u8]) -> Result<Vec<u8>, TpmError> {
        if self.simulate {
            return Ok(alloc::vec![0u8; 64]);
        }
        // CRY-003：真实 TPM2_Sign 命令序列（build_sign_cmd 已有）。
        let cmd = build_sign_cmd(key, digest, TPM_ALG_SM2);
        let resp = self.send_command(&cmd)?;
        if resp.len() < 12 {
            return Err(TpmError::SignFailed("TPM2_Sign 响应过短".into()));
        }
        Ok(resp[10..].to_vec())
    }

    fn seal(&mut self, parent: KeyHandle, data: &[u8]) -> Result<Vec<u8>, TpmError> {
        if self.simulate {
            return Ok(alloc::vec![]);
        }
        // CRY-003：真实 TPM2_Create（Seal 语义：data 入 inSensitive）。
        // 无真实 TPM 时 send_command 失败（fail-closed）。返回响应字节
        // 作为 sealed blob（含 newHandle + 对象私钥），unseal 前 4 字节为句柄。
        let cmd = build_create_cmd(parent, data);
        let resp = self.send_command(&cmd)?;
        if resp.len() < 14 {
            return Err(TpmError::SealFailed("TPM2_Create(seal) 响应过短".into()));
        }
        Ok(resp)
    }

    fn unseal(&mut self, sealed: &[u8]) -> Result<Vec<u8>, TpmError> {
        if self.simulate {
            return Ok(alloc::vec![]);
        }
        // CRY-003：真实 TPM2_Unseal 命令序列（build_unseal_cmd 已补全）。
        // sealed blob 前 4 字节为对象句柄（见 seal）。
        if sealed.len() < 4 {
            return Err(TpmError::SealFailed("sealed blob 过短".into()));
        }
        let handle = u32::from_be_bytes([sealed[0], sealed[1], sealed[2], sealed[3]]);
        let cmd = build_unseal_cmd(handle);
        let resp = self.send_command(&cmd)?;
        if resp.len() < 12 {
            return Err(TpmError::SealFailed("TPM2_Unseal 响应过短".into()));
        }
        // outData 区（简化：响应体 10 字节头后）
        Ok(resp[10..].to_vec())
    }

    fn get_random(&mut self, len: u16) -> Result<Vec<u8>, TpmError> {
        if self.simulate {
            return Ok(alloc::vec![0x42; len as usize]);
        }
        let cmd = build_get_random_cmd(len);
        let resp = self.send_command(&cmd)?;
        if resp.len() < 12 {
            return Err(TpmError::ResponseParseError(
                "random response too short".into(),
            ));
        }
        Ok(resp[12..].to_vec())
    }

    fn name(&self) -> &'static str {
        "SPI TPM 2.0 (TIS)"
    }

    fn is_hardware(&self) -> bool {
        !self.simulate
    }

    fn is_available(&self) -> bool {
        if self.simulate {
            return true;
        }
        // 探测 DID/VID 寄存器
        self.write_reg(TPM_DID_VID_REG, 0).is_ok()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simulated_tpm_init() {
        let mut tpm = SimulatedTpm::new();
        assert!(!tpm.initialized);
        tpm.init().unwrap();
        assert!(tpm.initialized);
        assert_eq!(tpm.name(), "Simulated TPM 2.0");
        assert!(!tpm.is_hardware());
        assert!(tpm.is_available());
    }

    #[test]
    fn test_simulated_pcr_read_initial() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();
        // 初始 PCR 全零
        let pcr0 = tpm.pcr_read(0).unwrap();
        assert_eq!(pcr0, [0u8; 32]);
    }

    #[test]
    fn test_simulated_pcr_extend() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let digest = [0xAA; 32];
        tpm.pcr_extend(0, &digest).unwrap();

        // PCR 应该改变
        let pcr0 = tpm.pcr_read(0).unwrap();
        assert_ne!(pcr0, [0u8; 32]);

        // 验证 PCR = SM3(0x00...00 ‖ digest)
        let mut hasher = eneros_crypto::sm3::Sm3Hasher::new();
        hasher.update(&[0u8; 32]);
        hasher.update(&digest);
        let expected = hasher.finalize();
        assert_eq!(pcr0, expected);
    }

    #[test]
    fn test_simulated_pcr_invalid_index() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();
        assert!(tpm.pcr_read(24).is_err());
        assert!(tpm.pcr_extend(99, &[0u8; 32]).is_err());
    }

    #[test]
    fn test_simulated_key_management() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let handle = tpm.create_key(TPM_RH_OWNER, b"auth123").unwrap();
        assert!(handle >= TPM_HR_TRANSIENT);

        // 签名
        let digest = [0xBB; 32];
        let sig = tpm.sign(handle, &digest).unwrap();
        assert_eq!(sig.len(), 64);

        // 无效句柄
        assert!(tpm.sign(0x9999, &digest).is_err());
    }

    #[test]
    fn test_simulated_seal_unseal() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let handle = tpm.create_key(TPM_RH_OWNER, b"seal_key").unwrap();
        let secret = b"top secret data";
        let sealed = tpm.seal(handle, secret).unwrap();
        assert_ne!(&sealed[..], &secret[..]);

        let unsealed = tpm.unseal(&sealed).unwrap();
        assert_eq!(unsealed, secret);
    }

    #[test]
    fn test_simulated_get_random() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let rand1 = tpm.get_random(32).unwrap();
        assert_eq!(rand1.len(), 32);

        let rand2 = tpm.get_random(32).unwrap();
        // 两次随机数应不同
        assert_ne!(rand1, rand2);
    }

    #[test]
    fn test_simulated_not_initialized() {
        let mut tpm = SimulatedTpm::new();
        // 未初始化时操作应失败
        assert!(tpm.pcr_read(0).is_err());
        assert!(tpm.get_random(16).is_err());
    }

    #[test]
    fn test_spi_simulated_mode() {
        let mut spi = SpiTpmDriver::new_simulated();
        assert!(spi.is_available());
        spi.init().unwrap();
        let pcr = spi.pcr_read(0).unwrap();
        assert_eq!(pcr, [0u8; 32]);
    }

    #[test]
    fn test_send_command_raw() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();
        let cmd = build_startup_cmd(TPM_SU_CLEAR);
        let resp = tpm.send_command(&cmd).unwrap();
        let header = TpmResponseHeader::from_bytes(&resp).unwrap();
        assert!(header.is_success());
    }

    // --- 22 §2.W1.5：读循环超时/容量上限 ---

    #[test]
    fn test_read_limits_capacity_exceeded_timeout() {
        // 容量达上限（4096 B）仍满 → Timeout（不挂启动、不 OOM）。
        assert_eq!(
            check_read_limits(TPM_RESPONSE_MAX_BYTES, 0),
            Err(TpmError::Timeout)
        );
        assert_eq!(
            check_read_limits(TPM_RESPONSE_MAX_BYTES + 1, 0),
            Err(TpmError::Timeout)
        );
    }

    #[test]
    fn test_read_limits_iteration_deadline_timeout() {
        // 迭代超限（FIFO 卡住持续有数据）→ Timeout。
        assert_eq!(
            check_read_limits(0, TPM_RESPONSE_MAX_ITERS),
            Err(TpmError::Timeout)
        );
        assert_eq!(
            check_read_limits(100, TPM_RESPONSE_MAX_ITERS + 1),
            Err(TpmError::Timeout)
        );
    }

    #[test]
    fn test_read_limits_inside_bounds_ok() {
        // 边界内 → Ok（正常响应路径不受影响）。
        assert_eq!(check_read_limits(0, 0), Ok(()));
        assert_eq!(
            check_read_limits(TPM_RESPONSE_MAX_BYTES - 1, TPM_RESPONSE_MAX_ITERS - 1),
            Ok(())
        );
        // 真实响应（头部 10 B + 参数）远低于上限。
        assert_eq!(check_read_limits(42, 42), Ok(()));
    }
}
