//! 协议配置（46 WP-46.3 配置驱动装配）。
//!
//! 7 个协议服务的 ACTIVE 配置源：`configs/protocol-<proto>.toml` —— 扁键值
//! 表（peer/unit-id/IOA 公共地址/topic 前缀/超时/重试；键集 = 各插件
//! `validate_config` 的 KNOWN_KEYS，单一事实源在本模块）。编译面产出
//! canonical KV 负载（进签名 RuntimeConfigBundle），svc-* 经 svc-config
//! `GET_ENTRY("protocol-<proto>")` 取回后 `decode_protocol` 还原成
//! `ProtocolConfig` 喂插件 `start`——**消灭一切源码 peer 常量**（46.3 DoD）。
//!
//! canonical 布局（自描述、无 extern 序列化依赖）：
//! ```text
//! [u8 count]  N×([u8 key_len][key..][u16 val_len][val..])
//! ```
//! 上限：count≤32、key≤64、val≤256、总长≤1024（超限即拒绝，fail-closed）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_protocol_abstract::ProtocolConfig;

use crate::ConfigError;

/// 7 个协议配置 ID（前缀 `protocol-`，与配置文件名一一对应）。
pub const PROTOCOL_CONFIG_IDS: &[&str] = &[
    "protocol-modbus-tcp",
    "protocol-modbus-rtu",
    "protocol-iec104",
    "protocol-mqtt",
    "protocol-mms",
    "protocol-goose",
    "protocol-sv",
    // 48 卷 ACTIVE 协议（能力壳；49 卷修齐 typed compiler 键表登记——
    // config compile 对全部 ACTIVE 配置要求可编译，缺登记即构建链阻断）。
    "protocol-dlt645",
    "protocol-iec101",
    "protocol-iec103",
    "protocol-gbt27930",
];

/// 每协议已知键（= 插件 validate_config 的 KNOWN_KEYS；未知键拒绝）。
pub fn known_keys(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        "protocol-modbus-tcp" => &["peer", "unit_id", "timeout_ms", "retries"],
        // SPEC-D WP-D.2：+ baud/parity/clock_hz（串口链路参数；与
        // modbus-rtu 插件 KNOWN_KEYS 同波次双源同步——两侧字面量锁定测试
        // 互为漂移闸，教训同 B 卷 F-M2）。消费面：svc-modbus-rtu 装配
        // SerialTransportFactory/Pl011UartHw 时解析注入。
        "protocol-modbus-rtu" => &[
            "slave_id",
            "baud",
            "parity",
            "clock_hz",
            "timeout_ms",
            "retries",
        ],
        "protocol-iec104" => &[
            "peer",
            "t3_timeout_ms",
            "t1_timeout_ms",
            "poll_interval_ms",
            "clock_sync_interval_ms",
        ],
        "protocol-mqtt" => &[
            "broker",
            "client_id",
            "keep_alive_ms",
            "control_topic",
            // 49（WP-49.2）：远程 dual-brain 传输面主题（可选）。
            "ai_request_topic",
            "ai_response_topic",
            // 50（WP-50.1 exporter 真通道）：遥测批量帧发布主题（可选）。
            "telemetry_topic",
            // 50（WP-50.2 mgmt 通道）：管理请求订阅 / 应答发布主题（可选）。
            "mgmt_request_topic",
            "mgmt_response_topic",
            // SPEC-B WP-B.2：CONNECT 凭据管道（可选；与 mqtt 插件 KNOWN_KEYS
            // 同波次双源同步——漏一处会被编译链未知键拒绝整包拒载）。
            "username",
            "password",
        ],
        // 48（WP-48.1..48.4）四协议键表（= 各 crate validate_config 已知键；
        // 49 修齐——与 configs/protocol-*.toml 一致，未知键拒绝语义不变）。
        "protocol-dlt645" => &["addr_bcd", "baud", "timeout_ms", "retries"],
        "protocol-iec101" => &[
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
        ],
        "protocol-iec103" => &["common_addr", "timeout_ms", "retries"],
        "protocol-gbt27930" => &[
            "charger_addr",
            "bms_addr",
            "handshake_timeout_ms",
            "param_timeout_ms",
            "charging_timeout_ms",
            "end_timeout_ms",
            "retransmit_max",
        ],
        "protocol-mms" => &["peer", "timeout_ms", "ap_title"],
        "protocol-goose" => &[
            "app_id",
            "conf_rev",
            "time_allowed_to_live_ms",
            "min_time_ms",
            "go_cb_ref",
            "dataset_ref",
            "dst_mac",
        ],
        "protocol-sv" => &["app_id", "dst_mac", "buffer_size"],
        _ => return None,
    })
}

