//! SM2 第三方互操作 KAT（21-B §2.C2）——手卷实现 ↔ OpenSSL（vetted 对端）。
//!
//! 向量来源：`tests/vectors/sm2-interop/vectors.json`（openssl pkeyutl
//! 生成，distid = 国标默认 "1234567812345678"，哈希锚定见
//! `vectors.json.sha256`）。禁止"自己生成自己验"——对端必须是
//! GMSSL / openssl 官方实现。
//!
//! 双向判据：
//! 1. **openssl → 手卷**：10 组 openssl 签名全部被手卷验签为有效；
//!    篡改消息/签名必须验签失败。
//! 2. **手卷 → openssl**：手卷签名被 openssl CLI 验签为有效
//!    （`pkeyutl -verify -rawin -digest sm3 -pkeyopt distid:1234567812345678`）。

use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::{sm2_verify, Sm2PrivateKey, Sm2PublicKey, Sm2Signature, Sm2Signer};

/// 确定性测试种子（与 `CsRng::new` 内部种子逐字节一致——WP-53.2：release
/// 构建下 `new()` 被门禁移除，集成测试改走 `from_seed` 保持序列不变）。
const TEST_FIXED_SEED: [u8; 32] = [
    0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36,
    0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32,
];

fn vectors_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/vectors/sm2-interop/vectors.json")
}

fn load_vectors() -> serde_json::Value {
    let text = std::fs::read_to_string(vectors_path())
        .expect("sm2 interop vectors.json must exist (tests/delivery/gen-sm2-vectors.sh)");
    serde_json::from_str(&text).expect("vectors.json must be valid JSON")
}

fn hex_decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

/// openssl 向量 → 手卷验签（10 组全 PASS + 篡改负向）。
#[test]
fn hand_rolled_verifies_all_openssl_vectors() {
    let doc = load_vectors();
    let vectors = doc["vectors"].as_array().expect("vectors array");
    assert!(
        vectors.len() >= 10,
        "C2 判据：至少 10 组第三方向量，实际 {}",
        vectors.len()
    );
    for v in vectors {
        let id = v["id"].as_str().unwrap();
        let message = v["message"].as_str().unwrap().as_bytes();
        let point: [u8; 65] = hex_decode(v["point"].as_str().unwrap())
            .try_into()
            .expect("point 65 bytes");
        let sig_der = hex_decode(v["sig_der"].as_str().unwrap());
        // DER → r||s（用 binding 的 DER 编解码——测试专用桥接）
        let (r, s) = eneros_crypto_gmssl::der::der_to_rs(&sig_der).expect("DER 解析失败");
        let mut rs = [0u8; 64];
        rs[..32].copy_from_slice(&r);
        rs[32..].copy_from_slice(&s);
        let sig = Sm2Signature::from_bytes(&rs);
        let pk = Sm2PublicKey::from_bytes(&point).expect("公钥点解析");
        let ok = Sm2Signer::new()
            .verify(message, &sig, &pk)
            .expect("手卷验签错误");
        assert!(ok, "{id}: openssl 签名未被手卷验证通过");
        // 篡改消息必须验签失败
        let mut tampered = message.to_vec();
        tampered[0] ^= 0x01;
        let ok = Sm2Signer::new()
            .verify(&tampered, &sig, &pk)
            .expect("verify");
        assert!(!ok, "{id}: 篡改消息被手卷接受");
        // 篡改签名必须验签失败
        let mut bad_rs = rs;
        bad_rs[63] ^= 0x01;
        let ok = Sm2Signer::new()
            .verify(message, &Sm2Signature::from_bytes(&bad_rs), &pk)
            .expect("verify");
        assert!(!ok, "{id}: 篡改签名被手卷接受");
    }
}

