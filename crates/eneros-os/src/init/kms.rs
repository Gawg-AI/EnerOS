//! 密钥管理服务（Key Management Service）
//!
//! 提供密钥生成、存储、轮换、访问控制功能。
//!
//! ## 存储后端
//! - **TPM 优先**：`tpm` feature 启用时，密钥存储在 TPM 硬件中（防提取）
//! - **软件回退**：默认使用 AES-256-GCM 加密的文件存储（Argon2 密钥派生）
//!
//! ## 支持的密钥类型
//! - `Ed25519`：签名密钥（用于 OTA 包签名、审计日志签名）
//! - `Aes256`：对称密钥（用于数据加密、HMAC）
//! - `HmacSha256`：HMAC 密钥（用于审计日志防篡改）
//!
//! ## 密钥轮换
//! - 按时间轮换（`rotation_days` 配置）
//! - 按使用次数轮换（`max_uses` 配置）
//! - 手动轮换（`rotate_key()` API）
//!
//! ## 访问控制
//! 每个密钥有 `allowed_consumers` 列表，只有列表中的进程/Agent 可访问。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// 密钥 ID 类型
pub type KeyId = String;

/// 密钥类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyType {
    /// Ed25519 签名密钥（32 字节私钥 + 32 字节公钥）
    Ed25519,
    /// AES-256 对称密钥（32 字节）
    Aes256,
    /// HMAC-SHA256 密钥（32 字节）
    HmacSha256,
}

impl KeyType {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyType::Ed25519 => "ed25519",
            KeyType::Aes256 => "aes256",
            KeyType::HmacSha256 => "hmac_sha256",
        }
    }

    pub fn key_len(&self) -> usize {
        match self {
            KeyType::Ed25519 => 32, // 私钥 32 字节
            KeyType::Aes256 => 32,
            KeyType::HmacSha256 => 32,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "ed25519" => Some(Self::Ed25519),
            "aes256" => Some(Self::Aes256),
            "hmac_sha256" => Some(Self::HmacSha256),
            _ => None,
        }
    }
}

/// 密钥算法（兼容旧命名）
pub type KeyAlgorithm = KeyType;

/// 密钥元数据（不含密钥材料）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// 密钥 ID（唯一标识）
    pub key_id: KeyId,
    /// 密钥类型
    pub key_type: KeyType,
    /// 密钥用途描述
    pub purpose: String,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 过期时间（None=永不过期）
    pub expires_at: Option<DateTime<Utc>>,
    /// 最后轮换时间
    pub last_rotated_at: Option<DateTime<Utc>>,
    /// 使用次数
    pub use_count: u64,
    /// 最大使用次数（None=不限）
    pub max_uses: Option<u64>,
    /// 允许的消费者列表（进程名/Agent ID）
    pub allowed_consumers: Vec<String>,
    /// 密钥版本（轮换时递增）
    pub version: u32,
    /// 是否已撤销
    pub revoked: bool,
}

impl KeyMetadata {
    /// 检查密钥是否已过期
    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.expires_at {
            return Utc::now() > exp;
        }
        false
    }

    /// 检查密钥是否需要轮换
    pub fn needs_rotation(&self, rotation_days: i64) -> bool {
        if self.revoked || self.is_expired() {
            return true;
        }
        if let Some(max) = self.max_uses {
            if self.use_count >= max {
                return true;
            }
        }
        if let Some(last) = self.last_rotated_at {
            return (Utc::now() - last).num_days() >= rotation_days;
        }
        // 从未轮换过，检查创建时间
        (Utc::now() - self.created_at).num_days() >= rotation_days
    }

    /// 检查消费者是否有权访问
    pub fn can_access(&self, consumer: &str) -> bool {
        if self.allowed_consumers.is_empty() {
            return true; // 无限制
        }
        self.allowed_consumers.contains(&consumer.to_string())
    }
}

/// 密钥条目（含密钥材料）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEntry {
    /// 元数据
    pub metadata: KeyMetadata,
    /// 密钥材料（明文，仅在内存中；drop 时自动零化）
    pub material: Zeroizing<Vec<u8>>,
}

impl KeyEntry {
    /// 创建新密钥条目
    pub fn new(
        key_id: impl Into<KeyId>,
        key_type: KeyType,
        purpose: impl Into<String>,
        material: Vec<u8>,
    ) -> Self {
        let now = Utc::now();
        Self {
            metadata: KeyMetadata {
                key_id: key_id.into(),
                key_type,
                purpose: purpose.into(),
                created_at: now,
                expires_at: None,
                last_rotated_at: None,
                use_count: 0,
                max_uses: None,
                allowed_consumers: Vec::new(),
                version: 1,
                revoked: false,
            },
            material: Zeroizing::new(material),
        }
    }

    /// 设置过期时间
    pub fn with_expiry(mut self, days: i64) -> Self {
        self.metadata.expires_at = Some(Utc::now() + Duration::days(days));
        self
    }

    /// 设置最大使用次数
    pub fn with_max_uses(mut self, max: u64) -> Self {
        self.metadata.max_uses = Some(max);
        self
    }

    /// 设置允许的消费者
    pub fn with_consumers(mut self, consumers: Vec<String>) -> Self {
        self.metadata.allowed_consumers = consumers;
        self
    }
}

