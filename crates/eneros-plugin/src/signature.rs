//! 插件签名验证 — Ed25519 签名生成与验证
//!
//! 为 EnerOS 插件提供基于 Ed25519 的数字签名验证，确保插件在加载前
//! 未被篡改且由可信签名者发布。
//!
//! # 文件格式
//! - `.sig` 文件：base64 编码的 64 字节 Ed25519 签名
//! - `.key` 文件：32 字节原始 Ed25519 私钥
//! - `.pub` 文件：32 字节原始 Ed25519 公钥
//!
//! # 验证流程
//! 1. 读取插件文件全部内容
//! 2. 读取 `.sig` 文件，base64 解码为 64 字节签名
//! 3. 遍历所有可信公钥，尝试 `verifying_key.verify(plugin_bytes, &signature)`
//! 4. 任一公钥验证成功 → `Valid { signer: key_id }`
//! 5. 所有公钥都失败 → `Invalid { reason: "signature verification failed" }`
//! 6. 如果没有可信公钥 → `UntrustedSigner { signer: "unknown" }`

use crate::error::PluginError;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use parking_lot::RwLock;
use rand::rngs::OsRng;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 签名验证结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationResult {
    /// 验证通过，signer 为签名者的 key_id（公钥 base64 编码前 8 字节）
    Valid { signer: String },
    /// 签名无效（文件被篡改或签名损坏）
    Invalid { reason: String },
    /// 签名文件缺失
    Missing,
    /// 签名者不在可信列表中
    UntrustedSigner { signer: String },
}

/// 可信公钥信息
#[derive(Debug, Clone)]
pub struct KeyInfo {
    /// 公钥标识（公钥 base64 编码前 8 字节）
    pub key_id: String,
    /// 公钥的 base64 编码（32 字节原始公钥编码为 44 字符）
    pub public_key: String,
    /// 公钥添加时间
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 插件签名验证器
///
/// 维护可信公钥列表，验证插件文件的 Ed25519 签名。
/// 验证器线程安全，内部使用 `RwLock` 保护可信公钥列表，
/// 支持并发读取与独占写入。
pub struct PluginSignatureVerifier {
    /// 可信公钥列表（key_id -> (VerifyingKey, created_at)）
    trusted_keys: RwLock<HashMap<String, (VerifyingKey, chrono::DateTime<chrono::Utc>)>>,
    /// 是否要求签名（true 时未签名插件被拒绝）
    require_signature: bool,
}

impl PluginSignatureVerifier {
    /// 创建验证器，从 `trusted_keys_dir` 加载可信公钥
    ///
    /// 遍历 `trusted_keys_dir` 下所有 `.pub` 文件，读取 32 字节原始公钥
    /// 并构造 `VerifyingKey` 加入可信列表。非 32 字节的文件被跳过。
    ///
    /// # 参数
    /// - `trusted_keys_dir`：可信公钥目录（不存在则创建空验证器）
    /// - `require_signature`：是否强制要求插件签名
    ///
    /// # 错误
    /// - `PluginError::Io`：读取目录或文件失败
    /// - `PluginError::SignatureInvalid`：公钥字节无法构造有效的 `VerifyingKey`
    pub fn new(trusted_keys_dir: &Path, require_signature: bool) -> Result<Self, PluginError> {
        let mut keys = HashMap::new();
        if trusted_keys_dir.exists() {
            for entry in fs::read_dir(trusted_keys_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("pub") {
                    let pub_key_bytes = fs::read(&path)?;
                    if pub_key_bytes.len() == 32 {
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&pub_key_bytes);
                        let verifying_key = VerifyingKey::from_bytes(&arr).map_err(|e| {
                            PluginError::SignatureInvalid(format!("invalid public key: {}", e))
                        })?;
                        let key_id = key_id_from_public_key(&verifying_key);
                        keys.insert(key_id, (verifying_key, chrono::Utc::now()));
                    }
                }
            }
        }
        Ok(Self {
            trusted_keys: RwLock::new(keys),
            require_signature,
        })
    }

    /// 创建空验证器（无可信密钥，用于测试）
    pub fn empty(require_signature: bool) -> Self {
        Self {
            trusted_keys: RwLock::new(HashMap::new()),
            require_signature,
        }
    }

