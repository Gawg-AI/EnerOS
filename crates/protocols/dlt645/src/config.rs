//! DL/T 645 配置（typed parser——经 `validate_config` 校验后的
//! [`ProtocolConfig`] 解析为本结构；每个字段真正进入运行路径）。
//!
//! 字段来源 = 蓝图 48 WP-48.1 卷内登记（RS485 半双工主从问答；波特率
//! 1200–9600 默认 2400 可配；表地址 12 位 BCD）。帧格式细节（CS 覆盖/
//! 控制码语义/DI 结构）属 codec 层，BLOCKED 待规约文本。

use eneros_protocol_abstract::{ProtocolConfig, ProtocolError};

/// 已知配置键（未知键 → `InvalidConfig`，WP-39.10 不变量）。
pub const KNOWN_KEYS: &[&str] = &["addr_bcd", "baud", "timeout_ms", "retries"];

/// 默认轮询超时（毫秒）。
pub const DEFAULT_TIMEOUT_MS: u64 = 1000;
/// 默认重试次数。
pub const DEFAULT_RETRIES: u8 = 2;
/// 波特率下限（蓝图 WP-48.1：1200–9600）。
pub const BAUD_MIN: u64 = 1200;
/// 波特率上限（蓝图 WP-48.1：1200–9600）。
pub const BAUD_MAX: u64 = 9600;
/// 缺省波特率（蓝图 WP-48.1：默认 2400）。
pub const DEFAULT_BAUD: u64 = 2400;

/// 校验配置：只允许已知键 + 值域检查（baud/retries 越界 → `InvalidConfig`）。
pub fn validate(cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
    eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
    if let Some(baud) = eneros_protocol_abstract::plugin::config_u64(cfg, "baud")? {
        if !(BAUD_MIN..=BAUD_MAX).contains(&baud) {
            return Err(ProtocolError::InvalidConfig);
        }
    }
    if let Some(r) = eneros_protocol_abstract::plugin::config_u64(cfg, "retries")? {
        if r > u8::MAX as u64 {
            return Err(ProtocolError::InvalidConfig);
        }
    }
    // addr_bcd：12 位十进制表地址（999999999999=广播）。
    if let Some(addr) = eneros_protocol_abstract::plugin::config_u64(cfg, "addr_bcd")? {
        if addr > 999_999_999_999 {
            return Err(ProtocolError::InvalidConfig);
        }
    }
    Ok(())
}

/// 已消费的运行配置（start 从校验过的配置本体解析——禁 default 顶替）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dlt645Config {
    /// 表地址（12 位十进制数值；线上 BCD 字节序属 codec BLOCKED 层）。
    pub addr_bcd: u64,
    /// 波特率。
    pub baud: u64,
    /// 收发超时（毫秒）。
    pub timeout_ms: u64,
    /// 重试次数。
    pub retries: u8,
}

impl Dlt645Config {
    /// 从校验过的 `ProtocolConfig` 解析（缺省键取默认值）。
    pub fn from_validated(cfg: &ProtocolConfig) -> Result<Self, ProtocolError> {
        validate(cfg)?;
        Ok(Self {
            addr_bcd: eneros_protocol_abstract::plugin::config_u64(cfg, "addr_bcd")?.unwrap_or(1),
            baud: eneros_protocol_abstract::plugin::config_u64(cfg, "baud")?
                .unwrap_or(DEFAULT_BAUD),
            timeout_ms: eneros_protocol_abstract::plugin::config_u64(cfg, "timeout_ms")?
                .unwrap_or(DEFAULT_TIMEOUT_MS),
            retries: u8::try_from(
                eneros_protocol_abstract::plugin::config_u64(cfg, "retries")?
                    .unwrap_or(DEFAULT_RETRIES as u64),
            )
            .map_err(|_| ProtocolError::InvalidConfig)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_keys() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("addr_bcd", "1");
        assert_eq!(validate(&cfg), Ok(()));
        cfg.insert("evil", "x");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn baud_range_enforced() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("baud", "2400");
        assert_eq!(validate(&cfg), Ok(()));
        cfg.insert("baud", "115200");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
        cfg.insert("baud", "600");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
        cfg.insert("baud", "abc");
        assert_eq!(
            eneros_protocol_abstract::plugin::config_u64(&cfg, "baud"),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn addr_bcd_upper_bound_is_broadcast_value() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("addr_bcd", "999999999999");
        assert_eq!(validate(&cfg), Ok(()));
        cfg.insert("addr_bcd", "1000000000000");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn defaults_and_consumption() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("addr_bcd", "123456789012");
        cfg.insert("baud", "4800");
        let c = Dlt645Config::from_validated(&cfg).expect("cfg");
        assert_eq!(c.addr_bcd, 123456789012);
        assert_eq!(c.baud, 4800);
        assert_eq!(c.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(c.retries, DEFAULT_RETRIES);
    }
}