/// 密钥库错误
#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("key not found: {0}")]
    NotFound(String),
    #[error("key already exists: {0}")]
    AlreadyExists(String),
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("decryption error: {0}")]
    Decryption(String),
    #[error("access denied: consumer '{0}' cannot access key '{1}'")]
    AccessDenied(String, String),
    #[error("key expired: {0}")]
    KeyExpired(String),
    #[error("key revoked: {0}")]
    KeyRevoked(String),
    #[error("invalid key material: {0}")]
    InvalidKey(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// KMS 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KmsConfig {
    /// 密钥库存储路径（软件回退模式）
    #[serde(default = "default_keystore_path")]
    pub keystore_path: PathBuf,
    /// 主密钥保护口令（Argon2 派生 KEK；drop 时自动零化）
    #[serde(default)]
    pub master_password: Zeroizing<String>,
    /// 默认轮换周期（天）
    #[serde(default = "default_rotation_days")]
    pub rotation_days: i64,
    /// 是否使用 TPM（true=优先 TPM，false=软件回退）
    #[serde(default)]
    pub use_tpm: bool,
    /// 密钥备份路径（可选）
    #[serde(default)]
    pub backup_path: Option<PathBuf>,
    /// Argon2 派生盐值（None 时由 KeyStore::new() 随机生成；明文存储，无需保密）
    #[serde(default)]
    pub salt: Option<Vec<u8>>,
}

fn default_keystore_path() -> PathBuf {
    PathBuf::from("/var/lib/eneros/keystore")
}

fn default_rotation_days() -> i64 {
    90 // 默认 90 天轮换
}

impl Default for KmsConfig {
    fn default() -> Self {
        Self {
            keystore_path: default_keystore_path(),
            master_password: Zeroizing::new(String::new()),
            rotation_days: default_rotation_days(),
            use_tpm: false,
            backup_path: None,
            salt: None,
        }
    }
}

/// 密钥库状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyStoreStatus {
    /// 密钥总数
    pub key_count: usize,
    /// 活跃密钥数（未过期、未撤销）
    pub active_keys: usize,
    /// 需要轮换的密钥数
    pub keys_needing_rotation: usize,
    /// 存储后端（"tpm" 或 "software"）
    pub backend: String,
    /// 密钥库路径
    pub path: String,
}

/// 密钥库（软件回退实现）
///
/// 使用 AES-256-GCM 加密密钥材料，Argon2 从口令派生 KEK。
/// 密钥库文件格式：JSON（元数据） + AES-GCM 密文（密钥材料）。
pub struct KeyStore {
    config: KmsConfig,
    /// 内存中的密钥缓存（key_id → KeyEntry）
    keys: parking_lot::RwLock<HashMap<KeyId, KeyEntry>>,
    /// 主密钥（AES-256，从口令派生；drop 时自动零化）
    master_key: Zeroizing<[u8; 32]>,
    /// 是否已加载
    loaded: parking_lot::RwLock<bool>,
}

impl KeyStore {
    /// 创建新密钥库
    pub fn new(mut config: KmsConfig) -> Result<Self, KeyStoreError> {
        if config.master_password.is_empty() {
            return Err(KeyStoreError::Config(
                "master_password must not be empty — set a strong password (>=16 chars)".into(),
            ));
        }

        // salt 为 None 时生成 16 字节随机 salt 并存入 config
        if config.salt.is_none() {
            config.salt = Some(generate_random_bytes(16)?);
        }

        // Argon2 从口令 + salt 派生 32 字节主密钥
        let master_key = Zeroizing::new(derive_key_from_password(
            &config.master_password,
            config.salt.as_ref().unwrap(),
        )?);

        Ok(Self {
            config,
            keys: parking_lot::RwLock::new(HashMap::new()),
            master_key,
            loaded: parking_lot::RwLock::new(false),
        })
    }

    /// 从文件加载密钥库
    pub fn load(mut config: KmsConfig) -> Result<Self, KeyStoreError> {
        // 从 keystore.index 恢复 salt（若存在），确保 master_key 派生与磁盘密文一致
        let index_path = config.keystore_path.join("keystore.index");
        if index_path.exists() {
            let content = std::fs::read_to_string(&index_path)?;
            if let Ok(store) = serde_json::from_str::<StoredKeyStore>(&content) {
                if let Some(salt) = store.salt {
                    config.salt = Some(salt);
                }
            }
            // 旧格式（Vec<StoredKeyEntry>）无 salt 字段，保留 config.salt 原值
        }
        let store = Self::new(config)?;
        store.load_from_disk()?;
        Ok(store)
    }

    /// 从磁盘加载密钥
    fn load_from_disk(&self) -> Result<(), KeyStoreError> {
        let mut loaded = self.loaded.write();
        if *loaded {
            return Ok(());
        }

        let index_path = self.config.keystore_path.join("keystore.index");
        if !index_path.exists() {
            *loaded = true;
            return Ok(()); // 首次使用，无密钥
        }

        let index_content = std::fs::read_to_string(&index_path)?;

        // 优先解析新格式（StoredKeyStore 含 salt），回退到旧格式（Vec<StoredKeyEntry>）
        let entries: Vec<StoredKeyEntry> =
            if let Ok(store) = serde_json::from_str::<StoredKeyStore>(&index_content) {
                store.entries
            } else {
                serde_json::from_str(&index_content)
                    .map_err(|e| KeyStoreError::Config(e.to_string()))?
            };

        let mut keys = self.keys.write();
        for stored in entries {
            let material = self.decrypt(&stored.ciphertext, &stored.nonce)?;
            let entry = KeyEntry {
                metadata: stored.metadata,
                material: Zeroizing::new(material),
            };
            keys.insert(entry.metadata.key_id.clone(), entry);
        }

        *loaded = true;
        Ok(())
    }