    /// 验证插件签名
    ///
    /// 读取插件文件与签名文件，使用可信公钥验证签名。
    ///
    /// # 参数
    /// - `plugin_path`：插件文件路径
    /// - `signature_path`：签名文件路径（`.sig` 文件）
    ///
    /// # 返回
    /// - `Valid { signer }`：签名验证通过，signer 为 key_id
    /// - `Missing`：签名文件不存在
    /// - `Invalid { reason }`：签名损坏或验证失败
    /// - `UntrustedSigner { signer }`：无可信公钥
    pub fn verify(
        &self,
        plugin_path: &Path,
        signature_path: &Path,
    ) -> Result<VerificationResult, PluginError> {
        // 1. 读取插件文件
        let plugin_bytes = fs::read(plugin_path)?;

        // 2. 读取签名文件（不存在返回 Missing）
        if !signature_path.exists() {
            return Ok(VerificationResult::Missing);
        }
        let sig_content = fs::read_to_string(signature_path)?;
        let sig_bytes = BASE64
            .decode(sig_content.trim().as_bytes())
            .map_err(|e| PluginError::SignatureInvalid(format!("invalid base64: {}", e)))?;

        // 3. 签名必须是 64 字节
        if sig_bytes.len() != 64 {
            return Ok(VerificationResult::Invalid {
                reason: format!("signature must be 64 bytes, got {}", sig_bytes.len()),
            });
        }
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let signature = Signature::from_bytes(&sig_arr);

        // 4. 遍历可信公钥，尝试验证
        let keys = self.trusted_keys.read();
        if keys.is_empty() {
            // 没有可信公钥，无法验证签名者
            return Ok(VerificationResult::UntrustedSigner {
                signer: "unknown".to_string(),
            });
        }
        for (key_id, (verifying_key, _)) in keys.iter() {
            if verifying_key.verify(&plugin_bytes, &signature).is_ok() {
                return Ok(VerificationResult::Valid {
                    signer: key_id.clone(),
                });
            }
        }

        // 5. 所有公钥都验证失败
        Ok(VerificationResult::Invalid {
            reason: "signature verification failed".to_string(),
        })
    }

    /// 验证插件（自动查找 `.sig` 文件）
    ///
    /// 根据 `require_signature` 配置处理未签名插件：
    /// - `require_signature = true`：未签名返回 `Missing`
    /// - `require_signature = false`：未签名返回 `Valid { signer: "unsigned" }`
    pub fn verify_plugin(&self, plugin_path: &Path) -> Result<VerificationResult, PluginError> {
        let sig_path = plugin_path.with_extension("sig");
        if !sig_path.exists() {
            if self.require_signature {
                return Ok(VerificationResult::Missing);
            } else {
                return Ok(VerificationResult::Valid {
                    signer: "unsigned".to_string(),
                });
            }
        }
        self.verify(plugin_path, &sig_path)
    }

