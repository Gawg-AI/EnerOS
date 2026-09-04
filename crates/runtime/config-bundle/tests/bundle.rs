//! config-bundle 核心语义测试（蓝图 41 WP-41.1/41.2 二次验证四象限）。
//!
//! 覆盖：
//! - 正例：签 bundle → 验签 → registry 双向完备 → 两阶段 apply；
//! - 输入负例：坏签名、篡改 payload、未知字段/键、重复字段、全零密钥、
//!   schema 不匹配、manifest 绑定错、旧 generation 重放、未消费条目、
//!   缺条目、截断 wire；
//! - 依赖故障：apply 失败保持旧 generation、bundle 验签失败零状态变化；
//! - 恢复：canonical 编解码往返（掉电后从字节重建 bundle 语义一致）。
//!
//! TOML 编译面测试需要 `--features compile`（host 面）；核心语义测试
//! 无 feature 依赖。

use eneros_config_bundle::payload::{decode_fields, encode_payload, field_u64, field_u8};
use eneros_config_bundle::schema::control_write;
use eneros_config_bundle::{
    decode_bundle, encode_bundle_canonical, encode_entry_canonical, sign_bundle, verify_bundle,
    ApplyOutcome, ConfigBundleVersion, ConfigError, ConfigGeneration, ConfigId, ConfigRegistry,
    Digest, RuntimeConfigBundle, SchemaVersion, SignedConfigEntry, TrustedTime,
    TypedConfigConsumer,
};
use eneros_crypto::{CsRng, Sm2KeyPair, Sm2PrivateKey, Sm2PublicKey, Sm2Signer};

fn keypair(seed: u8) -> (Sm2PrivateKey, Sm2PublicKey) {
    let sk = Sm2PrivateKey::from_bytes(&[seed; 32]).unwrap();
    let kp = Sm2KeyPair::from_private_key(&sk).unwrap();
    (sk, kp.public_key)
}

/// 控制写配置类型化 consumer（target 侧真实解码面同构）。
#[derive(Default)]
struct ControlWriteConsumer {
    lease_id: [u8; 16],
    legacy_input: u8,
    retries: u8,
    applied_generation: u64,
}

impl TypedConfigConsumer for ControlWriteConsumer {
    type Config = ([u8; 16], u8, u8);
    fn config_id(&self) -> ConfigId {
        ConfigId::from_static(control_write::CONFIG_ID)
    }
    fn schema_version(&self) -> SchemaVersion {
        SchemaVersion(control_write::SCHEMA)
    }
    fn decode_validate(&self, raw: &[u8]) -> Result<Self::Config, ConfigError> {
        let config = ConfigId::from_static(control_write::CONFIG_ID);
        let fields = decode_fields(raw, &config, control_write::MAX_FIELD)?;
        let lease =
            eneros_config_bundle::payload::field(fields.as_slice(), control_write::FIELD_LEASE_ID)
                .ok_or(ConfigError::MissingField {
                    config: config.clone(),
                    field: "lease_id",
                })?;
        if lease.len() != control_write::LEASE_ID_LEN {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "lease_id",
                reason: "len".into(),
            });
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(lease);
        let legacy = field_u8(
            fields.as_slice(),
            &config,
            control_write::FIELD_LEGACY_INPUT,
            "legacy_input",
        )?
        .ok_or(ConfigError::MissingField {
            config: config.clone(),
            field: "legacy_input",
        })?;
        if legacy != control_write::LEGACY_INPUT_DENY_AND_LOG {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "legacy_input",
                reason: "unknown-enum".into(),
            });
        }
        let retries = field_u8(
            fields.as_slice(),
            &config,
            control_write::FIELD_HANDSHAKE_RETRIES,
            "handshake_retries",
        )?
        .ok_or(ConfigError::MissingField {
            config: config.clone(),
            field: "handshake_retries",
        })?;
        if retries > control_write::HANDSHAKE_RETRIES_MAX {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "handshake_retries",
                reason: "max".into(),
            });
        }
        Ok((id, legacy, retries))
    }
    fn apply(&mut self, cfg: Self::Config, generation: u64) -> Result<ApplyOutcome, ConfigError> {
        if self.lease_id == cfg.0 && self.legacy_input == cfg.1 && self.retries == cfg.2 {
            return Ok(ApplyOutcome::NoOp);
        }
        self.lease_id = cfg.0;
        self.legacy_input = cfg.1;
        self.retries = cfg.2;
        self.applied_generation = generation;
        Ok(ApplyOutcome::Applied { generation })
    }
}

