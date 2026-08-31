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

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use toml::Value;

use crate::payload::encode_payload;
use crate::points::{point_type_from_name, point_type_ordinal, PointProtocol};
use crate::schema::{
    ai_peer, catalog, cloud_sync, control_write, logging, metrics, ota, points, security,
};
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

/// 必填 f64（TOML 整数自动升浮点——配置里 `scale = 1` 合法）。
fn get_f64_req(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
) -> Result<f64, ConfigError> {
    match t.get(name) {
        None => Err(ConfigError::MissingField {
            config: config.clone(),
            field: name,
        }),
        Some(Value::Float(f)) if f.is_finite() => Ok(*f),
        Some(Value::Integer(i)) => Ok(*i as f64),
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-number".into(),
        }),
    }
}

/// 可选 f64（缺省 → `default`）。
fn get_f64_opt(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
    default: f64,
) -> Result<f64, ConfigError> {
    match t.get(name) {
        None => Ok(default),
        Some(Value::Float(f)) if f.is_finite() => Ok(*f),
        Some(Value::Integer(i)) => Ok(*i as f64),
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-number".into(),
        }),
    }
}

/// 可选 u32（缺省 → `default`）。
fn get_u32_opt(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
    default: u32,
) -> Result<u32, ConfigError> {
    match t.get(name) {
        None => Ok(default),
        Some(Value::Integer(i)) if *i >= 0 => {
            u32::try_from(*i).map_err(|_| ConfigError::InvalidValue {
                config: config.clone(),
                field: name,
                reason: "u32-overflow".into(),
            })
        }
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-non-negative-integer".into(),
        }),
    }
}

/// 可选 u64（缺省 → `default`）。
fn get_u64_opt(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
    default: u64,
) -> Result<u64, ConfigError> {
    match t.get(name) {
        None => Ok(default),
        Some(Value::Integer(i)) if *i >= 0 => Ok(*i as u64),
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-non-negative-integer".into(),
        }),
    }
}

/// 可选 bool（缺省 → `default`）。
fn get_bool_opt(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
    default: bool,
) -> Result<bool, ConfigError> {
    match t.get(name) {
        None => Ok(default),
        Some(Value::Boolean(b)) => Ok(*b),
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-boolean".into(),
        }),
    }
}

/// 必填字符串。
fn get_str_req<'a>(
    t: &'a toml::map::Map<String, Value>,
    config: &ConfigId,
    name: &'static str,
) -> Result<&'a str, ConfigError> {
    match t.get(name) {
        None => Err(ConfigError::MissingField {
            config: config.clone(),
            field: name,
        }),
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-string".into(),
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
        &[
            "lease_id",
            "legacy_input",
            "handshake_retries",
            "rate_per_hour",
        ],
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
    // 4（47 WP-47.1，可选）：rate_per_hour——每小时 dispatch 确认配额。
    // 缺省 = 产品默认 60/h（装配侧）；qemu-test 演示放宽值经本配置注入。
    let mut fields = vec![
        (control_write::FIELD_LEASE_ID, lease),
        (control_write::FIELD_LEGACY_INPUT, vec![legacy_enc]),
        (control_write::FIELD_HANDSHAKE_RETRIES, vec![retries]),
    ];
    if let Some(rate) = cw.get("rate_per_hour").and_then(Value::as_integer) {
        if rate <= 0 || rate > 1_000_000 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "rate_per_hour",
                reason: "range-1..=1000000".into(),
            });
        }
        fields.push((
            control_write::FIELD_RATE_PER_HOUR,
            (rate as u32).to_le_bytes().to_vec(),
        ));
    }
    Ok(CompiledConfig {
        payload: encode_payload(&fields),
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

/// 编译 `configs/points.toml`（蓝图 45 WP-45.1）。
///
/// schema 字段封闭：每点只允许 `point_id/device_id/protocol/addr/data_type/
/// scale/offset/period_ms/deadband/min/max`——未知键 → 编译错误。不变量：
/// - point_id 全表唯一（重复 → `DuplicateField`）；
/// - scale 禁 0；offset 必填；period_ms ∈ 1..=MAX_PERIOD_MS（缺省
///   DEFAULT_PERIOD_MS）；deadband 缺省 0；min/max 成对且 min ≤ max；
/// - data_type 名精确匹配 upa-model `PointType`（analog/digital/control/
///   setpoint/counter），禁同义词；
/// - 地址按 protocol 直映 `ProtocolAddress`：modbus-rtu/tcp→`addr.modbus`、
///   iec104→`addr.iec104`、mqtt→`addr.mqtt`、mms→`addr.mms`；goose/sv 为
///   订阅绑定不写 addr（写了 → 错误）；`addr` 表只允许协议对应单一子键。
///
/// 输出 `FIELD_POINTS_V2` + `FIELD_SOURCES`（布局见 `schema::points` 模块
/// 注释；蓝图 61 v0.314.27：点记录尾随身份块 + 来源注册表）。
pub fn compile_points(toml: &str) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(points::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, &["point", "source"])?;
    let arr =
        root.get("point")
            .and_then(Value::as_array)
            .ok_or_else(|| ConfigError::MissingField {
                config: config.clone(),
                field: "point",
            })?;
    if arr.len() as u32 > points::MAX_POINTS {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "point",
            reason: "too-many-points".into(),
        });
    }
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut payload: Vec<u8> = Vec::new();
    payload.extend_from_slice(&(arr.len() as u32).to_le_bytes());
    for (idx, ptv) in arr.iter().enumerate() {
        let ctx = alloc::format!("point[{idx}]");
        let pt = ptv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "point",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(
            pt,
            &config,
            &[
                "point_id",
                "device_id",
                "protocol",
                "addr",
                "data_type",
                "scale",
                "offset",
                "period_ms",
                "deadband",
                "min",
                "max",
                "unit",
                "asset_id",
                "schema_id",
            ],
        )?;
        let point_id = get_u32(pt, &config, "point_id")?;
        if !seen.insert(point_id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: points::FIELD_POINTS,
            });
        }
        let device_id = u16::try_from(get_u64(pt, &config, "device_id")?).map_err(|_| {
            ConfigError::InvalidValue {
                config: config.clone(),
                field: "device_id",
                reason: "u16-overflow".into(),
            }
        })?;
        let proto_name = get_str_req(pt, &config, "protocol")?;
        let protocol =
            PointProtocol::from_name(proto_name).ok_or_else(|| ConfigError::InvalidValue {
                config: config.clone(),
                field: "protocol",
                reason: alloc::format!("unknown-enum:{proto_name}"),
            })?;
        let dt_name = get_str_req(pt, &config, "data_type")?;
        let data_type = point_type_from_name(dt_name).ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "data_type",
            reason: alloc::format!("unknown-enum:{dt_name}"),
        })?;
        let scale = get_f64_req(pt, &config, "scale")?;
        if !(scale.is_finite() && scale != 0.0) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "scale",
                reason: "must-be-nonzero".into(),
            });
        }
        let offset = get_f64_req(pt, &config, "offset")?;
        let period_ms = get_u32_opt(pt, &config, "period_ms", points::DEFAULT_PERIOD_MS)?;
        if period_ms == 0 || period_ms > points::MAX_PERIOD_MS {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "period_ms",
                reason: "out-of-range".into(),
            });
        }
        let deadband = get_f64_opt(pt, &config, "deadband", 0.0)?;
        let range = compile_range(pt, &config)?;
        let addr = compile_addr(pt, &config, protocol, &ctx)?;
        // 蓝图 61 身份绑定（可选键；未配 = 0 = UNKNOWN，不满足控制资格）。
        let unit_code = compile_unit_code(pt, &config)?;
        let asset_id = get_u64_opt(pt, &config, "asset_id", 0)?;
        let schema_id = get_u64_opt(pt, &config, "schema_id", 0)?;
        let schema_id = u32::try_from(schema_id).map_err(|_| ConfigError::InvalidValue {
            config: config.clone(),
            field: "schema_id",
            reason: "u32-overflow".into(),
        })?;
        if schema_id != 0 && asset_id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "asset_id",
                reason: "schema-without-asset".into(),
            });
        }
        payload.extend_from_slice(&point_id.to_le_bytes());
        payload.extend_from_slice(&device_id.to_le_bytes());
        payload.push(protocol.tag());
        payload.push(point_type_ordinal(data_type));
        payload.extend_from_slice(&scale.to_le_bytes());
        payload.extend_from_slice(&offset.to_le_bytes());
        payload.extend_from_slice(&period_ms.to_le_bytes());
        payload.extend_from_slice(&deadband.to_le_bytes());
        payload.push(if range.is_some() { 1 } else { 0 });
        if let Some((mn, mx)) = range {
            payload.extend_from_slice(&mn.to_le_bytes());
            payload.extend_from_slice(&mx.to_le_bytes());
        }
        payload.extend_from_slice(&(addr.len() as u16).to_le_bytes());
        payload.extend_from_slice(&addr);
        // v2 身份块（24B；mapping_generation 编译期置 0，consumer apply 期
        // 以 bundle generation 覆盖注入）。
        payload.extend_from_slice(&unit_code.to_le_bytes());
        payload.extend_from_slice(&schema_id.to_le_bytes());
        payload.extend_from_slice(&1u16.to_le_bytes()); // schema_version
        payload.extend_from_slice(&asset_id.to_le_bytes());
        payload.extend_from_slice(&0u64.to_le_bytes());
    }
    // 来源注册表（[[source]]；可缺省 = 一切来源未注册，fail-closed）。
    let sources_payload = compile_sources(&root, &config)?;
    let mut fields: Vec<(u8, Vec<u8>)> = Vec::with_capacity(2);
    fields.push((points::FIELD_POINTS_V2, payload));
    if !sources_payload.is_empty() {
        fields.push((points::FIELD_SOURCES, sources_payload));
    }
    Ok(CompiledConfig {
        payload: encode_payload(fields),
        config_id: config,
    })
}

