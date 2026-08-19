//! svc-config 领域逻辑：签名 RuntimeConfigBundle 装载、验签、consumer
//! 注册表应用、EIPC 查询面（entry / active digest / generation）。
//!
//! EIPC 协议（`b"CFG1"` magic + 子命令）：
//! - `GET_ENTRY`  = 0x01：payload = config_id 字节 → 响应 = canonical
//!   payload（无该配置 → `ST_INVALID`）；
//! - `GET_DIGEST` = 0x02：响应 = active bundle digest（32B）+
//!   generation(8B)；
//! - `GET_GENERATION` = 0x03：响应 = generation(8B)。
//!
//! fail-closed：bundle 缺失（未编译/未签名）→ 服务拒绝一切 GET_ENTRY
//! （`ST_UNAVAILABLE`）；验签失败 → 拒绝装载（零配置生效）。

use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use eneros_config_bundle::payload::{decode_fields, field_u32, field_u64, field_u8};
use eneros_config_bundle::points::PointsConsumer;
use eneros_config_bundle::schema::{cloud_sync, control_write, logging, metrics, ota};
use eneros_config_bundle::{
    ApplyOutcome, ConfigConsumer, ConfigError, ConfigId, ConfigRegistry, Digest,
    RuntimeConfigBundle, SchemaVersion, TypedConfigConsumer,
};
use eneros_crypto::{sm3_hash, Sm2PublicKey};

/// EIPC 子命令。
pub const CMD_GET_ENTRY: u8 = 0x01;
pub const CMD_GET_DIGEST: u8 = 0x02;
pub const CMD_GET_GENERATION: u8 = 0x03;
/// 协议 magic。
pub const PROTO_MAGIC: &[u8; 4] = b"CFG1";

/// 控制写配置（svc-control-dispatch 消费；canonical 解码面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlWriteConfig {
    pub lease_id: [u8; 16],
    pub legacy_input_deny_and_log: bool,
    pub handshake_retries: u8,
}

/// control-write 类型化 consumer（注册表成员）。
#[derive(Debug, Clone, Default)]
pub struct ControlWriteConsumer {
    pub config: Option<ControlWriteConfig>,
    pub generation: u64,
}

impl TypedConfigConsumer for ControlWriteConsumer {
    type Config = ControlWriteConfig;
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
        let mut lease_id = [0u8; 16];
        lease_id.copy_from_slice(lease);
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
        Ok(ControlWriteConfig {
            lease_id,
            legacy_input_deny_and_log: true,
            handshake_retries: retries,
        })
    }
    fn apply(&mut self, cfg: Self::Config, generation: u64) -> Result<ApplyOutcome, ConfigError> {
        match &self.config {
            Some(old) if *old == cfg => return Ok(ApplyOutcome::NoOp),
            _ => {}
        }
        self.config = Some(cfg);
        self.generation = generation;
        Ok(ApplyOutcome::Applied { generation })
    }
}

/// 共享 consumer 包装（注册表持有 + 服务侧读取同一实例）。
pub struct SharedConsumer(Rc<RefCell<ControlWriteConsumer>>);

impl ConfigConsumer for SharedConsumer {
    fn config_id(&self) -> ConfigId {
        ConfigConsumer::config_id(&*self.0.borrow())
    }
    fn schema_version(&self) -> SchemaVersion {
        ConfigConsumer::schema_version(&*self.0.borrow())
    }
    fn decode_raw(&self, raw: &[u8]) -> Result<(), ConfigError> {
        self.0.borrow().decode_raw(raw)
    }
    fn apply_raw(&mut self, raw: &[u8], generation: u64) -> Result<ApplyOutcome, ConfigError> {
        self.0.borrow_mut().apply_raw(raw, generation)
    }
}

/// ConfigService：bundle 装载 + 注册表 + EIPC 查询。
pub struct ConfigService {
    pub registry: ConfigRegistry,
    shared: Rc<RefCell<ControlWriteConsumer>>,
    pub bundle: Option<RuntimeConfigBundle>,
    pub load_error: Option<String>,
    /// 验签公钥（64B SM2 未压缩点；空 = 未注入 → fail-closed）。
    pub verify_pubkey: Option<Sm2PublicKey>,
}

