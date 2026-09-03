//! 协议配置（46 WP-46.3 配置驱动装配）。
//!
//! 11 个协议的 ACTIVE 配置源（48 卷起）：`configs/protocol-<proto>.toml` —— 扁键值
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

/// 协议配置 ID（前缀 `protocol-`，与配置文件名一一对应；48 卷起 11 个）。
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
///
/// 蓝图 64 WP-64.1.2：键表单源倒置——本函数委托
/// [`eneros_protocol_keys::by_config_id`]（12 协议 const 数组唯一实现，
/// 零依赖叶 crate），编译面与插件面同源消费，双面引用锁防漂移。
/// goose `vlan_id` / mqtt `qos`/`will_topic` 双面漂移（活缺陷，
/// R-64.a 红测实证 + 64.1 事实差异记录）随本收口修复——合法配置
/// 编译面拒收面消失，未知键拒绝语义不变。
///
/// cj188 例外：叶 crate 键表照存（单源无死角），但编译面显式返回
/// `None`——编译支持面扩容 = 行为新增，蓝图 64 §4 不做边界，独立裁决。
pub fn known_keys(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "protocol-cj188" => None,
        _ => eneros_protocol_keys::by_config_id(id),
    }
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

    /// SPEC-B WP-B.2/C2 键表锁定（蓝图 64 WP-64.1.3 迁移为引用锁）：
    /// protocol-mqtt 已知键 = 键表单源 `eneros_protocol_keys::MQTT`。
    /// 历史为 11 键字面锁（与插件面 13 键漂移实证——64.1 事实差异记录），
    /// 迁移 diff 见执行卡；键集合把守 = 本引用锁 + 叶 crate 自测
    /// （字面量只在叶 crate 一处）。
    #[test]
    fn b2_mqtt_known_keys_reference_lock() {
        assert_eq!(
            known_keys("protocol-mqtt"),
            Some(eneros_protocol_keys::MQTT),
            "config-bundle 侧 mqtt 键表须引用单源（eneros-protocol-keys）"
        );
    }

    /// SPEC-D WP-D.2 键表锁定（蓝图 64 WP-64.1.3 迁移为引用锁）：
    /// protocol-modbus-rtu 已知键 = 键表单源 `eneros_protocol_keys::MODBUS_RTU`。
    /// 迁移前字面锁（6 键全清单）与迁移后键集合一致（diff 见执行卡）。
    #[test]
    fn d_rtu_known_keys_reference_lock() {
        assert_eq!(
            known_keys("protocol-modbus-rtu"),
            Some(eneros_protocol_keys::MODBUS_RTU),
            "config-bundle 侧 rtu 键表须引用单源（eneros-protocol-keys）"
        );
    }

    // ========== 蓝图 64 WP-64.1.3 双面引用锁（config-bundle 侧 ×9） ==========
    // 语义：known_keys() 委托接线不裂 + 叶 crate 映射不漂（任何一侧回到
    // 硬编码或改叶内容必红）。插件侧对照锁在各协议 crate（双面合计 22 道）。

    #[test]
    fn r64_ref_lock_modbus_tcp() {
        assert_eq!(
            known_keys("protocol-modbus-tcp"),
            Some(eneros_protocol_keys::MODBUS_TCP)
        );
    }

    #[test]
    fn r64_ref_lock_iec104() {
        assert_eq!(
            known_keys("protocol-iec104"),
            Some(eneros_protocol_keys::IEC104_MASTER)
        );
    }

    #[test]
    fn r64_ref_lock_goose() {
        assert_eq!(
            known_keys("protocol-goose"),
            Some(eneros_protocol_keys::GOOSE)
        );
    }

    #[test]
    fn r64_ref_lock_sv() {
        assert_eq!(known_keys("protocol-sv"), Some(eneros_protocol_keys::SV));
    }

    #[test]
    fn r64_ref_lock_mms() {
        assert_eq!(known_keys("protocol-mms"), Some(eneros_protocol_keys::MMS));
    }

    #[test]
    fn r64_ref_lock_dlt645() {
        assert_eq!(
            known_keys("protocol-dlt645"),
            Some(eneros_protocol_keys::DLT645)
        );
    }

    #[test]
    fn r64_ref_lock_gbt27930() {
        assert_eq!(
            known_keys("protocol-gbt27930"),
            Some(eneros_protocol_keys::GBT27930)
        );
    }

    #[test]
    fn r64_ref_lock_iec101() {
        assert_eq!(
            known_keys("protocol-iec101"),
            Some(eneros_protocol_keys::IEC101)
        );
    }

    #[test]
    fn r64_ref_lock_iec103() {
        assert_eq!(
            known_keys("protocol-iec103"),
            Some(eneros_protocol_keys::IEC103)
        );
    }

    /// cj188 编译面例外锁（§4 不做边界）：叶 crate 键表照存，但编译面
    /// 显式 None（编译支持面扩容 = 行为新增，独立裁决）。
    #[test]
    fn r64_ref_lock_cj188_compile_face_stays_none() {
        assert_eq!(known_keys("protocol-cj188"), None);
        assert!(
            eneros_protocol_keys::CJ188.contains(&"addr_bcd"),
            "叶键表照存（单源无死角）"
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

    /// 蓝图 64 R-64.a 红测改写（WP-64.1.2 修复后形态）：goose `vlan_id`
    /// （插件真消费键：1..=4094 → TX 802.1Q 标签）编译通过且条目进
    /// canonical payload；真未知键仍拒（未知键拒绝语义不变）。
    /// 修复前形态（编译面拒 vlan_id，[01] §3 实证）见执行卡记录。
    /// （compile 门内测试：走 `toml_compile` 编译面。）
    #[cfg(feature = "compile")]
    #[test]
    fn r64_goose_vlan_id_now_compiles() {
        let with_vlan = r#"
app_id = 1
time_allowed_to_live_ms = 1000
go_cb_ref = "GOOSE1"
dataset_ref = "DS1"
vlan_id = 100
"#;
        let compiled = crate::toml_compile::compile_protocol("protocol-goose", with_vlan)
            .expect("修复后：vlan_id ∈ 键表单源（eneros-protocol-keys::GOOSE），编译必须通过");
        let decoded = decode_protocol(&compiled.payload).expect("decode");
        assert_eq!(decoded.get("vlan_id"), Some("100"), "vlan_id 条目须进产物");
        // 负例对照：真未知键仍被拒且拒因点名。
        let with_bogus = r#"
app_id = 1
time_allowed_to_live_ms = 1000
go_cb_ref = "GOOSE1"
dataset_ref = "DS1"
bogus_key = "x"
"#;
        let err = crate::toml_compile::compile_protocol("protocol-goose", with_bogus)
            .expect_err("未知键必须整包拒");
        assert!(
            format!("{err:?}").contains("bogus_key"),
            "拒因须点名：{err:?}"
        );
    }

    /// 蓝图 64 事实差异修复形态：mqtt `qos`/`will_topic`（插件面已声明
    /// 可选键）编译通过进产物；真未知键仍拒。（compile 门内测试。）
    #[cfg(feature = "compile")]
    #[test]
    fn r64_mqtt_optional_keys_now_compiles() {
        let with_qos = r#"
broker = "tcp://10.0.2.2:1883"
qos = "1"
will_topic = "will/x"
"#;
        let compiled = crate::toml_compile::compile_protocol("protocol-mqtt", with_qos)
            .expect("修复后：qos/will_topic ∈ 键表单源，编译必须通过");
        let decoded = decode_protocol(&compiled.payload).expect("decode");
        assert_eq!(decoded.get("qos"), Some("1"));
        assert_eq!(decoded.get("will_topic"), Some("will/x"));
        let err = crate::toml_compile::compile_protocol(
            "protocol-mqtt",
            "broker = \"tcp://10.0.2.2:1883\"\nbogus_key = \"x\"\n",
        )
        .expect_err("未知键必须整包拒");
        assert!(
            format!("{err:?}").contains("bogus_key"),
            "拒因须点名：{err:?}"
        );
    }
}