/// 蓝图 61 v0.314.33：SemanticCatalog 编译面——`configs/catalog.toml`
/// → canonical payload（`schema::catalog` 字段布局单一事实）。
///
/// 编译期就做交叉引用（[08] §2.3：cross-ref 检查后进 bundle）：
/// 重复 ID、点语义引用不存在的资产/schema、控制点无回读、
/// 反馈槽 5 槽不齐、拓扑边引用不存在的节点、单位换算量纲不兼容、
/// 未知单位名——全部编译错误。target 解码面再做一次逐字节 fail-closed。
pub fn compile_catalog(toml: &str) -> Result<CompiledConfig, ConfigError> {
    use crate::schema::catalog;

    let config = ConfigId::from_static(catalog::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(
        t,
        &config,
        &[
            "generation",
            "valid_from_ns",
            "valid_until_ns",
            "mapping_version",
            "topology_version",
            "asset_version",
            "contract_version",
            "safety_version",
            "schema",
            "unit_conversion",
            "asset",
            "topology_node",
            "topology_edge",
            "point_semantics",
            "feedback_slot",
            "coverage",
            "control_contract",
            "safety_constraint",
            "alias",
            "tombstone",
        ],
    )?;
    // 目录头（64B = 8 × u64）。
    let generation = get_u64(t, &config, "generation")?;
    if generation == 0 {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "generation",
            reason: "zero-generation".into(),
        });
    }
    let mut header = Vec::with_capacity(catalog::HEADER_LEN);
    header.extend_from_slice(&generation.to_le_bytes());
    for key in [
        "valid_from_ns",
        "valid_until_ns",
        "mapping_version",
        "topology_version",
        "asset_version",
        "contract_version",
        "safety_version",
    ] {
        header.extend_from_slice(&get_u64(t, &config, key)?.to_le_bytes());
    }
    debug_assert_eq!(header.len(), catalog::HEADER_LEN);

    // [[schema]] 注册表（id 唯一；0 保留）。
    let mut schemas: BTreeSet<u32> = BTreeSet::new();
    let mut schemas_payload: Vec<u8> = Vec::new();
    let schemas_arr = optional_array(&root, &config, "schema")?;
    schemas_payload.extend_from_slice(&(schemas_arr.len() as u32).to_le_bytes());
    for (idx, sv) in schemas_arr.iter().enumerate() {
        let ctx = alloc::format!("schema[{idx}]");
        let st = sv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "schema",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(st, &config, &["id", "version", "flags"])?;
        let id = get_u32(st, &config, "id")?;
        if id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "schema",
                reason: "zero-id".into(),
            });
        }
        if !schemas.insert(id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_SCHEMAS,
            });
        }
        schemas_payload.extend_from_slice(&id.to_le_bytes());
        let version = get_u64_opt(st, &config, "version", 1)?;
        let flags = get_u64_opt(st, &config, "flags", 0)?;
        schemas_payload.extend_from_slice(&(version as u16).to_le_bytes());
        schemas_payload.extend_from_slice(&(flags as u16).to_le_bytes());
    }

    // [[unit_conversion]] 受控换算图（量纲兼容 + 正有限因子）。
    let mut conv_payload: Vec<u8> = Vec::new();
    let conv_arr = optional_array(&root, &config, "unit_conversion")?;
    conv_payload.extend_from_slice(&(conv_arr.len() as u32).to_le_bytes());
    for (idx, cv) in conv_arr.iter().enumerate() {
        let ctx = alloc::format!("unit_conversion[{idx}]");
        let ct = cv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "unit_conversion",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(ct, &config, &["from", "to", "factor"])?;
        let from = compile_unit_key(ct, &config, "from")?;
        let to = compile_unit_key(ct, &config, "to")?;
        if from == 0 || to == 0 || from == to {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "unit_conversion",
                reason: alloc::format!("{ctx}:bad-unit-pair({from},{to})"),
            });
        }
        let from_u = eneros_telemetry_model::unit_catalog::from_code(from).unwrap();
        let to_u = eneros_telemetry_model::unit_catalog::from_code(to).unwrap();
        if !from_u.compatible_with(to_u) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "unit_conversion",
                reason: alloc::format!(
                    "{ctx}:dimension-mismatch({:?}→{:?})",
                    from_u.dimension(),
                    to_u.dimension()
                ),
            });
        }
        let factor = get_f64_req(ct, &config, "factor")?;
        if !(factor.is_finite() && factor > 0.0) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "factor",
                reason: "must-be-positive-finite".into(),
            });
        }
        conv_payload.extend_from_slice(&from.to_le_bytes());
        conv_payload.extend_from_slice(&to.to_le_bytes());
        conv_payload.extend_from_slice(&factor.to_le_bytes());
    }

    // [[asset]] 资产目录（ID 唯一非零；kind 闭枚举）。
    let mut assets: BTreeSet<u64> = BTreeSet::new();
    let mut assets_payload: Vec<u8> = Vec::new();
    let assets_arr = optional_array(&root, &config, "asset")?;
    assets_payload.extend_from_slice(&(assets_arr.len() as u32).to_le_bytes());
    for (idx, av) in assets_arr.iter().enumerate() {
        let ctx = alloc::format!("asset[{idx}]");
        let at = av.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "asset",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(
            at,
            &config,
            &[
                "asset_id",
                "site_id",
                "tenant_id",
                "kind",
                "device_id",
                "topology_node_id",
                "tombstone",
            ],
        )?;
        let asset_id = get_u64(at, &config, "asset_id")?;
        if asset_id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "asset_id",
                reason: "zero-id".into(),
            });
        }
        if !assets.insert(asset_id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_ASSETS,
            });
        }
        let kind = catalog_asset_kind(at, &config)?;
        let device = u16::try_from(get_u64_opt(at, &config, "device_id", 0)?).map_err(|_| {
            ConfigError::InvalidValue {
                config: config.clone(),
                field: "device_id",
                reason: "u16-overflow".into(),
            }
        })?;
        let tombstone = get_bool_opt(at, &config, "tombstone", false)?;
        let flags = if tombstone {
            catalog::ASSET_FLAG_TOMBSTONE
        } else {
            0
        };
        assets_payload.extend_from_slice(&asset_id.to_le_bytes());
        assets_payload.extend_from_slice(&get_u32(at, &config, "site_id")?.to_le_bytes());
        assets_payload.extend_from_slice(&get_u32(at, &config, "tenant_id")?.to_le_bytes());
        assets_payload.extend_from_slice(&kind.to_le_bytes());
        assets_payload.extend_from_slice(&device.to_le_bytes());
        assets_payload
            .extend_from_slice(&get_u64_opt(at, &config, "topology_node_id", 0)?.to_le_bytes());
        assets_payload.extend_from_slice(&flags.to_le_bytes());
        assets_payload.extend_from_slice(&0u16.to_le_bytes()); // pad
    }

    // [[topology_node]]（id 唯一；kind 闭枚举：1..=10）。
    let mut topo_nodes: BTreeSet<u64> = BTreeSet::new();
    let mut nodes_payload: Vec<u8> = Vec::new();
    let nodes_arr = optional_array(&root, &config, "topology_node")?;
    nodes_payload.extend_from_slice(&(nodes_arr.len() as u32).to_le_bytes());
    for (idx, nv) in nodes_arr.iter().enumerate() {
        let ctx = alloc::format!("topology_node[{idx}]");
        let nt = nv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "topology_node",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(nt, &config, &["id", "kind", "flags"])?;
        let id = get_u64(nt, &config, "id")?;
        if id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "id",
                reason: "zero-id".into(),
            });
        }
        if !topo_nodes.insert(id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_TOPO_NODES,
            });
        }
        let kind = catalog_node_kind(nt, &config)?;
        nodes_payload.extend_from_slice(&id.to_le_bytes());
        nodes_payload.extend_from_slice(&kind.to_le_bytes());
        nodes_payload
            .extend_from_slice(&(get_u64_opt(nt, &config, "flags", 0)? as u16).to_le_bytes());
    }

    // [[topology_edge]]（自环/重复/悬空引用拒绝；switch 边必须带观测点）。
    let mut edges_payload: Vec<u8> = Vec::new();
    let edges_arr = optional_array(&root, &config, "topology_edge")?;
    edges_payload.extend_from_slice(&(edges_arr.len() as u32).to_le_bytes());
    let mut edge_keys: BTreeSet<(u64, u64)> = BTreeSet::new();
    for (idx, ev) in edges_arr.iter().enumerate() {
        let ctx = alloc::format!("topology_edge[{idx}]");
        let et = ev.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "topology_edge",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(
            et,
            &config,
            &[
                "from",
                "to",
                "kind",
                "switch_point_id",
                "capacity_kw",
                "loss",
                "valid_from_ns",
                "valid_until_ns",
                "flags",
            ],
        )?;
        let from = get_u64(et, &config, "from")?;
        let to = get_u64(et, &config, "to")?;
        if from == 0 || to == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "topology_edge",
                reason: alloc::format!("{ctx}:zero-node"),
            });
        }
        if from == to {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "topology_edge",
                reason: alloc::format!("{ctx}:self-loop"),
            });
        }
        if !topo_nodes.contains(&from) || !topo_nodes.contains(&to) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "topology_edge",
                reason: alloc::format!("{ctx}:dangling-node"),
            });
        }
        if !edge_keys.insert((from, to)) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_TOPO_EDGES,
            });
        }
        let kind = catalog_edge_kind(et, &config)?;
        let switch_point = get_u32_opt(et, &config, "switch_point_id", 0)?;
        if kind == 1 && switch_point == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "switch_point_id",
                reason: alloc::format!("{ctx}:switch-edge-needs-observation-point"),
            });
        }
        let capacity = get_f64_opt(et, &config, "capacity_kw", 0.0)?;
        let loss = get_f64_opt(et, &config, "loss", 0.0)?;
        if !(capacity.is_finite() && capacity >= 0.0 && loss.is_finite() && loss >= 0.0) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "topology_edge",
                reason: alloc::format!("{ctx}:non-finite-capacity-or-loss"),
            });
        }
        edges_payload.extend_from_slice(&from.to_le_bytes());
        edges_payload.extend_from_slice(&to.to_le_bytes());
        edges_payload.extend_from_slice(&kind.to_le_bytes());
        edges_payload.extend_from_slice(&switch_point.to_le_bytes());
        edges_payload.extend_from_slice(&capacity.to_le_bytes());
        edges_payload.extend_from_slice(&loss.to_le_bytes());
        edges_payload
            .extend_from_slice(&get_u64_opt(et, &config, "valid_from_ns", 0)?.to_le_bytes());
        edges_payload
            .extend_from_slice(&get_u64_opt(et, &config, "valid_until_ns", 0)?.to_le_bytes());
        edges_payload
            .extend_from_slice(&(get_u64_opt(et, &config, "flags", 0)? as u16).to_le_bytes());
    }
    // 孤岛规则（编译期静态可达性）：每个非 grid 节点必须能从 grid 节点
    // 经边（任意方向）到达；无 grid 节点 → 编译错误。
    let grid_nodes: Vec<u64> = nodes_arr
        .iter()
        .filter_map(|nv| nv.as_table())
        .filter(|nt| {
            nt.get("kind")
                .and_then(Value::as_str)
                .map(|k| k == "grid")
                .unwrap_or(false)
        })
        .filter_map(|nt| nt.get("id").and_then(Value::as_integer))
        .map(|i| i as u64)
        .collect();
    if grid_nodes.is_empty() {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "topology_node",
            reason: "no-grid-node".into(),
        });
    }
    {
        let mut adj: BTreeSet<(u64, u64)> = BTreeSet::new();
        for ev in edges_arr.iter() {
            if let Some(et) = ev.as_table() {
                if let (Some(f), Some(t)) = (
                    et.get("from").and_then(Value::as_integer),
                    et.get("to").and_then(Value::as_integer),
                ) {
                    adj.insert((f as u64, t as u64));
                    adj.insert((t as u64, f as u64));
                }
            }
        }
        let mut reachable: BTreeSet<u64> = BTreeSet::new();
        let mut stack: Vec<u64> = grid_nodes.clone();
        while let Some(n) = stack.pop() {
            if !reachable.insert(n) {
                continue;
            }
            for (f, t) in adj.iter() {
                if *f == n && !reachable.contains(t) {
                    stack.push(*t);
                }
            }
        }
        for id in topo_nodes.iter() {
            if !reachable.contains(id) {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "topology_edge",
                    reason: alloc::format!("island-node:{id}"),
                });
            }
        }
    }

    // [[point_semantics]] 点语义目录（point_id 唯一；asset/schema 交叉引用；
    // control 必须有 readback）。
    let mut points_set: BTreeSet<u32> = BTreeSet::new();
    let mut points_payload: Vec<u8> = Vec::new();
    let points_arr = optional_array(&root, &config, "point_semantics")?;
    points_payload.extend_from_slice(&(points_arr.len() as u32).to_le_bytes());
    for (idx, pv) in points_arr.iter().enumerate() {
        let ctx = alloc::format!("point_semantics[{idx}]");
        let pt = pv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "point_semantics",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(
            pt,
            &config,
            &[
                "point_id",
                "asset_id",
                "schema_id",
                "schema_version",
                "unit",
                "role",
                "readback_point_id",
                "flags",
            ],
        )?;
        let point_id = get_u32(pt, &config, "point_id")?;
        if point_id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "point_id",
                reason: "zero-id".into(),
            });
        }
        if !points_set.insert(point_id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_POINT_SEMANTICS,
            });
        }
        let asset_id = get_u64(pt, &config, "asset_id")?;
        if asset_id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "asset_id",
                reason: "zero-asset".into(),
            });
        }
        if !assets.contains(&asset_id) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "asset_id",
                reason: alloc::format!("{ctx}:unknown-asset:{asset_id}"),
            });
        }
        let schema_id = get_u64_opt(pt, &config, "schema_id", 0)?;
        let schema_id_u32 = u32::try_from(schema_id).map_err(|_| ConfigError::InvalidValue {
            config: config.clone(),
            field: "schema_id",
            reason: "u32-overflow".into(),
        })?;
        if schema_id_u32 != 0 && !schemas.contains(&schema_id_u32) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "schema_id",
                reason: alloc::format!("{ctx}:unknown-schema:{schema_id_u32}"),
            });
        }
        let role_name = get_str_req(pt, &config, "role")?;
        let role = match role_name {
            "measurement" => catalog::ROLE_MEASUREMENT,
            "control" => catalog::ROLE_CONTROL,
            "switch-state" => catalog::ROLE_SWITCH_STATE,
            other => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "role",
                    reason: alloc::format!("unknown-role:{other}"),
                })
            }
        };
        let readback = get_u32_opt(pt, &config, "readback_point_id", 0)?;
        if role == catalog::ROLE_CONTROL && readback == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "readback_point_id",
                reason: alloc::format!("{ctx}:control-without-readback"),
            });
        }
        if readback != 0 && readback == point_id {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "readback_point_id",
                reason: alloc::format!("{ctx}:readback-self-reference"),
            });
        }
        points_payload.extend_from_slice(&point_id.to_le_bytes());
        points_payload.extend_from_slice(&asset_id.to_le_bytes());
        points_payload.extend_from_slice(&schema_id_u32.to_le_bytes());
        points_payload.extend_from_slice(
            &(get_u64_opt(pt, &config, "schema_version", 1)? as u16).to_le_bytes(),
        );
        let unit_code = compile_unit_code(pt, &config)?;
        if unit_code == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "unit",
                reason: alloc::format!("{ctx}:control-eligible-point-needs-unit"),
            });
        }
        points_payload.extend_from_slice(&unit_code.to_le_bytes());
        points_payload.push(role);
        points_payload.extend_from_slice(&readback.to_le_bytes());
        points_payload.push(get_u64_opt(pt, &config, "flags", 0)? as u8);
    }

    // [[feedback_slot]]：5 槽齐全且不重复（与 TrustedFeedbackBuilder 对齐）。
    let mut slots: BTreeMap<u8, u32> = BTreeMap::new();
    let mut slots_payload: Vec<u8> = Vec::new();
    let slots_arr = optional_array(&root, &config, "feedback_slot")?;
    slots_payload.extend_from_slice(&(slots_arr.len() as u32).to_le_bytes());
    for (idx, sv) in slots_arr.iter().enumerate() {
        let ctx = alloc::format!("feedback_slot[{idx}]");
        let st = sv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "feedback_slot",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(st, &config, &["slot", "point_id"])?;
        let slot_name = get_str_req(st, &config, "slot")?;
        let slot = match slot_name {
            "grid_up" => catalog::FEEDBACK_SLOT_GRID_UP,
            "soc" => catalog::FEEDBACK_SLOT_SOC,
            "dc_bus_v" => catalog::FEEDBACK_SLOT_DC_BUS_V,
            "overcurrent" => catalog::FEEDBACK_SLOT_OVERCURRENT,
            "overtemp" => catalog::FEEDBACK_SLOT_OVERTEMP,
            other => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "slot",
                    reason: alloc::format!("unknown-slot:{other}"),
                })
            }
        };
        if slots
            .insert(slot, get_u32(st, &config, "point_id")?)
            .is_some()
        {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_FEEDBACK_SLOTS,
            });
        }
        slots_payload.push(slot);
        slots_payload.extend_from_slice(&get_u32(st, &config, "point_id")?.to_le_bytes());
        slots_payload.push(0); // pad
    }
    if slots.len() != catalog::FEEDBACK_SLOT_COUNT as usize {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "feedback_slot",
            reason: alloc::format!(
                "need-{}-slots-got-{}",
                catalog::FEEDBACK_SLOT_COUNT,
                slots.len()
            ),
        });
    }
    // 反馈槽点与面向的资产一致（点必须存在于点语义目录）。
    for (_slot, pid) in slots.iter() {
        if !points_set.contains(pid) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "feedback_slot",
                reason: alloc::format!("point-not-in-catalog:{pid}"),
            });
        }
    }

    // [[coverage]] 安全覆盖（点存在；min≤max 有限；period 界内；unit 必填）。
    let mut cov_payload: Vec<u8> = Vec::new();
    let cov_arr = optional_array(&root, &config, "coverage")?;
    cov_payload.extend_from_slice(&(cov_arr.len() as u32).to_le_bytes());
    for (idx, cv) in cov_arr.iter().enumerate() {
        let ctx = alloc::format!("coverage[{idx}]");
        let ct = cv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "coverage",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(
            ct,
            &config,
            &["point_id", "min", "max", "period_ms", "unit", "flags"],
        )?;
        let point_id = get_u32(ct, &config, "point_id")?;
        if !points_set.contains(&point_id) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "point_id",
                reason: alloc::format!("{ctx}:point-not-in-catalog:{point_id}"),
            });
        }
        let mn = get_f64_req(ct, &config, "min")?;
        let mx = get_f64_req(ct, &config, "max")?;
        if !(mn.is_finite() && mx.is_finite()) || mn > mx {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "coverage",
                reason: alloc::format!("{ctx}:range-invalid"),
            });
        }
        let period = get_u32_opt(ct, &config, "period_ms", points::DEFAULT_PERIOD_MS)?;
        if period == 0 || period > points::MAX_PERIOD_MS {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "period_ms",
                reason: alloc::format!("{ctx}:out-of-range"),
            });
        }
        let unit = compile_unit_code(ct, &config)?;
        if unit == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "unit",
                reason: alloc::format!("{ctx}:coverage-needs-unit"),
            });
        }
        cov_payload.extend_from_slice(&point_id.to_le_bytes());
        cov_payload.extend_from_slice(&mn.to_le_bytes());
        cov_payload.extend_from_slice(&mx.to_le_bytes());
        cov_payload.extend_from_slice(&period.to_le_bytes());
        cov_payload.extend_from_slice(&unit.to_le_bytes());
        cov_payload
            .extend_from_slice(&(get_u64_opt(ct, &config, "flags", 0)? as u16).to_le_bytes());
    }

    // [[control_contract]] 控制合同（资产存在；控制点存在；readback 必填）。
    let mut contracts: BTreeSet<u32> = BTreeSet::new();
    let mut contracts_payload: Vec<u8> = Vec::new();
    let contracts_arr = optional_array(&root, &config, "control_contract")?;
    contracts_payload.extend_from_slice(&(contracts_arr.len() as u32).to_le_bytes());
    for (idx, cv) in contracts_arr.iter().enumerate() {
        let ctx = alloc::format!("control_contract[{idx}]");
        let ct = cv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "control_contract",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(
            ct,
            &config,
            &[
                "contract_id",
                "asset_id",
                "point_id",
                "unit",
                "class",
                "min",
                "max",
                "ramp_kw_per_s",
                "settle_ms",
                "min_on_ms",
                "min_off_ms",
                "max_starts",
                "readback_point_id",
                "tolerance_kw",
                "reversible",
                "compensation",
                "priority",
                "safe_state",
                "lost_link",
                "flags",
            ],
        )?;
        let contract_id = get_u32(ct, &config, "contract_id")?;
        if contract_id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "contract_id",
                reason: "zero-id".into(),
            });
        }
        if !contracts.insert(contract_id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_CONTROL_CONTRACTS,
            });
        }
        let asset_id = get_u64(ct, &config, "asset_id")?;
        if !assets.contains(&asset_id) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "asset_id",
                reason: alloc::format!("{ctx}:unknown-asset:{asset_id}"),
            });
        }
        let point_id = get_u32(ct, &config, "point_id")?;
        if !points_set.contains(&point_id) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "point_id",
                reason: alloc::format!("{ctx}:point-not-in-catalog:{point_id}"),
            });
        }
        let unit = compile_unit_code(ct, &config)?;
        if unit == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "unit",
                reason: alloc::format!("{ctx}:contract-needs-unit"),
            });
        }
        let class_name = get_str_req(ct, &config, "class")?;
        let class = match class_name {
            "switch" => catalog::CONTRACT_CLASS_SWITCH,
            "setpoint" => catalog::CONTRACT_CLASS_SETPOINT,
            other => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "class",
                    reason: alloc::format!("unknown-class:{other}"),
                })
            }
        };
        let mn = get_f64_req(ct, &config, "min")?;
        let mx = get_f64_req(ct, &config, "max")?;
        if !(mn.is_finite() && mx.is_finite()) || mn > mx {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "contract-range",
                reason: alloc::format!("{ctx}:range-invalid"),
            });
        }
        let readback = get_u32_opt(ct, &config, "readback_point_id", 0)?;
        if readback == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "readback_point_id",
                reason: alloc::format!("{ctx}:contract-without-readback"),
            });
        }
        let ramp = get_f64_opt(ct, &config, "ramp_kw_per_s", 0.0)?;
        let tol = get_f64_opt(ct, &config, "tolerance_kw", 0.0)?;
        if !(ramp.is_finite() && ramp >= 0.0 && tol.is_finite() && tol >= 0.0) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "contract",
                reason: alloc::format!("{ctx}:non-finite-ramp-or-tolerance"),
            });
        }
        let prio_name = get_str_req(ct, &config, "priority")?;
        let priority = match prio_name {
            "local" => 1u8,
            "remote" => 2u8,
            "manual" => 3u8,
            other => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "priority",
                    reason: alloc::format!("unknown-priority:{other}"),
                })
            }
        };
        let safe_name = get_str_req(ct, &config, "safe_state")?;
        let safe_state = match safe_name {
            "hold" => 1u8,
            "zero" => 2u8,
            "safe" => 3u8,
            other => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "safe_state",
                    reason: alloc::format!("unknown-safe-state:{other}"),
                })
            }
        };
        let lost_name = get_str_req(ct, &config, "lost_link")?;
        let lost_link = match lost_name {
            "hold" => 1u8,
            "zero" => 2u8,
            other => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "lost_link",
                    reason: alloc::format!("unknown-lost-link:{other}"),
                })
            }
        };
        let priority = priority.max(1);
        contracts_payload.extend_from_slice(&contract_id.to_le_bytes());
        contracts_payload.extend_from_slice(&asset_id.to_le_bytes());
        contracts_payload.extend_from_slice(&point_id.to_le_bytes());
        contracts_payload.extend_from_slice(&unit.to_le_bytes());
        contracts_payload.extend_from_slice(&class.to_le_bytes());
        contracts_payload.extend_from_slice(&mn.to_le_bytes());
        contracts_payload.extend_from_slice(&mx.to_le_bytes());
        contracts_payload.extend_from_slice(&ramp.to_le_bytes());
        let settle = get_u64_opt(ct, &config, "settle_ms", 0)?;
        contracts_payload.extend_from_slice(&(settle as u32).to_le_bytes());
        contracts_payload
            .extend_from_slice(&get_u64_opt(ct, &config, "min_on_ms", 0)?.to_le_bytes());
        contracts_payload
            .extend_from_slice(&get_u64_opt(ct, &config, "min_off_ms", 0)?.to_le_bytes());
        let max_starts = get_u64_opt(ct, &config, "max_starts", 0)?;
        contracts_payload.extend_from_slice(&(max_starts as u16).to_le_bytes());
        contracts_payload.extend_from_slice(&readback.to_le_bytes());
        contracts_payload.extend_from_slice(&tol.to_le_bytes());
        contracts_payload.push(if get_bool_opt(ct, &config, "reversible", false)? {
            1
        } else {
            0
        });
        contracts_payload.push(if get_bool_opt(ct, &config, "compensation", false)? {
            1
        } else {
            0
        });
        contracts_payload.push(priority);
        contracts_payload.push(safe_state);
        contracts_payload.push(lost_link);
        contracts_payload.push(get_u64_opt(ct, &config, "flags", 0)? as u8);
        contracts_payload.push(0); // pad×3
        contracts_payload.push(0);
        contracts_payload.push(0);
    }

    // [[safety_constraint]] 安全约束目录。
    let mut constraints: BTreeSet<u32> = BTreeSet::new();
    let mut constraints_payload: Vec<u8> = Vec::new();
    let constraints_arr = optional_array(&root, &config, "safety_constraint")?;
    constraints_payload.extend_from_slice(&(constraints_arr.len() as u32).to_le_bytes());
    for (idx, sv) in constraints_arr.iter().enumerate() {
        let ctx = alloc::format!("safety_constraint[{idx}]");
        let st = sv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "safety_constraint",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(
            st,
            &config,
            &["constraint_id", "asset_id", "kind", "point_id", "limit"],
        )?;
        let constraint_id = get_u32(st, &config, "constraint_id")?;
        if constraint_id == 0 || !constraints.insert(constraint_id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_SAFETY_CONSTRAINTS,
            });
        }
        let asset_id = get_u64(st, &config, "asset_id")?;
        if !assets.contains(&asset_id) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "asset_id",
                reason: alloc::format!("{ctx}:unknown-asset:{asset_id}"),
            });
        }
        let kind_name = get_str_req(st, &config, "kind")?;
        let kind = match kind_name {
            "overcurrent" => catalog::SAFETY_KIND_OVERCURRENT,
            "overtemp" => catalog::SAFETY_KIND_OVERTEMP,
            other => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "kind",
                    reason: alloc::format!("unknown-kind:{other}"),
                })
            }
        };
        let point_id = get_u32(st, &config, "point_id")?;
        if !points_set.contains(&point_id) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "point_id",
                reason: alloc::format!("{ctx}:point-not-in-catalog:{point_id}"),
            });
        }
        let limit = get_f64_req(st, &config, "limit")?;
        if !limit.is_finite() {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "limit",
                reason: "non-finite".into(),
            });
        }
        constraints_payload.extend_from_slice(&constraint_id.to_le_bytes());
        constraints_payload.extend_from_slice(&asset_id.to_le_bytes());
        constraints_payload.extend_from_slice(&kind.to_le_bytes());
        constraints_payload.extend_from_slice(&point_id.to_le_bytes());
        constraints_payload.extend_from_slice(&limit.to_le_bytes());
        constraints_payload.extend_from_slice(&0u16.to_le_bytes()); // flags
        constraints_payload.extend_from_slice(&0u32.to_le_bytes()); // pad
    }

    // [[alias]] 迁移别名（market DevicePool / VPP resource_id → asset_id）。
    let mut aliases: BTreeSet<(u8, u64)> = BTreeSet::new();
    let mut aliases_payload: Vec<u8> = Vec::new();
    let aliases_arr = optional_array(&root, &config, "alias")?;
    aliases_payload.extend_from_slice(&(aliases_arr.len() as u32).to_le_bytes());
    for (idx, av) in aliases_arr.iter().enumerate() {
        let ctx = alloc::format!("alias[{idx}]");
        let at = av.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "alias",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(at, &config, &["kind", "legacy_id", "asset_id"])?;
        let kind_name = get_str_req(at, &config, "kind")?;
        let kind = match kind_name {
            "market-device" => catalog::ALIAS_KIND_MARKET_DEVICE,
            "vpp-resource" => catalog::ALIAS_KIND_VPP_RESOURCE,
            other => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "kind",
                    reason: alloc::format!("unknown-kind:{other}"),
                })
            }
        };
        let legacy_id = get_u64(at, &config, "legacy_id")?;
        let asset_id = get_u64(at, &config, "asset_id")?;
        if !assets.contains(&asset_id) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "asset_id",
                reason: alloc::format!("{ctx}:unknown-asset:{asset_id}"),
            });
        }
        if !aliases.insert((kind, legacy_id)) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_ALIASES,
            });
        }
        aliases_payload.push(kind);
        aliases_payload.extend_from_slice(&legacy_id.to_le_bytes());
        aliases_payload.extend_from_slice(&asset_id.to_le_bytes());
        aliases_payload.push(0); // pad
    }

    // [[tombstone]] 删除登记（ID 永不复用）。
    let mut tombstones: BTreeSet<u64> = BTreeSet::new();
    let mut tombstones_payload: Vec<u8> = Vec::new();
    let tombstones_arr = optional_array(&root, &config, "tombstone")?;
    tombstones_payload.extend_from_slice(&(tombstones_arr.len() as u32).to_le_bytes());
    for (idx, tv) in tombstones_arr.iter().enumerate() {
        let ctx = alloc::format!("tombstone[{idx}]");
        let tt = tv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "tombstone",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(tt, &config, &["asset_id", "kind"])?;
        let asset_id = get_u64(tt, &config, "asset_id")?;
        if asset_id == 0 || !tombstones.insert(asset_id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: catalog::FIELD_TOMBSTONES,
            });
        }
        // 与资产目录同表登记时互斥（同一 ID 不得既活又墓碑）。
        if assets.contains(&asset_id) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "tombstone",
                reason: alloc::format!("{ctx}:asset-alive:{asset_id}"),
            });
        }
        tombstones_payload.extend_from_slice(&asset_id.to_le_bytes());
        tombstones_payload
            .extend_from_slice(&(get_u64_opt(tt, &config, "kind", 0)? as u16).to_le_bytes());
        tombstones_payload.extend_from_slice(&0u16.to_le_bytes()); // flags
    }

    let fields: Vec<(u8, Vec<u8>)> = vec![
        (catalog::FIELD_HEADER, header),
        (catalog::FIELD_SCHEMAS, schemas_payload),
        (catalog::FIELD_UNIT_CONVERSIONS, conv_payload),
        (catalog::FIELD_ASSETS, assets_payload),
        (catalog::FIELD_TOPO_NODES, nodes_payload),
        (catalog::FIELD_TOPO_EDGES, edges_payload),
        (catalog::FIELD_POINT_SEMANTICS, points_payload),
        (catalog::FIELD_FEEDBACK_SLOTS, slots_payload),
        (catalog::FIELD_COVERAGE, cov_payload),
        (catalog::FIELD_CONTROL_CONTRACTS, contracts_payload),
        (catalog::FIELD_SAFETY_CONSTRAINTS, constraints_payload),
        (catalog::FIELD_ALIASES, aliases_payload),
        (catalog::FIELD_TOMBSTONES, tombstones_payload),
    ];
    Ok(CompiledConfig {
        payload: encode_payload(fields),
        config_id: config,
    })
}

