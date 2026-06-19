//! Ed25519 签名（v0.22.0）
//!
//! 为 OTA 更新清单提供 Ed25519 签名与验证。
//! 签名/验证是纯计算，跨平台可用；私钥文件权限 0600 仅 Linux。
//! 密钥生成使用 OS 随机源（Linux: /dev/urandom, Windows: RtlGenRandom）。

use crate::update::error::UpdateError;
use crate::update::manifest::UpdateManifest;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{
    Signature, Signer, SigningKey as DalekSigningKey, Verifier, VerifyingKey as DalekVerifyingKey,
};
use std::path::Path;

/// Ed25519 私钥（封装 ed25519-dalek 的 SigningKey）
pub struct SigningKey(DalekSigningKey);

/// Ed25519 公钥（封装 ed25519-dalek 的 VerifyingKey）
pub struct VerifyingKey(DalekVerifyingKey);

#[cfg(target_os = "windows")]
#[link(name = "advapi32")]
extern "system" {
    fn SystemFunction036(random_buffer: *mut u8, random_buffer_length: u32) -> i32;
}

/// 从 OS 随机源填充缓冲区（密码学安全）
fn fill_random(buf: &mut [u8]) -> Result<(), UpdateError> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let mut file = std::fs::File::open("/dev/urandom")
            .map_err(|e| UpdateError::Key(format!("open /dev/urandom: {e}")))?;
        file.read_exact(buf)
            .map_err(|e| UpdateError::Key(format!("read /dev/urandom: {e}")))?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let ret = unsafe { SystemFunction036(buf.as_mut_ptr(), buf.len() as u32) };
        if ret == 0 {
            Err(UpdateError::Key(
                "SystemFunction036 (RtlGenRandom) failed".into(),
            ))
        } else {
            Ok(())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err(UpdateError::UnsupportedPlatform)
    }
}

/// 生成 Ed25519 密钥对（使用 OS 随机源）
pub fn generate_keypair() -> Result<(SigningKey, VerifyingKey), UpdateError> {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes)?;
    let signing = DalekSigningKey::from_bytes(&bytes);
    let verifying = signing.verifying_key();
    Ok((SigningKey(signing), VerifyingKey(verifying)))
}

/// 对 manifest 签名，返回 base64 编码的签名
pub fn sign_manifest(manifest: &UpdateManifest, key: &SigningKey) -> String {
    let payload = manifest.signing_payload();
    let signature: Signature = key.0.sign(&payload);
    BASE64.encode(signature.to_bytes())
}

/// 验证 manifest 的 signature 字段（常量时间比较）
pub fn verify_manifest(manifest: &UpdateManifest, pubkey: &VerifyingKey) -> bool {
    let signature_bytes = match BASE64.decode(manifest.signature.as_bytes()) {
        Ok(b) => b,
        Err(_) => return false,
    };
    if signature_bytes.len() != 64 {
        return false;
    }
    let mut arr = [0u8; 64];
    arr.copy_from_slice(&signature_bytes);
    let signature = Signature::from_bytes(&arr);
    let payload = manifest.signing_payload();
    pubkey.0.verify(&payload, &signature).is_ok()
}

/// 保存私钥（raw 32 bytes → base64 编码写入文件）。Linux 下设置权限 0600。
pub fn save_signing_key(key: &SigningKey, path: &Path) -> Result<(), UpdateError> {
    let encoded = BASE64.encode(key.0.to_bytes());
    write_file_secure(path, &encoded)
}

/// 从文件加载私钥
pub fn load_signing_key(path: &Path) -> Result<SigningKey, UpdateError> {
    let content = std::fs::read_to_string(path)?;
    let bytes = BASE64
        .decode(content.trim().as_bytes())
        .map_err(|e| UpdateError::Key(format!("invalid base64: {e}")))?;
    signing_key_from_bytes(&bytes)
}

/// 保存公钥（raw 32 bytes → base64 编码写入文件）
pub fn save_verifying_key(key: &VerifyingKey, path: &Path) -> Result<(), UpdateError> {
    let encoded = BASE64.encode(key.0.to_bytes());
    std::fs::write(path, encoded)?;
    Ok(())
}

