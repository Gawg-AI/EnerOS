//! 密钥存储与管理 (v0.24.0 — Task 2)
//!
//! 提供密钥存储抽象 trait `KeyStore` 及两个后端：
//! - `SoftwareKeyStore`: 基于 AES-256-GCM 加密的文件存储（默认，跨平台）
//! - `TpmKeyStore`: TPM 硬件密钥存储（stub，需 feature = "tpm" + Linux）
//!
//! 加密方案：
//! - 密钥派生：PBKDF2-HMAC-SHA256(passphrase, salt, 100000 iterations, 32 bytes)
//! - 加密：AES-256-GCM(derived_key, nonce, plaintext)
//! - 存储格式：salt(16) || nonce(12) || ciphertext || tag(16)

use aes_gcm::{
    aead::{generic_array::GenericArray, Aead, KeyInit},
    Aes256Gcm,
};
use chrono::{DateTime, Utc};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use thiserror::Error;

/// 加密参数
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const PBKDF2_ITERATIONS: u32 = 100_000;

/// 密钥存储错误
#[derive(Debug, Error)]
pub enum KeyStoreError {
    #[error("key not found: {0}")]
    KeyNotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("decryption error: {0}")]
    Decryption(String),
    #[error("invalid key format: {0}")]
    InvalidFormat(String),
    #[error("unsupported on this platform")]
    Unsupported,
    #[error("TPM error: {0}")]
    Tpm(String),
}

/// 密钥版本状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum KeyStatus {
    Active,
    Retired,
    Revoked,
}

/// 密钥版本信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyVersion {
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub status: KeyStatus,
}

/// 密钥元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInfo {
    pub key_id: String,
    pub versions: Vec<KeyVersion>,
}

/// 密钥存储 trait — 抽象不同后端（软件文件存储 / TPM 硬件）
pub trait KeyStore: Send + Sync {
    /// 存储密钥（创建新版本，旧 Active 版本标记为 Retired）
    fn store(&self, key_id: &str, key_data: &[u8]) -> Result<KeyVersion, KeyStoreError>;

    /// 加载密钥（None 加载 Active 版本，Some(v) 加载特定版本）
    fn load(&self, key_id: &str, version: Option<u32>) -> Result<Vec<u8>, KeyStoreError>;

    /// 轮换密钥（保留相同密钥数据，创建新版本，旧版本标记为 Retired）
    fn rotate(&self, key_id: &str) -> Result<KeyVersion, KeyStoreError>;

    /// 列出所有密钥
    fn list_keys(&self) -> Vec<KeyInfo>;

    /// 删除特定版本
    fn delete(&self, key_id: &str, version: u32) -> Result<(), KeyStoreError>;

    /// 返回 KeyStore 类型名称
    fn backend_name(&self) -> &'static str;
}

// ============================================================================
// SoftwareKeyStore — 基于 AES-256-GCM 加密的文件存储（默认实现，跨平台）
// ============================================================================

/// 软件密钥存储 — 使用 AES-256-GCM 加密密钥数据，元数据存储在 keys.json
pub struct SoftwareKeyStore {
    base_dir: PathBuf,
    passphrase: Vec<u8>,
    metadata: RwLock<HashMap<String, KeyInfo>>,
}

impl SoftwareKeyStore {
    /// 创建新的软件密钥存储
    pub fn new(base_dir: PathBuf, passphrase: Vec<u8>) -> Result<Self, KeyStoreError> {
        std::fs::create_dir_all(&base_dir)?;
        let store = Self {
            base_dir,
            passphrase,
            metadata: RwLock::new(HashMap::new()),
        };
        store.load_metadata()?;
        Ok(store)
    }