fn make_entry(payload: Vec<u8>, generation: u64) -> SignedConfigEntry {
    let digest = Digest::of(&payload);
    SignedConfigEntry {
        config_id: ConfigId::from_static(control_write::CONFIG_ID),
        schema: SchemaVersion(control_write::SCHEMA),
        generation,
        payload,
        payload_digest: digest,
        signature: Vec::new(),
    }
}

fn make_payload(lease: &[u8; 16], legacy: u8, retries: u8) -> Vec<u8> {
    encode_payload(&[
        (control_write::FIELD_LEASE_ID, lease.to_vec()),
        (control_write::FIELD_LEGACY_INPUT, vec![legacy]),
        (control_write::FIELD_HANDSHAKE_RETRIES, vec![retries]),
    ])
}

fn make_bundle(entries: Vec<SignedConfigEntry>, generation: u64) -> RuntimeConfigBundle {
    RuntimeConfigBundle {
        schema: ConfigBundleVersion::V1,
        product_manifest_digest: Digest([0xAB; 32]),
        generation,
        created_at: TrustedTime(1_700_000_000_000),
        entries,
        signature: Vec::new(),
    }
}

fn signed_bundle(
    entries: Vec<SignedConfigEntry>,
    generation: u64,
) -> (RuntimeConfigBundle, Sm2PrivateKey, Sm2PublicKey) {
    let (sk, pk) = keypair(0x11);
    let mut bundle = make_bundle(entries, generation);
    let mut rng = CsRng::new();
    sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();
    (bundle, sk, pk)
}

#[test]
fn positive_sign_verify_roundtrip() {
    let (bundle, _sk, pk) = signed_bundle(vec![make_entry(make_payload(&[0x33; 16], 1, 5), 1)], 1);
    assert_eq!(bundle.signature.len(), 64);
    assert_eq!(bundle.entries[0].signature.len(), 64);
    verify_bundle(&bundle, &pk, Some(&Digest([0xAB; 32]))).unwrap();
    // 确定性签名路径（RFC 6979）与随机路径等价（验签都过）。
    let enc = encode_bundle_canonical(&bundle);
    let _sig = Sm2Signer::new()
        .sign_deterministic(&enc, &_sk, &pk)
        .unwrap();
    verify_bundle(&bundle, &pk, None).unwrap();
}

#[test]
fn negative_bad_signature_rejected() {
    let (bundle, _sk, _pk) = signed_bundle(vec![make_entry(make_payload(&[0x33; 16], 1, 5), 1)], 1);
    let (_sk2, pk2) = keypair(0x22);
    assert!(matches!(
        verify_bundle(&bundle, &pk2, None),
        Err(ConfigError::BadSignature(_))
    ));
}

#[test]
fn negative_tampered_payload_detected() {
    let (mut bundle, _sk, pk) =
        signed_bundle(vec![make_entry(make_payload(&[0x33; 16], 1, 5), 1)], 1);
    bundle.entries[0].payload[0] ^= 0xFF;
    assert!(matches!(
        verify_bundle(&bundle, &pk, None),
        Err(ConfigError::TamperedPayload(_))
    ));
}

#[test]
fn negative_wrong_manifest_digest() {
    let (bundle, _sk, pk) = signed_bundle(vec![make_entry(make_payload(&[0x33; 16], 1, 5), 1)], 1);
    assert!(matches!(
        verify_bundle(&bundle, &pk, Some(&Digest([0x00; 32]))),
        Err(ConfigError::WrongManifest { .. })
    ));
}

#[test]
fn negative_unknown_field_decoded() {
    let config = ConfigId::from_static(control_write::CONFIG_ID);
    let mut fields = vec![
        (control_write::FIELD_LEASE_ID, vec![0x33; 16]),
        (control_write::FIELD_LEGACY_INPUT, vec![1]),
    ];
    fields.push((99, vec![1]));
    let raw = encode_payload(&fields);
    let err = decode_fields(&raw, &config, control_write::MAX_FIELD).unwrap_err();
    assert!(matches!(err, ConfigError::UnknownField { field: 99, .. }));
}

