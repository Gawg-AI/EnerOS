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

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use toml::Value;

use crate::payload::encode_payload;
use crate::points::{point_type_from_name, point_type_ordinal, PointProtocol};
use crate::schema::{ai_peer, cloud_sync, control_write, logging, metrics, ota, points, security};
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
/// 输出 `FIELD_POINTS` 自描述子格式（布局见 `schema::points` 模块注释）。
pub fn compile_points(toml: &str) -> Result<CompiledConfig, ConfigError> {
    let config = ConfigId::from_static(points::CONFIG_ID);
    let root: Value = toml::from_str(toml).map_err(|e| ConfigError::InvalidValue {
        config: config.clone(),
        field: "toml",
        reason: format!("parse:{e}"),
    })?;
    let t = table(&root, &config, "root")?;
    reject_unknown_keys(t, &config, &["point"])?;
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
    }
    Ok(CompiledConfig {
        payload: encode_payload(&[(points::FIELD_POINTS, payload.as_slice())]),
        config_id: config,
    })
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
