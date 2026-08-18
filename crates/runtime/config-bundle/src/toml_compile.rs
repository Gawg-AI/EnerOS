//! host 侧 TOML → canonical payload 编译面（feature `compile`；xtask
//! `config compile` 与测试使用；target 闭包不编译本模块）。
//!
//! 严格性（蓝图 WP-41.2 不变量）：
//! - 每个 ACTIVE 配置声明允许的 section/key 集合；**未知键、未知 section、
//!   重复键一律编译错误**（不接受 `# consumer:` 注释自证）；
//! - 全零密钥/哈希/占位字符串编译错误（`ZeroSecret`）；
//! - 类型/范围（u64/u32/u8/枚举/定长 hex）编译期校验；
//! - `root_key`（ota）等敏感值不在仓内落明文——由构建期注入
//!   （`compile_ota` 的 `injected_root_key: Option<&[u8;64]>`），文件里
//!   缺失即要求注入，注入值全零拒绝。
//!
//! 输出 canonical payload（`payload::encode_payload`），随后经
//! `RuntimeConfigBundle` 签名进介质；target 侧 `PayloadCursor` 解码。

use alloc::string::String;
use alloc::vec::Vec;

use toml::Value;

use crate::payload::encode_payload;
use crate::schema::{cloud_sync, control_write, logging, metrics, ota};
use crate::{ConfigError, ConfigId};

/// 编译结果：config_id + canonical payload。
#[derive(Debug)]
pub struct CompiledConfig {
    pub config_id: ConfigId,
    pub payload: Vec<u8>,
}

fn table<'a>(
    root: &'a Value,
    config: &ConfigId,
    name: &'static str,
) -> Result<&'a toml::map::Map<String, Value>, ConfigError> {
    root.as_table().ok_or_else(|| ConfigError::InvalidValue {
        config: config.clone(),
        field: name,
        reason: "expected-toml-table".into(),
    })
}

fn section<'a>(
    root: &'a Value,
    config: &ConfigId,
    name: &'static str,
) -> Result<&'a toml::map::Map<String, Value>, ConfigError> {
    root.get(name)
        .and_then(Value::as_table)
        .ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "missing-section".into(),
        })
}

fn reject_unknown_keys(
    table: &toml::map::Map<String, Value>,
    config: &ConfigId,
    allowed: &[&str],
) -> Result<(), ConfigError> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ConfigError::UnknownKey {
                config: config.clone(),
                key: key.clone(),
            });
        }
    }
    Ok(())
}

fn get_u64(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
) -> Result<u64, ConfigError> {
    match t.get(name) {
        None => Err(ConfigError::MissingField {
            config: config.clone(),
            field: name,
        }),
        Some(Value::Integer(i)) if *i >= 0 => Ok(*i as u64),
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-non-negative-integer".into(),
        }),
    }
}

fn get_u32(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
) -> Result<u32, ConfigError> {
    let v = get_u64(t, config, name)?;
    u32::try_from(v).map_err(|_| ConfigError::InvalidValue {
        config: config.clone(),
        field: name,
        reason: "u32-overflow".into(),
    })
}

fn get_u8(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
) -> Result<u8, ConfigError> {
    let v = get_u64(t, config, name)?;
    u8::try_from(v).map_err(|_| ConfigError::InvalidValue {
        config: config.clone(),
        field: name,
        reason: "u8-overflow".into(),
    })
}

fn get_bool(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
) -> Result<bool, ConfigError> {
    match t.get(name) {
        None => Err(ConfigError::MissingField {
            config: config.clone(),
            field: name,
        }),
        Some(Value::Boolean(b)) => Ok(*b),
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-boolean".into(),
        }),
    }
}

