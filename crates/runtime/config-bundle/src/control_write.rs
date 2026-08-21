//! 47（WP-47.1 速率可调）：`control-write` 类型化解码（装配侧 svc 读
//! bundle 注入——rate gate 每小时配额）。编码面 = toml_compile；字段 id
//! 单一事实在 `schema::control_write`。

use crate::payload::decode_fields;
use crate::{ConfigError, ConfigId};

/// 解码后的 control-write 配置（rate_per_hour 缺省 = 产品值 60）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlWriteConfig {
    /// 本地预置租约 ID（16B；两侧同源）。
    pub lease_id: [u8; 16],
    /// legacy_input 策略（1=deny-and-log）。
    pub legacy_input: u8,
    /// handshake 重试（≤5）。
    pub handshake_retries: u8,
    /// dispatch rate gate 每小时配额（None = 产品默认 60/h）。
    pub rate_per_hour: Option<u32>,
}

impl ControlWriteConfig {
    /// 解码 canonical payload；任一步失败 → Err（fail-closed）。
    pub fn decode(raw: &[u8]) -> Result<Self, ConfigError> {
        let config = ConfigId::from_static(crate::schema::control_write::CONFIG_ID);
        let fields = decode_fields(raw, &config, crate::schema::control_write::MAX_FIELD)?;
        let mut out = ControlWriteConfig {
            lease_id: [0; 16],
            legacy_input: 0,
            handshake_retries: 0,
            rate_per_hour: None,
        };
        for (id, value) in fields {
            match id {
                crate::schema::control_write::FIELD_LEASE_ID => {
                    if value.len() != crate::schema::control_write::LEASE_ID_LEN {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "lease_id",
                            reason: "len".into(),
                        });
                    }
                    out.lease_id.copy_from_slice(&value);
                }
                crate::schema::control_write::FIELD_LEGACY_INPUT => {
                    out.legacy_input = *value.first().ok_or_else(|| ConfigError::InvalidValue {
                        config: config.clone(),
                        field: "legacy_input",
                        reason: "empty".into(),
                    })?;
                }
                crate::schema::control_write::FIELD_HANDSHAKE_RETRIES => {
                    out.handshake_retries =
                        *value.first().ok_or_else(|| ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "handshake_retries",
                            reason: "empty".into(),
                        })?;
                }
                crate::schema::control_write::FIELD_RATE_PER_HOUR => {
                    if value.len() != 4 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "rate_per_hour",
                            reason: "len".into(),
                        });
                    }
                    let rate = u32::from_le_bytes(value[..4].try_into().map_err(|_| {
                        ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "rate_per_hour",
                            reason: "bytes".into(),
                        }
                    })?);
                    out.rate_per_hour = Some(rate);
                }
                _ => {}
            }
        }
        Ok(out)
    }
}
