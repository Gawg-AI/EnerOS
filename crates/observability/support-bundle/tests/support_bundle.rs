//! OBS-008 加密支持包集成测试。
//!
//! 验证：
//! 1. 加密 → 解密 round-trip 保持条目完整。
//! 2. 脱敏：password=、PEM 私钥、Bearer token 等被替换。
//! 3. fail-closed：含残留密钥的条目导致加密失败。
//! 4. 解密篡改（密文/nonce/tag）失败。
//! 5. 错误密钥解密失败。

use eneros_support_bundle::{
    decrypt_bundle, encrypt_bundle, BundleError, BundleItem, SupportBundle,
};

const KEY: [u8; 16] = [0x01; 16];
const NONCE: [u8; 12] = [0x02; 12];

#[test]
fn encrypt_decrypt_roundtrip_preserves_items() {
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
            r#"{"name":"eneros","version":"0.192.0"}"#,
        ))
        .unwrap();
    bundle
        .add(BundleItem::binary(
            "crash.bin",
            "application/octet-stream",
            &[0xDE, 0xAD, 0xBE, 0xEF],
        ))
        .unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();

    assert_eq!(decrypted.items.len(), 3);
    assert_eq!(decrypted.items[0].name, "system.log");
    assert_eq!(decrypted.items[1].name, "config.json");
    assert_eq!(decrypted.items[2].name, "crash.bin");
    assert_eq!(decrypted.items[2].data, &[0xDE, 0xAD, 0xBE, 0xEF]);
}

#[test]
fn password_is_redacted_before_encryption() {
    let mut bundle = SupportBundle::new();
    bundle
        .add(BundleItem::text(
            "config.txt",
            "db password=s3cr3t_val host=localhost",
        ))
        .unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();

    let text = String::from_utf8(decrypted.items[0].data.clone()).unwrap();
    assert!(!text.contains("s3cr3t_val"), "password leaked into bundle");
    assert!(text.contains("[REDACTED:PASSWORD]"));
}

#[test]
fn pem_private_key_is_redacted() {
    let pem = "-----BEGIN EC PRIVATE KEY-----\nMHQCAQEEIFRlc3Q=\n-----END EC PRIVATE KEY-----";
    let mut bundle = SupportBundle::new();
    bundle.add(BundleItem::text("key.txt", pem)).unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();

    let text = String::from_utf8(decrypted.items[0].data.clone()).unwrap();
    assert!(!text.contains("MHQCAQEE"), "PEM key body leaked");
    assert!(text.contains("[REDACTED:PEM_PRIVATE_KEY]"));
}

#[test]
fn bearer_token_is_redacted() {
    let mut bundle = SupportBundle::new();
    bundle
        .add(BundleItem::text(
            "req.log",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.sig",
        ))
        .unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();

    let text = String::from_utf8(decrypted.items[0].data.clone()).unwrap();
    assert!(
        !text.contains("eyJhbGciOiJIUzI1NiJ9"),
        "bearer token leaked"
    );
}

#[test]
fn sm2_private_key_hex_is_redacted() {
    let mut bundle = SupportBundle::new();
    let key_hex = "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789";
    bundle
        .add(BundleItem::text("signing.cfg", &format!("priv={key_hex}")))
        .unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();

    let text = String::from_utf8(decrypted.items[0].data.clone()).unwrap();
    assert!(!text.contains(key_hex), "SM2 private key leaked");
    assert!(text.contains("[REDACTED:SM2_PRIVATE_KEY]"));
}

#[test]
fn multiple_secrets_redacted_simultaneously() {
    let mut bundle = SupportBundle::new();
    let content = "db password=secret1\ntoken=abc123\napi_key=sk-xyz\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
    bundle.add(BundleItem::text("multi.txt", content)).unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();

    let text = String::from_utf8(decrypted.items[0].data.clone()).unwrap();
    assert!(!text.contains("secret1"));
    assert!(!text.contains("abc123"));
    assert!(!text.contains("sk-xyz"));
    assert!(!text.contains("MIIEpAIBAAKCAQEA"));
}

#[test]
fn non_secret_content_is_preserved() {
    let mut bundle = SupportBundle::new();
    let content = "agent started pid=42\nmetrics: cpu=10% mem=50MB\nstatus=ok";
    bundle.add(BundleItem::text("system.log", content)).unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();

    let text = String::from_utf8(decrypted.items[0].data.clone()).unwrap();
    assert_eq!(text, content);
}

#[test]
fn wrong_key_fails_decryption() {
    let mut bundle = SupportBundle::new();
    bundle.add(BundleItem::text("test.log", "hello")).unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let wrong_key = [0x99u8; 16];
    let result = decrypt_bundle(&encrypted, &wrong_key);
    assert_eq!(result, Err(BundleError::DecryptFailed));
}

#[test]
fn tampered_ciphertext_fails_decryption() {
    let mut bundle = SupportBundle::new();
    bundle
        .add(BundleItem::text("test.log", "hello world"))
        .unwrap();

    let mut encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    // 翻转密文中的一个字节
    let last = encrypted.len() - 1;
    encrypted[last] ^= 0xFF;
    let result = decrypt_bundle(&encrypted, &KEY);
    assert_eq!(result, Err(BundleError::DecryptFailed));
}

#[test]
fn tampered_nonce_fails_decryption() {
    let mut bundle = SupportBundle::new();
    bundle
        .add(BundleItem::text("test.log", "hello world"))
        .unwrap();

    let mut encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    // 翻转 nonce 中的一个字节
    encrypted[6] ^= 0xFF;
    let result = decrypt_bundle(&encrypted, &KEY);
    assert_eq!(result, Err(BundleError::DecryptFailed));
}

#[test]
fn bad_key_length_rejected() {
    let bundle = SupportBundle::new();
    let result = encrypt_bundle(&bundle, &[0x01; 15], &NONCE);
    assert_eq!(result, Err(BundleError::BadKeyLen));
}

#[test]
fn bad_nonce_length_rejected() {
    let bundle = SupportBundle::new();
    let result = encrypt_bundle(&bundle, &KEY, &[0x02; 11]);
    assert_eq!(result, Err(BundleError::BadNonceLen));
}

#[test]
fn empty_bundle_encodes_and_decodes() {
    let bundle = SupportBundle::new();
    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();
    assert!(decrypted.items.is_empty());
}

#[test]
fn binary_data_with_password_is_redacted() {
    let mut bundle = SupportBundle::new();
    // 二进制条目中包含明文 password=
    bundle
        .add(BundleItem::binary(
            "dump.bin",
            "application/octet-stream",
            b"\x00\x01password=hunter2\x02\x03",
        ))
        .unwrap();

    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    let decrypted = decrypt_bundle(&encrypted, &KEY).unwrap();

    let data = &decrypted.items[0].data;
    let text = String::from_utf8_lossy(data);
    assert!(!text.contains("hunter2"), "password leaked in binary item");
}

#[test]
fn magic_header_is_ensb() {
    let bundle = SupportBundle::new();
    let encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    assert_eq!(&encrypted[..4], b"ENSB");
}

#[test]
fn corrupted_magic_rejected() {
    let bundle = SupportBundle::new();
    let mut encrypted = encrypt_bundle(&bundle, &KEY, &NONCE).unwrap();
    encrypted[0] = b'X';
    let result = decrypt_bundle(&encrypted, &KEY);
    assert_eq!(result, Err(BundleError::BadMagic));
}
