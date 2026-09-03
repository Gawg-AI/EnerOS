//! 蓝图 64 WP-64.1.1（v0.317.0）：协议配置键表**单一实现**。
//!
//! 历史：键表在编译面（config-bundle `protocol.rs known_keys`）与运行面
//! （12 份协议插件私有 `KNOWN_KEYS`）双源镜像维护，已实证漂移——goose
//! `vlan_id` 插件收（消费于 802.1Q 标签）、编译拒（R-64.a/b 红测实证，
//! 同一合法配置两面裁决相反）；mqtt `qos`/`will_topic` 同类漂移（64.1
//! 开工事实差异记录：插件面 13 键、编译面 11 键）。本 crate 把键表倒置为
//! **插件域单源、双面消费**：config-bundle `known_keys()` 与全部插件
//! `KNOWN_KEYS` 都只引用本 crate 常量，双面引用锁（22 道）防断裂。
//!
//! 键序裁决（执行卡留证）：modbus-tcp 取 config-bundle 旧序（键序仅影响
//! reject 报错列举序，无语义）；modbus-rtu 双面同序；mqtt/goose/iec104
//! 等取插件面现序（mqtt 插件面 13 键序是唯一完整现序；goose 以插件实际
//! 消费为准——`vlan_id` 属真消费键）。
//!
//! 本 crate 刻意**零依赖**（对齐 `eneros-dispatch-state` 叶先例，R-64.a）：
//! 只放 `pub const` 键数组与 `by_config_id` 映射，零可执行逻辑——
//! config-bundle 经 root `default-features = false` 进 target 闭包，const
//! 切片进 rodata，零运行时开销（闭包审计见执行卡）。cj188 键表照存
//! （单源无死角），但编译面消费由 config-bundle 显式返回 `None`
//! （编译支持面扩容 = 行为新增，登记 §4 不做边界）。

#![cfg_attr(not(test), no_std)]

/// Modbus TCP 已知键（编译面旧序；插件面旧序 retries/timeout_ms 互换）。
pub const MODBUS_TCP: &[&str] = &["peer", "unit_id", "timeout_ms", "retries"];

/// Modbus RTU 已知键（SPEC-D WP-D.2：串口链路参数；双面同序）。
pub const MODBUS_RTU: &[&str] = &[
    "slave_id",
    "baud",
    "parity",
    "clock_hz",
    "timeout_ms",
    "retries",
];

/// IEC 60870-5-104 主站已知键（插件面现序）。
pub const IEC104_MASTER: &[&str] = &[
    "peer",
    "clock_sync_interval_ms",
    "t3_timeout_ms",
    "t1_timeout_ms",
    "poll_interval_ms",
];

/// MQTT 已知键（插件面 13 键现序——含 `qos`/`will_topic` 已声明键与
/// SPEC-B WP-B.2 凭据管道；编译面旧表 11 键属漂移滞后，本表为准）。
pub const MQTT: &[&str] = &[
    "broker",
    "client_id",
    "keep_alive_ms",
    "qos",
    "will_topic",
    "control_topic",
    "ai_request_topic",
    "ai_response_topic",
    "telemetry_topic",
    "mgmt_request_topic",
    "mgmt_response_topic",
    "username",
    "password",
];

/// IEC 61850 GOOSE 已知键（含 `vlan_id`——插件真消费键：1..=4094 → TX
/// 802.1Q 标签；本卷活缺陷修复的核心断言，叶自测锁死）。
pub const GOOSE: &[&str] = &[
    "app_id",
    "conf_rev",
    "time_allowed_to_live_ms",
    "min_time_ms",
    "go_cb_ref",
    "dataset_ref",
    "dst_mac",
    "vlan_id",
];

/// IEC 61850-9-2 SV 已知键。
pub const SV: &[&str] = &["app_id", "dst_mac", "buffer_size"];

/// IEC 61850 MMS 客户端已知键。
pub const MMS: &[&str] = &["peer", "timeout_ms", "ap_title"];

/// DL/T 645 已知键（WP-39.10 不变量）。
pub const DLT645: &[&str] = &["addr_bcd", "baud", "timeout_ms", "retries"];

/// GB/T 27930 已知键。
pub const GBT27930: &[&str] = &[
    "charger_addr",
    "bms_addr",
    "handshake_timeout_ms",
    "param_timeout_ms",
    "charging_timeout_ms",
    "end_timeout_ms",
    "retransmit_max",
];