/// 每协议必填键（插件 start 消费 `.ok_or`；缺失 → 编译期拒绝）。
pub fn required_keys(id: &str) -> &'static [&'static str] {
    match id {
        "protocol-modbus-tcp" => &["peer"],
        "protocol-iec104" => &["peer"],
        "protocol-mqtt" => &["broker"],
        "protocol-mms" => &["peer"],
        "protocol-goose" => &["go_cb_ref", "dataset_ref"],
        "protocol-sv" => &["app_id"],
        _ => &[],
    }
}

/// 编码 KV 列表为 canonical 负载（bounded；超限 Err）。
pub fn encode_protocol(cfg: &ProtocolConfig) -> Result<Vec<u8>, ConfigError> {
    if cfg.entries.len() > 32 {
        return Err(ConfigError::InvalidValue {
            config: crate::ConfigId::from_static("protocol"),
            field: "entries",
            reason: format!("count {} > 32", cfg.entries.len()),
        });
    }
    let mut out = Vec::new();
    out.push(cfg.entries.len() as u8);
    for (k, v) in &cfg.entries {
        if k.len() > 64 || v.len() > 256 {
            return Err(ConfigError::InvalidValue {
                config: crate::ConfigId::from_static("protocol"),
                field: "entry",
                reason: "key/val 超长".into(),
            });
        }
        out.push(k.len() as u8);
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(&(v.len() as u16).to_le_bytes());
        out.extend_from_slice(v.as_bytes());
    }
    if out.len() > 1024 {
        return Err(ConfigError::InvalidValue {
            config: crate::ConfigId::from_static("protocol"),
            field: "payload",
            reason: format!("total {} > 1024", out.len()),
        });
    }
    Ok(out)
}

/// 解码 canonical 负载为 `ProtocolConfig`（截断/越界 → Err，fail-closed）。
pub fn decode_protocol(payload: &[u8]) -> Result<ProtocolConfig, ConfigError> {
    let cfg_id = crate::ConfigId::from_static("protocol");
    let Some(&count) = payload.first() else {
        return Err(ConfigError::InvalidValue {
            config: cfg_id.clone(),
            field: "payload",
            reason: "empty".into(),
        });
    };
    if count as usize > 32 {
        return Err(ConfigError::InvalidValue {
            config: cfg_id.clone(),
            field: "count",
            reason: format!("{count} > 32"),
        });
    }
    let mut cfg = ProtocolConfig::default();
    let mut pos = 1usize;
    for _ in 0..count {
        let Some(&klen) = payload.get(pos) else {
            return Err(ConfigError::InvalidValue {
                config: cfg_id.clone(),
                field: "payload",
                reason: "truncated key_len".into(),
            });
        };
        pos += 1;
        let kend = pos
            .checked_add(klen as usize)
            .ok_or(ConfigError::InvalidValue {
                config: cfg_id.clone(),
                field: "payload",
                reason: "key overflow".into(),
            })?;
        let key = payload.get(pos..kend).ok_or(ConfigError::InvalidValue {
            config: cfg_id.clone(),
            field: "payload",
            reason: "truncated key".into(),
        })?;
        pos = kend;
        let vend = pos.checked_add(2).ok_or(ConfigError::InvalidValue {
            config: cfg_id.clone(),
            field: "payload",
            reason: "vlen overflow".into(),
        })?;
        let vlen = u16::from_le_bytes(
            payload
                .get(pos..vend)
                .ok_or(ConfigError::InvalidValue {
                    config: cfg_id.clone(),
                    field: "payload",
                    reason: "truncated vlen".into(),
                })?
                .try_into()
                .map_err(|_| ConfigError::InvalidValue {
                    config: cfg_id.clone(),
                    field: "payload",
                    reason: "vlen width".into(),
                })?,
        ) as usize;
        pos = vend;
        let vv = payload
            .get(pos..pos + vlen)
            .ok_or(ConfigError::InvalidValue {
                config: cfg_id.clone(),
                field: "payload",
                reason: "truncated val".into(),
            })?;
        pos += vlen;
        cfg.insert(
            core::str::from_utf8(key).map_err(|_| ConfigError::InvalidValue {
                config: cfg_id.clone(),
                field: "payload",
                reason: "key not utf8".into(),
            })?,
            core::str::from_utf8(vv).map_err(|_| ConfigError::InvalidValue {
                config: cfg_id.clone(),
                field: "payload",
                reason: "val not utf8".into(),
            })?,
        );
    }
    if pos != payload.len() {
        return Err(ConfigError::InvalidValue {
            config: cfg_id.clone(),
            field: "payload",
            reason: "trailing bytes".into(),
        });
    }
    Ok(cfg)
}