    /// 添加可信公钥
    ///
    /// # 参数
    /// - `public_key`：32 字节原始 Ed25519 公钥
    ///
    /// # 错误
    /// - `PluginError::SignatureInvalid`：公钥长度不为 32 字节或字节无法构造有效公钥
    pub fn add_trusted_key(&self, public_key: &[u8]) -> Result<(), PluginError> {
        if public_key.len() != 32 {
            return Err(PluginError::SignatureInvalid(format!(
                "public key must be 32 bytes, got {}",
                public_key.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(public_key);
        let verifying_key = VerifyingKey::from_bytes(&arr)
            .map_err(|e| PluginError::SignatureInvalid(format!("invalid public key: {}", e)))?;
        let key_id = key_id_from_public_key(&verifying_key);
        let mut keys = self.trusted_keys.write();
        keys.insert(key_id, (verifying_key, chrono::Utc::now()));
        Ok(())
    }

    /// 移除可信公钥
    ///
    /// # 参数
    /// - `key_id`：要移除的公钥标识
    ///
    /// # 错误
    /// - `PluginError::SignatureInvalid`：指定的 key_id 不存在
    pub fn remove_trusted_key(&self, key_id: &str) -> Result<(), PluginError> {
        let mut keys = self.trusted_keys.write();
        if keys.remove(key_id).is_none() {
            return Err(PluginError::SignatureInvalid(format!(
                "key not found: {}",
                key_id
            )));
        }
        Ok(())
    }

    /// 列出所有可信公钥
    pub fn list_trusted_keys(&self) -> Vec<KeyInfo> {
        let keys = self.trusted_keys.read();
        keys.iter()
            .map(|(key_id, (verifying_key, created_at))| KeyInfo {
                key_id: key_id.clone(),
                public_key: BASE64.encode(verifying_key.to_bytes()),
                created_at: *created_at,
            })
            .collect()
    }

    /// 是否要求签名
    pub fn require_signature(&self) -> bool {
        self.require_signature
    }
}

/// 生成 Ed25519 密钥对
///
/// 使用 OS 随机源生成 Ed25519 密钥对，写入 `output_dir`：
/// - `signing.key`：32 字节原始私钥
/// - `signing.pub`：32 字节原始公钥
///
/// # 返回
/// `(private_key_path, public_key_path)`
pub fn generate_keypair(output_dir: &Path) -> Result<(PathBuf, PathBuf), PluginError> {
    // 1. 生成随机 SigningKey（使用 OS 密码学安全随机源）
    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();

    // 2. 创建 output_dir（如不存在）
    fs::create_dir_all(output_dir)?;

    // 3. 写入私钥文件（.key，32 字节）
    let private_key_path = output_dir.join("signing.key");
    fs::write(&private_key_path, signing_key.to_bytes())?;

    // 4. 写入公钥文件（.pub，32 字节）
    let public_key_path = output_dir.join("signing.pub");
    fs::write(&public_key_path, verifying_key.to_bytes())?;

    Ok((private_key_path, public_key_path))
}

/// 对插件文件签名
///
/// 读取私钥与插件文件，生成 Ed25519 签名，base64 编码后写入 `.sig` 文件。
///
/// # 参数
/// - `plugin_path`：插件文件路径
/// - `private_key_path`：私钥文件路径（32 字节原始私钥）
///
/// # 返回
/// 签名文件路径（`plugin_path.with_extension("sig")`）
///
/// # 错误
/// - `PluginError::SignatureInvalid`：私钥长度不为 32 字节
/// - `PluginError::Io`：读取/写入文件失败
pub fn sign_plugin(
    plugin_path: &Path,
    private_key_path: &Path,
) -> Result<PathBuf, PluginError> {
    // 1. 读取私钥（32 字节）
    let key_bytes = fs::read(private_key_path)?;
    if key_bytes.len() != 32 {
        return Err(PluginError::SignatureInvalid(format!(
            "private key must be 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&key_bytes);
    let signing_key = SigningKey::from_bytes(&key_arr);

    // 2. 读取插件文件
    let plugin_bytes = fs::read(plugin_path)?;

    // 3. 签名（返回 64 字节 Signature）
    let signature: Signature = signing_key.sign(&plugin_bytes);

    // 4. base64 编码签名
    let sig_b64 = BASE64.encode(signature.to_bytes());

    // 5. 写入 .sig 文件
    let sig_path = plugin_path.with_extension("sig");
    fs::write(&sig_path, sig_b64)?;

    // 6. 返回签名文件路径
    Ok(sig_path)
}

/// 从公钥生成 key_id（base64 编码公钥前 8 字节）
///
/// 8 字节 base64 编码为 12 字符（含 padding），作为公钥的短标识符。
fn key_id_from_public_key(key: &VerifyingKey) -> String {
    let pub_key_bytes = key.to_bytes();
    BASE64.encode(&pub_key_bytes[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_generate_keypair() {
        let dir = tempdir().unwrap();
        let (priv_path, pub_path) = generate_keypair(dir.path()).unwrap();

        // 验证文件存在
        assert!(priv_path.exists(), "private key file should exist");
        assert!(pub_path.exists(), "public key file should exist");

        // 验证文件大小正确（32 字节）
        let priv_bytes = fs::read(&priv_path).unwrap();
        let pub_bytes = fs::read(&pub_path).unwrap();
        assert_eq!(priv_bytes.len(), 32, "private key must be 32 bytes");
        assert_eq!(pub_bytes.len(), 32, "public key must be 32 bytes");
    }

    #[test]
    fn test_sign_and_verify() {
        let dir = tempdir().unwrap();
        let (priv_path, pub_path) = generate_keypair(dir.path()).unwrap();

        // 创建插件文件
        let plugin_path = dir.path().join("myplugin.so");
        fs::write(&plugin_path, b"plugin binary content").unwrap();

        // 签名
        let sig_path = sign_plugin(&plugin_path, &priv_path).unwrap();
        assert!(sig_path.exists(), "signature file should exist");

        // 加载公钥到验证器
        let pub_bytes = fs::read(&pub_path).unwrap();
        let verifier = PluginSignatureVerifier::empty(true);
        verifier.add_trusted_key(&pub_bytes).unwrap();

        // 验证应通过
        let result = verifier.verify(&plugin_path, &sig_path).unwrap();
        assert!(
            matches!(result, VerificationResult::Valid { .. }),
            "expected Valid, got {:?}",
            result
        );
    }

    #[test]
    fn test_verify_tampered() {
        let dir = tempdir().unwrap();
        let (priv_path, pub_path) = generate_keypair(dir.path()).unwrap();

        // 创建并签名插件
        let plugin_path = dir.path().join("myplugin.so");
        fs::write(&plugin_path, b"original content").unwrap();
        let sig_path = sign_plugin(&plugin_path, &priv_path).unwrap();

        // 篡改插件文件
        fs::write(&plugin_path, b"tampered content").unwrap();

        // 验证应失败
        let pub_bytes = fs::read(&pub_path).unwrap();
        let verifier = PluginSignatureVerifier::empty(true);
        verifier.add_trusted_key(&pub_bytes).unwrap();

        let result = verifier.verify(&plugin_path, &sig_path).unwrap();
        assert!(
            matches!(result, VerificationResult::Invalid { .. }),
            "expected Invalid, got {:?}",
            result
        );
    }

    #[test]
    fn test_verify_missing_signature() {
        let dir = tempdir().unwrap();
        let plugin_path = dir.path().join("myplugin.so");
        fs::write(&plugin_path, b"plugin content").unwrap();

        let verifier = PluginSignatureVerifier::empty(true);
        let sig_path = dir.path().join("nonexistent.sig");
        let result = verifier.verify(&plugin_path, &sig_path).unwrap();
        assert_eq!(result, VerificationResult::Missing);
    }

    #[test]
    fn test_verify_untrusted_signer() {
        let dir = tempdir().unwrap();
        let (priv_path, _pub_path) = generate_keypair(dir.path()).unwrap();

        // 创建并签名插件
        let plugin_path = dir.path().join("myplugin.so");
        fs::write(&plugin_path, b"plugin content").unwrap();
        let sig_path = sign_plugin(&plugin_path, &priv_path).unwrap();

        // 验证器没有加载任何可信公钥
        let verifier = PluginSignatureVerifier::empty(true);
        let result = verifier.verify(&plugin_path, &sig_path).unwrap();
        assert!(
            matches!(result, VerificationResult::UntrustedSigner { .. }),
            "expected UntrustedSigner, got {:?}",
            result
        );
    }

    #[test]
    fn test_require_signature_true() {
        let dir = tempdir().unwrap();
        let plugin_path = dir.path().join("myplugin.so");
        fs::write(&plugin_path, b"plugin content").unwrap();

        // require_signature=true，无 .sig 文件 → Missing
        let verifier = PluginSignatureVerifier::empty(true);
        let result = verifier.verify_plugin(&plugin_path).unwrap();
        assert_eq!(result, VerificationResult::Missing);
    }

    #[test]
    fn test_require_signature_false() {
        let dir = tempdir().unwrap();
        let plugin_path = dir.path().join("myplugin.so");
        fs::write(&plugin_path, b"plugin content").unwrap();

        // require_signature=false，无 .sig 文件 → Valid { signer: "unsigned" }
        let verifier = PluginSignatureVerifier::empty(false);
        let result = verifier.verify_plugin(&plugin_path).unwrap();
        assert!(
            matches!(result, VerificationResult::Valid { ref signer } if signer == "unsigned"),
            "expected Valid with signer='unsigned', got {:?}",
            result
        );
    }

    #[test]
    fn test_add_remove_trusted_key() {
        let dir = tempdir().unwrap();
        let (_priv_path, pub_path) = generate_keypair(dir.path()).unwrap();
        let pub_bytes = fs::read(&pub_path).unwrap();

        let verifier = PluginSignatureVerifier::empty(true);

        // 添加可信公钥
        verifier.add_trusted_key(&pub_bytes).unwrap();
        let keys = verifier.list_trusted_keys();
        assert_eq!(keys.len(), 1, "should have 1 trusted key after add");

        // 获取 key_id 用于移除
        let key_id = keys[0].key_id.clone();

        // 移除可信公钥
        verifier.remove_trusted_key(&key_id).unwrap();
        let keys = verifier.list_trusted_keys();
        assert_eq!(keys.len(), 0, "should have 0 trusted keys after remove");

        // 再次移除应失败（key_id 不存在）
        let result = verifier.remove_trusted_key(&key_id);
        assert!(result.is_err(), "removing non-existent key should fail");
    }

    #[test]
    fn test_list_trusted_keys() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        let (_priv_path1, pub_path1) = generate_keypair(dir1.path()).unwrap();
        let (_priv_path2, pub_path2) = generate_keypair(dir2.path()).unwrap();

        let pub_bytes1 = fs::read(&pub_path1).unwrap();
        let pub_bytes2 = fs::read(&pub_path2).unwrap();

        let verifier = PluginSignatureVerifier::empty(true);
        verifier.add_trusted_key(&pub_bytes1).unwrap();
        verifier.add_trusted_key(&pub_bytes2).unwrap();

        let keys = verifier.list_trusted_keys();
        assert_eq!(keys.len(), 2, "should have 2 trusted keys");

        // 验证 KeyInfo 字段
        for key in &keys {
            assert!(!key.key_id.is_empty(), "key_id should not be empty");
            assert!(!key.public_key.is_empty(), "public_key should not be empty");
            // public_key 是 base64 编码的 32 字节
            let decoded = BASE64.decode(key.public_key.as_bytes()).unwrap();
            assert_eq!(decoded.len(), 32, "decoded public key must be 32 bytes");
        }
    }

    #[test]
    fn test_key_id_from_public_key() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();

        let key_id = key_id_from_public_key(&verifying_key);

        // key_id 应为 base64 编码的 8 字节 = 12 字符（含 padding）
        assert_eq!(key_id.len(), 12, "key_id should be 12 chars (base64 of 8 bytes)");

        // 验证 key_id 是 base64 编码的前 8 字节
        let pub_bytes = verifying_key.to_bytes();
        let expected = BASE64.encode(&pub_bytes[..8]);
        assert_eq!(key_id, expected);
    }

    #[test]
    fn test_new_loads_trusted_keys_from_dir() {
        let dir = tempdir().unwrap();
        let (_priv_path, _pub_path) = generate_keypair(dir.path()).unwrap();

        // 从目录加载 .pub 文件
        let verifier = PluginSignatureVerifier::new(dir.path(), true).unwrap();
        let keys = verifier.list_trusted_keys();
        assert_eq!(keys.len(), 1, "should load 1 key from directory");
        assert!(verifier.require_signature(), "require_signature should be true");
    }

    #[test]
    fn test_add_trusted_key_invalid_length() {
        let verifier = PluginSignatureVerifier::empty(true);
        // 31 字节 → 错误
        let result = verifier.add_trusted_key(&[0u8; 31]);
        assert!(result.is_err(), "adding 31-byte key should fail");
    }

    #[test]
    fn test_sign_plugin_invalid_key_length() {
        let dir = tempdir().unwrap();
        let plugin_path = dir.path().join("plugin.so");
        fs::write(&plugin_path, b"content").unwrap();
        let key_path = dir.path().join("bad.key");
        fs::write(&key_path, [0u8; 31]).unwrap();

        let result = sign_plugin(&plugin_path, &key_path);
        assert!(result.is_err(), "signing with 31-byte key should fail");
    }

    #[test]
    fn test_verify_wrong_signature_length() {
        let dir = tempdir().unwrap();
        let plugin_path = dir.path().join("plugin.so");
        fs::write(&plugin_path, b"content").unwrap();
        let sig_path = dir.path().join("plugin.sig");
        // 32 字节 base64 编码（不是 64 字节）→ Invalid
        fs::write(&sig_path, BASE64.encode([0u8; 32])).unwrap();

        let verifier = PluginSignatureVerifier::empty(true);
        let result = verifier.verify(&plugin_path, &sig_path).unwrap();
        assert!(
            matches!(result, VerificationResult::Invalid { .. }),
            "expected Invalid for wrong signature length, got {:?}",
            result
        );
    }

    #[test]
    fn test_verify_plugin_with_valid_signature() {
        let dir = tempdir().unwrap();
        let (priv_path, pub_path) = generate_keypair(dir.path()).unwrap();

        let plugin_path = dir.path().join("myplugin.so");
        fs::write(&plugin_path, b"plugin binary content").unwrap();
        sign_plugin(&plugin_path, &priv_path).unwrap();

        let pub_bytes = fs::read(&pub_path).unwrap();
        let verifier = PluginSignatureVerifier::empty(true);
        verifier.add_trusted_key(&pub_bytes).unwrap();

        // verify_plugin 自动查找 .sig 文件
        let result = verifier.verify_plugin(&plugin_path).unwrap();
        assert!(
            matches!(result, VerificationResult::Valid { .. }),
            "expected Valid, got {:?}",
            result
        );
    }

    #[test]
    fn test_verify_with_multiple_trusted_keys() {
        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();
        let (priv_a, pub_a) = generate_keypair(dir_a.path()).unwrap();
        let (_priv_b, pub_b) = generate_keypair(dir_b.path()).unwrap();

        // 创建插件文件并用密钥 A 签名
        let plugin_path = dir_a.path().join("myplugin.so");
        fs::write(&plugin_path, b"plugin content").unwrap();
        let sig_path = sign_plugin(&plugin_path, &priv_a).unwrap();

        // 将两个公钥都加入验证器
        let pub_bytes_a = fs::read(&pub_a).unwrap();
        let pub_bytes_b = fs::read(&pub_b).unwrap();
        let verifier = PluginSignatureVerifier::empty(true);
        verifier.add_trusted_key(&pub_bytes_a).unwrap();
        verifier.add_trusted_key(&pub_bytes_b).unwrap();
        assert_eq!(verifier.list_trusted_keys().len(), 2);

        // 验证应通过，且 signer 为密钥 A 的 key_id
        let result = verifier.verify(&plugin_path, &sig_path).unwrap();
        match result {
            VerificationResult::Valid { signer } => {
                // signer 应为密钥 A 的 key_id
                let expected_key_id = key_id_from_public_key(
                    &VerifyingKey::from_bytes(&pub_bytes_a.try_into().unwrap()).unwrap(),
                );
                assert_eq!(signer, expected_key_id, "signer should match key A's key_id");
            }
            other => panic!("expected Valid, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_with_multiple_trusted_keys_wrong_signer() {
        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();
        let (priv_a, _pub_a) = generate_keypair(dir_a.path()).unwrap();
        let (_priv_b, pub_b) = generate_keypair(dir_b.path()).unwrap();

        // 创建插件文件并用密钥 A 签名
        let plugin_path = dir_a.path().join("myplugin.so");
        fs::write(&plugin_path, b"plugin content").unwrap();
        let sig_path = sign_plugin(&plugin_path, &priv_a).unwrap();

        // 验证器只加载密钥 B 的公钥（不加载密钥 A）
        let pub_bytes_b = fs::read(&pub_b).unwrap();
        let verifier = PluginSignatureVerifier::empty(true);
        verifier.add_trusted_key(&pub_bytes_b).unwrap();

        // 验证应失败（签名是用密钥 A 签的，但验证器只有密钥 B）
        let result = verifier.verify(&plugin_path, &sig_path).unwrap();
        assert!(
            matches!(result, VerificationResult::Invalid { .. }),
            "expected Invalid when only wrong key is trusted, got {:?}",
            result
        );
    }
}