    /// 保存密钥库到磁盘
    fn save_to_disk(&self) -> Result<(), KeyStoreError> {
        let dir = &self.config.keystore_path;
        std::fs::create_dir_all(dir)?;

        let keys = self.keys.read();
        let mut stored_entries = Vec::with_capacity(keys.len());

        for entry in keys.values() {
            let (ciphertext, nonce) = self.encrypt(&entry.material)?;
            stored_entries.push(StoredKeyEntry {
                metadata: entry.metadata.clone(),
                ciphertext,
                nonce,
            });
        }

        // 写入 salt（明文，无需保密）+ 加密后的密钥条目
        let store = StoredKeyStore {
            salt: self.config.salt.clone(),
            entries: stored_entries,
        };
        let json = serde_json::to_string_pretty(&store)
            .map_err(|e| KeyStoreError::Config(e.to_string()))?;

        let index_path = dir.join("keystore.index");
        let tmp_path = dir.join("keystore.index.tmp");
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(&tmp_path, &index_path)?; // 原子写入

        // 设置文件权限为 0600（仅 owner 可读写），非 Linux 平台 no-op
        set_file_permissions(&index_path);

        Ok(())
    }

    /// 生成新密钥
    pub fn generate_key(
        &self,
        key_id: impl Into<KeyId>,
        key_type: KeyType,
        purpose: impl Into<String>,
    ) -> Result<KeyEntry, KeyStoreError> {
        // 先加载磁盘上的已有密钥，避免覆盖
        self.ensure_loaded()?;

        let key_id = key_id.into();
        let material = generate_key_material(key_type)?;

        let entry = KeyEntry::new(key_id.clone(), key_type, purpose, material);

        {
            let mut keys = self.keys.write();
            if keys.contains_key(&key_id) {
                return Err(KeyStoreError::AlreadyExists(key_id));
            }
            keys.insert(key_id.clone(), entry.clone());
        }

        self.save_to_disk()?;
        Ok(entry)
    }

    /// 导入外部密钥
    pub fn import_key(
        &self,
        key_id: impl Into<KeyId>,
        key_type: KeyType,
        purpose: impl Into<String>,
        material: Vec<u8>,
    ) -> Result<KeyEntry, KeyStoreError> {
        if material.len() != key_type.key_len() {
            return Err(KeyStoreError::InvalidKey(format!(
                "expected {} bytes, got {}",
                key_type.key_len(),
                material.len()
            )));
        }

        // 先加载磁盘上的已有密钥，避免覆盖
        self.ensure_loaded()?;

        let key_id = key_id.into();
        let entry = KeyEntry::new(key_id.clone(), key_type, purpose, material);

        {
            let mut keys = self.keys.write();
            if keys.contains_key(&key_id) {
                return Err(KeyStoreError::AlreadyExists(key_id));
            }
            keys.insert(key_id.clone(), entry.clone());
        }

        self.save_to_disk()?;
        Ok(entry)
    }

    /// 获取密钥（检查访问权限和使用限制）
    pub fn get_key(
        &self,
        key_id: &str,
        consumer: &str,
    ) -> Result<KeyEntry, KeyStoreError> {
        self.ensure_loaded()?;

        let mut keys = self.keys.write();
        let entry = keys
            .get_mut(key_id)
            .ok_or_else(|| KeyStoreError::NotFound(key_id.to_string()))?;

        // 检查撤销
        if entry.metadata.revoked {
            return Err(KeyStoreError::KeyRevoked(key_id.to_string()));
        }

        // 检查过期
        if entry.metadata.is_expired() {
            return Err(KeyStoreError::KeyExpired(key_id.to_string()));
        }

        // 检查访问权限
        if !entry.metadata.can_access(consumer) {
            return Err(KeyStoreError::AccessDenied(
                consumer.to_string(),
                key_id.to_string(),
            ));
        }

        // 递增使用计数
        entry.metadata.use_count += 1;
        let should_persist = entry.metadata.use_count % 100 == 0;
        let result = entry.clone();
        drop(keys);

        // 每 100 次使用持久化一次，避免每次都写磁盘
        if should_persist {
            self.save_to_disk()?;
        }

        Ok(result)
    }

    /// 获取密钥元数据（不含密钥材料，不递增使用计数）
    pub fn get_metadata(&self, key_id: &str) -> Result<KeyMetadata, KeyStoreError> {
        self.ensure_loaded()?;
        let keys = self.keys.read();
        keys.get(key_id)
            .map(|e| e.metadata.clone())
            .ok_or_else(|| KeyStoreError::NotFound(key_id.to_string()))
    }

    /// 列出所有密钥元数据
    pub fn list_keys(&self) -> Result<Vec<KeyMetadata>, KeyStoreError> {
        self.ensure_loaded()?;
        let keys = self.keys.read();
        Ok(keys.values().map(|e| e.metadata.clone()).collect())
    }