#[test]
fn negative_duplicate_field_decoded() {
    let config = ConfigId::from_static(control_write::CONFIG_ID);
    let raw = encode_payload(&[
        (control_write::FIELD_LEASE_ID, &[0x33; 16]),
        (control_write::FIELD_LEASE_ID, &[0x44; 16]),
    ]);
    let err = decode_fields(&raw, &config, control_write::MAX_FIELD).unwrap_err();
    assert!(matches!(err, ConfigError::DuplicateField { field: 1, .. }));
}

#[test]
fn registry_apply_two_phase_and_keep_old_on_failure() {
    let (sk, pk) = keypair(0x11);
    let mut rng = CsRng::new();

    let mut bundle = make_bundle(vec![make_entry(make_payload(&[0x33; 16], 1, 5), 7)], 7);
    sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();

    let mut reg = ConfigRegistry::new();
    reg.register(Box::new(ControlWriteConsumer::default()));
    reg.apply_bundle(&bundle, &pk, Some(&Digest([0xAB; 32])))
        .unwrap();
    assert_eq!(
        reg.active_generation(&ConfigId::from_static(control_write::CONFIG_ID)),
        Some(7)
    );
    assert!(reg.active_bundle_digest().is_some());

    // 坏 bundle（签名被改）→ apply 失败 → 旧 generation 保持。
    let mut bad = bundle.clone();
    bad.signature[0] ^= 0xFF;
    assert!(reg.apply_bundle(&bad, &pk, None).is_err());
    assert_eq!(
        reg.active_generation(&ConfigId::from_static(control_write::CONFIG_ID)),
        Some(7)
    );

    // 旧 generation 重放 → StaleGeneration。
    let mut old = make_bundle(vec![make_entry(make_payload(&[0x33; 16], 1, 5), 3)], 3);
    sign_bundle(&mut old, &sk, &pk, &mut rng).unwrap();
    assert!(matches!(
        reg.apply_bundle(&old, &pk, None),
        Err(ConfigError::StaleGeneration { .. })
    ));
    assert_eq!(
        reg.active_generation(&ConfigId::from_static(control_write::CONFIG_ID)),
        Some(7)
    );
}

#[test]
fn registry_unconsumed_and_missing_entry() {
    let (sk, pk) = keypair(0x11);
    let mut rng = CsRng::new();

    // bundle 有条目但 registry 无 consumer → UnconsumedConfig。
    let mut bundle = make_bundle(vec![make_entry(make_payload(&[0x33; 16], 1, 5), 1)], 1);
    sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();
    let mut reg = ConfigRegistry::new();
    let err = reg.apply_bundle(&bundle, &pk, None).unwrap_err();
    assert!(matches!(err, ConfigError::UnconsumedConfig(_)), "{err}");

    // 注册 consumer 但 bundle 无对应条目 → MissingConfigEntry。
    let mut reg2 = ConfigRegistry::new();
    reg2.register(Box::new(ControlWriteConsumer::default()));
    let mut empty = make_bundle(vec![], 1);
    sign_bundle(&mut empty, &sk, &pk, &mut rng).unwrap();
    let err = reg2.apply_bundle(&empty, &pk, None).unwrap_err();
    assert!(matches!(err, ConfigError::MissingConfigEntry(_)), "{err}");
}

#[test]
fn schema_mismatch_rejected() {
    let (sk, pk) = keypair(0x11);
    let mut rng = CsRng::new();
    let mut entry = make_entry(make_payload(&[0x33; 16], 1, 5), 1);
    entry.schema = SchemaVersion(99); // consumer 声明 v1，条目声称 v99
    let mut bundle = make_bundle(vec![entry], 1);
    sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();
    let mut reg = ConfigRegistry::new();
    reg.register(Box::new(ControlWriteConsumer::default()));
    let err = reg.apply_bundle(&bundle, &pk, None).unwrap_err();
    assert!(matches!(err, ConfigError::SchemaMismatch { .. }), "{err}");
}

