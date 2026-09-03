//! IEC 60870-5-103 配置（typed parser；键集=蓝图 48 WP-48.3 可登记项）。

use eneros_protocol_abstract::{ProtocolConfig, ProtocolError};

/// 已知配置键（未知键 → `InvalidConfig`；蓝图 64 WP-64.1.2 起单源
/// 引用 `eneros-protocol-keys`）。
pub const KNOWN_KEYS: &[&str] = eneros_protocol_keys::IEC103;

/// 校验配置。
pub fn validate(cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
    eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
    if let Some(addr) = eneros_protocol_abstract::plugin::config_u64(cfg, "common_addr")? {
        if addr == 0 || addr > u16::MAX as u64 {
            return Err(ProtocolError::InvalidConfig);
        }
    }
    if let Some(r) = eneros_protocol_abstract::plugin::config_u64(cfg, "retries")? {
        if r > u8::MAX as u64 {
            return Err(ProtocolError::InvalidConfig);
        }
    }
    Ok(())
}

/// 已消费的运行配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iec103Config {
    /// 公共地址（1 起，0 保留）。
    pub common_addr: u16,
    /// 收发超时（毫秒）。
    pub timeout_ms: u64,
    /// 重试次数。
    pub retries: u8,
}

impl Iec103Config {
    /// 从校验过的 `ProtocolConfig` 解析。
    pub fn from_validated(cfg: &ProtocolConfig) -> Result<Self, ProtocolError> {
        validate(cfg)?;
        Ok(Self {
            common_addr: eneros_protocol_abstract::plugin::config_u64(cfg, "common_addr")?
                .unwrap_or(1) as u16,
            timeout_ms: eneros_protocol_abstract::plugin::config_u64(cfg, "timeout_ms")?
                .unwrap_or(1000),
            retries: eneros_protocol_abstract::plugin::config_u64(cfg, "retries")?.unwrap_or(2)
                as u8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_and_zero_addr() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("common_addr", "1");
        assert_eq!(validate(&cfg), Ok(()));
        cfg.insert("evil", "x");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
        let mut zero = ProtocolConfig::default();
        zero.insert("common_addr", "0");
        assert_eq!(validate(&zero), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn consumes_every_field() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("common_addr", "3");
        cfg.insert("timeout_ms", "500");
        cfg.insert("retries", "1");
        let c = Iec103Config::from_validated(&cfg).expect("cfg");
        assert_eq!(
            c,
            Iec103Config {
                common_addr: 3,
                timeout_ms: 500,
                retries: 1
            }
        );
    }

    /// 蓝图 64 WP-64.1.3 双面引用锁（插件侧）：KNOWN_KEYS = 键表单源
    /// `eneros_protocol_keys::IEC103`；对侧锁在 config-bundle
    /// protocol.rs r64_ref_lock_iec103。
    #[test]
    fn r64_ref_lock_iec103() {
        assert_eq!(KNOWN_KEYS, eneros_protocol_keys::IEC103);
    }
}
