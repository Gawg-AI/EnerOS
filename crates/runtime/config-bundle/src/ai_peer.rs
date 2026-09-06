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
    /// 68.5 WP-68.5.3：D3 降级 lease HMAC 键（KMS 注入链供给，bundle 载荷
    /// 32B；svc-ai-worker 自签有界 lease 的验签钥）。
    pub degrade_lease_key: [u8; 32],
    /// D3 降级 lease 有效时长秒（到期不复位即拒降级维持——D3 fail-closed）。
    pub degrade_lease_ttl_secs: u32,
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
            degrade_lease_key: [0u8; 32],
            degrade_lease_ttl_secs: 0,
        };
        let mut seen = [false; 9];
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
                crate::schema::ai_peer::FIELD_DEGRADE_LEASE_KEY => {
                    if value.len() != 32 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "degrade_lease_key",
                            reason: "len".into(),
                        });
                    }
                    if value.iter().all(|&b| b == 0) {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "degrade_lease_key",
                            reason: "all-zero".into(),
                        });
                    }
                    out.degrade_lease_key = value.try_into().expect("checked len");
                    seen[id as usize] = true;
                }
                crate::schema::ai_peer::FIELD_DEGRADE_LEASE_TTL_SECS => {
                    if value.len() != 4 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "degrade_lease_ttl_secs",
                            reason: "len".into(),
                        });
                    }
                    out.degrade_lease_ttl_secs =
                        u32::from_le_bytes(value.try_into().expect("checked len"));
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
        if !(DEGRADE_LEASE_TTL_SECS_MIN..=DEGRADE_LEASE_TTL_SECS_MAX)
            .contains(&out.degrade_lease_ttl_secs)
        {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "degrade_lease_ttl_secs",
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

    /// 测试键（32B 非零；与 configs/kms-dev/degrade-lease.key 开发档模板
    /// 同形态——值域复检只拒全零）。
    const TEST_LEASE_KEY: [u8; 32] = [0x5B; 32];

    fn compile_payload(
        timeout_ms: u64,
        unhealthy_after: u32,
        probe_every: u32,
        max_power_kw: f64,
        ttl_secs: u32,
    ) -> alloc::vec::Vec<u8> {
        compile_payload_full(
            timeout_ms,
            unhealthy_after,
            probe_every,
            max_power_kw,
            ttl_secs,
            3600,
        )
    }

    fn compile_payload_full(
        timeout_ms: u64,
        unhealthy_after: u32,
        probe_every: u32,
        max_power_kw: f64,
        ttl_secs: u32,
        lease_ttl: u32,
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
            (
                crate::schema::ai_peer::FIELD_DEGRADE_LEASE_KEY,
                TEST_LEASE_KEY.to_vec(),
            ),
            (
                crate::schema::ai_peer::FIELD_DEGRADE_LEASE_TTL_SECS,
                lease_ttl.to_le_bytes().to_vec(),
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
                learn_enabled: true,
                degrade_lease_key: TEST_LEASE_KEY,
                degrade_lease_ttl_secs: 3600,
            }
        );
    }

    /// 68.5 WP-68.5.3：lease 键全零拒绝 + TTL 值域（[60, 86400]）。
    #[test]
    fn degrade_lease_fields_validated() {
        let mut zero = compile_payload(5_000, 3, 5, 5.0, 60);
        // 键字段在 ttl_secs 之前：定位 learn_enabled(1B)+key(32B) 尾段改零。
        let n = zero.len();
        for b in &mut zero[n - 32 - 1 - 4..n - 1 - 4] {
            *b = 0;
        }
        assert!(AiPeerConfig::decode(&zero).is_err(), "全零键必须拒绝");
        // TTL 越界（<60 / >86400）拒绝。
        assert!(AiPeerConfig::decode(&compile_payload_full(5_000, 3, 5, 5.0, 60, 59)).is_err());
        assert!(AiPeerConfig::decode(&compile_payload_full(5_000, 3, 5, 5.0, 60, 86_401)).is_err());
    }

    /// SPEC-E WP-E.3：learn_enabled 0/1 语义 + 非 0/1 拒绝 + 缺键拒绝。
    #[test]
    fn learn_enabled_gate_semantics() {
        let mk = |v: u8| {
            encode_payload(&[(crate::schema::ai_peer::FIELD_LEARN_ENABLED, alloc::vec![v])])
        };
        // 单独字段必然缺其余七键 → MissingField（全必配纪律不受新键稀释）。
        assert!(AiPeerConfig::decode(&mk(0)).is_err());
        // 完整载荷 learn_enabled=false（字段 = id1+len2+value；learn_enabled
        // 值字节后跟 key(35B)+ttl(7B) 两个完整字段 ⇒ 索引 n−43）。
        let raw_off = compile_payload(5_000, 3, 5, 5.0, 60);
        let mut patched = raw_off.clone();
        let n = patched.len();
        patched[n - 43] = 0;
        let cfg = AiPeerConfig::decode(&patched).expect("decode off");
        assert!(!cfg.learn_enabled);
        // 非 0/1 拒绝。
        let mut bad = compile_payload(5_000, 3, 5, 5.0, 60);
        let n = bad.len();
        bad[n - 43] = 2;
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