/// IEC 60870-5-101 已知键。
pub const IEC101: &[&str] = &[
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

/// IEC 60870-5-103 已知键。
pub const IEC103: &[&str] = &["common_addr", "timeout_ms", "retries"];

/// CJ/T 188 已知键（编译面暂不消费——config-bundle 显式 None，见 crate doc）。
pub const CJ188: &[&str] = &["addr_bcd", "baud", "timeout_ms", "retries"];

/// 按 config_id（`protocol-<proto>`）取键表；未登记返回 `None`。
///
/// 注意：`protocol-cj188` 在此返回 `Some`（单源无死角），但 config-bundle
/// 编译面对其显式返回 `None`（编译支持面扩容属行为新增，独立裁决）。
pub fn by_config_id(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        "protocol-modbus-tcp" => MODBUS_TCP,
        "protocol-modbus-rtu" => MODBUS_RTU,
        "protocol-iec104" => IEC104_MASTER,
        "protocol-mqtt" => MQTT,
        "protocol-goose" => GOOSE,
        "protocol-sv" => SV,
        "protocol-mms" => MMS,
        "protocol-dlt645" => DLT645,
        "protocol-gbt27930" => GBT27930,
        "protocol-iec101" => IEC101,
        "protocol-iec103" => IEC103,
        "protocol-cj188" => CJ188,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全部键表注册进 by_config_id（映射与常量一一对应，未登记 ID → None）。
    #[test]
    fn by_config_id_covers_every_array() {
        let table: &[(&str, &[&str])] = &[
            ("protocol-modbus-tcp", MODBUS_TCP),
            ("protocol-modbus-rtu", MODBUS_RTU),
            ("protocol-iec104", IEC104_MASTER),
            ("protocol-mqtt", MQTT),
            ("protocol-goose", GOOSE),
            ("protocol-sv", SV),
            ("protocol-mms", MMS),
            ("protocol-dlt645", DLT645),
            ("protocol-gbt27930", GBT27930),
            ("protocol-iec101", IEC101),
            ("protocol-iec103", IEC103),
            ("protocol-cj188", CJ188),
        ];
        for (id, keys) in table {
            assert_eq!(by_config_id(id), Some(*keys), "映射失配：{id}");
        }
        assert_eq!(by_config_id("protocol-unknown"), None);
        assert_eq!(by_config_id(""), None);
    }

    /// 每表非空、键名语法 `[a-z0-9_]+`、无重复（改键必经本测）。
    #[test]
    fn arrays_wellformed_no_duplicates() {
        let all: &[(&str, &[&str])] = &[
            ("MODBUS_TCP", MODBUS_TCP),
            ("MODBUS_RTU", MODBUS_RTU),
            ("IEC104_MASTER", IEC104_MASTER),
            ("MQTT", MQTT),
            ("GOOSE", GOOSE),
            ("SV", SV),
            ("MMS", MMS),
            ("DLT645", DLT645),
            ("GBT27930", GBT27930),
            ("IEC101", IEC101),
            ("IEC103", IEC103),
            ("CJ188", CJ188),
        ];
        for (name, keys) in all {
            assert!(!keys.is_empty(), "{name} 空表");
            for k in *keys {
                assert!(
                    !k.is_empty()
                        && k.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                    "{name} 键名语法失范：{k:?}"
                );
            }
            for i in 0..keys.len() {
                for j in i + 1..keys.len() {
                    assert_ne!(keys[i], keys[j], "{name} 重复键：{}", keys[i]);
                }
            }
        }
    }

    /// 本卷核心断言（WP-64.1.2 活缺陷修复依赖）：GOOSE 含 vlan_id——
    /// 插件实际消费（802.1Q 标签）必须能在编译面通过。
    #[test]
    fn goose_contains_vlan_id() {
        assert!(GOOSE.contains(&"vlan_id"), "GOOSE 键表必须含 vlan_id");
    }

    /// 事实差异修复锚点：MQTT 含 qos/will_topic（插件面已声明键，编译面
    /// 旧表漏收 = 活漂移；64.1 开工事实差异记录）。
    #[test]
    fn mqtt_contains_plugin_declared_optional_keys() {
        assert!(MQTT.contains(&"qos"), "MQTT 键表必须含 qos");
        assert!(MQTT.contains(&"will_topic"), "MQTT 键表必须含 will_topic");
    }

    /// 叶内容字面锁（WP-64.1.3 变异验证实证补齐）：引用锁两侧同源，天然
    /// 测不出叶内容被改——键集合的**内容**只能由本测（全仓唯一字面处 =
    /// 本 crate 生产代码 + 本测的对照字面）把守。改任何键名/增删键必红。
    /// （变异验证 2026-09-03：改 MQTT broker→broker_mut，补锁前全绿 =
    /// 守卫缺口实证；补锁后必红。）
    #[test]
    fn content_locked_literals() {
        assert_eq!(MODBUS_TCP, &["peer", "unit_id", "timeout_ms", "retries"]);
        assert_eq!(
            MODBUS_RTU,
            &[
                "slave_id",
                "baud",
                "parity",
                "clock_hz",
                "timeout_ms",
                "retries"
            ]
        );
        assert_eq!(
            IEC104_MASTER,
            &[
                "peer",
                "clock_sync_interval_ms",
                "t3_timeout_ms",
                "t1_timeout_ms",
                "poll_interval_ms",
            ]
        );
        assert_eq!(
            MQTT,
            &[
                "broker",
                "client_id",
                "keep_alive_ms",
                "qos",
                "will_topic",
                "control_topic",
                "ai_request_topic",
                "ai_response_topic",
                "telemetry_topic",
                "mgmt_request_topic",
                "mgmt_response_topic",
                "username",
                "password",
            ]
        );
        assert_eq!(
            GOOSE,
            &[
                "app_id",
                "conf_rev",
                "time_allowed_to_live_ms",
                "min_time_ms",
                "go_cb_ref",
                "dataset_ref",
                "dst_mac",
                "vlan_id",
            ]
        );
        assert_eq!(SV, &["app_id", "dst_mac", "buffer_size"]);
        assert_eq!(MMS, &["peer", "timeout_ms", "ap_title"]);
        assert_eq!(DLT645, &["addr_bcd", "baud", "timeout_ms", "retries"]);
        assert_eq!(
            GBT27930,
            &[
                "charger_addr",
                "bms_addr",
                "handshake_timeout_ms",
                "param_timeout_ms",
                "charging_timeout_ms",
                "end_timeout_ms",
                "retransmit_max",
            ]
        );
        assert_eq!(
            IEC101,
            &[
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
            ]
        );
        assert_eq!(IEC103, &["common_addr", "timeout_ms", "retries"]);
        assert_eq!(CJ188, &["addr_bcd", "baud", "timeout_ms", "retries"]);
    }
}
