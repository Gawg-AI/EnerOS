//! `xtask config compile`（蓝图 41 WP-41.2）：把 ACTIVE 配置编译为签名
//! RuntimeConfigBundle（`product/config-bundle.bin` + 验签公钥
//! `product/config-bundle-pubkey.hex`），供 svc-config 内嵌。
//!
//! - 只编译 `configs/registry.toml` 中 ACTIVE 且存在于 release profile
//!   manifest configs（consumer 双向链接）的配置；
//! - 未知字段/重复字段/全零密钥/枚举非法 → 编译失败（Gate 转红）；
//! - ota.toml 的 root_key 由 `--root-key-hex` 注入（KMS/产线），文件不落
//!   明文；全零拒绝；
//! - 签名密钥 `--sign-key-hex`（SM2 私钥 32B hex；测试用仓库测试密钥，
//!   release 用 KMS 密钥）；公钥 65B hex（04 前缀）写入 pubkey 文件。

use std::fs;
use std::path::Path;

use super::{invalid, read_json, DynResult};

/// 生成并签名配置 bundle。
/// `sign_key_hex`：SM2 私钥（32B hex，可空=测试键）；`root_key_hex`：
/// ota root_key（64B hex，可空=从 ota.toml 读取）。
pub fn config_compile(
    root: &Path,
    profile_id: &str,
    sign_key_hex: Option<&str>,
    root_key_hex: Option<&str>,
) -> DynResult<std::path::PathBuf> {
    // 1. 读取 registry 与 release profile 的 configs 链接。
    let registry_path = root.join("configs/registry.toml");
    let registry_text = fs::read_to_string(&registry_path)
        .map_err(|e| invalid(format!("configs/registry.toml 缺失：{e}")))?;
    let registry: toml::Value = toml::from_str(&registry_text)
        .map_err(|e| invalid(format!("configs/registry.toml 语法错误：{e}")))?;
    let active = registry
        .get("active")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| invalid("registry.toml 缺少 [active]"))?;

    let profile_path = root
        .join("product/profiles")
        .join(format!("{profile_id}.json"));
    let profile: serde_json::Value = read_json(&profile_path)?;
    let manifest_configs: Vec<serde_json::Value> = profile
        .get("configs")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let manifest_digest_bytes;
    // manifest digest：从 profile 反推——configs 链接本身即声明；bundle
    // 的 product_manifest_digest 绑定由 gate（manifest verify）复核。
    // 简化：digest = sha256(profile 规范化 configs/services 节)。
    {
        let mut canon = serde_json::json!({
            "configs": manifest_configs,
            "services": profile.get("services").cloned().unwrap_or(serde_json::Value::Null),
        });
        let _ = &mut canon;
        let json = serde_json::to_vec(&canon)?;
        manifest_digest_bytes = {
            use sha2::Digest;
            sha2::Sha256::digest(&json).into()
        };
    }

    // 2. 编译每个 ACTIVE 配置。
    let mut bundle = eneros_config_bundle::RuntimeConfigBundle::new(
        eneros_config_bundle::Digest(manifest_digest_bytes),
        1,
        eneros_config_bundle::TrustedTime(now_unix_ms()),
    );
    let mut compiled_ids: Vec<String> = Vec::new();
    for (name, _entry) in active {
        // manifest 双向链接检查（consumer 服务必须存在于 profile）。
        let consumer_ok = manifest_configs.iter().any(|c| {
            c.get("id").and_then(serde_json::Value::as_str) == Some(name.as_str())
                && c.get("consumer")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
        });
        if !consumer_ok {
            return Err(invalid(format!(
                "config compile: {name} 未在 {profile_id} manifest configs 声明 consumer（CF-01）"
            )));
        }
        let path = root.join("configs").join(format!("{name}.toml"));
        let text = fs::read_to_string(&path)
            .map_err(|e| invalid(format!("config compile: 读 {name}.toml 失败：{e}")))?;
        let injected: Option<[u8; 64]> = match root_key_hex {
            Some(hex) => {
                let bytes = hex_decode(hex)?;
                if bytes.len() != 64 {
                    return Err(invalid("--root-key-hex 必须为 128 hex（64B）"));
                }
                let mut out = [0u8; 64];
                out.copy_from_slice(&bytes);
                Some(out)
            }
            None => None,
        };
        let compiled =
            eneros_config_bundle::toml_compile::compile_by_id(name, &text, injected.as_ref())?
                .ok_or_else(|| {
                    invalid(format!(
                        "config compile: {name} 无 typed compiler（未登记）"
                    ))
                })?;
        let digest = eneros_config_bundle::Digest::of(&compiled.payload);
        bundle
            .entries
            .push(eneros_config_bundle::SignedConfigEntry {
                config_id: compiled.config_id,
                schema: eneros_config_bundle::SchemaVersion(1),
                generation: 1,
                payload: compiled.payload,
                payload_digest: digest,
                signature: Vec::new(),
            });
        compiled_ids.push(name.clone());
        println!(
            "CONFIG-COMPILE: {name} → {}B",
            bundle.entries.last().unwrap().payload.len()
        );
    }

    // 3. 签名（SM2；密钥注入或测试键）。
    let (sk, pk) = resolve_signing_key(sign_key_hex)?;
    let mut rng = eneros_crypto::CsRng::new();
    eneros_config_bundle::sign_bundle(&mut bundle, &sk, &pk, &mut rng)
        .map_err(|e| invalid(format!("config bundle 签名失败：{e}")))?;

    // 4. 写产物。
    let out_dir = root.join("product");
    let bin = out_dir.join("config-bundle.bin");
    // 完整序列化（canonical + 全部签名）——只写 canonical 会丢签名，
    // 目标侧 decode 后验签必败（QEMU 实测 BadSignature 根因）。
    fs::write(&bin, eneros_config_bundle::encode_bundle_full(&bundle))
        .map_err(|e| invalid(format!("写 config-bundle.bin 失败：{e}")))?;
    let pk_hex: String = pk
        .to_bytes_uncompressed()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let pkf = out_dir.join("config-bundle-pubkey.hex");
    fs::write(&pkf, &pk_hex).map_err(|e| invalid(format!("写 pubkey 失败：{e}")))?;
    println!(
        "CONFIG-COMPILE: → {}（{} entries，{}B）\n             pubkey → {}",
        bin.display(),
        compiled_ids.join(","),
        fs::metadata(&bin).map(|m| m.len()).unwrap_or(0),
        pkf.display()
    );
    Ok(bin)
}