/// 云同步配置（canonical 解码面；消费者 svc-cloud-sync）。
#[derive(Debug, Clone, Default)]
pub struct CloudSyncConsumer {
    pub max_events: u32,
    pub batch_size: u32,
    pub max_retries: u32,
    pub backoff_base_ms: u64,
    pub dead_letter_capacity: u32,
}

impl TypedConfigConsumer for CloudSyncConsumer {
    type Config = CloudSyncConsumer;
    fn config_id(&self) -> ConfigId {
        ConfigId::from_static(cloud_sync::CONFIG_ID)
    }
    fn schema_version(&self) -> SchemaVersion {
        SchemaVersion(cloud_sync::SCHEMA)
    }
    fn decode_validate(&self, raw: &[u8]) -> Result<Self::Config, ConfigError> {
        let config = ConfigId::from_static(cloud_sync::CONFIG_ID);
        let fields = decode_fields(raw, &config, cloud_sync::MAX_FIELD)?;
        Ok(CloudSyncConsumer {
            max_events: field_u32(&fields, &config, cloud_sync::FIELD_MAX_EVENTS, "max_events")?
                .unwrap_or(0),
            batch_size: field_u32(&fields, &config, cloud_sync::FIELD_BATCH_SIZE, "batch_size")?
                .unwrap_or(0),
            max_retries: field_u32(
                &fields,
                &config,
                cloud_sync::FIELD_MAX_RETRIES,
                "max_retries",
            )?
            .unwrap_or(0),
            backoff_base_ms: field_u64(
                &fields,
                &config,
                cloud_sync::FIELD_BACKOFF_BASE_MS,
                "backoff_base_ms",
            )?
            .unwrap_or(0),
            dead_letter_capacity: field_u32(
                &fields,
                &config,
                cloud_sync::FIELD_DEAD_LETTER_CAPACITY,
                "dead_letter_capacity",
            )?
            .unwrap_or(0),
        })
    }
    fn apply(&mut self, cfg: Self::Config, generation: u64) -> Result<ApplyOutcome, ConfigError> {
        self.max_events = cfg.max_events;
        self.batch_size = cfg.batch_size;
        self.max_retries = cfg.max_retries;
        self.backoff_base_ms = cfg.backoff_base_ms;
        self.dead_letter_capacity = cfg.dead_letter_capacity;
        Ok(ApplyOutcome::Applied { generation })
    }
}

/// 日志配置（消费者 svc-observability）。
#[derive(Debug, Clone, Default)]
pub struct LoggingConsumer {
    pub min_level: u8,
    pub buffer_capacity: u32,
    pub storage_capacity: u32,
    pub rotation: u8,
}

impl TypedConfigConsumer for LoggingConsumer {
    type Config = LoggingConsumer;
    fn config_id(&self) -> ConfigId {
        ConfigId::from_static(logging::CONFIG_ID)
    }
    fn schema_version(&self) -> SchemaVersion {
        SchemaVersion(logging::SCHEMA)
    }
    fn decode_validate(&self, raw: &[u8]) -> Result<Self::Config, ConfigError> {
        let config = ConfigId::from_static(logging::CONFIG_ID);
        let fields = decode_fields(raw, &config, logging::MAX_FIELD)?;
        let min_level =
            field_u8(&fields, &config, logging::FIELD_MIN_LEVEL, "min_level")?.unwrap_or(0);
        if min_level > logging::LEVEL_TRACE {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "min_level",
                reason: "range".into(),
            });
        }
        Ok(LoggingConsumer {
            min_level,
            buffer_capacity: field_u32(
                &fields,
                &config,
                logging::FIELD_BUFFER_CAPACITY,
                "buffer_capacity",
            )?
            .unwrap_or(0),
            storage_capacity: field_u32(
                &fields,
                &config,
                logging::FIELD_STORAGE_CAPACITY,
                "storage_capacity",
            )?
            .unwrap_or(0),
            rotation: field_u8(&fields, &config, logging::FIELD_ROTATION, "rotation")?.unwrap_or(0),
        })
    }
    fn apply(&mut self, cfg: Self::Config, generation: u64) -> Result<ApplyOutcome, ConfigError> {
        *self = cfg;
        Ok(ApplyOutcome::Applied { generation })
    }
}

