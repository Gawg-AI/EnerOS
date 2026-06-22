use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Supported protocol types
///
/// 派生 `PartialEq/Eq/Hash` 用于哈希与比较；`Serialize/Deserialize` 手动实现，
/// 以便 `Custom("iec103")` 序列化为 `"custom:iec103"`，而内置变体保持
/// 原有外部标签形式（如 `"Iec61850"`），保证向后兼容。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProtocolType {
    /// IEC 61850 - Substation automation
    Iec61850,
    /// IEC 60870-5-104 - Telecontrol
    Iec104,
    /// GOOSE - Generic Object Oriented Substation Events
    Goose,
    /// SV - Sampled Values
    Sv,
    /// MQTT - Message Queuing Telemetry Transport
    Mqtt,
    /// Modbus - Industrial communication
    Modbus,
    /// OPC UA - Open Platform Communications Unified Architecture
    OpcUa,
    /// DNP3 - Distributed Network Protocol
    Dnp3,
    /// 自定义协议（由插件提供，如 IEC 60870-5-103）
    Custom(String),
}

impl Serialize for ProtocolType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ProtocolType::Iec61850 => serializer.serialize_unit_variant("ProtocolType", 0, "Iec61850"),
            ProtocolType::Iec104 => serializer.serialize_unit_variant("ProtocolType", 1, "Iec104"),
            ProtocolType::Goose => serializer.serialize_unit_variant("ProtocolType", 2, "Goose"),
            ProtocolType::Sv => serializer.serialize_unit_variant("ProtocolType", 3, "Sv"),
            ProtocolType::Mqtt => serializer.serialize_unit_variant("ProtocolType", 4, "Mqtt"),
            ProtocolType::Modbus => serializer.serialize_unit_variant("ProtocolType", 5, "Modbus"),
            ProtocolType::OpcUa => serializer.serialize_unit_variant("ProtocolType", 6, "OpcUa"),
            ProtocolType::Dnp3 => serializer.serialize_unit_variant("ProtocolType", 7, "Dnp3"),
            ProtocolType::Custom(name) => {
                serializer.serialize_str(&format!("custom:{}", name))
            }
        }
    }
}

impl<'de> Deserialize<'de> for ProtocolType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "Iec61850" => Ok(ProtocolType::Iec61850),
            "Iec104" => Ok(ProtocolType::Iec104),
            "Goose" => Ok(ProtocolType::Goose),
            "Sv" => Ok(ProtocolType::Sv),
            "Mqtt" => Ok(ProtocolType::Mqtt),
            "Modbus" => Ok(ProtocolType::Modbus),
            "OpcUa" => Ok(ProtocolType::OpcUa),
            "Dnp3" => Ok(ProtocolType::Dnp3),
            other => {
                if let Some(name) = other.strip_prefix("custom:") {
                    Ok(ProtocolType::Custom(name.to_string()))
                } else {
                    Err(serde::de::Error::unknown_variant(
                        other,
                        &[
                            "Iec61850",
                            "Iec104",
                            "Goose",
                            "Sv",
                            "Mqtt",
                            "Modbus",
                            "OpcUa",
                            "Dnp3",
                            "custom:<name>",
                        ],
                    ))
                }
            }
        }
    }
}

impl ProtocolType {
    /// Get protocol name
    pub fn name(&self) -> &str {
        match self {
            ProtocolType::Iec61850 => "IEC 61850",
            ProtocolType::Iec104 => "IEC 60870-5-104",
            ProtocolType::Goose => "GOOSE",
            ProtocolType::Sv => "SV",
            ProtocolType::Mqtt => "MQTT",
            ProtocolType::Modbus => "Modbus",
            ProtocolType::OpcUa => "OPC UA",
            ProtocolType::Dnp3 => "DNP3",
            ProtocolType::Custom(name) => name.as_str(),
        }
    }

    /// Get default port
    pub fn default_port(&self) -> u16 {
        match self {
            ProtocolType::Iec61850 => 102,
            ProtocolType::Iec104 => 2404,
            ProtocolType::Goose => 0, // Layer 2, no port
            ProtocolType::Sv => 0,    // Layer 2, no port
            ProtocolType::Mqtt => 1883,
            ProtocolType::Modbus => 502,
            ProtocolType::OpcUa => 4840,
            ProtocolType::Dnp3 => 20000,
            ProtocolType::Custom(_) => 0, // 自定义协议默认端口未知
        }
    }

    /// Check if protocol is real-time capable (< 10ms)
    pub fn is_realtime(&self) -> bool {
        matches!(
            self,
            ProtocolType::Goose | ProtocolType::Sv
        )
    }

    /// Check if protocol uses TCP
    pub fn uses_tcp(&self) -> bool {
        matches!(
            self,
            ProtocolType::Iec61850
                | ProtocolType::Iec104
                | ProtocolType::Mqtt
                | ProtocolType::Modbus
                | ProtocolType::OpcUa
                | ProtocolType::Dnp3
        )
    }

    /// Check if protocol uses UDP
    pub fn uses_udp(&self) -> bool {
        matches!(self, ProtocolType::Mqtt)  // 只有 MQTT 走 UDP（如果适用）
        // GOOSE/SV 不走 UDP，它们走 Layer 2
    }

