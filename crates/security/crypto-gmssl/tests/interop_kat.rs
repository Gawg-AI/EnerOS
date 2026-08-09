//! 互操作 KAT（21-B C1/C2）：binding（OpenSSL 3）验证第三方向量。
//!
//! 向量来源：`tests/vectors/sm2-interop/vectors.json`——openssl CLI 生成
//! （vetted 对端，distid 国标默认）。测试前须通过哈希锚定校验
//! （`vectors.json.sha256`，CI KAT job 执行）。
//!
//! 判据：10 组 openssl 签名全部被 binding 验签为有效；篡改消息/签名
//! 必须验签失败。

// 测试文件允许 panic 断言（仓库惯例：测试模块豁免 disallowed-macros）。
#![allow(clippy::disallowed_macros)]

use eneros_crypto_gmssl::sm2;

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

#[test]
fn all_openssl_vectors_verify() {
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
        let ok = sm2::verify_der(message, &point, &sig_der)
            .unwrap_or_else(|e| panic!("{id}: verify_der error: {e}"));
        assert!(ok, "{id}: openssl 签名未被 binding 验证通过");
        // 篡改消息必须验签失败
        let mut tampered = message.to_vec();
        tampered[0] ^= 0x01;
        let ok = sm2::verify_der(&tampered, &point, &sig_der).expect("verify");
        assert!(!ok, "{id}: 篡改消息被接受");
        // 篡改签名必须验签失败
        let mut bad_sig = sig_der.clone();
        let last = bad_sig.len() - 1;
        bad_sig[last] ^= 0x01;
        let ok = sm2::verify_der(message, &point, &bad_sig).expect("verify");
        assert!(!ok, "{id}: 篡改签名被接受");
    }
}

#[test]
fn sm3_cross_check_against_openssl_cli() {
    // 额外交叉验证：binding SM3 与 openssl CLI 摘要一致（10 组消息）。
    let doc = load_vectors();
    for v in doc["vectors"].as_array().unwrap() {
        let message = v["message"].as_str().unwrap();
        let hash = eneros_crypto_gmssl::sm3::hash(message.as_bytes());
        let out = std::process::Command::new("openssl")
            .args(["dgst", "-sm3"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("openssl dgst");
        let mut child = out;
        {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .expect("stdin")
                .write_all(message.as_bytes())
                .expect("write");
        }
        let output = child.wait_with_output().expect("wait");
        assert!(output.status.success(), "openssl dgst failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let cli_hex = stdout.split_whitespace().last().expect("hash hex");
        assert_eq!(hex::encode(hash), cli_hex.trim().to_lowercase());
    }
}