/// 指标配置（消费者 svc-observability）。
#[derive(Debug, Clone, Default)]
pub struct MetricsConsumer {
    pub flush_interval_ms: u64,
    pub buffer_capacity: u32,
    pub per_metric_capacity: u32,
    pub retention: u8,
    pub defaults_mask: u32,
}

impl TypedConfigConsumer for MetricsConsumer {
    type Config = MetricsConsumer;
    fn config_id(&self) -> ConfigId {
        ConfigId::from_static(metrics::CONFIG_ID)
    }
    fn schema_version(&self) -> SchemaVersion {
        SchemaVersion(metrics::SCHEMA)
    }
    fn decode_validate(&self, raw: &[u8]) -> Result<Self::Config, ConfigError> {
        let config = ConfigId::from_static(metrics::CONFIG_ID);
        let fields = decode_fields(raw, &config, metrics::MAX_FIELD)?;
        Ok(MetricsConsumer {
            flush_interval_ms: field_u64(
                &fields,
                &config,
                metrics::FIELD_FLUSH_INTERVAL_MS,
                "flush_interval_ms",
            )?
            .unwrap_or(0),
            buffer_capacity: field_u32(
                &fields,
                &config,
                metrics::FIELD_BUFFER_CAPACITY,
                "buffer_capacity",
            )?
            .unwrap_or(0),
            per_metric_capacity: field_u32(
                &fields,
                &config,
                metrics::FIELD_PER_METRIC_CAPACITY,
                "per_metric_capacity",
            )?
            .unwrap_or(0),
            retention: field_u8(&fields, &config, metrics::FIELD_RETENTION, "retention")?
                .unwrap_or(0),
            defaults_mask: field_u32(
                &fields,
                &config,
                metrics::FIELD_DEFAULTS_MASK,
                "defaults_mask",
            )?
            .unwrap_or(0),
        })
    }
    fn apply(&mut self, cfg: Self::Config, generation: u64) -> Result<ApplyOutcome, ConfigError> {
        *self = cfg;
        Ok(ApplyOutcome::Applied { generation })
    }
}

/// OTA 配置（消费者 svc-ota；root_key 敏感——仅服务内使用，不落日志）。
#[derive(Debug, Clone)]
pub struct OtaConsumer {
    pub partition_blocks: u64,
    pub chunk_size: u32,
    pub max_boot_attempts: u32,
    pub root_key: [u8; 64],
}

impl Default for OtaConsumer {
    fn default() -> Self {
        OtaConsumer {
            partition_blocks: 0,
            chunk_size: 0,
            max_boot_attempts: 0,
            root_key: [0u8; 64],
        }
    }
}