    /// 是否使用 Layer 2 以太网（非 IP/UDP/TCP）
    pub fn uses_layer2(&self) -> bool {
        matches!(self, ProtocolType::Goose | ProtocolType::Sv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_names() {
        assert_eq!(ProtocolType::Iec61850.name(), "IEC 61850");
        assert_eq!(ProtocolType::Modbus.name(), "Modbus");
        assert_eq!(ProtocolType::Mqtt.name(), "MQTT");
        assert_eq!(ProtocolType::Dnp3.name(), "DNP3");
    }

    #[test]
    fn test_default_ports() {
        assert_eq!(ProtocolType::Iec61850.default_port(), 102);
        assert_eq!(ProtocolType::Iec104.default_port(), 2404);
        assert_eq!(ProtocolType::Modbus.default_port(), 502);
        assert_eq!(ProtocolType::Mqtt.default_port(), 1883);
        assert_eq!(ProtocolType::OpcUa.default_port(), 4840);
        assert_eq!(ProtocolType::Dnp3.default_port(), 20000);
    }

    #[test]
    fn test_realtime_protocols() {
        assert!(ProtocolType::Goose.is_realtime());
        assert!(ProtocolType::Sv.is_realtime());
        assert!(!ProtocolType::Modbus.is_realtime());
        assert!(!ProtocolType::Iec61850.is_realtime());
    }

    #[test]
    fn test_tcp_protocols() {
        assert!(ProtocolType::Iec61850.uses_tcp());
        assert!(ProtocolType::Modbus.uses_tcp());
        assert!(ProtocolType::Mqtt.uses_tcp());
        assert!(!ProtocolType::Goose.uses_tcp());
        assert!(!ProtocolType::Sv.uses_tcp());
    }

    #[test]
    fn test_udp_protocols() {
        // GOOSE/SV 不走 UDP，走 Layer 2 以太网
        assert!(!ProtocolType::Goose.uses_udp());
        assert!(!ProtocolType::Sv.uses_udp());
        assert!(!ProtocolType::Modbus.uses_udp());
        assert!(!ProtocolType::Iec104.uses_udp());
    }

    #[test]
    fn test_layer2_protocols() {
        // GOOSE/SV 走 Layer 2 以太网（EtherType）
        assert!(ProtocolType::Goose.uses_layer2());
        assert!(ProtocolType::Sv.uses_layer2());
        // 其他协议不走 Layer 2
        assert!(!ProtocolType::Iec104.uses_layer2());
        assert!(!ProtocolType::Modbus.uses_layer2());
        assert!(!ProtocolType::Mqtt.uses_layer2());
        assert!(!ProtocolType::Iec61850.uses_layer2());
    }

    #[test]
    fn test_serde_roundtrip() {
        let proto = ProtocolType::Iec61850;
        let json = serde_json::to_string(&proto).unwrap();
        let deserialized: ProtocolType = serde_json::from_str(&json).unwrap();
        assert_eq!(proto, deserialized);
    }

    #[test]
    fn test_builtin_serde_format_unchanged() {
        // 内置变体应保持原有外部标签序列化格式，保证向后兼容
        assert_eq!(
            serde_json::to_string(&ProtocolType::Iec61850).unwrap(),
            "\"Iec61850\""
        );
        assert_eq!(
            serde_json::to_string(&ProtocolType::Modbus).unwrap(),
            "\"Modbus\""
        );
        assert_eq!(
            serde_json::to_string(&ProtocolType::Dnp3).unwrap(),
            "\"Dnp3\""
        );
    }

    #[test]
    fn test_custom_protocol_serde() {
        let proto = ProtocolType::Custom("iec103".to_string());
        let json = serde_json::to_string(&proto).unwrap();
        assert_eq!(json, "\"custom:iec103\"");
        let deserialized: ProtocolType = serde_json::from_str(&json).unwrap();
        assert_eq!(proto, deserialized);
    }

    #[test]
    fn test_custom_protocol_name_and_port() {
        let proto = ProtocolType::Custom("iec103".to_string());
        assert_eq!(proto.name(), "iec103");
        assert_eq!(proto.default_port(), 0);
        assert!(!proto.is_realtime());
        assert!(!proto.uses_tcp());
        assert!(!proto.uses_udp());
        assert!(!proto.uses_layer2());
    }

    #[test]
    fn test_custom_protocol_eq_hash() {
        let a = ProtocolType::Custom("iec103".to_string());
        let b = ProtocolType::Custom("iec103".to_string());
        let c = ProtocolType::Custom("modbus-rtu".to_string());
        assert_eq!(a, b);
        assert_ne!(a, c);
        // 验证可作为 HashMap 键
        let mut map = std::collections::HashMap::new();
        map.insert(a, 1);
        assert_eq!(map.get(&b), Some(&1));
    }

    #[test]
    fn test_custom_protocol_deserialize_invalid() {
        // 无效字符串应反序列化失败
        let result: Result<ProtocolType, _> = serde_json::from_str("\"unknown-proto\"");
        assert!(result.is_err());
    }
}