    /// 加载元数据文件（keys.json）。文件不存在时初始化空元数据。
    fn load_metadata(&self) -> Result<(), KeyStoreError> {
        let path = self.metadata_file_path();
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&path)?;
        let map: HashMap<String, KeyInfo> = serde_json::from_str(&content)
            .map_err(|e| KeyStoreError::InvalidFormat(format!("deserialize metadata: {e}")))?;
        let mut metadata = self.metadata.write().unwrap();
        *metadata = map;
        Ok(())
    }

    /// 将元数据写入磁盘（静态方法，调用方负责持锁）
    fn save_metadata_to_disk(
        metadata: &HashMap<String, KeyInfo>,
        path: &std::path::Path,
    ) -> Result<(), KeyStoreError> {
        let json = serde_json::to_string_pretty(metadata)
            .map_err(|e| KeyStoreError::InvalidFormat(format!("serialize metadata: {e}")))?;
        std::fs::write(path, json)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// 加密密钥数据（AES-256-GCM）
    /// 存储格式：salt(16) || nonce(12) || ciphertext || tag(16)
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, KeyStoreError> {
        // 生成随机 salt
        let mut salt = [0u8; SALT_LEN];
        fill_random(&mut salt)?;

        // PBKDF2 派生密钥
        let mut derived_key = [0u8; KEY_LEN];
        pbkdf2_hmac::<Sha256>(
            &self.passphrase,
            &salt,
            PBKDF2_ITERATIONS,
            &mut derived_key,
        );

        // 生成随机 nonce
        let mut nonce_bytes = [0u8; NONCE_LEN];
        fill_random(&mut nonce_bytes)?;

        // AES-256-GCM 加密
        let key = GenericArray::from_slice(&derived_key);
        let cipher = Aes256Gcm::new(key);
        let nonce = GenericArray::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| KeyStoreError::Encryption(format!("AES-GCM encrypt: {e}")))?;

        // 拼接输出：salt || nonce || ciphertext(含 tag)
        let mut output = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
        output.extend_from_slice(&salt);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    /// 解密密钥数据
    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, KeyStoreError> {
        if data.len() < SALT_LEN + NONCE_LEN {
            return Err(KeyStoreError::Decryption("data too short".into()));
        }

        let salt = &data[..SALT_LEN];
        let nonce_slice = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
        let ciphertext = &data[SALT_LEN + NONCE_LEN..];

        // PBKDF2 派生密钥
        let mut derived_key = [0u8; KEY_LEN];
        pbkdf2_hmac::<Sha256>(
            &self.passphrase,
            salt,
            PBKDF2_ITERATIONS,
            &mut derived_key,
        );

        // AES-256-GCM 解密
        let key = GenericArray::from_slice(&derived_key);
        let cipher = Aes256Gcm::new(key);

        let nonce_arr: [u8; NONCE_LEN] = nonce_slice
            .try_into()
            .map_err(|_| KeyStoreError::Decryption("invalid nonce length".into()))?;
        let nonce = GenericArray::from_slice(&nonce_arr);

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| KeyStoreError::Decryption(format!("AES-GCM decrypt: {e}")))
    }

    /// 密钥文件路径：base_dir/{key_id}_v{version}.enc
    fn key_file_path(&self, key_id: &str, version: u32) -> PathBuf {
        self.base_dir.join(format!("{}_v{}.enc", key_id, version))
    }

    /// 元数据文件路径：base_dir/keys.json
    fn metadata_file_path(&self) -> PathBuf {
        self.base_dir.join("keys.json")
    }

    /// 写入密钥文件并设置权限（Linux 0600）
    fn write_key_file(&self, path: &std::path::Path, data: &[u8]) -> Result<(), KeyStoreError> {
        std::fs::write(path, data)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

impl KeyStore for SoftwareKeyStore {
    fn store(&self, key_id: &str, key_data: &[u8]) -> Result<KeyVersion, KeyStoreError> {
        let mut metadata = self.metadata.write().unwrap();

        // 确定新版本号（现有最大版本 + 1，无则 1）
        let new_version = metadata
            .get(key_id)
            .map(|info| info.versions.iter().map(|v| v.version).max().unwrap_or(0) + 1)
            .unwrap_or(1);

        // 加密并写入文件
        let enc_data = self.encrypt(key_data)?;
        let path = self.key_file_path(key_id, new_version);
        self.write_key_file(&path, &enc_data)?;

        // 更新元数据：旧 Active 标记 Retired，新增 Active 版本
        let key_info = metadata.entry(key_id.to_string()).or_insert_with(|| KeyInfo {
            key_id: key_id.to_string(),
            versions: Vec::new(),
        });
        for v in &mut key_info.versions {
            if v.status == KeyStatus::Active {
                v.status = KeyStatus::Retired;
            }
        }
        let new_kv = KeyVersion {
            version: new_version,
            created_at: Utc::now(),
            status: KeyStatus::Active,
        };
        key_info.versions.push(new_kv.clone());

        // 保存元数据
        Self::save_metadata_to_disk(&metadata, &self.metadata_file_path())?;

        Ok(new_kv)
    }

    fn load(&self, key_id: &str, version: Option<u32>) -> Result<Vec<u8>, KeyStoreError> {
        let metadata = self.metadata.read().unwrap();

        let key_info = metadata
            .get(key_id)
            .ok_or_else(|| KeyStoreError::KeyNotFound(key_id.to_string()))?;

        // 确定版本：指定版本或 Active 版本
        let ver = match version {
            Some(v) => v,
            None => key_info
                .versions
                .iter()
                .find(|v| v.status == KeyStatus::Active)
                .map(|v| v.version)
                .ok_or_else(|| {
                    KeyStoreError::KeyNotFound(format!("{}: no active version", key_id))
                })?,
        };

        // 验证版本存在
        if !key_info.versions.iter().any(|v| v.version == ver) {
            return Err(KeyStoreError::KeyNotFound(format!(
                "{}: version {}",
                key_id, ver
            )));
        }

        let path = self.key_file_path(key_id, ver);
        // 释放读锁后再进行文件 IO
        drop(metadata);

        let enc_data = std::fs::read(&path)?;
        self.decrypt(&enc_data)
    }

    fn rotate(&self, key_id: &str) -> Result<KeyVersion, KeyStoreError> {
        let mut metadata = self.metadata.write().unwrap();

        let key_info = metadata
            .get(key_id)
            .ok_or_else(|| KeyStoreError::KeyNotFound(key_id.to_string()))?;

        // 查找当前 Active 版本
        let active_version = key_info
            .versions
            .iter()
            .find(|v| v.status == KeyStatus::Active)
            .map(|v| v.version)
            .ok_or_else(|| {
                KeyStoreError::KeyNotFound(format!("{}: no active version", key_id))
            })?;

        // 读取并解密当前 Active 版本的密钥数据
        let enc_path = self.key_file_path(key_id, active_version);
        let enc_data = std::fs::read(&enc_path)?;
        let plaintext = self.decrypt(&enc_data)?;

        // 确定新版本号
        let new_version = key_info.versions.iter().map(|v| v.version).max().unwrap_or(0) + 1;

        // 重新加密并写入新文件
        let new_enc_data = self.encrypt(&plaintext)?;
        let new_path = self.key_file_path(key_id, new_version);
        self.write_key_file(&new_path, &new_enc_data)?;

        // 更新元数据：旧 Active 标记 Retired，新增 Active 版本
        let key_info = metadata.get_mut(key_id).unwrap();
        for v in &mut key_info.versions {
            if v.status == KeyStatus::Active {
                v.status = KeyStatus::Retired;
            }
        }
        let new_kv = KeyVersion {
            version: new_version,
            created_at: Utc::now(),
            status: KeyStatus::Active,
        };
        key_info.versions.push(new_kv.clone());

        Self::save_metadata_to_disk(&metadata, &self.metadata_file_path())?;

        Ok(new_kv)
    }

    fn list_keys(&self) -> Vec<KeyInfo> {
        let metadata = self.metadata.read().unwrap();
        metadata.values().cloned().collect()
    }

    fn delete(&self, key_id: &str, version: u32) -> Result<(), KeyStoreError> {
        let mut metadata = self.metadata.write().unwrap();

        let key_info = metadata
            .get_mut(key_id)
            .ok_or_else(|| KeyStoreError::KeyNotFound(key_id.to_string()))?;

        // 验证版本存在
        if !key_info.versions.iter().any(|v| v.version == version) {
            return Err(KeyStoreError::KeyNotFound(format!(
                "{}: version {}",
                key_id, version
            )));
        }

        // 从元数据中移除该版本
        key_info.versions.retain(|v| v.version != version);

        // 删除加密文件
        let path = self.key_file_path(key_id, version);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }

        // 若无剩余版本，移除整个 key
        if key_info.versions.is_empty() {
            metadata.remove(key_id);
        }

        Self::save_metadata_to_disk(&metadata, &self.metadata_file_path())?;

        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "software"
    }
}