/// 编译 `configs/control-write.toml`。
pub fn compile_control_write(toml: &str) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(control_write::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, &["control_write"])?;
    let cw = section(&root, &config, "control_write")?;
    reject_unknown_keys(
        cw,
        &config,
        &["lease_id", "legacy_input", "handshake_retries"],
    )?;

    // 1: lease_id——16B hex；全零拒绝；长度错拒绝。
    let lease_hex =
        cw.get("lease_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ConfigError::MissingField {
                config: config.clone(),
                field: "lease_id",
            })?;
    let lease = crate::payload::hex_decode(lease_hex, &config, "lease_id")?;
    if lease.len() != control_write::LEASE_ID_LEN {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "lease_id",
            reason: format!("expected-{}-bytes", control_write::LEASE_ID_LEN),
        });
    }
    if crate::is_zero_bytes(&lease) {
        return Err(ConfigError::ZeroSecret {
            config: config.clone(),
            field: "lease_id",
        });
    }
    // 2: legacy_input——唯一合法值 "deny-and-log"。
    let legacy = cw
        .get("legacy_input")
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::MissingField {
            config: config.clone(),
            field: "legacy_input",
        })?;
    let legacy_enc = match legacy {
        "deny-and-log" => control_write::LEGACY_INPUT_DENY_AND_LOG,
        other => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "legacy_input",
                reason: format!("unknown-enum:{other}"),
            })
        }
    };
    // 3: handshake_retries——u8 ≤5。
    let retries = get_u8(cw, &config, "handshake_retries")?;
    if retries > control_write::HANDSHAKE_RETRIES_MAX {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "handshake_retries",
            reason: format!("max-{}", control_write::HANDSHAKE_RETRIES_MAX),
        });
    }
    Ok(CompiledConfig {
        payload: encode_payload(&[
            (control_write::FIELD_LEASE_ID, lease),
            (control_write::FIELD_LEGACY_INPUT, vec![legacy_enc]),
            (control_write::FIELD_HANDSHAKE_RETRIES, vec![retries]),
        ]),
        config_id: config,
    })
}

/// 编译 `configs/ota.toml`。`injected_root_key`：构建期注入的 64B SM2
/// 未压缩点（KMS/产线）；文件内 `root_key_hex` 存在时优先使用，两者皆无
/// → MissingField；全零 → ZeroSecret。
pub fn compile_ota(
    toml: &str,
    injected_root_key: Option<&[u8; 64]>,
) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(ota::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, &["partition", "upgrade", "security"])?;
    let part = section(&root, &config, "partition")?;
    reject_unknown_keys(
        part,
        &config,
        &[
            "meta_block",
            "slot_a_base",
            "partition_blocks",
            "slot_b_base",
        ],
    )?;
    let upg = section(&root, &config, "upgrade")?;
    reject_unknown_keys(
        upg,
        &config,
        &[
            "chunk_size",
            "max_boot_attempts",
            "download_timeout_secs",
            "download_retries",
            "total_download_timeout_secs",
        ],
    )?;
    let sec = section(&root, &config, "security")?;
    reject_unknown_keys(
        sec,
        &config,
        &["root_key_hex", "signature_mandatory", "sm3_constant_time"],
    )?;

    let mut fields: Vec<(u8, Vec<u8>)> = vec![
        (
            ota::FIELD_META_BLOCK,
            get_u64(part, &config, "meta_block")?.to_le_bytes().to_vec(),
        ),
        (
            ota::FIELD_SLOT_A_BASE,
            get_u64(part, &config, "slot_a_base")?
                .to_le_bytes()
                .to_vec(),
        ),
        (
            ota::FIELD_PARTITION_BLOCKS,
            get_u64(part, &config, "partition_blocks")?
                .to_le_bytes()
                .to_vec(),
        ),
        (
            ota::FIELD_SLOT_B_BASE,
            get_u64(part, &config, "slot_b_base")?
                .to_le_bytes()
                .to_vec(),
        ),
        (
            ota::FIELD_CHUNK_SIZE,
            get_u32(upg, &config, "chunk_size")?.to_le_bytes().to_vec(),
        ),
        (
            ota::FIELD_MAX_BOOT_ATTEMPTS,
            get_u32(upg, &config, "max_boot_attempts")?
                .to_le_bytes()
                .to_vec(),
        ),
        (
            ota::FIELD_DOWNLOAD_TIMEOUT_SECS,
            get_u32(upg, &config, "download_timeout_secs")?
                .to_le_bytes()
                .to_vec(),
        ),
        (
            ota::FIELD_DOWNLOAD_RETRIES,
            get_u32(upg, &config, "download_retries")?
                .to_le_bytes()
                .to_vec(),
        ),
        (
            ota::FIELD_TOTAL_DOWNLOAD_TIMEOUT_SECS,
            get_u32(upg, &config, "total_download_timeout_secs")?
                .to_le_bytes()
                .to_vec(),
        ),
    ];
    // root_key：文件内 hex（产线模板不落）或构建期注入；全零拒绝。
    let root_key: Vec<u8> = match sec.get("root_key_hex").and_then(Value::as_str) {
        Some(hex) => crate::payload::hex_decode(hex, &config, "root_key_hex")?,
        None => match injected_root_key {
            Some(k) => k.to_vec(),
            None => {
                return Err(ConfigError::MissingField {
                    config: config.clone(),
                    field: "root_key",
                })
            }
        },
    };
    if root_key.len() != ota::ROOT_KEY_LEN {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "root_key",
            reason: format!("expected-{}-bytes", ota::ROOT_KEY_LEN),
        });
    }
    if crate::is_zero_bytes(&root_key) {
        return Err(ConfigError::ZeroSecret {
            config: config.clone(),
            field: "root_key",
        });
    }
    fields.push((ota::FIELD_ROOT_KEY, root_key));
    // signature_mandatory 必须为 true（无旁路开关）。
    let mandatory = get_bool(sec, &config, "signature_mandatory")?;
    if !mandatory {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "signature_mandatory",
            reason: "must-be-true".into(),
        });
    }
    fields.push((ota::FIELD_SIGNATURE_MANDATORY, vec![1u8]));
    // sm3_constant_time 必须为 true（常时比较无旁路开关）。
    let ct = get_bool(sec, &config, "sm3_constant_time")?;
    if !ct {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "sm3_constant_time",
            reason: "must-be-true".into(),
        });
    }
    fields.push((ota::FIELD_SM3_CONSTANT_TIME, vec![1u8]));
    fields.sort_by_key(|(id, _)| *id);
    Ok(CompiledConfig {
        payload: encode_payload(&fields),
        config_id: config,
    })
}