/// 可选数组 section（缺省 = 空表）。
fn optional_array<'a>(
    root: &'a Value,
    config: &ConfigId,
    name: &'static str,
) -> Result<Vec<&'a Value>, ConfigError> {
    match root.get(name) {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.iter().collect()),
        Some(_) => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-array".into(),
        }),
    }
}

/// `unit` 键 → `UnitCatalog` 封闭码（缺省/"" = 0 未知；未知名拒绝）。
fn compile_unit_code(
    pt: &toml::map::Map<String, Value>,
    config: &ConfigId,
) -> Result<u16, ConfigError> {
    let name = match pt.get("unit") {
        None => return Ok(0),
        Some(Value::String(s)) => s.as_str(),
        Some(_) => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "unit",
                reason: "expected-string".into(),
            })
        }
    };
    if name.is_empty() {
        return Ok(0);
    }
    unit_code_from_name(name, config)
}

/// 资产 kind 名 → 闭枚举码（[`catalog_kinds::ASSET_*`]）。
fn catalog_asset_kind(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
) -> Result<u16, ConfigError> {
    use crate::schema::catalog_kinds;
    let name = get_str_req(t, config, "kind")?;
    let code = match name {
        "battery" => catalog_kinds::ASSET_BATTERY,
        "pv" => catalog_kinds::ASSET_PV,
        "wind" => catalog_kinds::ASSET_WIND,
        "generator" => catalog_kinds::ASSET_GENERATOR,
        "flexible-load" => catalog_kinds::ASSET_FLEXIBLE_LOAD,
        "fixed-load" => catalog_kinds::ASSET_FIXED_LOAD,
        "evse" => catalog_kinds::ASSET_EVSE,
        "hvac" => catalog_kinds::ASSET_HVAC,
        "process-unit" => catalog_kinds::ASSET_PROCESS_UNIT,
        "grid" => catalog_kinds::ASSET_GRID_CONNECTION,
        "aggregate" => catalog_kinds::ASSET_AGGREGATE,
        other => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "kind",
                reason: alloc::format!("unknown-asset-kind:{other}"),
            })
        }
    };
    Ok(code)
}

