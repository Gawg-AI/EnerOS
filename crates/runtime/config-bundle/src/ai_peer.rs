//! 49（WP-49.2 远程 dual-brain）：`ai-peer` 类型化解码（svc-ai-worker 装配
//! 侧消费——RemoteBrain 状态机参数）。编码面 = toml_compile；字段 id 单一
//! 事实在 `schema::ai_peer`。全部字段必配（编译侧强制），解码侧值域复检。

use crate::payload::decode_fields;
use crate::{ConfigError, ConfigId};

/// 解码后的 ai-peer 配置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiPeerConfig {
    /// 在途请求超时毫秒（产品缺省 5000——经配置显式给出，非代码缺省）。
    pub timeout_ms: u64,
    /// 连续超时→Unhealthy 阈值（产品缺省 3）。
    pub unhealthy_after: u32,
    /// Unhealthy 探测周期 tick 数（产品缺省 5）。
    pub probe_every: u32,
    /// 功率幅值预算 kW。
    pub max_power_kw: f64,
    /// 提案 TTL 上限秒。
    pub ttl_secs: u32,
    /// SPEC-E WP-E.3：有界学习闸（dev/qemu=true、release 归 owner 裁决；
    /// false ⇒ 零学习零持久，System1 阈值恒出厂——行为等价旧链路）。
    pub learn_enabled: bool,
}

