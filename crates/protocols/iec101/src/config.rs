//! IEC 60870-5-101 配置（typed parser；键集=蓝图 48 WP-48.2 卷内登记）。
//!
//! 超时/重传参数的**缺省值**属规约表（t0/t1/t2/t3/k/w 具体数值待规约文本
//! 核对）——本卷不设魔法缺省：超时组键必须显式配置（fail-closed），防止
//! 凭记忆填数。地址值域按 u16 承载（1/2 字节可配，线上编码属 BLOCKED 层）。

use eneros_protocol_abstract::{ProtocolConfig, ProtocolError};

/// 已知配置键（未知键 → `InvalidConfig`）。
pub const KNOWN_KEYS: &[&str] = &[
    "link_addr",
    "common_addr",
    "link_addr_size",
    "common_addr_size",
    "balanced",
    "t0_ms",
    "t1_ms",
    "t2_ms",
    "t3_ms",
    "k",
    "w",
    "timeout_ms",
];

/// 校验配置：未知键拒绝 + 地址尺寸 ∈ {1,2} + k/w 正值。
pub fn validate(cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
    eneros_protocol_abstract::plugin::reject_unknown_keys(cfg, KNOWN_KEYS)?;
    for key in ["link_addr_size", "common_addr_size"] {
        if let Some(v) = eneros_protocol_abstract::plugin::config_u64(cfg, key)? {
            if v != 1 && v != 2 {
                return Err(ProtocolError::InvalidConfig);
            }
        }
    }
    for key in ["k", "w", "link_addr", "common_addr"] {
        // 值域先于 from_validated 截断检查：k/w ∈ 1..=u16::MAX，地址 ≤u16::MAX
        // （PROJECT.md 数值纪律——禁 as 截断绕过校验）。
        if let Some(v) = eneros_protocol_abstract::plugin::config_u64(cfg, key)? {
            let nonzero_ok = !matches!(key, "k" | "w") || v > 0;
            if !nonzero_ok || v > u16::MAX as u64 {
                return Err(ProtocolError::InvalidConfig);
            }
        }
    }
    Ok(())
}

/// 已消费的运行配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iec101Config {
    /// 链路地址。
    pub link_addr: u16,
    /// ASDU 公共地址。
    pub common_addr: u16,
    /// 链路地址字节数（1|2）。
    pub link_addr_size: u8,
    /// 公共地址字节数（1|2）。
    pub common_addr_size: u8,
    /// 平衡模式（false=非平衡主站召唤）。
    pub balanced: bool,
    /// 超时组 t0/t1/t2/t3（毫秒；规约表数值 BLOCKED，须显式配置）。
    pub t0_ms: u64,
    pub t1_ms: u64,
    pub t2_ms: u64,
    pub t3_ms: u64,
    /// k/w 窗口。
    pub k: u16,
    pub w: u16,
    /// 单次收发超时（毫秒）。
    pub timeout_ms: u64,
}

impl Iec101Config {
    /// 从校验过的 `ProtocolConfig` 解析；t0-t3/k/w 缺失 →
    /// `InvalidConfig`（规约表数值 BLOCKED——禁凭记忆缺省，fail-closed）。
    pub fn from_validated(cfg: &ProtocolConfig) -> Result<Self, ProtocolError> {
        validate(cfg)?;
        let req = |key: &str| -> Result<u64, ProtocolError> {
            eneros_protocol_abstract::plugin::config_u64(cfg, key)?
                .ok_or(ProtocolError::InvalidConfig)
        };
        let conv_u16 = |key: &str, default: u64| -> Result<u16, ProtocolError> {
            u16::try_from(
                eneros_protocol_abstract::plugin::config_u64(cfg, key)?.unwrap_or(default),
            )
            .map_err(|_| ProtocolError::InvalidConfig)
        };
        Ok(Self {
            link_addr: conv_u16("link_addr", 1)?,
            common_addr: conv_u16("common_addr", 1)?,
            link_addr_size: u8::try_from(
                eneros_protocol_abstract::plugin::config_u64(cfg, "link_addr_size")?.unwrap_or(1),
            )
            .map_err(|_| ProtocolError::InvalidConfig)?,
            common_addr_size: u8::try_from(
                eneros_protocol_abstract::plugin::config_u64(cfg, "common_addr_size")?.unwrap_or(1),
            )
            .map_err(|_| ProtocolError::InvalidConfig)?,
            balanced: matches!(cfg.get("balanced"), Some("true") | Some("1")),
            t0_ms: req("t0_ms")?,
            t1_ms: req("t1_ms")?,
            t2_ms: req("t2_ms")?,
            t3_ms: req("t3_ms")?,
            k: conv_u16("k", 12)?,
            w: conv_u16("w", 8)?,
            timeout_ms: eneros_protocol_abstract::plugin::config_u64(cfg, "timeout_ms")?
                .unwrap_or(1000),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_cfg() -> ProtocolConfig {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("link_addr", "1");
        cfg.insert("common_addr", "1");
        cfg.insert("t0_ms", "30");
        cfg.insert("t1_ms", "300");
        cfg.insert("t2_ms", "200");
        cfg.insert("t3_ms", "20000");
        cfg.insert("k", "12");
        cfg.insert("w", "8");
        cfg
    }

    #[test]
    fn rejects_unknown_keys() {
        let p = full_cfg();
        assert_eq!(validate(&p), Ok(()));
        let mut bad = full_cfg();
        bad.insert("evil", "1");
        assert_eq!(validate(&bad), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn addr_size_must_be_1_or_2() {
        let mut cfg = full_cfg();
        cfg.insert("link_addr_size", "2");
        assert_eq!(validate(&cfg), Ok(()));
        cfg.insert("link_addr_size", "4");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn out_of_range_values_rejected_before_truncation() {
        // k=65536 截断成 0 可绕过非零校验——validate 必须先拒（审查修复）。
        let mut cfg = full_cfg();
        cfg.insert("k", "65536");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
        let mut cfg = full_cfg();
        cfg.insert("w", "70000");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
        let mut cfg = full_cfg();
        cfg.insert("link_addr", "70000");
        assert_eq!(validate(&cfg), Err(ProtocolError::InvalidConfig));
    }

    #[test]
    fn timeout_table_is_required_no_memory_defaults() {
        // t0-t3/k/w 缺失 → InvalidConfig（禁凭记忆填规约表数值）。
        let mut cfg = ProtocolConfig::default();
        cfg.insert("link_addr", "1");
        assert_eq!(
            Iec101Config::from_validated(&cfg),
            Err(ProtocolError::InvalidConfig)
        );
    }

    #[test]
    fn full_config_consumes_every_field() {
        let c = Iec101Config::from_validated(&full_cfg()).expect("cfg");
        assert_eq!(c.link_addr, 1);
        assert_eq!(c.link_addr_size, 1);
        assert!(!c.balanced);
        assert_eq!(c.t3_ms, 20000);
        assert_eq!(c.k, 12);
        assert_eq!(c.w, 8);
        let mut b = full_cfg();
        b.insert("balanced", "true");
        assert!(Iec101Config::from_validated(&b).expect("cfg").balanced);
    }
}