    /// 轮换密钥（生成新密钥材料，保留 key_id，版本递增）
    pub fn rotate_key(&self, key_id: &str) -> Result<KeyEntry, KeyStoreError> {
        self.ensure_loaded()?;

        let mut keys = self.keys.write();
        let entry = keys
            .get_mut(key_id)
            .ok_or_else(|| KeyStoreError::NotFound(key_id.to_string()))?;

        // 撤销的密钥不允许轮换（必须先恢复或删除后重建）
        if entry.metadata.revoked {
            return Err(KeyStoreError::KeyRevoked(key_id.to_string()));
        }

        // 生成新密钥材料
        let new_material = generate_key_material(entry.metadata.key_type)?;
        entry.material = Zeroizing::new(new_material);
        entry.metadata.version += 1;
        entry.metadata.last_rotated_at = Some(Utc::now());
        entry.metadata.use_count = 0;

        let result = entry.clone();
        drop(keys);

        self.save_to_disk()?;
        Ok(result)
    }

    /// 撤销密钥
    pub fn revoke_key(&self, key_id: &str) -> Result<(), KeyStoreError> {
        self.ensure_loaded()?;

        let mut keys = self.keys.write();
        let entry = keys
            .get_mut(key_id)
            .ok_or_else(|| KeyStoreError::NotFound(key_id.to_string()))?;
        entry.metadata.revoked = true;
        drop(keys);

        self.save_to_disk()
    }

    /// 删除密钥
    pub fn delete_key(&self, key_id: &str) -> Result<(), KeyStoreError> {
        self.ensure_loaded()?;

        let mut keys = self.keys.write();
        keys.remove(key_id)
            .ok_or_else(|| KeyStoreError::NotFound(key_id.to_string()))?;
        drop(keys);

        self.save_to_disk()
    }

    /// 获取密钥库状态
    pub fn status(&self) -> Result<KeyStoreStatus, KeyStoreError> {
        self.ensure_loaded()?;
        let keys = self.keys.read();
        let key_count = keys.len();
        let active_keys = keys
            .values()
            .filter(|e| !e.metadata.revoked && !e.metadata.is_expired())
            .count();
        let keys_needing_rotation = keys
            .values()
            .filter(|e| e.metadata.needs_rotation(self.config.rotation_days))
            .count();

        Ok(KeyStoreStatus {
            key_count,
            active_keys,
            keys_needing_rotation,
            // TPM 不可用时回退到软件后端；如实报告 "software (tpm requested but unavailable)"
            backend: if self.config.use_tpm {
                "software (tpm requested but unavailable)".to_string()
            } else {
                "software".to_string()
            },
            path: self.config.keystore_path.to_string_lossy().to_string(),
        })
    }

    /// 备份密钥库到指定路径
    pub fn backup(&self, backup_path: &Path) -> Result<(), KeyStoreError> {
        self.ensure_loaded()?;

        let keys = self.keys.read();
        let mut stored_entries = Vec::with_capacity(keys.len());

        for entry in keys.values() {
            let (ciphertext, nonce) = self.encrypt(&entry.material)?;
            stored_entries.push(StoredKeyEntry {
                metadata: entry.metadata.clone(),
                ciphertext,
                nonce,
            });
        }

        let json = serde_json::to_string_pretty(&stored_entries)
            .map_err(|e| KeyStoreError::Config(e.to_string()))?;

        if let Some(parent) = backup_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(backup_path, json)?;

        // 设置备份文件权限为 0600，非 Linux 平台 no-op
        set_file_permissions(backup_path);

        Ok(())
    }

    /// 从备份恢复密钥库
    pub fn restore(&self, backup_path: &Path) -> Result<usize, KeyStoreError> {
        if !backup_path.exists() {
            return Err(KeyStoreError::NotFound(
                backup_path.to_string_lossy().to_string(),
            ));
        }

        let content = std::fs::read_to_string(backup_path)?;
        let entries: Vec<StoredKeyEntry> =
            serde_json::from_str(&content).map_err(|e| KeyStoreError::Config(e.to_string()))?;

        let mut keys = self.keys.write();
        keys.clear();
        let mut count = 0;

        for stored in entries {
            let material = self.decrypt(&stored.ciphertext, &stored.nonce)?;
            let entry = KeyEntry {
                metadata: stored.metadata,
                material: Zeroizing::new(material),
            };
            keys.insert(entry.metadata.key_id.clone(), entry);
            count += 1;
        }

        drop(keys);
        self.save_to_disk()?;

        Ok(count)
    }

    /// 获取配置
    pub fn config(&self) -> &KmsConfig {
        &self.config
    }

    // ---- 内部辅助方法 ----

    fn ensure_loaded(&self) -> Result<(), KeyStoreError> {
        let loaded = self.loaded.read();
        if *loaded {
            return Ok(());
        }
        drop(loaded);
        self.load_from_disk()
    }