/// KV 迭代器（编译面用；保持键序）。
pub fn iter_key_values(id: &str, cfg: &ProtocolConfig) -> alloc::vec::Vec<(String, String)> {
    let _ = id;
    cfg.entries.clone()
}

/// 46（WP-46.3）：协议配置 typed consumer（每协议一份实例；注册进
/// svc-config ConfigRegistry——registry 双向完备性要求 bundle 内每个
/// 配置都有 consumer，否则 fail-closed `unconsumed-config`）。
pub struct ProtocolKvConsumer {
    id: &'static str,
    /// 最近应用的协议配置（诊断/校验保持）。
    pub last: ProtocolConfig,
    /// 生效 generation。
    pub generation: u64,
}

impl ProtocolKvConsumer {
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            last: ProtocolConfig::default(),
            generation: 0,
        }
    }
}

impl crate::TypedConfigConsumer for ProtocolKvConsumer {
    type Config = ProtocolConfig;
    fn config_id(&self) -> crate::ConfigId {
        crate::ConfigId::from_static(self.id)
    }
    fn schema_version(&self) -> crate::SchemaVersion {
        crate::SchemaVersion(1)
    }
    fn decode_validate(&self, raw: &[u8]) -> Result<Self::Config, crate::ConfigError> {
        decode_protocol(raw)
    }
    fn apply(
        &mut self,
        cfg: Self::Config,
        generation: u64,
    ) -> Result<crate::ApplyOutcome, crate::ConfigError> {
        if self.last.entries == cfg.entries && self.generation == generation {
            return Ok(crate::ApplyOutcome::NoOp);
        }
        self.last = cfg;
        self.generation = generation;
        Ok(crate::ApplyOutcome::Applied { generation })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TypedConfigConsumer;

    #[test]
    fn encode_decode_roundtrip_all_fields() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "10.0.2.2:2404");
        cfg.insert("t3_timeout_ms", "20000");
        cfg.insert("poll_interval_ms", "30000");
        let bytes = encode_protocol(&cfg).expect("encode");
        let back = decode_protocol(&bytes).expect("decode");
        assert_eq!(back.get("peer"), Some("10.0.2.2:2404"));
        assert_eq!(back.get("t3_timeout_ms"), Some("20000"));
        assert_eq!(back.get("poll_interval_ms"), Some("30000"));
        assert_eq!(back.get("nope"), None);
    }

    #[test]
    fn decode_rejects_truncated_and_trailing() {
        assert!(decode_protocol(&[]).is_err());
        // 声明的 mark count=1 但无条目 → 截断。
        assert!(decode_protocol(&[1]).is_err());
        // 尾部垃圾字节拒绝。
        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "x");
        let mut bytes = encode_protocol(&cfg).expect("encode");
        bytes.push(0);
        assert!(decode_protocol(&bytes).is_err());
    }

    #[test]
    fn known_keys_cover_all_protocol_ids() {
        for id in PROTOCOL_CONFIG_IDS {
            assert!(known_keys(id).is_some(), "缺失 known_keys: {id}");
        }
        assert!(known_keys("protocol-unknown").is_none());
        assert_eq!(
            required_keys("protocol-goose"),
            &["go_cb_ref", "dataset_ref"]
        );
    }

    /// SPEC-B WP-B.2/C2 键表锁定：protocol-mqtt 已知键全清单对照字面量。
    /// 与 crates/protocols/mqtt/src/plugin.rs KNOWN_KEYS 双源——任一侧
    /// 增删键必须两侧同波次同步，本测试与插件侧 b2_known_keys_literal_lock
    /// 互为漂移闸（漏同步 = 编译链未知键拒绝整包拒载）。
    #[test]
    fn b2_mqtt_known_keys_literal_lock_includes_credentials() {
        const EXPECTED: &[&str] = &[
            "broker",
            "client_id",
            "keep_alive_ms",
            "control_topic",
            "ai_request_topic",
            "ai_response_topic",
            "telemetry_topic",
            "mgmt_request_topic",
            "mgmt_response_topic",
            "username",
            "password",
        ];
        assert_eq!(
            known_keys("protocol-mqtt"),
            Some(EXPECTED),
            "config-bundle 侧 mqtt 键表漂移——须与 mqtt 插件 KNOWN_KEYS 同步"
        );
    }

    /// SPEC-D WP-D.2 键表锁定：protocol-modbus-rtu 已知键全清单对照字面量。
    /// 与 crates/protocols/modbus-rtu/src/plugin.rs `KNOWN_KEYS` 双源——任一
    /// 侧增删键必须两侧同波次同步，本测试与插件侧 d_known_keys_literal_lock
    /// 互为漂移闸（漏同步 = 编译链未知键拒绝整包拒载）。
    #[test]
    fn d_rtu_known_keys_literal_lock() {
        const EXPECTED: &[&str] = &[
            "slave_id",
            "baud",
            "parity",
            "clock_hz",
            "timeout_ms",
            "retries",
        ];
        assert_eq!(
            known_keys("protocol-modbus-rtu"),
            Some(EXPECTED),
            "config-bundle 侧 rtu 键表漂移——须与 modbus-rtu 插件 KNOWN_KEYS 同步"
        );
    }

    #[test]
    fn encode_rejects_oversized() {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("big", "a".repeat(300).as_str());
        assert!(encode_protocol(&cfg).is_err());
    }

    #[test]
    fn protocol_kv_consumer_apply_roundtrip_and_noop() {
        // 46.3：每协议 one consumer（registry 双向完备性）；decode_validate →
        // apply → 同 generation+同内容 = NoOp；内容变 → Applied。
        let mut c = ProtocolKvConsumer::new("protocol-iec104");
        assert_eq!(
            c.config_id(),
            crate::ConfigId::from_static("protocol-iec104")
        );
        assert_eq!(c.schema_version(), crate::SchemaVersion(1));

        let mut cfg = ProtocolConfig::default();
        cfg.insert("peer", "10.0.2.2:2404");
        cfg.insert("t1_timeout_ms", "15000");
        let raw = encode_protocol(&cfg).expect("encode");
        let decoded = c.decode_validate(&raw).expect("decode/validate");
        assert_eq!(decoded.get("peer"), Some("10.0.2.2:2404"));

        let out = c.apply(decoded, 7).expect("apply");
        assert_eq!(out, crate::ApplyOutcome::Applied { generation: 7 });
        assert_eq!(c.last.get("peer"), Some("10.0.2.2:2404"));
        assert_eq!(c.generation, 7);

        // 同 generation 同内容重放 → NoOp（不重复计数）。
        let out = c.apply(c.last.clone(), 7).expect("reapply");
        assert_eq!(out, crate::ApplyOutcome::NoOp);

        // 内容变化 → 新 generation Applied。
        let mut cfg2 = ProtocolConfig::default();
        cfg2.insert("peer", "10.0.2.2:2405");
        let raw2 = encode_protocol(&cfg2).expect("encode");
        let out = c
            .apply(c.decode_validate(&raw2).expect("decode2"), 8)
            .expect("apply2");
        assert_eq!(out, crate::ApplyOutcome::Applied { generation: 8 });
        assert_eq!(c.last.get("peer"), Some("10.0.2.2:2405"));
    }

    #[test]
    fn protocol_kv_consumer_rejects_garbage_payload() {
        let c = ProtocolKvConsumer::new("protocol-modbus-tcp");
        assert!(c.decode_validate(b"\xff\xff\xff").is_err());
        assert!(c.decode_validate(b"").is_err());
    }
}