/// 从文件加载公钥
pub fn load_verifying_key(path: &Path) -> Result<VerifyingKey, UpdateError> {
    let content = std::fs::read_to_string(path)?;
    let bytes = BASE64
        .decode(content.trim().as_bytes())
        .map_err(|e| UpdateError::Key(format!("invalid base64: {e}")))?;
    verifying_key_from_bytes(&bytes)
}

/// 从 32 字节构造私钥
pub fn signing_key_from_bytes(bytes: &[u8]) -> Result<SigningKey, UpdateError> {
    if bytes.len() != 32 {
        return Err(UpdateError::Key(format!(
            "signing key requires 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(bytes);
    Ok(SigningKey(DalekSigningKey::from_bytes(&arr)))
}

/// 从 32 字节构造公钥
pub fn verifying_key_from_bytes(bytes: &[u8]) -> Result<VerifyingKey, UpdateError> {
    if bytes.len() != 32 {
        return Err(UpdateError::Key(format!(
            "verifying key requires 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(bytes);
    DalekVerifyingKey::from_bytes(&arr)
        .map(VerifyingKey)
        .map_err(|e| UpdateError::Key(format!("invalid verifying key: {e}")))
}

#[cfg(target_os = "linux")]
fn write_file_secure(path: &Path, content: &str) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, content)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn write_file_secure(path: &Path, content: &str) -> Result<(), UpdateError> {
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::ab_partition::Slot;
    use crate::update::manifest::ImageEntry;
    use chrono::Utc;

    fn sample_manifest() -> UpdateManifest {
        UpdateManifest {
            version: "1.0".to_string(),
            target_slot: Slot::B,
            image_version: "0.22.0".to_string(),
            images: vec![ImageEntry {
                name: "rootfs.img".to_string(),
                sha256: "abc123".to_string(),
                size: 1024,
            }],
            created_at: Utc::now(),
            signature: String::new(),
        }
    }

    #[test]
    fn test_keypair_generation() {
        let (signing, verifying) = generate_keypair().unwrap();
        // 公钥应与私钥对应
        let expected = signing.0.verifying_key();
        assert_eq!(expected.to_bytes(), verifying.0.to_bytes());
    }

    #[test]
    fn test_sign_and_verify() {
        let (signing, verifying) = generate_keypair().unwrap();
        let mut manifest = sample_manifest();
        manifest.signature = sign_manifest(&manifest, &signing);
        assert!(verify_manifest(&manifest, &verifying));
    }

    #[test]
    fn test_verify_tampered_manifest() {
        let (signing, verifying) = generate_keypair().unwrap();
        let mut manifest = sample_manifest();
        manifest.signature = sign_manifest(&manifest, &signing);
        // 篡改镜像版本号
        manifest.image_version = "0.99.0".to_string();
        assert!(!verify_manifest(&manifest, &verifying));
    }

    #[test]
    fn test_key_save_load_roundtrip() {
        let temp = std::env::temp_dir().join("eneros_signer_roundtrip_test");
        let _ = std::fs::remove_file(&temp);

        let (signing, verifying) = generate_keypair().unwrap();
        save_signing_key(&signing, &temp).unwrap();
        let loaded = load_signing_key(&temp).unwrap();

        let mut manifest = sample_manifest();
        manifest.signature = sign_manifest(&manifest, &loaded);
        assert!(verify_manifest(&manifest, &verifying));

        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn test_key_from_bytes() {
        let bytes = [42u8; 32];
        let signing = signing_key_from_bytes(&bytes).unwrap();
        let verifying = signing.0.verifying_key();

        let mut manifest = sample_manifest();
        manifest.signature = sign_manifest(&manifest, &signing);
        assert!(verify_manifest(&manifest, &VerifyingKey(verifying)));

        // 公钥从字节构造
        let verifying2 = verifying_key_from_bytes(&verifying.to_bytes()).unwrap();
        assert!(verify_manifest(&manifest, &verifying2));

        // 错误长度应返回错误
        assert!(signing_key_from_bytes(&[0u8; 31]).is_err());
        assert!(verifying_key_from_bytes(&[0u8; 31]).is_err());
    }
}