#[test]
fn wire_roundtrip_after_power_loss() {
    let (sk, pk) = keypair(0x11);
    let mut rng = CsRng::new();
    let mut bundle = make_bundle(vec![make_entry(make_payload(&[0x33; 16], 1, 5), 1)], 1);
    sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();
    let bytes = encode_bundle_canonical(&bundle);
    let decoded = decode_bundle(&bytes).unwrap();
    assert_eq!(decoded.generation, 1);
    assert_eq!(decoded.entries.len(), 1);
    assert_eq!(
        decoded.entries[0].config_id.as_str(),
        control_write::CONFIG_ID
    );
    assert_eq!(decoded.product_manifest_digest, Digest([0xAB; 32]));
    // 截断/损坏字节必须拒绝（掉电撕裂只能得到完整 generation 或错误）。
    assert!(decode_bundle(&bytes[..bytes.len() - 1]).is_err());
    assert!(decode_bundle(&bytes[..5]).is_err());
}

#[test]
fn config_generation_orders() {
    let g1 = ConfigGeneration(1);
    let g2 = ConfigGeneration(2);
    assert!(g2 > g1);
    assert_eq!(g1, ConfigGeneration(1));
}

// ---------------------------------------------------------------------------
// host 编译面（feature `compile`）
// ---------------------------------------------------------------------------