    /// AES-256-GCM 加密
    fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), KeyStoreError> {
        let cipher = Aes256Gcm::new_from_slice(&*self.master_key)
            .map_err(|e| KeyStoreError::Encryption(e.to_string()))?;

        // 生成随机 nonce（12 字节）
        let nonce_bytes = generate_random_bytes(12)?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| KeyStoreError::Encryption(e.to_string()))?;

        Ok((ciphertext, nonce_bytes.to_vec()))
    }

    /// AES-256-GCM 解密
    fn decrypt(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>, KeyStoreError> {
        if nonce.len() != 12 {
            return Err(KeyStoreError::Decryption(
                "nonce must be 12 bytes".into(),
            ));
        }

        let cipher = Aes256Gcm::new_from_slice(&*self.master_key)
            .map_err(|e| KeyStoreError::Decryption(e.to_string()))?;

        let nonce = Nonce::from_slice(nonce);
        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| KeyStoreError::Decryption(e.to_string()))
    }
}

/// 存储的密钥条目（密钥材料已加密）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredKeyEntry {
    metadata: KeyMetadata,
    /// AES-GCM 密文
    ciphertext: Vec<u8>,
    /// AES-GCM nonce（12 字节）
    nonce: Vec<u8>,
}

/// keystore.index 文件格式：salt（明文）+ 加密后的密钥条目列表
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredKeyStore {
    /// Argon2 派生盐值（明文存储，无需保密）
    #[serde(default)]
    salt: Option<Vec<u8>>,
    /// 加密后的密钥条目
    entries: Vec<StoredKeyEntry>,
}

/// 设置文件权限为 0600（仅 owner 可读写）。
/// 非 Unix 平台为 no-op（开发环境 Windows 兼容）。
fn set_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Argon2 从口令派生 32 字节密钥
fn derive_key_from_password(
    password: &str,
    salt: &[u8],
) -> Result<[u8; 32], KeyStoreError> {
    use argon2::{Algorithm, Argon2, Params, Version};

    let params = Params::new(64 * 1024, 3, 4, Some(32))
        .map_err(|e| KeyStoreError::Config(format!("argon2 params: {}", e)))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut output = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut output)
        .map_err(|e| KeyStoreError::Config(format!("argon2 derive: {}", e)))?;

    Ok(output)
}

/// 生成指定类型的密钥材料
fn generate_key_material(key_type: KeyType) -> Result<Vec<u8>, KeyStoreError> {
    match key_type {
        KeyType::Ed25519 => {
            // Ed25519 私钥 32 字节
            use ed25519_dalek::SigningKey;
            let mut csprng = rand::rngs::OsRng;
            let signing_key = SigningKey::generate(&mut csprng);
            Ok(signing_key.to_bytes().to_vec())
        }
        KeyType::Aes256 | KeyType::HmacSha256 => {
            // AES-256 / HMAC-SHA256：32 字节随机
            generate_random_bytes(32)
        }
    }
}