/// 编译 `configs/cloud-sync.toml`。
pub fn compile_cloud_sync(toml: &str) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(cloud_sync::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, &["event_store", "delta_sync", "retry_queue"])?;
    let es = section(&root, &config, "event_store")?;
    reject_unknown_keys(es, &config, &["max_events"])?;
    let ds = section(&root, &config, "delta_sync")?;
    reject_unknown_keys(ds, &config, &["batch_size", "compression"])?;
    let rq = section(&root, &config, "retry_queue")?;
    reject_unknown_keys(
        rq,
        &config,
        &["max_retries", "backoff_base_ms", "dead_letter_capacity"],
    )?;

    let compression = ds
        .get("compression")
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::MissingField {
            config: config.clone(),
            field: "compression",
        })?;
    let comp_enc = match compression {
        "None" => cloud_sync::COMPRESSION_NONE,
        other => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "compression",
                reason: format!("unknown-enum:{other}"),
            })
        }
    };
    Ok(CompiledConfig {
        payload: encode_payload(&[
            (
                cloud_sync::FIELD_MAX_EVENTS,
                get_u32(es, &config, "max_events")?.to_le_bytes().to_vec(),
            ),
            (
                cloud_sync::FIELD_BATCH_SIZE,
                get_u32(ds, &config, "batch_size")?.to_le_bytes().to_vec(),
            ),
            (cloud_sync::FIELD_COMPRESSION, vec![comp_enc]),
            (
                cloud_sync::FIELD_MAX_RETRIES,
                get_u32(rq, &config, "max_retries")?.to_le_bytes().to_vec(),
            ),
            (
                cloud_sync::FIELD_BACKOFF_BASE_MS,
                get_u64(rq, &config, "backoff_base_ms")?
                    .to_le_bytes()
                    .to_vec(),
            ),
            (
                cloud_sync::FIELD_DEAD_LETTER_CAPACITY,
                get_u32(rq, &config, "dead_letter_capacity")?
                    .to_le_bytes()
                    .to_vec(),
            ),
        ]),
        config_id: config,
    })
}