fn now_unix_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hex_decode(hex: &str) -> DynResult<Vec<u8>> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("非法 hex"));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let b = hex.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let hi = (b[i] as char).to_digit(16).unwrap() as u8;
        let lo = (b[i + 1] as char).to_digit(16).unwrap() as u8;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

/// 签名密钥：显式注入优先；否则仓库测试密钥（`product/security/config-test-key.hex`，
/// 缺失则用固定测试种子——仅 CI/开发）。
fn resolve_signing_key(
    sign_key_hex: Option<&str>,
) -> DynResult<(eneros_crypto::Sm2PrivateKey, eneros_crypto::Sm2PublicKey)> {
    let sk_bytes: [u8; 32] = match sign_key_hex {
        Some(hex) => {
            let bytes = hex_decode(hex)?;
            if bytes.len() != 32 {
                return Err(invalid("--sign-key-hex 必须为 64 hex（32B）"));
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(&bytes);
            out
        }
        None => {
            // 固定测试私钥（与 config-bundle 测试向量一致；release 必须
            // 注入 KMS 密钥——pubkey 会随产物进介质，测试键仅 CI）。
            [0x41u8; 32]
        }
    };
    let sk = eneros_crypto::Sm2PrivateKey::from_bytes(&sk_bytes)
        .map_err(|e| invalid(format!("私钥无效：{e:?}")))?;
    let kp = eneros_crypto::Sm2KeyPair::from_private_key(&sk)
        .map_err(|e| invalid(format!("公钥派生失败：{e:?}")))?;
    Ok((sk, kp.public_key))
}

/// 测试用：sm2 私钥种子合法性（32B 非零即合法标量大概率可用）。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decode_roundtrip() {
        assert_eq!(hex_decode("00ff10").unwrap(), vec![0x00, 0xFF, 0x10]);
        assert!(hex_decode("0ff").is_err());
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn resolve_test_key() {
        let (sk, pk) = resolve_signing_key(None).unwrap();
        let sig =
            eneros_crypto::sm2_sign(b"hello", &sk, &pk, &mut eneros_crypto::CsRng::new()).unwrap();
        assert!(eneros_crypto::sm2_verify(b"hello", &sig, &pk).unwrap());
    }
}