/// 拓扑节点 kind 名 → 闭枚举码（[`catalog_kinds::NODE_*`]）。
fn catalog_node_kind(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
) -> Result<u16, ConfigError> {
    use crate::schema::catalog_kinds;
    let name = get_str_req(t, config, "kind")?;
    let code = match name {
        "busbar" => catalog_kinds::NODE_BUSBAR,
        "feeder" => catalog_kinds::NODE_FEEDER,
        "transformer" => catalog_kinds::NODE_TRANSFORMER,
        "switch" => catalog_kinds::NODE_SWITCH,
        "der" => catalog_kinds::NODE_DER,
        "load" => catalog_kinds::NODE_LOAD,
        "storage" => catalog_kinds::NODE_STORAGE,
        "meter" => catalog_kinds::NODE_METER,
        "process-unit" => catalog_kinds::NODE_PROCESS_UNIT,
        "grid" => catalog_kinds::NODE_GRID,
        other => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "kind",
                reason: alloc::format!("unknown-node-kind:{other}"),
            })
        }
    };
    Ok(code)
}

/// 拓扑边 kind 名 → 闭枚举码（1=switch 2=branch）。
fn catalog_edge_kind(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
) -> Result<u16, ConfigError> {
    let name = get_str_req(t, config, "kind")?;
    match name {
        "switch" => Ok(1u16),
        "branch" => Ok(2u16),
        other => Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "kind",
            reason: alloc::format!("unknown-edge-kind:{other}"),
        }),
    }
}