/// 编译 `configs/metrics.toml`。
pub fn compile_metrics(toml: &str) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(metrics::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, &["collector", "store", "defaults"])?;
    let col = section(&root, &config, "collector")?;
    reject_unknown_keys(col, &config, &["flush_interval_ms", "buffer_capacity"])?;
    let store = section(&root, &config, "store")?;
    reject_unknown_keys(store, &config, &["per_metric_capacity", "retention"])?;
    let defs = section(&root, &config, "defaults")?;
    let mut allowed_defs: Vec<&str> = metrics::DEFAULTS_KEYS.to_vec();
    allowed_defs.push("business_optimization_calls");
    reject_unknown_keys(defs, &config, &allowed_defs)?;

    let retention = store
        .get("retention")
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::MissingField {
            config: config.clone(),
            field: "retention",
        })?;
    let ret_enc = match retention {
        "drop_oldest" => metrics::RETENTION_DROP_OLDEST,
        "reject_new" => metrics::RETENTION_REJECT_NEW,
        other => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "retention",
                reason: format!("unknown-enum:{other}"),
            })
        }
    };
    // defaults：8 个布尔 → bitmask（bit0..=bit7）。
    let mut mask: u32 = 0;
    for (i, key) in allowed_defs.iter().enumerate() {
        if get_bool(defs, &config, key)? {
            mask |= 1 << i;
        }
    }
    Ok(CompiledConfig {
        payload: encode_payload(&[
            (
                metrics::FIELD_FLUSH_INTERVAL_MS,
                get_u64(col, &config, "flush_interval_ms")?
                    .to_le_bytes()
                    .to_vec(),
            ),
            (
                metrics::FIELD_BUFFER_CAPACITY,
                get_u32(col, &config, "buffer_capacity")?
                    .to_le_bytes()
                    .to_vec(),
            ),
            (
                metrics::FIELD_PER_METRIC_CAPACITY,
                get_u32(store, &config, "per_metric_capacity")?
                    .to_le_bytes()
                    .to_vec(),
            ),
            (metrics::FIELD_RETENTION, vec![ret_enc]),
            (metrics::FIELD_DEFAULTS_MASK, mask.to_le_bytes().to_vec()),
        ]),
        config_id: config,
    })
}

/// 编译 `configs/logging.toml`。
pub fn compile_logging(toml: &str) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(logging::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, &["logger", "storage", "fields"])?;
    let logger = section(&root, &config, "logger")?;
    reject_unknown_keys(logger, &config, &["min_level", "buffer_capacity"])?;
    let storage = section(&root, &config, "storage")?;
    reject_unknown_keys(storage, &config, &["capacity", "rotation"])?;
    let fields = section(&root, &config, "fields")?;
    reject_unknown_keys(fields, &config, &["max_custom_fields"])?;

    let level = logger
        .get("min_level")
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::MissingField {
            config: config.clone(),
            field: "min_level",
        })?;
    let level_enc = match level {
        "ERROR" => logging::LEVEL_ERROR,
        "WARN" => logging::LEVEL_WARN,
        "INFO" => logging::LEVEL_INFO,
        "DEBUG" => logging::LEVEL_DEBUG,
        "TRACE" => logging::LEVEL_TRACE,
        other => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "min_level",
                reason: format!("unknown-enum:{other}"),
            })
        }
    };
    let rotation = storage
        .get("rotation")
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::MissingField {
            config: config.clone(),
            field: "rotation",
        })?;
    let rot_enc = match rotation {
        "drop_oldest" => logging::ROTATION_DROP_OLDEST,
        "reject_new" => logging::ROTATION_REJECT_NEW,
        other => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "rotation",
                reason: format!("unknown-enum:{other}"),
            })
        }
    };
    Ok(CompiledConfig {
        payload: encode_payload(&[
            (logging::FIELD_MIN_LEVEL, vec![level_enc]),
            (
                logging::FIELD_BUFFER_CAPACITY,
                get_u32(logger, &config, "buffer_capacity")?
                    .to_le_bytes()
                    .to_vec(),
            ),
            (
                logging::FIELD_STORAGE_CAPACITY,
                get_u32(storage, &config, "capacity")?
                    .to_le_bytes()
                    .to_vec(),
            ),
            (logging::FIELD_ROTATION, vec![rot_enc]),
            (
                logging::FIELD_MAX_CUSTOM_FIELDS,
                get_u32(fields, &config, "max_custom_fields")?
                    .to_le_bytes()
                    .to_vec(),
            ),
        ]),
        config_id: config,
    })
}

/// 编译分派（按 config_id 字符串；未登记 → None）。
pub fn compile_by_id(
    config_id: &str,
    toml: &str,
    injected_root_key: Option<&[u8; 64]>,
) -> Result<Option<CompiledConfig>, ConfigError> {
    match config_id {
        control_write::CONFIG_ID => compile_control_write(toml).map(Some),
        ota::CONFIG_ID => compile_ota(toml, injected_root_key).map(Some),
        cloud_sync::CONFIG_ID => compile_cloud_sync(toml).map(Some),
        metrics::CONFIG_ID => compile_metrics(toml).map(Some),
        logging::CONFIG_ID => compile_logging(toml).map(Some),
        _ => Ok(None),
    }
}