// ============================================================================
// TpmKeyStore — TPM 硬件密钥存储（stub，真实实现需 feature = "tpm" + Linux）
// ============================================================================

/// TPM 密钥存储（stub — 当前未编译真实 TPM 支持）
pub struct TpmKeyStore;

impl TpmKeyStore {
    /// 创建 TPM 密钥存储。当前 stub 始终返回错误。
    pub fn new() -> Result<Self, KeyStoreError> {
        Err(KeyStoreError::Tpm(
            "TPM support not compiled in".to_string(),
        ))
    }
}

// 当 feature = "tpm" 且 Linux 时，在此处实现真实 TpmKeyStore（未来扩展）
// 当前 stub 不实现 KeyStore trait

// ============================================================================
// detect_keystore 工厂函数
// ============================================================================

/// 检测可用的 KeyStore 后端
///
/// Linux + tpm feature 下检测 /dev/tpmrm0，TPM 可用返回 TpmKeyStore；
/// 否则回退到 SoftwareKeyStore。非 Linux 始终返回 SoftwareKeyStore。
pub fn detect_keystore(
    base_dir: PathBuf,
    passphrase: Vec<u8>,
) -> Result<Box<dyn KeyStore>, KeyStoreError> {
    #[cfg(all(target_os = "linux", feature = "tpm"))]
    {
        if std::path::Path::new("/dev/tpmrm0").exists() {
            match TpmKeyStore::new() {
                Ok(store) => return Ok(Box::new(store)),
                Err(e) => {
                    eprintln!(
                        "WARN: TPM init failed, falling back to software keystore: {}",
                        e
                    );
                }
            }
        }
    }

    Ok(Box::new(SoftwareKeyStore::new(base_dir, passphrase)?))
}