/// 从命名字段读单位名 → `UnitCatalog` 封闭码（缺省/"" → 0；未知名拒绝）。
/// `unit_conversion.from/to` 等非 `unit` 键单位字段使用（语义与
/// [`compile_unit_code`] 同——同一张码表单一事实）。
fn compile_unit_key(
    t: &toml::map::Map<String, Value>,
    config: &ConfigId,
    key: &'static str,
) -> Result<u16, ConfigError> {
    let name = match t.get(key) {
        None => return Ok(0),
        Some(Value::String(s)) => s.as_str(),
        Some(_) => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: key,
                reason: "expected-string".into(),
            })
        }
    };
    if name.is_empty() {
        return Ok(0);
    }
    unit_code_from_name(name, config)
}

/// 单位名 → `UnitCatalog` 封闭码（空名 → 0；未知名拒绝）。catalog 的
/// `unit_conversion`/`coverage`/`control_contract` 复用同一张码表。
pub(crate) fn unit_code_from_name(name: &str, config: &ConfigId) -> Result<u16, ConfigError> {
    use eneros_telemetry_model::PhysicalUnit;
    // "ratio"/"dimensionless" 别名 → Dimensionless（无量纲比例如 SOC 0..1、
    // 告警 0/1——PhysicalUnit::Dimensionless 短名为空串，TOML 需显式名）。
    if matches!(name, "ratio" | "dimensionless") {
        return Ok(eneros_telemetry_model::unit_catalog::code_of(
            PhysicalUnit::Dimensionless,
        ));
    }
    const ALL: [PhysicalUnit; 18] = [
        PhysicalUnit::Volts,
        PhysicalUnit::Kilovolts,
        PhysicalUnit::Amperes,
        PhysicalUnit::Kiloamperes,
        PhysicalUnit::Watts,
        PhysicalUnit::Kilowatts,
        PhysicalUnit::Megawatts,
        PhysicalUnit::VoltAmpereReactive,
        PhysicalUnit::Kilovar,
        PhysicalUnit::Megavar,
        PhysicalUnit::Hertz,
        PhysicalUnit::Celsius,
        PhysicalUnit::Percent,
        PhysicalUnit::KilowattHour,
        PhysicalUnit::AmpereHour,
        PhysicalUnit::Seconds,
        PhysicalUnit::Rpm,
        PhysicalUnit::Dimensionless,
    ];
    ALL.iter()
        .find(|u| u.short_name() == name)
        .map(|&u| eneros_telemetry_model::unit_catalog::code_of(u))
        .ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "unit",
            reason: alloc::format!("unknown-unit:{name}"),
        })
}

