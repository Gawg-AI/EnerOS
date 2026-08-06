//! 支持包二进制格式与 SM4-GCM 加密（OBS-008）。
//!
//! 格式：
//! ```text
//! magic: [u8; 4] = b"ENSB"
//! version: u16 = 1
//! nonce: [u8; 12]        — SM4-GCM nonce（调用方注入硬件熵生成）
//! tag: [u8; 16]          — SM4-GCM 认证标签
//! payload_len: u32       — 密文长度
//! payload: [u8; payload_len]  — SM4-GCM 密文（明文 = 序列化的条目）
//! ```
//!
//! 明文格式（条目序列化）：
//! ```text
//! item_count: u16
//! for each item:
//!   name_len: u8
//!   name: [u8; name_len]
//!   content_type_len: u8
//!   content_type: [u8; content_type_len]
//!   data_len: u32
//!   data: [u8; data_len]
//! ```

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm4::gcm::Sm4Gcm;

use crate::redact::{verify_no_secrets, Redactor};

/// 魔数 `b"ENSB"`（EnerOS Support Bundle）。
pub const MAGIC: [u8; 4] = *b"ENSB";

/// 当前格式版本。
pub const VERSION: u16 = 1;

/// 最大条目数。
pub const MAX_ITEMS: usize = 64;

/// 最大单条目数据长度。
pub const MAX_ITEM_DATA_LEN: usize = 256 * 1024;

/// 最大名称长度。
pub const MAX_NAME_LEN: usize = 64;

/// 单个诊断条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleItem {
    /// 条目名称（如 `system.log`、`config.json`、`crash-dump.bin`）。
    pub name: String,
    /// 内容类型（如 `text/plain`、`application/json`、`application/octet-stream`）。
    pub content_type: String,
    /// 原始数据。
    pub data: Vec<u8>,
}

impl BundleItem {
    /// 创建文本条目。
    pub fn text(name: &str, data: &str) -> Self {
        Self {
            name: name.to_owned(),
            content_type: "text/plain".to_owned(),
            data: data.as_bytes().to_vec(),
        }
    }

    /// 创建 JSON 条目。
    pub fn json(name: &str, data: &str) -> Self {
        Self {
            name: name.to_owned(),
            content_type: "application/json".to_owned(),
            data: data.as_bytes().to_vec(),
        }
    }

    /// 创建二进制条目。
    pub fn binary(name: &str, content_type: &str, data: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            content_type: content_type.to_owned(),
            data: data.to_vec(),
        }
    }
}

/// 支持包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportBundle {
    /// 条目列表。
    pub items: Vec<BundleItem>,
}

impl SupportBundle {
    /// 创建空支持包。
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// 添加条目。
    pub fn add(&mut self, item: BundleItem) -> Result<(), BundleError> {
        if self.items.len() >= MAX_ITEMS {
            return Err(BundleError::TooManyItems);
        }
        if item.name.len() > MAX_NAME_LEN {
            return Err(BundleError::NameTooLong);
        }
        if item.data.len() > MAX_ITEM_DATA_LEN {
            return Err(BundleError::DataTooLarge);
        }
        self.items.push(item);
        Ok(())
    }
}

impl Default for SupportBundle {
    fn default() -> Self {
        Self::new()
    }
}

/// 编解码错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// 条目过多。
    TooManyItems,
    /// 名称过长。
    NameTooLong,
    /// 数据过大。
    DataTooLarge,
    /// 内容类型过长。
    ContentTypeTooLong,
    /// 序列化失败。
    SerializeFailed,
    /// 反序列化失败。
    DeserializeFailed,
    /// 密钥长度错误（必须 16 字节）。
    BadKeyLen,
    /// Nonce 长度错误（必须 12 字节）。
    BadNonceLen,
    /// 解密失败（认证标签不匹配）。
    DecryptFailed,
    /// 发现残留密钥（redaction 验证失败）。
    SecretLeakDetected,
    /// 数据过短。
    TooShort,
    /// 魔数不匹配。
    BadMagic,
    /// 版本不支持。
    UnsupportedVersion,
    /// 长度字段非法。
    BadLength,
}