impl TypedConfigConsumer for OtaConsumer {
    type Config = OtaConsumer;
    fn config_id(&self) -> ConfigId {
        ConfigId::from_static(ota::CONFIG_ID)
    }
    fn schema_version(&self) -> SchemaVersion {
        SchemaVersion(ota::SCHEMA)
    }
    fn decode_validate(&self, raw: &[u8]) -> Result<Self::Config, ConfigError> {
        let config = ConfigId::from_static(ota::CONFIG_ID);
        let fields = decode_fields(raw, &config, ota::MAX_FIELD)?;
        let root_key_bytes = eneros_config_bundle::payload::field(&fields, ota::FIELD_ROOT_KEY)
            .ok_or(ConfigError::MissingField {
                config: config.clone(),
                field: "root_key",
            })?;
        if root_key_bytes.len() != ota::ROOT_KEY_LEN {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "root_key",
                reason: "len".into(),
            });
        }
        if eneros_config_bundle::is_zero_bytes(root_key_bytes) {
            return Err(ConfigError::ZeroSecret {
                config: config.clone(),
                field: "root_key",
            });
        }
        let mut root_key = [0u8; 64];
        root_key.copy_from_slice(root_key_bytes);
        Ok(OtaConsumer {
            partition_blocks: field_u64(
                &fields,
                &config,
                ota::FIELD_PARTITION_BLOCKS,
                "partition_blocks",
            )?
            .unwrap_or(0),
            chunk_size: field_u32(&fields, &config, ota::FIELD_CHUNK_SIZE, "chunk_size")?
                .unwrap_or(0),
            max_boot_attempts: field_u32(
                &fields,
                &config,
                ota::FIELD_MAX_BOOT_ATTEMPTS,
                "max_boot_attempts",
            )?
            .unwrap_or(0),
            root_key,
        })
    }
    fn apply(&mut self, cfg: Self::Config, generation: u64) -> Result<ApplyOutcome, ConfigError> {
        self.partition_blocks = cfg.partition_blocks;
        self.chunk_size = cfg.chunk_size;
        self.max_boot_attempts = cfg.max_boot_attempts;
        self.root_key = cfg.root_key;
        Ok(ApplyOutcome::Applied { generation })
    }
}

impl Default for ConfigService {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigService {
    pub fn new() -> Self {
        let shared = Rc::new(RefCell::new(ControlWriteConsumer::default()));
        let mut registry = ConfigRegistry::new();
        registry.register(alloc::boxed::Box::new(SharedConsumer(shared.clone())));
        // 全部 ACTIVE 配置的 consumer 注册（registry 双向完备性：
        // bundle 每条目必须被注册，注册者必须被 bundle 覆盖）。
        registry.register(alloc::boxed::Box::new(CloudSyncConsumer::default()));
        registry.register(alloc::boxed::Box::new(LoggingConsumer::default()));
        registry.register(alloc::boxed::Box::new(MetricsConsumer::default()));
        registry.register(alloc::boxed::Box::new(OtaConsumer::default()));
        // 蓝图 45：点表（bundle 含 points 条目即须有消费；否则 UnconsumedConfig）。
        registry.register(alloc::boxed::Box::new(PointsConsumer::default()));
        ConfigService {
            registry,
            shared,
            bundle: None,
            load_error: None,
            verify_pubkey: None,
        }
    }

    /// 当前生效的 control-write 配置（consumer 应用结果）。
    pub fn control_write(&self) -> Option<ControlWriteConfig> {
        self.shared.borrow().config.clone()
    }

    /// 装载并应用 bundle（生产路径：`verify_pubkey` 必须注入）。
    pub fn load_bundle(
        &mut self,
        bundle: RuntimeConfigBundle,
        expected_manifest_digest: Option<&Digest>,
    ) -> Result<(), ConfigError> {
        let pk = self
            .verify_pubkey
            .ok_or(ConfigError::Storage("no-verify-pubkey"))?;
        // 启动路径：bundle 级 SM2 验签 + 条目 digest（条目级签名在
        // GET_ENTRY 按需验证——QEMU TCG 下 6 次 SM2 超 readiness 窗口）。
        eneros_config_bundle::verify_bundle_startup(&bundle, &pk, expected_manifest_digest)?;
        self.registry
            .apply_bundle(&bundle, &pk, expected_manifest_digest)?;
        self.bundle = Some(bundle);
        self.load_error = None;
        Ok(())
    }

    /// 从嵌入字节装载（生产入口；无字节/无公钥 → fail-closed 拒绝）。
    pub fn load_embedded(&mut self, bytes: &[u8], pubkey_hex: &str) -> Result<(), ConfigError> {
        if bytes.is_empty() {
            self.load_error = Some("no-signed-bundle-embedded".into());
            return Err(ConfigError::Storage("no-signed-bundle-embedded"));
        }
        let key_bytes = match decode_hex_pubkey(pubkey_hex) {
            Some(k) => k,
            None => {
                self.load_error = Some("no-verify-pubkey-embedded".into());
                return Err(ConfigError::Storage("no-verify-pubkey-embedded"));
            }
        };
        self.verify_pubkey = Some(key_bytes);
        let bundle = eneros_config_bundle::decode_bundle_full(bytes).map_err(|e| {
            self.load_error = Some(format!("decode:{e}"));
            e
        })?;
        self.load_bundle(bundle, None).map_err(|e| {
            self.load_error = Some(format!("verify:{e}"));
            e
        })
    }

