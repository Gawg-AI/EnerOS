//! 49（WP-49.2 远程 dual-brain）：`ai-peer` 类型化解码（svc-ai-worker 装配
//! 侧消费——RemoteBrain 状态机参数）。编码面 = toml_compile；字段 id 单一
//! 事实在 `schema::ai_peer`。全部字段必配（编译侧强制），解码侧值域复检。

use crate::payload::decode_fields;
use crate::{ConfigError, ConfigId};

/// 解码后的 ai-peer 配置。
#[derive(Debug, Clone, PartialEq)]
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
    /// 蓝图 70.10 WP-70.10.2：结构化计划参数（**可选**——缺省 = 无结构化
    /// 输入源，走 v1 迁移轨；有 = 目标侧装配目录派生规格源）。
    pub plan: Option<PlanParams>,
}

/// 结构化计划参数（时域 + 目标引用）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanParams {
    /// 步长 ns（>0）。
    pub step_ns: u64,
    /// 步数（1..=64）。
    pub step_count: u32,
    /// 目标种类：1=min_cost 2=max_revenue。
    pub objective: u8,
    /// 每步电价 元/kWh（len == step_count；有限）。
    pub prices_per_kwh: alloc::vec::Vec<f64>,
}

impl PlanParams {
    /// 目标种类词形（host 契约闭集）。
    pub fn objective_word(&self) -> &'static str {
        if self.objective == 2 {
            "max_revenue"
        } else {
            "min_cost"
        }
    }
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
            plan: None,
        };
        let mut seen = [false; 10];
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
                crate::schema::ai_peer::FIELD_PLAN_PARAMS => {
                    // step_ns(8) + step_count(4) + objective(1) + len(4) + prices(8n)
                    if value.len() < 17 {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "plan_params",
                            reason: "len".into(),
                        });
                    }
                    let step_ns = u64::from_le_bytes(value[0..8].try_into().expect("checked len"));
                    let step_count =
                        u32::from_le_bytes(value[8..12].try_into().expect("checked len"));
                    let objective = value[12];
                    let n = u32::from_le_bytes(value[13..17].try_into().expect("checked len"));
                    let expect = 17usize
                        .checked_add((n as usize).saturating_mul(8))
                        .ok_or_else(|| ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "plan_params",
                            reason: "len-overflow".into(),
                        })?;
                    if value.len() != expect {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "plan_params",
                            reason: "len".into(),
                        });
                    }
                    if step_ns == 0
                        || step_count == 0
                        || step_count > 64
                        || n != step_count
                        || !(objective == 1 || objective == 2)
                    {
                        return Err(ConfigError::InvalidValue {
                            config: config.clone(),
                            field: "plan_params",
                            reason: "range".into(),
                        });
                    }
                    let mut prices_per_kwh = alloc::vec::Vec::with_capacity(n as usize);
                    for i in 0..n as usize {
                        let o = 17 + i * 8;
                        let f =
                            f64::from_le_bytes(value[o..o + 8].try_into().expect("checked len"));
                        if !f.is_finite() {
                            return Err(ConfigError::InvalidValue {
                                config: config.clone(),
                                field: "plan_params",
                                reason: "non-finite-price".into(),
                            });
                        }
                        prices_per_kwh.push(f);
                    }
                    out.plan = Some(PlanParams {
                        step_ns,
                        step_count,
                        objective,
                        prices_per_kwh,
                    });
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
        // 全必配：缺任一字段即拒绝（**字段 9 = 结构化计划参数例外**——
        // 可选：缺省 = 无结构化输入源，走 v1 迁移轨）。
        let optional = crate::schema::ai_peer::FIELD_PLAN_PARAMS as usize;
        for (i, ok) in seen.iter().enumerate() {
            if i == 0 || i == optional {
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
                plan: None,
            }
        );
    }

    /// 蓝图 70.10：结构化计划参数（可选字段）往返 + 值域拒绝。
    /// 缺字段（既有 payload）⇒ `plan = None`（v1 迁移轨，行为不变）。
    #[test]
    fn plan_params_optional_roundtrip_and_range() {
        let base = compile_payload(5_000, 3, 5, 5.0, 60);
        assert!(AiPeerConfig::decode(&base).expect("decode").plan.is_none());
        // 追加字段 9：step_ns 60e9 ‖ steps 2 ‖ objective 2 ‖ len 2 ‖ [0.5, 1.5]
        let mut raw = base.clone();
        let mut v: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        v.extend_from_slice(&60_000_000_000u64.to_le_bytes());
        v.extend_from_slice(&2u32.to_le_bytes());
        v.push(2);
        v.extend_from_slice(&2u32.to_le_bytes());
        v.extend_from_slice(&0.5f64.to_le_bytes());
        v.extend_from_slice(&1.5f64.to_le_bytes());
        raw.push(crate::schema::ai_peer::FIELD_PLAN_PARAMS);
        raw.extend_from_slice(&(v.len() as u16).to_le_bytes());
        raw.extend_from_slice(&v);
        let cfg = AiPeerConfig::decode(&raw).expect("decode with plan");
        let plan = cfg.plan.expect("plan present");
        assert_eq!(plan.step_ns, 60_000_000_000);
        assert_eq!(plan.step_count, 2);
        assert_eq!(plan.objective_word(), "max_revenue");
        assert_eq!(plan.prices_per_kwh, alloc::vec![0.5, 1.5]);
        // 价格长度与步数不符 → 拒。
        let mut bad: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        bad.extend_from_slice(&60_000_000_000u64.to_le_bytes());
        bad.extend_from_slice(&2u32.to_le_bytes());
        bad.push(1);
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.extend_from_slice(&0.5f64.to_le_bytes());
        let mut raw_bad = base.clone();
        raw_bad.push(crate::schema::ai_peer::FIELD_PLAN_PARAMS);
        raw_bad.extend_from_slice(&(bad.len() as u16).to_le_bytes());
        raw_bad.extend_from_slice(&bad);
        assert!(AiPeerConfig::decode(&raw_bad).is_err(), "长度不符必须拒");
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
