//! GB/T 27930 配置（typed parser；键集=蓝图 48 WP-48.4 可登记项）。
//!
//! **fail-closed 纪律**：超时表数值与源/目的节点地址属规约附录
//! （BLOCKED 待文本）——本卷不设任何缺省：全部键必须显式配置，
//! 防止凭记忆填数（与 WP-48.2 t0-t3 同纪律）。

use eneros_protocol_abstract::{ProtocolConfig, ProtocolError};

/// 已知配置键（未知键 → `InvalidConfig`）。
pub const KNOWN_KEYS: &[&str] = &[
    "charger_addr",
    "bms_addr",
    "handshake_timeout_ms",
    "param_timeout_ms",
    "charging_timeout_ms",
    "end_timeout_ms",
    "retransmit_max",
];

/// 校验配置：未知键拒绝 + 全键必配 + 正值。
pub fn validate(cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
    eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
    for key in KNOWN_KEYS.iter() {
        if cfg.get(key).is_none() {
            // 规约表/地址表 BLOCKED——禁凭记忆缺省，必须显式配置。
            return Err(ProtocolError::InvalidConfig);
        }
    }
    for key in [
        "handshake_timeout_ms",
        "param_timeout_ms",
        "charging_timeout_ms",
        "end_timeout_ms",
    ] {
        let v = eneros_protocol_abstract::plugin::config_u64(cfg, key)?;
        if v.map(|v| v == 0).unwrap_or(true) {
            return Err(ProtocolError::InvalidConfig);
        }
    }
    Ok(())
}

/// 已消费的运行配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gbt27930Config {
    /// 充电机节点地址（规约附录地址表 BLOCKED——须显式配置）。
    pub charger_addr: u8,
    /// BMS 节点地址（同上）。
    pub bms_addr: u8,
    /// 握手相超时（毫秒）。
    pub handshake_timeout_ms: u64,
    /// 参数配置相超时（毫秒）。
    pub param_timeout_ms: u64,
    /// 充电相看门狗超时（毫秒）。
    pub charging_timeout_ms: u64,
    /// 结束相超时（毫秒）。
    pub end_timeout_ms: u64,
    /// 每报文最大重发次数。
    pub retransmit_max: u8,
}

impl Gbt27930Config {
    /// 从校验过的 `ProtocolConfig` 解析（缺任一键 → `InvalidConfig`）。
    pub fn from_validated(cfg: &ProtocolConfig) -> Result<Self, ProtocolError> {
        validate(cfg)?;
        let req = |key: &str| -> Result<u64, ProtocolError> {
            eneros_protocol_abstract::plugin::config_u64(cfg, key)?
                .ok_or(ProtocolError::InvalidConfig)
        };
        Ok(Self {
            charger_addr: req("charger_addr")? as u8,
            bms_addr: req("bms_addr")? as u8,
            handshake_timeout_ms: req("handshake_timeout_ms")?,
            param_timeout_ms: req("param_timeout_ms")?,
            charging_timeout_ms: req("charging_timeout_ms")?,
            end_timeout_ms: req("end_timeout_ms")?,
            retransmit_max: req("retransmit_max")? as u8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_cfg() -> ProtocolConfig {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("charger_addr", "86");
        cfg.insert("bms_addr", "1");
        cfg.insert("handshake_timeout_ms", "5000");
        cfg.insert("param_timeout_ms", "1000");
        cfg.insert("charging_timeout_ms", "1000");
        cfg.insert("end_timeout_ms", "500");
        cfg.insert("retransmit_max", "3");
        cfg
    }

    #[test]
    fn all_keys_required_no_memory_defaults() {
        assert_eq!(validate(&full_cfg()), Ok(()));
        // 缺任一键 → InvalidConfig（地址表/超时表 BLOCKED——禁凭记忆）。
        for key in KNOWN_KEYS.iter() {
            let mut cfg = full_cfg();
            cfg.entries.retain(|(k, _)| k != *key);
            assert_eq!(
                validate(&cfg),
                Err(ProtocolError::InvalidConfig),
                "missing {key} must be rejected"
            );
        }
    }

    #[test]
    fn zero_timeout_rejected() {
        let mut cfg = full_cfg();
        cfg.insert("charging_timeout_ms", "0");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn unknown_key_rejected_and_values_consumed() {
        let mut cfg = full_cfg();
        cfg.insert("evil", "x");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
        let c = Gbt27930Config::from_validated(&full_cfg()).expect("cfg");
        assert_eq!(c.charger_addr, 86);
        assert_eq!(c.bms_addr, 1);
        assert_eq!(c.retransmit_max, 3);
    }
}