    pub fn active_digest(&self) -> Option<Digest> {
        self.registry.active_bundle_digest().copied()
    }

    pub fn generation(&self) -> u64 {
        self.bundle.as_ref().map(|b| b.generation).unwrap_or(0)
    }

    /// 处理一个 EIPC 查询（`cmd` + `arg`）→ 响应字节。
    /// 返回 `Err` 表示查询失败（调用方映射为错误响应）。
    pub fn handle_query(&self, cmd: u8, arg: &[u8]) -> Result<Vec<u8>, &'static str> {
        match cmd {
            CMD_GET_ENTRY => {
                let id = ConfigId(arg.iter().map(|b| *b as char).collect());
                let entry = self
                    .bundle
                    .as_ref()
                    .and_then(|b| b.entry(&id))
                    .ok_or("unknown-config")?;
                // 按需条目级 SM2 验签（防篡改纵深；失败 → 拒绝服务）。
                let pk = self.verify_pubkey.as_ref().ok_or("no-verify-pubkey")?;
                let enc = eneros_config_bundle::encode_entry_canonical(entry);
                let sig = eneros_crypto::Sm2Signature::from_bytes(
                    entry
                        .signature
                        .as_slice()
                        .try_into()
                        .map_err(|_| "bad-entry-signature")?,
                );
                let ok =
                    eneros_crypto::sm2_verify(&enc, &sig, pk).map_err(|_| "sm2-verify-failed")?;
                if !ok {
                    return Err("bad-entry-signature");
                }
                Ok(entry.payload.clone())
            }
            CMD_GET_DIGEST => {
                let digest = self.active_digest().ok_or("no-active-bundle")?;
                let mut out = Vec::with_capacity(40);
                out.extend_from_slice(digest.as_bytes());
                out.extend_from_slice(&self.generation().to_le_bytes());
                Ok(out)
            }
            CMD_GET_GENERATION => Ok(self.generation().to_le_bytes().to_vec()),
            _ => Err("unknown-cmd"),
        }
    }
}

/// 解析 65B SM2 公钥 hex（04 前缀 + x‖y；校验长度；全零拒绝）。
pub fn decode_hex_pubkey(hex: &str) -> Option<Sm2PublicKey> {
    let hex = hex.trim();
    if hex.len() != 130 {
        return None;
    }
    let bytes =
        eneros_config_bundle::payload::hex_decode(hex, &ConfigId::from_static("pubkey"), "pubkey")
            .ok()?;
    if bytes.len() != 65 || bytes[0] != 0x04 || bytes[1..].iter().all(|b| *b == 0) {
        return None;
    }
    Sm2PublicKey::from_bytes(&bytes).ok()
}

/// 计算 bundle digest（active config digest 上报语义）。
pub fn bundle_digest(bundle: &RuntimeConfigBundle) -> Digest {
    Digest(sm3_hash(&eneros_config_bundle::encode_bundle_canonical(
        bundle,
    )))
}

#[cfg(test)]
mod tests {
    use eneros_config_bundle::{
        ConfigBundleVersion, Digest as CbDigest, SignedConfigEntry, TrustedTime,
    };
    use eneros_crypto::{CsRng, Sm2KeyPair, Sm2PrivateKey, Sm2Signer};

    use super::*;

    fn sign_helpers() -> (Sm2PrivateKey, Sm2PublicKey) {
        let sk = Sm2PrivateKey::from_bytes(&[0x41; 32]).unwrap();
        let kp = Sm2KeyPair::from_private_key(&sk).unwrap();
        (sk, kp.public_key)
    }