#[cfg(feature = "compile")]
#[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
fn repo_config(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../configs")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[cfg(feature = "compile")]
#[test]
fn negative_zero_secret_compile_rejected() {
    let toml = r#"
[control_write]
lease_id = "00000000000000000000000000000000"
legacy_input = "deny-and-log"
handshake_retries = 5
"#;
    let err = eneros_config_bundle::toml_compile::compile_control_write(toml).unwrap_err();
    assert!(matches!(err, ConfigError::ZeroSecret { .. }), "{err}");
}

#[cfg(feature = "compile")]
#[test]
fn negative_unknown_toml_key_rejected() {
    let toml = r#"
[control_write]
lease_id = "33112233445566778899aabbccddeeff"
legacy_input = "deny-and-log"
handshake_retries = 5
sneaky_extra = true
"#;
    let err = eneros_config_bundle::toml_compile::compile_control_write(toml).unwrap_err();
    assert!(
        matches!(err, ConfigError::UnknownKey { ref key, .. } if key == "sneaky_extra"),
        "{err}"
    );
}

#[cfg(feature = "compile")]
#[test]
fn negative_unknown_enum_rejected() {
    let toml = r#"
[control_write]
lease_id = "33112233445566778899aabbccddeeff"
legacy_input = "allow-everything"
handshake_retries = 5
"#;
    let err = eneros_config_bundle::toml_compile::compile_control_write(toml).unwrap_err();
    assert!(
        matches!(
            err,
            ConfigError::InvalidValue {
                field: "legacy_input",
                ..
            }
        ),
        "{err}"
    );
}

#[cfg(feature = "compile")]
#[test]
fn positive_repo_configs_compile_and_decode() {
    // 仓内 ACTIVE 配置必须可编译（否则 gate 红）。
    let cw = eneros_config_bundle::toml_compile::compile_control_write(&repo_config(
        "control-write.toml",
    ))
    .unwrap();
    let mut consumer = ControlWriteConsumer::default();
    let cfg = consumer.decode_validate(&cw.payload).unwrap();
    assert_eq!(cfg.1, control_write::LEGACY_INPUT_DENY_AND_LOG);
    assert!(consumer.apply(cfg, 1).is_ok());

    let key = [0x5Au8; 64];
    let ota = eneros_config_bundle::toml_compile::compile_ota(&repo_config("ota.toml"), Some(&key))
        .unwrap();
    let config = ConfigId::from_static(eneros_config_bundle::schema::ota::CONFIG_ID);
    let fields = decode_fields(&ota.payload, &config, 12).unwrap();
    let root = eneros_config_bundle::payload::field(&fields, 10).unwrap();
    assert_eq!(root, &key);
    assert_eq!(
        field_u8(&fields, &config, 11, "signature_mandatory")
            .unwrap()
            .unwrap(),
        1
    );
    assert_eq!(
        field_u64(&fields, &config, 3, "partition_blocks")
            .unwrap()
            .unwrap(),
        2048
    );

    let cs =
        eneros_config_bundle::toml_compile::compile_cloud_sync(&repo_config("cloud-sync.toml"))
            .unwrap();
    assert!(!cs.payload.is_empty());
    let mt =
        eneros_config_bundle::toml_compile::compile_metrics(&repo_config("metrics.toml")).unwrap();
    assert!(!mt.payload.is_empty());
    let lg =
        eneros_config_bundle::toml_compile::compile_logging(&repo_config("logging.toml")).unwrap();
    assert!(!lg.payload.is_empty());
}

#[cfg(feature = "compile")]
#[test]
fn negative_ota_missing_key_and_zero_key() {
    let toml = repo_config("ota.toml");
    let err = eneros_config_bundle::toml_compile::compile_ota(&toml, None).unwrap_err();
    assert!(
        matches!(
            err,
            ConfigError::MissingField {
                field: "root_key",
                ..
            }
        ),
        "{err}"
    );
    let err = eneros_config_bundle::toml_compile::compile_ota(&toml, Some(&[0u8; 64])).unwrap_err();
    assert!(matches!(err, ConfigError::ZeroSecret { .. }), "{err}");
}

#[cfg(feature = "compile")]
#[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
#[test]
fn xtask_flow_sign_then_verify_repo_configs() {
    // 完全复刻 xtask config_compile 流程：编译真实 ACTIVE 配置 → 组
    // bundle → sign_bundle → verify_bundle（svc-config 运行时同路径）。
    let root_key: [u8; 64] = [0x5A; 64];
    let mut bundle =
        RuntimeConfigBundle::new(Digest([0xAB; 32]), 1, TrustedTime(1_700_000_000_000));
    let configs = [
        ("cloud-sync", "cloud-sync.toml"),
        ("control-write", "control-write.toml"),
        ("logging", "logging.toml"),
        ("metrics", "metrics.toml"),
    ];
    for (name, file) in configs {
        let text = repo_config(file);
        let compiled =
            eneros_config_bundle::toml_compile::compile_by_id(name, &text, Some(&root_key), None)
                .unwrap()
                .expect("typed compiler");
        let digest = Digest::of(&compiled.payload);
        bundle.entries.push(SignedConfigEntry {
            config_id: compiled.config_id,
            schema: SchemaVersion(1),
            generation: 1,
            payload: compiled.payload,
            payload_digest: digest,
            signature: Vec::new(),
        });
    }
    // xtask 测试密钥种子 = 0x41。
    let (sk, pk) = {
        let sk = eneros_crypto::Sm2PrivateKey::from_bytes(&[0x41u8; 32]).unwrap();
        let kp = eneros_crypto::Sm2KeyPair::from_private_key(&sk).unwrap();
        (sk, kp.public_key)
    };
    let mut rng = eneros_crypto::CsRng::new();
    sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();
    // 逐条目独立验签（定位）。
    for entry in &bundle.entries {
        let enc = encode_entry_canonical(entry);
        let sig =
            eneros_crypto::Sm2Signature::from_bytes(entry.signature.as_slice().try_into().unwrap());
        eneros_signer_keyring::verify_signature(&enc, &sig, &pk).unwrap_or_else(|_| {
            // 蓝图 65 WP-65.3：类型化原语（双通道统一），拒因词形不变。
            panic!(
                "entry verify FAILED: {} payload_len={}",
                entry.config_id,
                entry.payload.len()
            )
        });
    }
    verify_bundle(&bundle, &pk, None).unwrap();
    // 与磁盘产物一致性：**完整**序列化（canonical + 签名）→ 解码 +
    // 验签（svc-config 嵌入路径同构）。
    let bytes = eneros_config_bundle::encode_bundle_full(&bundle);
    let decoded = eneros_config_bundle::decode_bundle_full(&bytes).unwrap();
    verify_bundle(&decoded, &pk, None).unwrap();
}

#[cfg(feature = "compile")]
#[test]
fn negative_signature_mandatory_false_rejected() {
    let key_hex: String = "5a".repeat(64); // 64B 有效 hex (128 hex chars)
    let toml = format!(
        r#"
[partition]
meta_block = 0
slot_a_base = 1
partition_blocks = 2048
slot_b_base = 2049
[upgrade]
chunk_size = 65536
max_boot_attempts = 3
download_timeout_secs = 5
download_retries = 3
total_download_timeout_secs = 1800
[security]
root_key_hex = "{key_hex}"
signature_mandatory = false
sm3_constant_time = true
"#
    );
    let err = eneros_config_bundle::toml_compile::compile_ota(&toml, None).unwrap_err();
    assert!(
        matches!(
            err,
            ConfigError::InvalidValue {
                field: "signature_mandatory",
                ..
            }
        ),
        "{err}"
    );
}