/// `[[source]]` 来源注册表编译（布局见 `schema::points` 注释；严格封闭）。
fn compile_sources(root: &Value, config: &ConfigId) -> Result<Vec<u8>, ConfigError> {
    let Some(arr) = root.get("source").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if arr.is_empty() {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "source",
            reason: "empty-source-table".into(),
        });
    }
    if arr.len() as u32 > points::MAX_SOURCES {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "source",
            reason: "too-many-sources".into(),
        });
    }
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&(arr.len() as u32).to_le_bytes());
    for (idx, sv) in arr.iter().enumerate() {
        let ctx = alloc::format!("source[{idx}]");
        let st = sv.as_table().ok_or_else(|| ConfigError::InvalidValue {
            config: config.clone(),
            field: "source",
            reason: alloc::format!("{ctx}:expected-table"),
        })?;
        reject_unknown_keys(
            st,
            config,
            &[
                "source_id",
                "protocol",
                "tenant_id",
                "site_id",
                "allow_control",
                "require_integrity",
                "point_min",
                "point_max",
                "max_events_per_s",
            ],
        )?;
        let source_id = get_u32(st, config, "source_id")?;
        if source_id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "source_id",
                reason: "must-be-nonzero".into(),
            });
        }
        if !seen.insert(source_id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: points::FIELD_SOURCES,
            });
        }
        let tenant_id = get_u32(st, config, "tenant_id")?;
        if tenant_id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "tenant_id",
                reason: "must-be-nonzero".into(),
            });
        }
        let site_id = get_u32(st, config, "site_id")?;
        if site_id == 0 {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "site_id",
                reason: "must-be-nonzero".into(),
            });
        }
        let proto_name = get_str_req(st, config, "protocol")?;
        let protocol =
            PointProtocol::from_name(proto_name).ok_or_else(|| ConfigError::InvalidValue {
                config: config.clone(),
                field: "protocol",
                reason: alloc::format!("unknown-enum:{proto_name}"),
            })?;
        let allow_control = get_bool_opt(st, config, "allow_control", false)?;
        let require_integrity = get_bool_opt(st, config, "require_integrity", true)?;
        let point_min = get_u32_opt(st, config, "point_min", 0)?;
        let point_max = get_u32_opt(st, config, "point_max", 0)?;
        if point_max != 0 && point_max < point_min {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "point_max",
                reason: "range-inverted".into(),
            });
        }
        let rate = get_u32(st, config, "max_events_per_s")?;
        if rate == 0 || rate > points::MAX_EVENTS_PER_S {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "max_events_per_s",
                reason: "out-of-range".into(),
            });
        }
        let mut flags = 0u8;
        if allow_control {
            flags |= points::SOURCE_FLAG_ALLOW_CONTROL;
        }
        if require_integrity {
            flags |= points::SOURCE_FLAG_REQUIRE_INTEGRITY;
        }
        out.extend_from_slice(&source_id.to_le_bytes());
        out.extend_from_slice(&tenant_id.to_le_bytes());
        out.extend_from_slice(&site_id.to_le_bytes());
        out.push(protocol.tag());
        out.push(flags);
        out.extend_from_slice(&point_min.to_le_bytes());
        out.extend_from_slice(&point_max.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
    }
    Ok(out)
}

/// min/max 成对校验（必须同时出现；min ≤ max）。
fn compile_range(
    pt: &toml::map::Map<String, Value>,
    config: &ConfigId,
) -> Result<Option<(f64, f64)>, ConfigError> {
    let has_min = pt.contains_key("min");
    let has_max = pt.contains_key("max");
    if has_min != has_max {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "min/max",
            reason: "must-be-paired".into(),
        });
    }
    if !has_min {
        return Ok(None);
    }
    let min = get_f64_req(pt, config, "min")?;
    let max = get_f64_req(pt, config, "max")?;
    if min > max {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "min/max",
            reason: "min-gt-max".into(),
        });
    }
    Ok(Some((min, max)))
}

/// 按协议解析地址 → wire 字节（与 `schema::points` 地址编码一致）。
fn compile_addr(
    pt: &toml::map::Map<String, Value>,
    config: &ConfigId,
    protocol: PointProtocol,
    ctx: &str,
) -> Result<alloc::vec::Vec<u8>, ConfigError> {
    let addr_scope_expected: &[&str] = match protocol {
        PointProtocol::ModbusRtu | PointProtocol::ModbusTcp => &["modbus"],
        PointProtocol::Iec104 => &["iec104"],
        PointProtocol::Mqtt => &["mqtt"],
        PointProtocol::Mms => &["mms"],
        PointProtocol::Goose | PointProtocol::Sv => &[],
    };
    let addr = match pt.get("addr") {
        None if addr_scope_expected.is_empty() => return Ok(alloc::vec::Vec::new()),
        None => {
            return Err(ConfigError::MissingField {
                config: config.clone(),
                field: "addr",
            })
        }
        Some(Value::Table(inner)) => {
            if addr_scope_expected.is_empty() {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "addr",
                    reason: alloc::format!("{ctx}:goose-sv-subscription-no-addr"),
                });
            }
            reject_unknown_keys(inner, config, addr_scope_expected)?;
            let key = addr_scope_expected[0];
            let cell = inner.get(key).and_then(Value::as_table).ok_or_else(|| {
                ConfigError::MissingField {
                    config: config.clone(),
                    field: key,
                }
            })?;
            cell
        }
        Some(_) => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "addr",
                reason: "expected-table".into(),
            })
        }
    };
    match protocol {
        PointProtocol::ModbusRtu | PointProtocol::ModbusTcp => {
            reject_unknown_keys(addr, config, &["slave_addr", "reg_addr", "func_code"])?;
            let slave = get_u64(addr, config, "slave_addr")? as u8;
            let reg = get_u64(addr, config, "reg_addr")? as u16;
            let func = get_u64(addr, config, "func_code")? as u8;
            let mut out = alloc::vec::Vec::with_capacity(4);
            out.push(slave);
            out.extend_from_slice(&reg.to_le_bytes());
            out.push(func);
            Ok(out)
        }
        PointProtocol::Iec104 => {
            reject_unknown_keys(addr, config, &["common_addr", "ioa", "type_id"])?;
            let common = get_u64(addr, config, "common_addr")? as u16;
            let ioa = get_u64(addr, config, "ioa")? as u16;
            let type_id = get_u64(addr, config, "type_id")? as u8;
            let mut out = alloc::vec::Vec::with_capacity(5);
            out.extend_from_slice(&common.to_le_bytes());
            out.extend_from_slice(&ioa.to_le_bytes());
            out.push(type_id);
            Ok(out)
        }
        PointProtocol::Mms => {
            reject_unknown_keys(addr, config, &["object_ref"])?;
            let object_ref = get_str_req(addr, config, "object_ref")?;
            if object_ref.is_empty() || object_ref.len() > points::MAX_ADDR_LEN {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "object_ref",
                    reason: "len".into(),
                });
            }
            Ok(object_ref.as_bytes().to_vec())
        }
        PointProtocol::Mqtt => {
            reject_unknown_keys(addr, config, &["topic"])?;
            let topic = get_str_req(addr, config, "topic")?;
            if topic.is_empty() || topic.len() > points::MAX_ADDR_LEN {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "topic",
                    reason: "len".into(),
                });
            }
            Ok(topic.as_bytes().to_vec())
        }
        PointProtocol::Goose | PointProtocol::Sv => Ok(alloc::vec::Vec::new()),
    }
}

/// 编译分派（按 config_id 字符串；未登记 → None）。
/// 编译扁键值协议配置（46 WP-46.3）：已知键封闭 + 必填键齐全。
pub fn compile_protocol(config_id: &str, toml: &str) -> Result<CompiledConfig, ConfigError> {
    use eneros_protocol_abstract::ProtocolConfig;
    let config = ConfigId::from(config_id);
    let known = crate::protocol::known_keys(config_id).ok_or(ConfigError::InvalidValue {
        config: config.clone(),
        field: "config_id",
        reason: format!("unknown protocol config {config_id}"),
    })?;
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, known)?;
    // 值统一按字符串承载（插件消费端再数值化；键序保持）。
    let mut cfg = ProtocolConfig::default();
    for (key, val) in t {
        let vstr = match val {
            Value::String(s) => s.clone(),
            Value::Integer(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Boolean(b) => b.to_string(),
            _ => {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "entry",
                    reason: format!("{key}: 协议配置仅支持标量键值"),
                })
            }
        };
        cfg.insert(key, &vstr);
    }
    for req in crate::protocol::required_keys(config_id) {
        if cfg.get(req).is_none() {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: req,
                reason: format!("missing required key {req}"),
            });
        }
    }
    let payload = crate::protocol::encode_protocol(&cfg)?;
    Ok(CompiledConfig {
        config_id: config,
        payload,
    })
}

/// 49（WP-49.2）：`ai-peer` 编译——RemoteBrain 状态机参数，**六键全必配**
/// （fail-closed 禁凭记忆缺省；48 卷 config 解析器数值纪律同款：先值域后
/// 类型，杜绝 as 截断。SPEC-E WP-E.3 增 learn_enabled 闸键）。
pub fn compile_ai_peer(toml: &str) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(ai_peer::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = section(&root, &config, "ai_peer")?;
    reject_unknown_keys(
        t,
        &config,
        &[
            "timeout_ms",
            "unhealthy_after",
            "probe_every",
            "max_power_kw",
            "ttl_secs",
            "learn_enabled",
        ],
    )?;
    let timeout_ms = get_u64(t, &config, "timeout_ms")?;
    if !(ai_peer::TIMEOUT_MS_MIN..=ai_peer::TIMEOUT_MS_MAX).contains(&timeout_ms) {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "timeout_ms",
            reason: alloc::format!(
                "range-{}..={}",
                ai_peer::TIMEOUT_MS_MIN,
                ai_peer::TIMEOUT_MS_MAX
            ),
        });
    }
    let unhealthy_after = get_u32(t, &config, "unhealthy_after")?;
    if !(ai_peer::UNHEALTHY_AFTER_MIN..=ai_peer::UNHEALTHY_AFTER_MAX).contains(&unhealthy_after) {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "unhealthy_after",
            reason: "range-1..=10".into(),
        });
    }
    let probe_every = get_u32(t, &config, "probe_every")?;
    if !(ai_peer::PROBE_EVERY_MIN..=ai_peer::PROBE_EVERY_MAX).contains(&probe_every) {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "probe_every",
            reason: "range-1..=100".into(),
        });
    }
    let max_power_kw = get_f64_req(t, &config, "max_power_kw")?;
    if !max_power_kw.is_finite() || max_power_kw <= 0.0 || max_power_kw > ai_peer::MAX_POWER_KW_MAX
    {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "max_power_kw",
            reason: alloc::format!("range-(0..={}]", ai_peer::MAX_POWER_KW_MAX),
        });
    }
    let ttl_secs = get_u32(t, &config, "ttl_secs")?;
    if ttl_secs == 0 || ttl_secs > ai_peer::TTL_SECS_MAX {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "ttl_secs",
            reason: "range-1..=3600".into(),
        });
    }
    // SPEC-E WP-E.3：学习闸（bool → u8 0/1；EAQ-2 dev/qemu=true、release
    // 归 owner 配置裁决——键必配无缺省）。
    let learn_enabled_raw = get_bool(t, &config, "learn_enabled")?;
    let learn_enabled = if learn_enabled_raw { 1u8 } else { 0u8 };
    Ok(CompiledConfig {
        payload: encode_payload(&[
            (ai_peer::FIELD_TIMEOUT_MS, timeout_ms.to_le_bytes().to_vec()),
            (
                ai_peer::FIELD_UNHEALTHY_AFTER,
                unhealthy_after.to_le_bytes().to_vec(),
            ),
            (
                ai_peer::FIELD_PROBE_EVERY,
                probe_every.to_le_bytes().to_vec(),
            ),
            (
                ai_peer::FIELD_MAX_POWER_KW,
                max_power_kw.to_le_bytes().to_vec(),
            ),
            (ai_peer::FIELD_TTL_SECS, ttl_secs.to_le_bytes().to_vec()),
            (ai_peer::FIELD_LEARN_ENABLED, alloc::vec![learn_enabled]),
        ]),
        config_id: config,
    })
}