    fn make_entry(payload: Vec<u8>, generation: u64) -> SignedConfigEntry {
        let digest = CbDigest::of(&payload);
        SignedConfigEntry {
            config_id: ConfigId::from_static(control_write::CONFIG_ID),
            schema: SchemaVersion(control_write::SCHEMA),
            generation,
            payload,
            payload_digest: digest,
            signature: Vec::new(),
        }
    }

    /// 全量 ACTIVE 条目（registry 双向完备：含 45 点表）。
    fn make_all_entries(control_write_payload: Vec<u8>) -> Vec<SignedConfigEntry> {
        use eneros_config_bundle::schema::{cloud_sync, logging, metrics, ota, points};
        let mut entries = vec![make_entry(control_write_payload, 3)];
        let root_key = [0x5Au8; 64];
        let configs = [
            (cloud_sync::CONFIG_ID, "cloud-sync.toml", 0u8),
            (logging::CONFIG_ID, "logging.toml", 0),
            (metrics::CONFIG_ID, "metrics.toml", 0),
            (ota::CONFIG_ID, "ota.toml", 1),
            (points::CONFIG_ID, "points.toml", 0),
        ];
        for (id, file, need_key) in configs {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../configs")
                .join(file);
            let text = std::fs::read_to_string(&path).unwrap();
            let injected = if need_key == 1 { Some(&root_key) } else { None };
            let compiled = eneros_config_bundle::toml_compile::compile_by_id(id, &text, injected)
                .unwrap()
                .unwrap();
            let digest = CbDigest::of(&compiled.payload);
            entries.push(SignedConfigEntry {
                config_id: compiled.config_id,
                schema: SchemaVersion(1),
                generation: 3,
                payload: compiled.payload,
                payload_digest: digest,
                signature: Vec::new(),
            });
        }
        entries
    }

    fn make_bundle(payload: Vec<u8>) -> RuntimeConfigBundle {
        RuntimeConfigBundle {
            schema: ConfigBundleVersion::V1,
            product_manifest_digest: CbDigest([0x41; 32]),
            generation: 3,
            created_at: TrustedTime(1_700_000_000_000),
            entries: make_all_entries(payload),
            signature: Vec::new(),
        }
    }

    #[test]
    fn positive_load_and_query() {
        let (sk, pk) = sign_helpers();
        let mut rng = CsRng::new();
        let payload = eneros_config_bundle::payload::encode_payload(&[
            (control_write::FIELD_LEASE_ID, vec![0x33; 16]),
            (control_write::FIELD_LEGACY_INPUT, vec![1]),
            (control_write::FIELD_HANDSHAKE_RETRIES, vec![4]),
        ]);
        let mut bundle = make_bundle(payload.clone());
        eneros_config_bundle::sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();

        let mut svc = ConfigService::new();
        svc.verify_pubkey = Some(pk);
        svc.load_bundle(bundle, Some(&CbDigest([0x41; 32])))
            .unwrap();
        assert!(svc.control_write().is_some());
        assert_eq!(svc.control_write().unwrap().handshake_retries, 4);
        assert_eq!(svc.generation(), 3);
        // EIPC 查询。
        let entry = svc
            .handle_query(CMD_GET_ENTRY, control_write::CONFIG_ID.as_bytes())
            .unwrap();
        assert_eq!(entry, payload);
        let digest = svc.handle_query(CMD_GET_DIGEST, &[]).unwrap();
        assert_eq!(digest.len(), 40);
        let gen = svc.handle_query(CMD_GET_GENERATION, &[]).unwrap();
        assert_eq!(u64::from_le_bytes(gen.try_into().unwrap()), 3);
    }

    #[test]
    fn negative_bad_signature_rejected_no_config() {
        let (sk, pk) = sign_helpers();
        let (_sk2, pk2) = {
            let sk2 = Sm2PrivateKey::from_bytes(&[0x42; 32]).unwrap();
            let kp2 = Sm2KeyPair::from_private_key(&sk2).unwrap();
            (sk2, kp2.public_key)
        };
        let mut rng = CsRng::new();
        let payload = eneros_config_bundle::payload::encode_payload(&[
            (control_write::FIELD_LEASE_ID, vec![0x33; 16]),
            (control_write::FIELD_LEGACY_INPUT, vec![1]),
            (control_write::FIELD_HANDSHAKE_RETRIES, vec![4]),
        ]);
        let mut bundle = make_bundle(payload);
        eneros_config_bundle::sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();
        let mut svc = ConfigService::new();
        svc.verify_pubkey = Some(pk2); // 错误公钥
        assert!(svc.load_bundle(bundle, None).is_err());
        assert!(svc.control_write().is_none());
        assert!(svc.active_digest().is_none());
    }