/// 手卷签名 → openssl CLI 验证（动态反向，全部 10 组）。
#[test]
fn openssl_cli_verifies_hand_rolled_signatures() {
    let doc = load_vectors();
    for v in doc["vectors"].as_array().unwrap() {
        let id = v["id"].as_str().unwrap();
        let message = v["message"].as_str().unwrap();
        let d: [u8; 32] = hex_decode(v["d"].as_str().unwrap())
            .try_into()
            .expect("d 32 bytes");
        let point: [u8; 65] = hex_decode(v["point"].as_str().unwrap())
            .try_into()
            .expect("point 65 bytes");
        let sk = Sm2PrivateKey::from_bytes(&d).expect("私钥解析");
        let pk = Sm2PublicKey::from_bytes(&point).expect("公钥解析");
        let mut rng = CsRng::from_seed(&TEST_FIXED_SEED);
        // 手卷签名（Sm2Signer 默认 user_id = "1234567812345678"，与
        // openssl 侧 distid 一致——互操作前提）
        let sig = Sm2Signer::new()
            .sign(message.as_bytes(), &sk, &pk, &mut rng)
            .expect("手卷签名");
        // r||s → DER
        let rs = sig.to_bytes();
        let der = eneros_crypto_gmssl::der::rs_to_der(
            &rs[..32].try_into().expect("r"),
            &rs[32..].try_into().expect("s"),
        )
        .expect("DER 编码");
        // openssl CLI 验证（vetted 对端；distid 必须显式）
        let dir = std::env::temp_dir().join(format!("sm2-interop-{id}"));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let msg_file = dir.join("msg.bin");
        let sig_file = dir.join("sig.der");
        let pub_file = dir.join("pub.pem");
        std::fs::write(&msg_file, message.as_bytes()).expect("msg write");
        std::fs::write(&sig_file, &der).expect("sig write");
        // SPKI DER：前缀固定（SM2 OID 1.2.156.10197.1.301）+ 04||X||Y
        let mut spki = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x81, 0x1c, 0xcf, 0x55, 0x01, 0x82, 0x2d, 0x03, 0x42, 0x00,
        ];
        spki.extend_from_slice(&point);
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            base64_encode(&spki)
        );
        std::fs::write(&pub_file, pem).expect("pub write");
        let out = std::process::Command::new("openssl")
            .args([
                "pkeyutl",
                "-verify",
                "-pubin",
                "-inkey",
                pub_file.to_str().expect("path"),
                "-in",
                msg_file.to_str().expect("path"),
                "-rawin",
                "-digest",
                "sm3",
                "-pkeyopt",
                "distid:1234567812345678",
                "-sigfile",
                sig_file.to_str().expect("path"),
            ])
            .output()
            .expect("openssl pkeyutl");
        assert!(
            out.status.success(),
            "{id}: openssl 拒绝手卷签名\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("Verified"),
            "{id}: openssl 输出缺 Verified"
        );
    }
}

/// 顶层便捷函数：手卷 sm2_verify（free function）也被 10 组向量覆盖。
#[test]
fn free_function_verify_matches_signer() {
    let doc = load_vectors();
    for v in doc["vectors"].as_array().unwrap() {
        let message = v["message"].as_str().unwrap().as_bytes();
        let point: [u8; 65] = hex_decode(v["point"].as_str().unwrap())
            .try_into()
            .expect("point");
        let sig_der = hex_decode(v["sig_der"].as_str().unwrap());
        let (r, s) = eneros_crypto_gmssl::der::der_to_rs(&sig_der).expect("der");
        let mut rs = [0u8; 64];
        rs[..32].copy_from_slice(&r);
        rs[32..].copy_from_slice(&s);
        let pk = Sm2PublicKey::from_bytes(&point).expect("pk");
        let ok = sm2_verify(message, &Sm2Signature::from_bytes(&rs), &pk).expect("verify");
        assert!(ok, "{}: free sm2_verify 失败", v["id"].as_str().unwrap());
    }
}

/// 简易 Base64（RFC 4648；避免引入额外依赖）。
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