/// 加密支持包并导出为二进制。
///
/// 流程：
/// 1. 对每个文本/JSON 条目执行脱敏（`Redactor`）。
/// 2. 对脱敏后的文本执行 `verify_no_secrets`（fail-closed）。
/// 3. 序列化所有条目为明文。
/// 4. 使用 SM4-GCM 加密明文。
///
/// `key` 必须为 16 字节。`nonce` 必须为 12 字节，由调用方注入硬件熵生成
/// （同一 (key, nonce) 组合绝不可重复使用）。
pub fn encrypt_bundle(
    bundle: &SupportBundle,
    key: &[u8],
    nonce: &[u8],
) -> Result<Vec<u8>, BundleError> {
    if key.len() != 16 {
        return Err(BundleError::BadKeyLen);
    }
    if nonce.len() != 12 {
        return Err(BundleError::BadNonceLen);
    }

    let redactor = Redactor::new();

    // 1. 脱敏 + 验证
    let mut redacted_items: Vec<BundleItem> = Vec::with_capacity(bundle.items.len());
    for item in &bundle.items {
        let mut redacted = item.clone();
        let is_text = item.content_type.starts_with("text/")
            || item.content_type.starts_with("application/json")
            || item.content_type.starts_with("application/x-yaml");

        if is_text {
            // 文本条目：脱敏后验证
            let text = bytes_to_string_lossy(&item.data);
            let (redacted_text, _) = redactor.redact(&text);
            if verify_no_secrets(&redacted_text).is_err() {
                return Err(BundleError::SecretLeakDetected);
            }
            redacted.data = redacted_text.into_bytes();
        } else {
            // 二进制条目：扫描是否意外包含明文密钥模式
            if let Ok(text) = core::str::from_utf8(&item.data) {
                if verify_no_secrets(text).is_err() {
                    // 尝试脱敏
                    let (redacted_text, _) = redactor.redact(text);
                    if verify_no_secrets(&redacted_text).is_ok() {
                        redacted.data = redacted_text.into_bytes();
                    } else {
                        return Err(BundleError::SecretLeakDetected);
                    }
                }
            }
        }
        redacted_items.push(redacted);
    }

    // 2. 序列化
    let plaintext = serialize_items(&redacted_items)?;

    // 3. SM4-GCM 加密
    let mut key_arr = [0u8; 16];
    key_arr.copy_from_slice(key);
    let cipher = Sm4Gcm::new(&key_arr);
    let aad = b"eneros-support-bundle-v1";
    let (ciphertext, tag) = cipher.encrypt(nonce.try_into().unwrap(), &plaintext, aad);

    // 4. 组装输出
    let mut output = Vec::with_capacity(4 + 2 + 12 + 16 + 4 + ciphertext.len());
    output.extend_from_slice(&MAGIC);
    output.extend_from_slice(&VERSION.to_le_bytes());
    output.extend_from_slice(nonce);
    output.extend_from_slice(&tag);
    output.extend_from_slice(&(ciphertext.len() as u32).to_le_bytes());
    output.extend_from_slice(&ciphertext);

    Ok(output)
}

/// 解密支持包二进制并返回条目列表。
pub fn decrypt_bundle(data: &[u8], key: &[u8]) -> Result<SupportBundle, BundleError> {
    if key.len() != 16 {
        return Err(BundleError::BadKeyLen);
    }
    if data.len() < 4 + 2 + 12 + 16 + 4 {
        return Err(BundleError::TooShort);
    }
    if data[..4] != MAGIC {
        return Err(BundleError::BadMagic);
    }
    let version = u16::from_le_bytes([data[4], data[5]]);
    if version != VERSION {
        return Err(BundleError::UnsupportedVersion);
    }
    let nonce = &data[6..18];
    let tag = &data[18..34];
    let payload_len = u32::from_le_bytes([data[34], data[35], data[36], data[37]]) as usize;
    if data.len() < 38 + payload_len {
        return Err(BundleError::BadLength);
    }
    let ciphertext = &data[38..38 + payload_len];

    let mut key_arr = [0u8; 16];
    key_arr.copy_from_slice(key);
    let cipher = Sm4Gcm::new(&key_arr);
    let aad = b"eneros-support-bundle-v1";
    let plaintext = cipher
        .decrypt(
            nonce.try_into().unwrap(),
            ciphertext,
            aad,
            tag.try_into().unwrap(),
        )
        .map_err(|_| BundleError::DecryptFailed)?;

    let items = deserialize_items(&plaintext)?;
    Ok(SupportBundle { items })
}