    #[test]
    fn negative_embedded_missing_fails_closed() {
        let mut svc = ConfigService::new();
        assert!(svc.load_embedded(&[], "").is_err());
        assert!(svc
            .handle_query(CMD_GET_ENTRY, control_write::CONFIG_ID.as_bytes())
            .is_err());
    }

    #[test]
    fn negative_unknown_entry_query() {
        let (sk, pk) = sign_helpers();
        let mut rng = CsRng::new();
        let payload = eneros_config_bundle::payload::encode_payload(&[
            (control_write::FIELD_LEASE_ID, vec![0x33; 16]),
            (control_write::FIELD_LEGACY_INPUT, vec![1]),
            (control_write::FIELD_HANDSHAKE_RETRIES, vec![4]),
        ]);
        let mut bundle = make_bundle(payload);
        eneros_config_bundle::sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();
        let mut svc = ConfigService::new();
        svc.verify_pubkey = Some(pk);
        svc.load_bundle(bundle, None).unwrap();
        assert!(svc.handle_query(CMD_GET_ENTRY, b"nope").is_err());
        assert!(svc.handle_query(0x99, &[]).is_err());
    }

    #[test]
    fn pubkey_hex_parse() {
        // 64 字节非零 → 可解析。
        let (_sk, pk) = sign_helpers();
        let bytes = pk.to_bytes_uncompressed();
        assert_eq!(bytes.len(), 65);
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert!(decode_hex_pubkey(&hex).is_some());
        assert!(decode_hex_pubkey(&"00".repeat(130)).is_none());
        assert!(decode_hex_pubkey("").is_none());
    }

    #[test]
    fn all_consumers_apply_propagate_generation() {
        // 蓝图 41：apply 结果必须携带实际 generation，不能固定 0。
        let mut cloud = CloudSyncConsumer::default();
        assert_eq!(
            cloud.apply(CloudSyncConsumer::default(), 42).unwrap(),
            ApplyOutcome::Applied { generation: 42 }
        );
        let mut logging = LoggingConsumer::default();
        assert_eq!(
            logging.apply(LoggingConsumer::default(), 43).unwrap(),
            ApplyOutcome::Applied { generation: 43 }
        );
        let mut metrics = MetricsConsumer::default();
        assert_eq!(
            metrics.apply(MetricsConsumer::default(), 44).unwrap(),
            ApplyOutcome::Applied { generation: 44 }
        );
        let mut ota = OtaConsumer::default();
        assert_eq!(
            ota.apply(OtaConsumer::default(), 45).unwrap(),
            ApplyOutcome::Applied { generation: 45 }
        );
    }

    #[test]
    fn deterministic_signature_vector() {
        // 确定性签名（RFC 6979）与随机路径等价（验签均过）。
        let (sk, pk) = sign_helpers();
        let payload = eneros_config_bundle::payload::encode_payload(&[
            (control_write::FIELD_LEASE_ID, vec![0x33; 16]),
            (control_write::FIELD_LEGACY_INPUT, vec![1]),
            (control_write::FIELD_HANDSHAKE_RETRIES, vec![4]),
        ]);
        let mut bundle = make_bundle(payload);
        let enc = eneros_config_bundle::encode_bundle_canonical(&bundle);
        let _sig = Sm2Signer::new().sign_deterministic(&enc, &sk, &pk).unwrap();
        let mut rng = CsRng::new();
        eneros_config_bundle::sign_bundle(&mut bundle, &sk, &pk, &mut rng).unwrap();
        eneros_config_bundle::verify_bundle(&bundle, &pk, None).unwrap();
    }
}
