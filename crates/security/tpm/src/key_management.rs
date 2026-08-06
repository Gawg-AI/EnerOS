//! TPM 密钥层次管理 (workspace v0.192.0).
//!
//! 管理 TPM 密钥层次结构：EK → SRK → AK → Agent Keys。
//! 密钥在 TPM 内部生成和使用，私钥不可导出。

use alloc::vec::Vec;

use crate::commands::*;
use crate::driver::{KeyHandle, TpmDriver};
use crate::error::TpmError;

// ============================================================
// Key Hierarchy
// ============================================================

/// TPM 密钥层次结构.
///
/// 按照 TCG 规范定义密钥层次：
/// - EK (Endorsement Key): 硬件内置，不可导出
/// - SRK (Storage Root Key): 存储密钥根
/// - AK (Attestation Key): 证明密钥
/// - Agent Keys: Agent 签名密钥
#[derive(Debug, Clone)]
pub struct KeyHierarchy {
    /// 背书密钥句柄（硬件内置）
    pub ek: Option<KeyHandle>,
    /// 存储根密钥句柄
    pub srk: Option<KeyHandle>,
    /// 证明密钥句柄
    pub ak: Option<KeyHandle>,
    /// Agent 签名密钥句柄列表
    pub agent_keys: Vec<KeyHandle>,
}

impl KeyHierarchy {
    /// 创建空密钥层次.
    pub fn new() -> Self {
        Self {
            ek: None,
            srk: None,
            ak: None,
            agent_keys: Vec::new(),
        }
    }

    /// 密钥层次是否完整（EK + SRK + AK）.
    pub fn is_complete(&self) -> bool {
        self.ek.is_some() && self.srk.is_some() && self.ak.is_some()
    }

    /// 已创建的 Agent 密钥数量.
    pub fn agent_key_count(&self) -> usize {
        self.agent_keys.len()
    }
}

impl Default for KeyHierarchy {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Key Manager
// ============================================================

/// TPM 密钥管理器.
///
/// 管理密钥的创建、加载、签名和封装操作。
pub struct KeyManager<'a> {
    driver: &'a mut dyn TpmDriver,
    hierarchy: KeyHierarchy,
}

impl<'a> KeyManager<'a> {
    /// 创建密钥管理器.
    pub fn new(driver: &'a mut dyn TpmDriver) -> Self {
        Self {
            driver,
            hierarchy: KeyHierarchy::new(),
        }
    }

    /// 获取当前密钥层次.
    pub fn hierarchy(&self) -> &KeyHierarchy {
        &self.hierarchy
    }

    /// 初始化密钥层次（创建 SRK 和 AK）.
    pub fn init_hierarchy(&mut self, auth: &[u8]) -> Result<(), TpmError> {
        // 创建 SRK（Storage Root Key）
        let srk = self.driver.create_key(TPM_RH_OWNER, auth)?;
        self.hierarchy.srk = Some(srk);

        // 创建 AK（Attestation Key）
        let ak = self.driver.create_key(srk, auth)?;
        self.hierarchy.ak = Some(ak);

        Ok(())
    }

    /// 创建 Agent 签名密钥.
    pub fn create_agent_key(&mut self, auth: &[u8]) -> Result<KeyHandle, TpmError> {
        let parent = self
            .hierarchy
            .srk
            .ok_or_else(|| TpmError::KeyCreationFailed("SRK not initialized".into()))?;
        let handle = self.driver.create_key(parent, auth)?;
        self.hierarchy.agent_keys.push(handle);
        Ok(handle)
    }

    /// 使用指定密钥签名.
    pub fn sign(&mut self, key: KeyHandle, data: &[u8]) -> Result<Vec<u8>, TpmError> {
        // 先计算 SM3 摘要
        let digest = eneros_crypto::sm3::hash(data);
        self.driver.sign(key, &digest)
    }

    /// 使用 AK 签名（证明）.
    pub fn attest(&mut self, data: &[u8]) -> Result<Vec<u8>, TpmError> {
        let ak = self.hierarchy.ak.ok_or(TpmError::InvalidKeyHandle(0))?;
        self.sign(ak, data)
    }

    /// 封装数据到 SRK（绑定 PCR 状态）.
    pub fn seal(&mut self, data: &[u8]) -> Result<Vec<u8>, TpmError> {
        let srk = self
            .hierarchy
            .srk
            .ok_or_else(|| TpmError::SealFailed("SRK not initialized".into()))?;
        self.driver.seal(srk, data)
    }

    /// 解封数据.
    pub fn unseal(&mut self, sealed: &[u8]) -> Result<Vec<u8>, TpmError> {
        self.driver.unseal(sealed)
    }

    /// 获取 TPM 随机数.
    pub fn get_random(&mut self, len: u16) -> Result<Vec<u8>, TpmError> {
        self.driver.get_random(len)
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::SimulatedTpm;

    #[test]
    fn test_key_hierarchy_new() {
        let kh = KeyHierarchy::new();
        assert!(!kh.is_complete());
        assert_eq!(kh.agent_key_count(), 0);
    }

    #[test]
    fn test_key_manager_init_hierarchy() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = KeyManager::new(&mut tpm);
        mgr.init_hierarchy(b"owner_auth").unwrap();

        assert!(mgr.hierarchy().srk.is_some());
        assert!(mgr.hierarchy().ak.is_some());
    }

    #[test]
    fn test_key_manager_create_agent_key() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = KeyManager::new(&mut tpm);
        mgr.init_hierarchy(b"auth").unwrap();

        let key1 = mgr.create_agent_key(b"agent1_auth").unwrap();
        let key2 = mgr.create_agent_key(b"agent2_auth").unwrap();
        assert_ne!(key1, key2);
        assert_eq!(mgr.hierarchy().agent_key_count(), 2);
    }

    #[test]
    fn test_key_manager_sign() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = KeyManager::new(&mut tpm);
        mgr.init_hierarchy(b"auth").unwrap();
        let key = mgr.create_agent_key(b"sign_key").unwrap();

        let sig = mgr.sign(key, b"message to sign").unwrap();
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn test_key_manager_attest() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = KeyManager::new(&mut tpm);
        mgr.init_hierarchy(b"auth").unwrap();

        let sig = mgr.attest(b"platform state").unwrap();
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn test_key_manager_seal_unseal() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = KeyManager::new(&mut tpm);
        mgr.init_hierarchy(b"seal_auth").unwrap();

        let secret = b"very secret key material";
        let sealed = mgr.seal(secret).unwrap();
        let unsealed = mgr.unseal(&sealed).unwrap();
        assert_eq!(unsealed, secret);
    }

    #[test]
    fn test_key_manager_no_srk_error() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = KeyManager::new(&mut tpm);
        // 未初始化 hierarchy 时创建 agent key 应失败
        assert!(mgr.create_agent_key(b"auth").is_err());
        assert!(mgr.seal(b"data").is_err());
    }

    #[test]
    fn test_key_manager_get_random() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = KeyManager::new(&mut tpm);
        let rand = mgr.get_random(32).unwrap();
        assert_eq!(rand.len(), 32);
    }
}