/// 序列化条目列表为明文二进制。
fn serialize_items(items: &[BundleItem]) -> Result<Vec<u8>, BundleError> {
    if items.len() > MAX_ITEMS {
        return Err(BundleError::TooManyItems);
    }
    let mut buf = Vec::new();
    buf.extend_from_slice(&(items.len() as u16).to_le_bytes());
    for item in items {
        if item.name.len() > MAX_NAME_LEN {
            return Err(BundleError::NameTooLong);
        }
        if item.content_type.len() > 255 {
            return Err(BundleError::ContentTypeTooLong);
        }
        if item.data.len() > MAX_ITEM_DATA_LEN {
            return Err(BundleError::DataTooLarge);
        }
        buf.push(item.name.len() as u8);
        buf.extend_from_slice(item.name.as_bytes());
        buf.push(item.content_type.len() as u8);
        buf.extend_from_slice(item.content_type.as_bytes());
        buf.extend_from_slice(&(item.data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&item.data);
    }
    Ok(buf)
}

/// 反序列化条目列表。
fn deserialize_items(data: &[u8]) -> Result<Vec<BundleItem>, BundleError> {
    if data.len() < 2 {
        return Err(BundleError::DeserializeFailed);
    }
    let count = u16::from_le_bytes([data[0], data[1]]) as usize;
    if count > MAX_ITEMS {
        return Err(BundleError::TooManyItems);
    }
    let mut off = 2usize;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        if off >= data.len() {
            return Err(BundleError::DeserializeFailed);
        }
        let name_len = data[off] as usize;
        off += 1;
        if off + name_len > data.len() {
            return Err(BundleError::DeserializeFailed);
        }
        let name = bytes_to_string_lossy(&data[off..off + name_len]);
        off += name_len;

        if off >= data.len() {
            return Err(BundleError::DeserializeFailed);
        }
        let ct_len = data[off] as usize;
        off += 1;
        if off + ct_len > data.len() {
            return Err(BundleError::DeserializeFailed);
        }
        let content_type = bytes_to_string_lossy(&data[off..off + ct_len]);
        off += ct_len;

        if off + 4 > data.len() {
            return Err(BundleError::DeserializeFailed);
        }
        let data_len =
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize;
        off += 4;
        if off + data_len > data.len() {
            return Err(BundleError::DeserializeFailed);
        }
        let item_data = data[off..off + data_len].to_vec();
        off += data_len;

        items.push(BundleItem {
            name,
            content_type,
            data: item_data,
        });
    }
    Ok(items)
}