impl AiPeerConfig {
    /// 解码 canonical payload；任一步失败 → Err（fail-closed）。
    pub fn decode(raw: &[u8]) -> Result<Self, ConfigError> {
        let config = ConfigId::from_static(crate::schema::ai_peer::CONFIG_ID);
        let fields = decode_fields(raw, &config, crate::schema::ai_peer::MAX_FIELD)?;
        let mut out = AiPeerConfig {
            timeout_ms: 0,
            unhealthy_after: 0,
            probe_every: 0,
            max_power_kw: 0.0,
            ttl_secs: 0,
            learn_enabled: false,
        };
        let mut seen = [false; 7];
        for (id, value) in fields {
            match id {
                crate::schema::ai_peer::FIELD_TIMEOUT_MS => {
                    if value.len() != 8 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "timeout_ms",
                            reason: "len".into(),
                        });
                    }
                    out.timeout_ms = u64::from_le_bytes(value.try_into().expect("checked len"));
                    seen[id as usize] = true;
                }
                crate::schema::ai_peer::FIELD_UNHEALTHY_AFTER => {
                    if value.len() != 4 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "unhealthy_after",
                            reason: "len".into(),
                        });
                    }
                    out.unhealthy_after =
                        u32::from_le_bytes(value.try_into().expect("checked len"));
                    seen[id as usize] = true;
                }
                crate::schema::ai_peer::FIELD_PROBE_EVERY => {
                    if value.len() != 4 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "probe_every",
                            reason: "len".into(),
                        });
                    }
                    out.probe_every = u32::from_le_bytes(value.try_into().expect("checked len"));
                    seen[id as usize] = true;
                }
                crate::schema::ai_peer::FIELD_MAX_POWER_KW => {
                    if value.len() != 8 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "max_power_kw",
                            reason: "len".into(),
                        });
                    }
                    out.max_power_kw = f64::from_le_bytes(value.try_into().expect("checked len"));
                    seen[id as usize] = true;
                }
                crate::schema::ai_peer::FIELD_TTL_SECS => {
                    if value.len() != 4 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "ttl_secs",
                            reason: "len".into(),
                        });
                    }
                    out.ttl_secs = u32::from_le_bytes(value.try_into().expect("checked len"));
                    seen[id as usize] = true;
                }
                crate::schema::ai_peer::FIELD_LEARN_ENABLED => {
                    if value.len() != 1 || value[0] > 1 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "learn_enabled",
                            reason: "bool".into(),
                        });
                    }
                    out.learn_enabled = value[0] == 1;
                    seen[id as usize] = true;
                }
                other => {
                    return Err(ConfigError::InvalidValue {
                        config: config.clone(),
                        field: "unknown-field-id",
                        reason: alloc::format!("{other}"),
                    })
                }
            }
        }
        // 全必配：缺任一字段即拒绝。
        for (i, ok) in seen.iter().enumerate() {
            if i == 0 {
                continue;
            }
            if !ok && i <= crate::schema::ai_peer::MAX_FIELD as usize {
                return Err(ConfigError::MissingField {
                    config: config.clone(),
                    field: "required-field",
                });
            }
        }
        // 值域复检（与编译侧同域——防绕过编译直改 bundle）。
        use crate::schema::ai_peer::*;
        if !(TIMEOUT_MS_MIN..=TIMEOUT_MS_MAX).contains(&out.timeout_ms) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "timeout_ms",
                reason: "range".into(),
            });
        }
        if !(UNHEALTHY_AFTER_MIN..=UNHEALTHY_AFTER_MAX).contains(&out.unhealthy_after) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "unhealthy_after",
                reason: "range".into(),
            });
        }
        if !(PROBE_EVERY_MIN..=PROBE_EVERY_MAX).contains(&out.probe_every) {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "probe_every",
                reason: "range".into(),
            });
        }
        if !out.max_power_kw.is_finite()
            || out.max_power_kw <= 0.0
            || out.max_power_kw > MAX_POWER_KW_MAX
        {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "max_power_kw",
                reason: "range".into(),
            });
        }
        if out.ttl_secs == 0 || out.ttl_secs > TTL_SECS_MAX {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "ttl_secs",
                reason: "range".into(),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::encode_payload;

    fn compile_payload(
        timeout_ms: u64,
        unhealthy_after: u32,
        probe_every: u32,
        max_power_kw: f64,
        ttl_secs: u32,
    ) -> alloc::vec::Vec<u8> {
        encode_payload(&[
            (
                crate::schema::ai_peer::FIELD_TIMEOUT_MS,
                timeout_ms.to_le_bytes().to_vec(),
            ),
            (
                crate::schema::ai_peer::FIELD_UNHEALTHY_AFTER,
                unhealthy_after.to_le_bytes().to_vec(),
            ),
            (
                crate::schema::ai_peer::FIELD_PROBE_EVERY,
                probe_every.to_le_bytes().to_vec(),
            ),
            (
                crate::schema::ai_peer::FIELD_MAX_POWER_KW,
                max_power_kw.to_le_bytes().to_vec(),
            ),
            (
                crate::schema::ai_peer::FIELD_TTL_SECS,
                ttl_secs.to_le_bytes().to_vec(),
            ),
            (
                crate::schema::ai_peer::FIELD_LEARN_ENABLED,
                alloc::vec![1u8],
            ),
        ])
    }

    #[test]
    fn roundtrip_all_fields_required() {
        let raw = compile_payload(5_000, 3, 5, 5.0, 60);
        let cfg = AiPeerConfig::decode(&raw).expect("decode");
        assert_eq!(
            cfg,
            AiPeerConfig {
                timeout_ms: 5_000,
                unhealthy_after: 3,
                probe_every: 5,
                max_power_kw: 5.0,
                ttl_secs: 60,
                learn_enabled: true
            }
        );
    }

    /// SPEC-E WP-E.3：learn_enabled 0/1 语义 + 非 0/1 拒绝 + 缺键拒绝。
    #[test]
    fn learn_enabled_gate_semantics() {
        let mk = |v: u8| {
            encode_payload(&[(crate::schema::ai_peer::FIELD_LEARN_ENABLED, alloc::vec![v])])
        };
        // 单独字段必然缺其余五键 → MissingField（全必配纪律不受新键稀释）。
        assert!(AiPeerConfig::decode(&mk(0)).is_err());
        // 完整载荷 learn_enabled=false。
        let raw_off = compile_payload(5_000, 3, 5, 5.0, 60);
        let mut patched = raw_off.clone();
        // 末字段即 learn_enabled（encode_payload 按序拼接）——改末字节 0。
        let n = patched.len();
        patched[n - 1] = 0;
        let cfg = AiPeerConfig::decode(&patched).expect("decode off");
        assert!(!cfg.learn_enabled);
        // 非 0/1 拒绝。
        let mut bad = compile_payload(5_000, 3, 5, 5.0, 60);
        let n = bad.len();
        bad[n - 1] = 2;
        assert!(AiPeerConfig::decode(&bad).is_err());
    }

    #[test]
    fn missing_field_rejected() {
        let raw = encode_payload(&[(
            crate::schema::ai_peer::FIELD_TIMEOUT_MS,
            5_000u64.to_le_bytes().to_vec(),
        )]);
        assert!(AiPeerConfig::decode(&raw).is_err(), "缺字段必须拒绝");
    }

    #[test]
    fn unknown_field_id_rejected() {
        let raw = compile_payload(5_000, 3, 5, 5.0, 60);
        let mut bad = raw;
        bad.push(9); // 未知 id（>MAX_FIELD）
        bad.extend_from_slice(&[1, 0, 0]);
        assert!(AiPeerConfig::decode(&bad).is_err());
    }

    #[test]
    fn out_of_range_values_rejected_at_decode() {
        // timeout 超 60000。
        assert!(AiPeerConfig::decode(&compile_payload(60_001, 3, 5, 5.0, 60)).is_err());
        // unhealthy_after 0。
        assert!(AiPeerConfig::decode(&compile_payload(5_000, 0, 5, 5.0, 60)).is_err());
        // power 非有限。
        assert!(AiPeerConfig::decode(&compile_payload(5_000, 3, 5, f64::NAN, 60)).is_err());
        // power 超上限。
        assert!(AiPeerConfig::decode(&compile_payload(5_000, 3, 5, 1001.0, 60)).is_err());
        // ttl 0。
        assert!(AiPeerConfig::decode(&compile_payload(5_000, 3, 5, 5.0, 0)).is_err());
    }
}