// ============================================================================
// 内部工具函数
// ============================================================================

#[cfg(target_os = "windows")]
#[link(name = "advapi32")]
extern "system" {
    fn SystemFunction036(random_buffer: *mut u8, random_buffer_length: u32) -> i32;
}

/// 从 OS 密码学安全随机源填充缓冲区
fn fill_random(buf: &mut [u8]) -> Result<(), KeyStoreError> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let mut file = std::fs::File::open("/dev/urandom")
            .map_err(|e| KeyStoreError::Encryption(format!("open /dev/urandom: {e}")))?;
        file.read_exact(buf)
            .map_err(|e| KeyStoreError::Encryption(format!("read /dev/urandom: {e}")))?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let ret = unsafe { SystemFunction036(buf.as_mut_ptr(), buf.len() as u32) };
        if ret == 0 {
            Err(KeyStoreError::Encryption(
                "SystemFunction036 (RtlGenRandom) failed".into(),
            ))
        } else {
            Ok(())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err(KeyStoreError::Unsupported)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建测试用临时目录（清理旧残留）
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eneros_keystore_test_{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// 清理测试目录
    fn cleanup(dir: &PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_software_keystore_store_and_load() {
        let dir = test_dir("store_and_load");
        let store = SoftwareKeyStore::new(dir.clone(), b"test_passphrase".to_vec()).unwrap();

        let key_data = b"super_secret_key_32_bytes_long!!!";
        let kv = store.store("master_key", key_data).unwrap();
        assert_eq!(kv.version, 1);
        assert_eq!(kv.status, KeyStatus::Active);

        // 加载 Active 版本
        let loaded = store.load("master_key", None).unwrap();
        assert_eq!(loaded, key_data);

        // 加载指定版本
        let loaded_v1 = store.load("master_key", Some(1)).unwrap();
        assert_eq!(loaded_v1, key_data);

        cleanup(&dir);
    }

    #[test]
    fn test_software_keystore_rotate() {
        let dir = test_dir("rotate");
        let store = SoftwareKeyStore::new(dir.clone(), b"test_passphrase".to_vec()).unwrap();

        let key_data = b"original_key_data_for_rotate!!";
        let kv1 = store.store("signing_key", key_data).unwrap();
        assert_eq!(kv1.version, 1);

        // 轮换密钥
        let kv2 = store.rotate("signing_key").unwrap();
        assert_eq!(kv2.version, 2);
        assert_eq!(kv2.status, KeyStatus::Active);

        // 验证旧版本标记为 Retired
        let keys = store.list_keys();
        let key_info = keys.iter().find(|k| k.key_id == "signing_key").unwrap();
        let v1 = key_info.versions.iter().find(|v| v.version == 1).unwrap();
        assert_eq!(v1.status, KeyStatus::Retired);
        let v2 = key_info.versions.iter().find(|v| v.version == 2).unwrap();
        assert_eq!(v2.status, KeyStatus::Active);

        // 验证轮换后数据一致（默认加载 Active = v2）
        let loaded = store.load("signing_key", None).unwrap();
        assert_eq!(loaded, key_data);

        // 验证旧版本数据也可加载
        let loaded_v1 = store.load("signing_key", Some(1)).unwrap();
        assert_eq!(loaded_v1, key_data);

        cleanup(&dir);
    }

    #[test]
    fn test_software_keystore_list_keys() {
        let dir = test_dir("list_keys");
        let store = SoftwareKeyStore::new(dir.clone(), b"test_passphrase".to_vec()).unwrap();

        store.store("key_a", b"data_a_32_bytes_pad_pad_pad!!").unwrap();
        store.store("key_b", b"data_b_32_bytes_pad_pad_pad!!").unwrap();
        store.store("key_c", b"data_c_32_bytes_pad_pad_pad!!").unwrap();

        let keys = store.list_keys();
        assert_eq!(keys.len(), 3);

        let key_ids: Vec<&str> = keys.iter().map(|k| k.key_id.as_str()).collect();
        assert!(key_ids.contains(&"key_a"));
        assert!(key_ids.contains(&"key_b"));
        assert!(key_ids.contains(&"key_c"));

        cleanup(&dir);
    }

    #[test]
    fn test_software_keystore_delete() {
        let dir = test_dir("delete");
        let store = SoftwareKeyStore::new(dir.clone(), b"test_passphrase".to_vec()).unwrap();

        store.store("del_key", b"delete_test_data_32_bytes!!!").unwrap();
        store.rotate("del_key").unwrap(); // v2

        // 删除 v1
        store.delete("del_key", 1).unwrap();

        // v1 应不可加载
        assert!(store.load("del_key", Some(1)).is_err());

        // v2 仍可加载
        let loaded = store.load("del_key", Some(2)).unwrap();
        assert_eq!(loaded, b"delete_test_data_32_bytes!!!");

        // 元数据中 v1 应已移除
        let keys = store.list_keys();
        let key_info = keys.iter().find(|k| k.key_id == "del_key").unwrap();
        assert_eq!(key_info.versions.len(), 1);
        assert_eq!(key_info.versions[0].version, 2);

        // 删除最后一个版本后，key 应从元数据移除
        store.delete("del_key", 2).unwrap();
        let keys = store.list_keys();
        assert!(keys.iter().all(|k| k.key_id != "del_key"));

        cleanup(&dir);
    }

    #[test]
    fn test_software_keystore_key_not_found() {
        let dir = test_dir("key_not_found");
        let store = SoftwareKeyStore::new(dir.clone(), b"test_passphrase".to_vec()).unwrap();

        // 加载不存在的密钥
        let err = store.load("nonexistent", None).unwrap_err();
        assert!(matches!(err, KeyStoreError::KeyNotFound(_)));

        // 加载不存在的版本
        store.store("exists", b"exists_data_32_bytes_pad_pad!!").unwrap();
        let err = store.load("exists", Some(99)).unwrap_err();
        assert!(matches!(err, KeyStoreError::KeyNotFound(_)));

        // 删除不存在的密钥
        let err = store.delete("nonexistent", 1).unwrap_err();
        assert!(matches!(err, KeyStoreError::KeyNotFound(_)));

        cleanup(&dir);
    }

    #[test]
    fn test_software_keystore_metadata_persistence() {
        let dir = test_dir("persistence");
        let key_data = b"persistence_test_data_32_bytes!";

        // 第一次创建 store 并存储密钥
        {
            let store =
                SoftwareKeyStore::new(dir.clone(), b"test_passphrase".to_vec()).unwrap();
            store.store("persist_key", key_data).unwrap();
        }

        // 重新加载 store，验证元数据持久化
        {
            let store =
                SoftwareKeyStore::new(dir.clone(), b"test_passphrase".to_vec()).unwrap();
            let loaded = store.load("persist_key", None).unwrap();
            assert_eq!(loaded, key_data);

            let keys = store.list_keys();
            assert_eq!(keys.len(), 1);
            assert_eq!(keys[0].key_id, "persist_key");
        }

        cleanup(&dir);
    }

    #[test]
    fn test_detect_keystore_returns_software() {
        let dir = test_dir("detect");
        let keystore = detect_keystore(dir.clone(), b"test_passphrase".to_vec()).unwrap();
        assert_eq!(keystore.backend_name(), "software");

        // 验证返回的 keystore 可正常使用
        let kv = keystore.store("test_key", b"detect_test_data_32_bytes!!").unwrap();
        assert_eq!(kv.version, 1);

        let loaded = keystore.load("test_key", None).unwrap();
        assert_eq!(loaded, b"detect_test_data_32_bytes!!");

        cleanup(&dir);
    }
}