/// 生成随机字节
fn generate_random_bytes(len: usize) -> Result<Vec<u8>, KeyStoreError> {
    use rand::RngCore;
    let mut bytes = vec![0u8; len];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(dir: &Path) -> KmsConfig {
        KmsConfig {
            keystore_path: dir.to_path_buf(),
            master_password: Zeroizing::new("test-password-16-chars".to_string()),
            rotation_days: 90,
            use_tpm: false,
            backup_path: None,
            salt: None,
        }
    }

    #[test]
    fn test_key_type_as_str() {
        assert_eq!(KeyType::Ed25519.as_str(), "ed25519");
        assert_eq!(KeyType::Aes256.as_str(), "aes256");
        assert_eq!(KeyType::HmacSha256.as_str(), "hmac_sha256");
    }

    #[test]
    fn test_key_type_from_str() {
        assert_eq!(KeyType::from_str("ed25519"), Some(KeyType::Ed25519));
        assert_eq!(KeyType::from_str("aes256"), Some(KeyType::Aes256));
        assert_eq!(
            KeyType::from_str("hmac_sha256"),
            Some(KeyType::HmacSha256)
        );
        assert_eq!(KeyType::from_str("invalid"), None);
    }

    #[test]
    fn test_key_type_key_len() {
        assert_eq!(KeyType::Ed25519.key_len(), 32);
        assert_eq!(KeyType::Aes256.key_len(), 32);
        assert_eq!(KeyType::HmacSha256.key_len(), 32);
    }

    #[test]
    fn test_key_metadata_is_expired() {
        let now = Utc::now();
        let meta = KeyMetadata {
            key_id: "test".into(),
            key_type: KeyType::Aes256,
            purpose: "test".into(),
            created_at: now,
            expires_at: Some(now - Duration::days(1)),
            last_rotated_at: None,
            use_count: 0,
            max_uses: None,
            allowed_consumers: vec![],
            version: 1,
            revoked: false,
        };
        assert!(meta.is_expired());

        let meta2 = KeyMetadata {
            expires_at: Some(now + Duration::days(1)),
            ..meta.clone()
        };
        assert!(!meta2.is_expired());

        let meta3 = KeyMetadata {
            expires_at: None,
            ..meta
        };
        assert!(!meta3.is_expired());
    }

    #[test]
    fn test_key_metadata_needs_rotation() {
        let now = Utc::now();
        let meta = KeyMetadata {
            key_id: "test".into(),
            key_type: KeyType::Aes256,
            purpose: "test".into(),
            created_at: now - Duration::days(100),
            expires_at: None,
            last_rotated_at: None,
            use_count: 0,
            max_uses: None,
            allowed_consumers: vec![],
            version: 1,
            revoked: false,
        };
        // 创建 100 天前，轮换周期 90 天 → 需要轮换
        assert!(meta.needs_rotation(90));

        let meta2 = KeyMetadata {
            created_at: now - Duration::days(10),
            ..meta
        };
        // 创建 10 天前，轮换周期 90 天 → 不需要
        assert!(!meta2.needs_rotation(90));
    }

    #[test]
    fn test_key_metadata_needs_rotation_by_uses() {
        let now = Utc::now();
        let meta = KeyMetadata {
            key_id: "test".into(),
            key_type: KeyType::Aes256,
            purpose: "test".into(),
            created_at: now,
            expires_at: None,
            last_rotated_at: Some(now),
            use_count: 1000,
            max_uses: Some(1000),
            allowed_consumers: vec![],
            version: 1,
            revoked: false,
        };
        assert!(meta.needs_rotation(90));
    }

    #[test]
    fn test_key_metadata_can_access() {
        let meta = KeyMetadata {
            key_id: "test".into(),
            key_type: KeyType::Aes256,
            purpose: "test".into(),
            created_at: Utc::now(),
            expires_at: None,
            last_rotated_at: None,
            use_count: 0,
            max_uses: None,
            allowed_consumers: vec!["agent-1".into(), "agent-2".into()],
            version: 1,
            revoked: false,
        };
        assert!(meta.can_access("agent-1"));
        assert!(meta.can_access("agent-2"));
        assert!(!meta.can_access("agent-3"));

        // 空列表 = 无限制
        let meta2 = KeyMetadata {
            allowed_consumers: vec![],
            ..meta
        };
        assert!(meta2.can_access("anyone"));
    }

    #[test]
    fn test_keystore_new_empty_password_rejected() {
        let config = KmsConfig {
            master_password: Zeroizing::new(String::new()),
            ..Default::default()
        };
        let result = KeyStore::new(config);
        assert!(matches!(result, Err(KeyStoreError::Config(_))));
    }

    #[test]
    fn test_keystore_new_valid() {
        let config = KmsConfig {
            master_password: Zeroizing::new("valid-password-16-chars".to_string()),
            ..Default::default()
        };
        let store = KeyStore::new(config).unwrap();
        assert_eq!(store.config().rotation_days, 90);
    }

    #[test]
    fn test_generate_key_material_ed25519() {
        let material = generate_key_material(KeyType::Ed25519).unwrap();
        assert_eq!(material.len(), 32);
    }

    #[test]
    fn test_generate_key_material_aes256() {
        let material = generate_key_material(KeyType::Aes256).unwrap();
        assert_eq!(material.len(), 32);
    }

    #[test]
    fn test_generate_random_bytes() {
        let bytes1 = generate_random_bytes(32).unwrap();
        let bytes2 = generate_random_bytes(32).unwrap();
        assert_eq!(bytes1.len(), 32);
        assert_eq!(bytes2.len(), 32);
        assert_ne!(bytes1, bytes2); // 极低概率相同
    }

    #[test]
    fn test_derive_key_from_password() {
        let salt = b"eneros-salt-16byte";
        let key1 = derive_key_from_password("password", salt).unwrap();
        let key2 = derive_key_from_password("password", salt).unwrap();
        let key3 = derive_key_from_password("different", salt).unwrap();

        assert_eq!(key1, key2); // 相同口令+盐 → 相同密钥
        assert_ne!(key1, key3); // 不同口令 → 不同密钥
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let config = KmsConfig {
            master_password: Zeroizing::new("test-password-16-chars".to_string()),
            ..Default::default()
        };
        let store = KeyStore::new(config).unwrap();

        let plaintext = b"secret key material";
        let (ciphertext, nonce) = store.encrypt(plaintext).unwrap();
        let decrypted = store.decrypt(&ciphertext, &nonce).unwrap();

        assert_eq!(decrypted, plaintext);
        assert_ne!(ciphertext, plaintext.to_vec());
    }

    #[test]
    fn test_decrypt_wrong_nonce() {
        let config = KmsConfig {
            master_password: Zeroizing::new("test-password-16-chars".to_string()),
            ..Default::default()
        };
        let store = KeyStore::new(config).unwrap();

        let plaintext = b"secret";
        let (ciphertext, _) = store.encrypt(plaintext).unwrap();
        let wrong_nonce = vec![0u8; 12];
        let result = store.decrypt(&ciphertext, &wrong_nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_invalid_nonce_length() {
        let config = KmsConfig {
            master_password: Zeroizing::new("test-password-16-chars".to_string()),
            ..Default::default()
        };
        let store = KeyStore::new(config).unwrap();

        let result = store.decrypt(b"ciphertext", b"short");
        assert!(matches!(result, Err(KeyStoreError::Decryption(_))));
    }

    #[test]
    fn test_keystore_generate_and_get_key() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-test-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        // 生成密钥
        let entry = store
            .generate_key("test-key-1", KeyType::Aes256, "test purpose")
            .unwrap();
        assert_eq!(entry.metadata.key_id, "test-key-1");
        assert_eq!(entry.metadata.key_type, KeyType::Aes256);
        assert_eq!(entry.material.len(), 32);
        assert_eq!(entry.metadata.version, 1);
        assert_eq!(entry.metadata.use_count, 0);

        // 获取密钥
        let retrieved = store.get_key("test-key-1", "test-consumer").unwrap();
        assert_eq!(retrieved.material, entry.material);
        assert_eq!(retrieved.metadata.use_count, 1); // 使用计数递增

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_generate_duplicate_fails() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-dup-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        store
            .generate_key("dup-key", KeyType::Aes256, "test")
            .unwrap();
        let result = store.generate_key("dup-key", KeyType::Aes256, "test");
        assert!(matches!(result, Err(KeyStoreError::AlreadyExists(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_get_nonexistent() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-notfound-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        let result = store.get_key("nonexistent", "consumer");
        assert!(matches!(result, Err(KeyStoreError::NotFound(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_access_control() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-acl-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        // 生成带访问控制的密钥
        store
            .generate_key("protected-key", KeyType::Aes256, "test")
            .unwrap();

        // 设置允许的消费者
        {
            let mut keys = store.keys.write();
            let entry = keys.get_mut("protected-key").unwrap();
            entry.metadata.allowed_consumers = vec!["authorized-agent".into()];
        }

        // 授权消费者可以访问
        let result = store.get_key("protected-key", "authorized-agent");
        assert!(result.is_ok());

        // 未授权消费者被拒绝
        let result = store.get_key("protected-key", "unauthorized");
        assert!(matches!(result, Err(KeyStoreError::AccessDenied(_, _))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_rotate_key() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-rotate-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        let original = store
            .generate_key("rotate-key", KeyType::Aes256, "test")
            .unwrap();

        let rotated = store.rotate_key("rotate-key").unwrap();
        assert_eq!(rotated.metadata.version, 2);
        assert_ne!(rotated.material, original.material);
        assert_eq!(rotated.metadata.use_count, 0);
        assert!(rotated.metadata.last_rotated_at.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_revoke_key() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-revoke-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        store
            .generate_key("revoke-key", KeyType::Aes256, "test")
            .unwrap();

        store.revoke_key("revoke-key").unwrap();

        let result = store.get_key("revoke-key", "consumer");
        assert!(matches!(result, Err(KeyStoreError::KeyRevoked(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_delete_key() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-delete-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        store
            .generate_key("delete-key", KeyType::Aes256, "test")
            .unwrap();

        store.delete_key("delete-key").unwrap();

        let result = store.get_key("delete-key", "consumer");
        assert!(matches!(result, Err(KeyStoreError::NotFound(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_list_keys() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-list-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        store.generate_key("key-1", KeyType::Aes256, "test1").unwrap();
        store
            .generate_key("key-2", KeyType::Ed25519, "test2")
            .unwrap();
        store
            .generate_key("key-3", KeyType::HmacSha256, "test3")
            .unwrap();

        let list = store.list_keys().unwrap();
        assert_eq!(list.len(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_status() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-status-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        store.generate_key("key-1", KeyType::Aes256, "test1").unwrap();
        store
            .generate_key("key-2", KeyType::Ed25519, "test2")
            .unwrap();

        let status = store.status().unwrap();
        assert_eq!(status.key_count, 2);
        assert_eq!(status.active_keys, 2);
        assert_eq!(status.keys_needing_rotation, 0); // 新生成的密钥不需要轮换
        assert_eq!(status.backend, "software"); // use_tpm=false

        // 验证 use_tpm=true 时的报告格式：TPM 不可用，如实报告 software 回退
        let dir2 = std::env::temp_dir().join(format!(
            "eneros-kms-status-tpm-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir2).unwrap();
        let config2 = KmsConfig {
            use_tpm: true,
            ..test_config(&dir2)
        };
        let store2 = KeyStore::new(config2).unwrap();
        let status2 = store2.status().unwrap();
        assert_eq!(status2.backend, "software (tpm requested but unavailable)");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn test_keystore_persistence() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-persist-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 第一次：创建并生成密钥
        {
            let config = test_config(&dir);
            let store = KeyStore::new(config).unwrap();
            store
                .generate_key("persist-key", KeyType::Aes256, "persistence test")
                .unwrap();
        }

        // 第二次：重新加载，密钥应存在
        {
            let config = test_config(&dir);
            let store = KeyStore::load(config).unwrap();
            let entry = store.get_key("persist-key", "test").unwrap();
            assert_eq!(entry.metadata.key_id, "persist-key");
            assert_eq!(entry.material.len(), 32);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_backup_and_restore() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-backup-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let backup_dir = std::env::temp_dir().join(format!(
            "eneros-kms-backup-target-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&backup_dir);
        std::fs::create_dir_all(&dir).unwrap();

        let backup_path = backup_dir.join("keystore.backup");
        let backup_salt;

        // 生成密钥并备份
        {
            let config = test_config(&dir);
            let store = KeyStore::new(config).unwrap();
            // 备份文件用同一 salt 加密，恢复时必须使用相同 salt 派生 master_key
            backup_salt = store.config().salt.clone();
            store
                .generate_key("backup-key-1", KeyType::Aes256, "test1")
                .unwrap();
            store
                .generate_key("backup-key-2", KeyType::Ed25519, "test2")
                .unwrap();

            store.backup(&backup_path).unwrap();
            assert!(backup_path.exists());
        }

        // 恢复到新位置（使用与备份相同的 salt，确保 master_key 一致以解密密文）
        {
            let restore_dir = dir.join("restored");
            let mut config = test_config(&restore_dir);
            config.salt = backup_salt;
            let store = KeyStore::new(config).unwrap();

            let count = store.restore(&backup_path).unwrap();
            assert_eq!(count, 2);

            let entry = store.get_key("backup-key-1", "test").unwrap();
            assert_eq!(entry.metadata.key_id, "backup-key-1");
        }

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&backup_dir);
    }

    #[test]
    fn test_keystore_import_key() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-import-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        let material = vec![0x42u8; 32];
        let entry = store
            .import_key("imported-key", KeyType::Aes256, "imported", material.clone())
            .unwrap();
        assert_eq!(entry.material.as_slice(), material.as_slice());

        let retrieved = store.get_key("imported-key", "test").unwrap();
        assert_eq!(retrieved.material.as_slice(), material.as_slice());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keystore_import_wrong_length() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-import-wrong-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();

        let result = store.import_key(
            "bad-key",
            KeyType::Aes256,
            "test",
            vec![0u8; 16], // 错误长度
        );
        assert!(matches!(result, Err(KeyStoreError::InvalidKey(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_kms_config_default() {
        let config = KmsConfig::default();
        assert_eq!(config.keystore_path, PathBuf::from("/var/lib/eneros/keystore"));
        assert_eq!(config.rotation_days, 90);
        assert!(!config.use_tpm);
        assert!(config.master_password.is_empty());
    }

    #[test]
    fn test_kms_config_parse() {
        let toml_str = r#"
keystore_path = "/tmp/test-keystore"
master_password = "my-password-16-chars"
rotation_days = 30
use_tpm = true
"#;
        let config: KmsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.keystore_path, PathBuf::from("/tmp/test-keystore"));
        assert_eq!(config.master_password.as_str(), "my-password-16-chars");
        assert_eq!(config.rotation_days, 30);
        assert!(config.use_tpm);
    }

    #[test]
    fn test_keystore_status_serialize() {
        let status = KeyStoreStatus {
            key_count: 5,
            active_keys: 3,
            keys_needing_rotation: 1,
            backend: "software".to_string(),
            path: "/var/lib/eneros/keystore".to_string(),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"key_count\":5"));
        assert!(json.contains("\"backend\":\"software\""));
    }

    #[test]
    fn test_keystore_random_salt() {
        let dir1 = std::env::temp_dir().join(format!(
            "eneros-kms-salt1-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let dir2 = std::env::temp_dir().join(format!(
            "eneros-kms-salt2-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::create_dir_all(&dir2).unwrap();

        let config1 = test_config(&dir1);
        let config2 = test_config(&dir2);
        let store1 = KeyStore::new(config1).unwrap();
        let store2 = KeyStore::new(config2).unwrap();

        let salt1 = store1.config().salt.clone().unwrap();
        let salt2 = store2.config().salt.clone().unwrap();
        // 两个独立 KeyStore 应生成不同的随机 salt
        assert_ne!(salt1, salt2);
        assert_eq!(salt1.len(), 16);
        assert_eq!(salt2.len(), 16);

        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn test_generate_key_does_not_overwrite_existing() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-nooverwrite-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 第一次：创建并生成 key1
        {
            let config = test_config(&dir);
            let store = KeyStore::new(config).unwrap();
            store
                .generate_key("key1", KeyType::Aes256, "test1")
                .unwrap();
        }

        // 第二次：重新 load 同一配置，生成 key2，验证 key1 仍然存在
        {
            let config = test_config(&dir);
            let store = KeyStore::load(config).unwrap();
            store
                .generate_key("key2", KeyType::Aes256, "test2")
                .unwrap();

            let list = store.list_keys().unwrap();
            assert_eq!(list.len(), 2);
            assert!(list.iter().any(|m| m.key_id == "key1"));
            assert!(list.iter().any(|m| m.key_id == "key2"));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rotate_revoked_key_fails() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-rotaterevoked-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = test_config(&dir);
        let store = KeyStore::new(config).unwrap();
        store
            .generate_key("rev-key", KeyType::Aes256, "test")
            .unwrap();
        store.revoke_key("rev-key").unwrap();

        // 撤销的密钥不允许轮换
        let result = store.rotate_key("rev-key");
        assert!(matches!(result, Err(KeyStoreError::KeyRevoked(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_use_count_persists_across_reload() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-kms-usecount-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 创建并生成密钥，调用 get_key 101 次
        {
            let config = test_config(&dir);
            let store = KeyStore::new(config).unwrap();
            store
                .generate_key("count-key", KeyType::Aes256, "test")
                .unwrap();

            for _ in 0..101 {
                let _ = store.get_key("count-key", "consumer").unwrap();
            }
        }

        // 重新 load，验证 use_count 已持久化（第 100 次调用时保存）
        {
            let config = test_config(&dir);
            let store = KeyStore::load(config).unwrap();
            let meta = store.get_metadata("count-key").unwrap();
            assert!(meta.use_count > 0);
            assert_eq!(meta.use_count, 100);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 生成测试用唯一 ID（模拟 UUID，不依赖 uuid crate）
    fn uuid_like() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::SeqCst)
    }
}