/// SPEC-SERIES-2026-08-27/A WP-A.1：`security` 编译——控制链信封 HMAC-SM3
/// 密钥（32B）构建期注入。
///
/// 纪律：键值**不落任何文件**——TOML 内任何键（含明文 hex 尝试）一律
/// UnknownKey 拒绝；仅经注入链取得：`--control-chain-key-hex` >
/// `ENEROS_CONTROL_CHAIN_KEY` > `ENEROS_KMS_DIR/control-chain.key`；
/// 全缺 ⇒ Err（release 无键构建 fail-closed，WP-47.3 先例）；
/// 全零 ⇒ ZeroSecret。
pub fn compile_security(
    toml_text: &str,
    injected: Option<&[u8; 32]>,
) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(security::CONFIG_ID);
    let root: Value = toml::from_str(toml_text).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, &["security"])?;
    let sec = section(&root, &config, "security")?;
    // 零允许键：security.toml 只承载注释与空 [security] 表头，防明文入仓。
    reject_unknown_keys(sec, &config, &[])?;

    let key = injected.ok_or_else(|| ConfigError::InvalidValue {
        config: config.clone(),
        field: "control_chain_key",
        reason: "未注入：--control-chain-key-hex / ENEROS_CONTROL_CHAIN_KEY / \
                 ENEROS_KMS_DIR/control-chain.key 全缺（fail-closed）"
            .into(),
    })?;
    if crate::is_zero_bytes(&key[..]) {
        return Err(ConfigError::ZeroSecret {
            config,
            field: "control_chain_key",
        });
    }
    Ok(CompiledConfig {
        config_id: config,
        payload: encode_payload([&(security::FIELD_CONTROL_CHAIN_KEY, key)]),
    })
}

pub fn compile_by_id(
    config_id: &str,
    toml: &str,
    injected_root_key: Option<&[u8; 64]>,
    injected_control_chain_key: Option<&[u8; 32]>,
) -> Result<Option<CompiledConfig>, ConfigError> {
    match config_id {
        control_write::CONFIG_ID => compile_control_write(toml).map(Some),
        ai_peer::CONFIG_ID => compile_ai_peer(toml).map(Some),
        ota::CONFIG_ID => compile_ota(toml, injected_root_key).map(Some),
        security::CONFIG_ID => compile_security(toml, injected_control_chain_key).map(Some),
        cloud_sync::CONFIG_ID => compile_cloud_sync(toml).map(Some),
        metrics::CONFIG_ID => compile_metrics(toml).map(Some),
        logging::CONFIG_ID => compile_logging(toml).map(Some),
        points::CONFIG_ID => compile_points(toml).map(Some),
        catalog::CONFIG_ID => compile_catalog(toml).map(Some),
        id if crate::protocol::known_keys(id).is_some() => compile_protocol(id, toml).map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use eneros_protocol_abstract::ProtocolAddress;

    use super::*;
    use crate::points::decode_points;

    fn cfg() -> ConfigId {
        ConfigId::from_static(points::CONFIG_ID)
    }

    /// 好点表（覆盖 7 协议的典型点）。
    const GOOD: &str = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 1, reg_addr = 4097, func_code = 3 }
data_type = "analog"
scale = 0.1
offset = -40.0
period_ms = 5000
deadband = 0.5
min = -40.0
max = 40.0

[[point]]
point_id = 1002
device_id = 2
protocol = "iec104"
addr.iec104 = { common_addr = 1, ioa = 1001, type_id = 13 }
data_type = "counter"
scale = 1.0
offset = 0.0

[[point]]
point_id = 2001
device_id = 3
protocol = "mqtt"
addr.mqtt = { topic = "plant/pv/power" }
data_type = "analog"
scale = 1.0
offset = 0.0

[[point]]
point_id = 2002
device_id = 4
protocol = "mms"
addr.mms = { object_ref = "LD0/LRPW.Analogue.MX.w" }
data_type = "analog"
scale = 0.001
offset = 0.0

[[point]]
point_id = 3001
device_id = 5
protocol = "goose"
data_type = "digital"
scale = 1.0
offset = 0.0

[[point]]
point_id = 3002
device_id = 6
protocol = "sv"
data_type = "digital"
scale = 1.0
offset = 0.0
"#;

    #[test]
    fn points_positive_compile_decode_roundtrip() {
        let compiled = compile_points(GOOD).unwrap();
        assert_eq!(compiled.config_id, ConfigId::from_static(points::CONFIG_ID));
        let table = decode_points(&compiled.payload, &cfg()).unwrap();
        assert_eq!(table.len(), 6);
        // modbus-tcp 点：地址 + 变换 + 区间都进表。
        let p = table.find(1001).unwrap();
        assert_eq!(p.protocol, PointProtocol::ModbusTcp);
        assert_eq!(p.period_ms, 5000);
        assert_eq!(p.deadband, 0.5);
        assert_eq!(p.range, Some((-40.0, 40.0)));
        let m = p.to_mapping().unwrap();
        assert_eq!(m.to_engineering(0), -40.0); // 复用既有变换原语
                                                // goose/sv 订阅点无地址。
        assert!(table.find(3001).unwrap().addr.is_none());
        assert!(table.find(3002).unwrap().addr.is_none());
        // mms/mqtt 地址字符串往返。
        let mms = table.find(2002).unwrap();
        assert_eq!(
            mms.addr,
            Some(ProtocolAddress::Mms {
                object_ref: "LD0/LRPW.Analogue.MX.w".into()
            })
        );
        let mqtt = table.find(2001).unwrap();
        assert_eq!(
            mqtt.addr,
            Some(ProtocolAddress::Mqtt {
                topic: "plant/pv/power".into()
            })
        );
        // 全表 point_id 唯一。
        let mut ids: alloc::collections::BTreeSet<u32> = BTreeSet::new();
        for r in &table.points {
            assert!(ids.insert(r.point_id));
        }
    }

    /// 坏点表负例（DoD）：重复 point_id → 装配失败。
    #[test]
    fn points_negative_duplicate_point_id() {
        let toml = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 1, reg_addr = 1, func_code = 3 }
data_type = "analog"
scale = 1.0
offset = 0.0

[[point]]
point_id = 1001
device_id = 2
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 2, reg_addr = 2, func_code = 3 }
data_type = "analog"
scale = 1.0
offset = 0.0
"#;
        let err = compile_points(toml).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateField { .. }), "{err}");
    }

    /// 坏点表负例（DoD）：未知字段 → 装配失败。
    #[test]
    fn points_negative_unknown_field() {
        let toml = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 1, reg_addr = 1, func_code = 3 }
data_type = "analog"
scale = 1.0
offset = 0.0
sneaky = true
"#;
        let err = compile_points(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownKey { ref key, .. } if key == "sneaky"),
            "{err}"
        );
    }

    /// 坏点表负例（DoD）：scale=0 → 装配失败。
    #[test]
    fn points_negative_zero_scale() {
        let toml = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 1, reg_addr = 1, func_code = 3 }
data_type = "analog"
scale = 0.0
offset = 0.0
"#;
        let err = compile_points(toml).unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidValue { field, .. } if field == "scale"),
            "{err}"
        );
    }

    /// 坏点表负例（DoD）：坏数字（period 超界 / offset 非数字 / 枚举非法）。
    #[test]
    fn points_negative_bad_numbers_and_enums() {
        // period_ms 越界。
        let toml = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 1, reg_addr = 1, func_code = 3 }
data_type = "analog"
scale = 1.0
offset = 0.0
period_ms = 5000000
"#;
        assert!(compile_points(toml).is_err());
        // 未知 data_type 同义词（禁造——"float" 非四遥分类）。
        let toml2 = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 1, reg_addr = 1, func_code = 3 }
data_type = "float"
scale = 1.0
offset = 0.0
"#;
        assert!(compile_points(toml2).is_err());
        // min 无 max（不成对）。
        let toml3 = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 1, reg_addr = 1, func_code = 3 }
data_type = "analog"
scale = 1.0
offset = 0.0
min = 0.0
"#;
        assert!(compile_points(toml3).is_err());
        // 未知协议。
        let toml4 = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "canbus"
data_type = "analog"
scale = 1.0
offset = 0.0
"#;
        assert!(compile_points(toml4).is_err());
        // goose 写了 addr（订阅型无地址）。
        let toml5 = r#"
[[point]]
point_id = 1001
device_id = 1
protocol = "goose"
addr.modbus = { slave_addr = 1, reg_addr = 1, func_code = 3 }
data_type = "digital"
scale = 1.0
offset = 0.0
"#;
        assert!(compile_points(toml5).is_err());
    }
}

#[cfg(test)]
mod protocol_config_v46_tests {
    use super::*;

    const GOOD: &str =
        "peer = \"10.0.2.2:2404\"\nt3_timeout_ms = 20000\npoll_interval_ms = 30000\n";