/// 从字节切片构造 String（lossy：无效 UTF-8 替换为 U+FFFD）。
fn bytes_to_string_lossy(data: &[u8]) -> String {
    match core::str::from_utf8(data) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            let mut s = String::with_capacity(data.len());
            let mut i = 0;
            while i < data.len() {
                match core::str::from_utf8(&data[i..]) {
                    Ok(valid) => {
                        s.push_str(valid);
                        break;
                    }
                    Err(e) => {
                        let valid_up_to = e.valid_up_to();
                        if valid_up_to > 0 {
                            s.push_str(
                                core::str::from_utf8(&data[i..i + valid_up_to]).unwrap_or(""),
                            );
                        }
                        s.push('\u{FFFD}');
                        i += valid_up_to + 1;
                    }
                }
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 16] {
        [0x42u8; 16]
    }

    fn test_nonce() -> [u8; 12] {
        [0x11u8; 12]
    }

    #[test]
    fn encrypt_decrypt_roundtrip_clean() {
        let mut bundle = SupportBundle::new();
        bundle
            .add(BundleItem::text(
                "system.log",
                "agent started\nagent stopped",
            ))
            .unwrap();
        bundle
            .add(BundleItem::json(
                "config.json",
                r#"{"version":"1.0","node":"edge-01"}"#,
            ))
            .unwrap();

        let key = test_key();
        let nonce = test_nonce();
        let encrypted = encrypt_bundle(&bundle, &key, &nonce).expect("encrypt");
        assert!(encrypted[..4] == MAGIC);
        assert!(encrypted.len() > 38);

        let decrypted = decrypt_bundle(&encrypted, &key).expect("decrypt");
        assert_eq!(decrypted.items.len(), 2);
        assert_eq!(decrypted.items[0].name, "system.log");
        assert_eq!(decrypted.items[1].name, "config.json");
    }

    #[test]
    fn encrypt_redacts_secrets_before_encryption() {
        let mut bundle = SupportBundle::new();
        bundle
            .add(BundleItem::text(
                "env.cfg",
                "db_conn=password=s3cr3t host=db\napi_key=sk-1234567890abcdef",
            ))
            .unwrap();

        let key = test_key();
        let nonce = test_nonce();
        let encrypted = encrypt_bundle(&bundle, &key, &nonce).expect("encrypt");

        // 解密后验证密钥已被脱敏
        let decrypted = decrypt_bundle(&encrypted, &key).expect("decrypt");
        let text = core::str::from_utf8(&decrypted.items[0].data).unwrap();
        assert!(!text.contains("s3cr3t"), "password value must be redacted");
        assert!(
            !text.contains("sk-1234567890abcdef"),
            "api key value must be redacted"
        );
        assert!(text.contains("[REDACTED:"));
    }

    #[test]
    fn encrypt_rejects_bad_key_len() {
        let bundle = SupportBundle::new();
        let bad_key = [0u8; 15];
        let nonce = test_nonce();
        assert_eq!(
            encrypt_bundle(&bundle, &bad_key, &nonce),
            Err(BundleError::BadKeyLen)
        );
    }

    #[test]
    fn encrypt_rejects_bad_nonce_len() {
        let bundle = SupportBundle::new();
        let key = test_key();
        let bad_nonce = [0u8; 11];
        assert_eq!(
            encrypt_bundle(&bundle, &key, &bad_nonce),
            Err(BundleError::BadNonceLen)
        );
    }

    #[test]
    fn decrypt_rejects_bad_magic() {
        let key = test_key();
        let mut data = vec![0u8; 40];
        data[0..4].copy_from_slice(b"XXXX");
        assert_eq!(decrypt_bundle(&data, &key), Err(BundleError::BadMagic));
    }

    #[test]
    fn decrypt_rejects_tampered_ciphertext() {
        let mut bundle = SupportBundle::new();
        bundle
            .add(BundleItem::text("log.txt", "hello world"))
            .unwrap();
        let key = test_key();
        let nonce = test_nonce();
        let mut encrypted = encrypt_bundle(&bundle, &key, &nonce).expect("encrypt");

        // 篡改密文最后一个字节
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;

        assert_eq!(
            decrypt_bundle(&encrypted, &key),
            Err(BundleError::DecryptFailed)
        );
    }

    #[test]
    fn decrypt_rejects_bad_version() {
        let key = test_key();
        let mut data = vec![0u8; 40];
        data[0..4].copy_from_slice(&MAGIC);
        data[4..6].copy_from_slice(&999u16.to_le_bytes()); // 不支持的版本
        assert_eq!(
            decrypt_bundle(&data, &key),
            Err(BundleError::UnsupportedVersion)
        );
    }

    #[test]
    fn encrypt_empty_bundle() {
        let bundle = SupportBundle::new();
        let key = test_key();
        let nonce = test_nonce();
        let encrypted = encrypt_bundle(&bundle, &key, &nonce).expect("encrypt");
        let decrypted = decrypt_bundle(&encrypted, &key).expect("decrypt");
        assert_eq!(decrypted.items.len(), 0);
    }

    #[test]
    fn pem_private_key_in_text_is_redacted() {
        let pem = "-----BEGIN EC PRIVATE KEY-----\nMHQCAQEE...\n-----END EC PRIVATE KEY-----";
        let mut bundle = SupportBundle::new();
        bundle.add(BundleItem::text("cert.cfg", pem)).unwrap();

        let key = test_key();
        let nonce = test_nonce();
        let encrypted = encrypt_bundle(&bundle, &key, &nonce).expect("encrypt");
        let decrypted = decrypt_bundle(&encrypted, &key).expect("decrypt");
        let text = core::str::from_utf8(&decrypted.items[0].data).unwrap();
        assert!(
            !text.contains("MHQCAQEE"),
            "PEM private key body must be redacted"
        );
        assert!(text.contains("[REDACTED:PEM_PRIVATE_KEY]"));
    }

    #[test]
    fn binary_item_with_embedded_secret_is_redacted() {
        // 二进制条目意外包含明文 password= 也应被脱敏
        let raw = b"binary\x00password=secret123\x00data".to_vec();
        let mut bundle = SupportBundle::new();
        bundle
            .add(BundleItem::binary(
                "core.bin",
                "application/octet-stream",
                &raw,
            ))
            .unwrap();

        let key = test_key();
        let nonce = test_nonce();
        let encrypted = encrypt_bundle(&bundle, &key, &nonce).expect("encrypt");
        let decrypted = decrypt_bundle(&encrypted, &key).expect("decrypt");
        let text = core::str::from_utf8(&decrypted.items[0].data).unwrap();
        assert!(!text.contains("secret123"));
    }
}
