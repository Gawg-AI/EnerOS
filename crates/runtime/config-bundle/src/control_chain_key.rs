//! SPEC-SERIES-2026-08-27/A WP-A.1：`security` 类型化解码——装配侧
//! svc-control-dispatch / svc-device-agent 取控制链信封 HMAC-SM3 密钥。
//!
//! 编码面 = [`crate::toml_compile::compile_security`]（构建期注入）；
//! 本文件只做 canonical payload → 密钥的严格解码：缺字段/长度≠32/全零
//! 一律 Err（fail-closed），调用方据此打出 `CHAIN-KEY-MISSING` 拒载。

use crate::payload::decode_fields;
use crate::{ConfigError, ConfigId};

/// 解码后的 security 配置节（当前仅承载信封密钥）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlChainKeyConfig {
    /// 信封 HMAC-SM3 密钥（32B）。
    pub control_chain_key: [u8; 32],
}

impl ControlChainKeyConfig {
    /// 解码 canonical payload；任一步失败 → Err（fail-closed）。
    pub fn decode(raw: &[u8]) -> Result<Self, ConfigError> {
        let config = ConfigId::from_static(crate::schema::security::CONFIG_ID);
        let fields = decode_fields(raw, &config, crate::schema::security::MAX_FIELD)?;
        let mut out = Self {
            control_chain_key: [0; 32],
        };
        let mut seen = false;
        for (id, value) in fields {
            if id == crate::schema::security::FIELD_CONTROL_CHAIN_KEY {
                if value.len() != crate::schema::security::CONTROL_CHAIN_KEY_LEN {
                    return Err(ConfigError::InvalidValue {
                        config: config.clone(),
                        field: "control_chain_key",
                        reason: "len".into(),
                    });
                }
                out.control_chain_key.copy_from_slice(&value);
                seen = true;
            }
        }
        if !seen {
            return Err(ConfigError::MissingField {
                config,
                field: "control_chain_key",
            });
        }
        if out.control_chain_key == [0u8; 32] {
            return Err(ConfigError::ZeroSecret {
                config: ConfigId::from_static(crate::schema::security::CONFIG_ID),
                field: "control_chain_key",
            });
        }
        Ok(out)
    }
}