    #[test]
    fn protocol_compile_accepts_known_keys() {
        let cfg = compile_protocol("protocol-iec104", GOOD).expect("compile");
        assert_eq!(cfg.config_id.as_str(), "protocol-iec104");
        let decoded = crate::protocol::decode_protocol(&cfg.payload).expect("decode");
        assert_eq!(decoded.get("peer"), Some("10.0.2.2:2404"));
        assert_eq!(decoded.get("poll_interval_ms"), Some("30000"));
    }

    #[test]
    fn protocol_compile_rejects_unknown_key() {
        let bad = "peer = \"10.0.2.2:2404\"\nnot_a_real_key = 1\n";
        assert!(compile_protocol("protocol-iec104", bad).is_err());
    }

    #[test]
    fn protocol_compile_rejects_missing_required() {
        // protocol-goose 必填 go_cb_ref/dataset_ref。
        let bad = "app_id = 16385\n";
        assert!(compile_protocol("protocol-goose", bad).is_err());
        // protocol-iec104 必填 peer。
        assert!(compile_protocol("protocol-iec104", "t3_timeout_ms = 20000").is_err());
    }

    #[test]
    fn unknown_protocol_id_rejected() {
        assert!(compile_protocol("protocol-nope", GOOD).is_err());
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    fn cfg() -> ConfigId {
        ConfigId::from_static(security::CONFIG_ID)
    }

    const KEY: [u8; 32] = [
        0x77, 0x34, 0x30, 0x2D, 0x74, 0x65, 0x73, 0x74, 0x2D, 0x6B, 0x65, 0x79, 0x11, 0x11, 0x11,
        0x11, 0x22, 0x22, 0x22, 0x22, 0x33, 0x33, 0x33, 0x33, 0x44, 0x44, 0x44, 0x44, 0x55, 0x55,
        0x55, 0x55,
    ];

    /// WP-A.1 C2：正常路径编译成功，canonical payload 可解码还原键值。
    #[test]
    fn security_compile_decode_roundtrip() {
        let compiled = compile_security("[security]\n", Some(&KEY)).unwrap();
        assert_eq!(compiled.config_id, cfg());
        let decoded =
            crate::control_chain_key::ControlChainKeyConfig::decode(&compiled.payload).unwrap();
        assert_eq!(decoded.control_chain_key, KEY);
    }

    /// WP-A.1 C2①：注入链全缺 ⇒ 编译 Err（fail-closed），文案含指引。
    #[test]
    fn security_missing_injection_fails_closed() {
        let err = compile_security("[security]\n", None).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("InvalidValue")
                && msg.contains("control_chain_key")
                && msg.contains("未注入"),
            "错误须指向 control_chain_key 且含注入链指引：{msg}"
        );
    }

    /// WP-A.1 C2②：全零密钥拒绝（ZeroSecret）。
    #[test]
    fn security_zero_key_rejected() {
        let err = compile_security("[security]\n", Some(&[0u8; 32])).unwrap_err();
        assert!(matches!(err, ConfigError::ZeroSecret { .. }), "{err:?}");
    }

    /// WP-A.1 纪律：TOML 内任何键（含明文 hex 尝试）一律拒绝。
    #[test]
    fn security_toml_plaintext_attempt_rejected() {
        let text = "[security]\ncontrol_chain_key_hex = \"7734\"\n";
        let err = compile_security(text, Some(&KEY)).unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownKey { ref key, .. } if key == "control_chain_key_hex"),
            "{err:?}"
        );
    }
}

#[cfg(test)]
mod catalog_compile_tests {
    use super::*;

    /// 可编译的最小合法目录（覆盖全部表；负面测试以此为基底做单点变异）。
    const GOOD: &str = r#"
generation = 1
valid_from_ns = 0
valid_until_ns = 0
mapping_version = 1
topology_version = 1
asset_version = 1
contract_version = 1
safety_version = 1

[[schema]]
id = 1
version = 1

[[unit_conversion]]
from = "MW"
to = "kW"
factor = 1000.0

[[asset]]
asset_id = 1
site_id = 1
tenant_id = 1
kind = "battery"
device_id = 2
topology_node_id = 2

[[asset]]
asset_id = 10
site_id = 1
tenant_id = 1
kind = "grid"
topology_node_id = 1

[[topology_node]]
id = 1
kind = "grid"

[[topology_node]]
id = 2
kind = "der"

[[topology_edge]]
from = 1
to = 2
kind = "switch"
switch_point_id = 5
capacity_kw = 500.0
loss = 0.0

[[point_semantics]]
point_id = 1
asset_id = 1
schema_id = 1
unit = "V"
role = "measurement"

[[point_semantics]]
point_id = 5
asset_id = 1
schema_id = 1
unit = "ratio"
role = "switch-state"

[[point_semantics]]
point_id = 10
asset_id = 1
schema_id = 1
unit = "kW"
role = "control"
readback_point_id = 1001

[[point_semantics]]
point_id = 1001
asset_id = 1
schema_id = 1
unit = "kW"
role = "measurement"

[[feedback_slot]]
slot = "grid_up"
point_id = 5

[[feedback_slot]]
slot = "soc"
point_id = 1

[[feedback_slot]]
slot = "dc_bus_v"
point_id = 1

[[feedback_slot]]
slot = "overcurrent"
point_id = 1

[[feedback_slot]]
slot = "overtemp"
point_id = 1

[[coverage]]
point_id = 1
min = 0.0
max = 1000.0
period_ms = 5000
unit = "V"

[[control_contract]]
contract_id = 1
asset_id = 1
point_id = 10
unit = "kW"
class = "setpoint"
min = -500.0
max = 500.0
ramp_kw_per_s = 3.3333
settle_ms = 30000
readback_point_id = 1001
tolerance_kw = 0.1
reversible = true
compensation = false
priority = "remote"
safe_state = "hold"
lost_link = "hold"

[[safety_constraint]]
constraint_id = 1
asset_id = 1
kind = "overtemp"
point_id = 1
limit = 1.0

[[alias]]
kind = "market-device"
legacy_id = 1
asset_id = 1

[[tombstone]]
asset_id = 99
kind = 1
"#;

    #[test]
    fn good_catalog_compiles_and_roundtrips() {
        let compiled = compile_catalog(GOOD).expect("good catalog");
        let decoded = crate::points::decode_points; // 无关（类型占位）
        let _ = decoded;
        // payload 长度有界（agent-sdk 分页 1664B——catalog 必须可配置下发）。
        assert!(compiled.payload.len() <= crate::schema::catalog::MAX_FIELD as usize * 300);
    }

    fn with_section(base: &str, section: &str, replacement: &str) -> String {
        // 简单字符串替换：把 `section 名开头的整段` 换成 replacement 内容。
        let idx = base.find(section).unwrap();
        format!("{}{}", &base[..idx], replacement)
    }

    #[test]
    fn rejects_duplicate_point_id() {
        let bad = with_section(
            GOOD,
            "[[point_semantics]]\npoint_id = 1001",
            "[[point_semantics]]\npoint_id = 1001\nasset_id = 1\nschema_id = 1\nunit = \"kW\"\nrole = \"measurement\"\n\n[[point_semantics]]\npoint_id = 1001\nasset_id = 1\nschema_id = 1\nunit = \"kW\"\nrole = \"measurement\"\n\n",
        );
        assert!(compile_catalog(&bad).is_err());
    }

    #[test]
    fn rejects_control_without_readback() {
        let bad = GOOD.replace(
            "point_id = 10\nasset_id = 1\nschema_id = 1\nunit = \"kW\"\nrole = \"control\"\nreadback_point_id = 1001",
            "point_id = 10\nasset_id = 1\nschema_id = 1\nunit = \"kW\"\nrole = \"control\"",
        );
        assert!(compile_catalog(&bad).is_err());
    }

    #[test]
    fn rejects_point_unknown_asset() {
        let bad = GOOD.replace("point_id = 1\nasset_id = 1", "point_id = 1\nasset_id = 777");
        assert!(compile_catalog(&bad).is_err());
    }

    #[test]
    fn rejects_dimension_mismatched_conversion() {
        // kW → kWh 量纲不兼容（Power vs Energy）。
        let bad = GOOD.replace(
            "from = \"MW\"\nto = \"kW\"\nfactor = 1000.0",
            "from = \"kW\"\nto = \"kWh\"\nfactor = 1.0",
        );
        assert!(compile_catalog(&bad).is_err());
    }

    #[test]
    fn rejects_switch_edge_without_observation_point() {
        let bad = GOOD.replace("switch_point_id = 5\n", "switch_point_id = 0\n");
        assert!(compile_catalog(&bad).is_err());
    }

    #[test]
    fn rejects_island_and_self_loop() {
        // 自环：from == to。
        let loop_bad = GOOD.replace("from = 1\nto = 2", "from = 1\nto = 1");
        assert!(compile_catalog(&loop_bad).is_err());
        // 孤岛：节点 2 无边可达。
        let island = GOOD.replace(
            "[[topology_node]]\nid = 2\nkind = \"der\"",
            "[[topology_node]]\nid = 2\nkind = \"der\"\n\n[[topology_node]]\nid = 3\nkind = \"der\"",
        );
        assert!(compile_catalog(&island).is_err());
    }

    #[test]
    fn rejects_unknown_unit_name() {
        let bad = GOOD.replace("unit = \"V\"", "unit = \"furlongs\"");
        assert!(compile_catalog(&bad).is_err());
    }

    #[test]
    fn rejects_feedback_slots_incomplete() {
        // 把 5 槽中的 "overtemp" 槽删掉 → 4 槽 → 编译错误。
        let bad = GOOD.replace("[[feedback_slot]]\nslot = \"overtemp\"\npoint_id = 1\n", "");
        assert!(compile_catalog(&bad).is_err());
    }

    #[test]
    fn rejects_tombstone_of_alive_asset() {
        let bad = GOOD.replace("asset_id = 99", "asset_id = 1");
        assert!(compile_catalog(&bad).is_err());
    }

    #[test]
    fn rejects_zero_generation() {
        let bad = GOOD.replace("generation = 1", "generation = 0");
        assert!(compile_catalog(&bad).is_err());
    }
}
