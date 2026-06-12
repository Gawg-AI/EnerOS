use serde::{Deserialize, Serialize};

/// Supported protocol types
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        assert!(ProtocolType::Goose.uses_udp());
        assert!(ProtocolType::Sv.uses_udp());
        assert!(!ProtocolType::Modbus.uses_udp());
    }

    #[test]
    fn test_serde_roundtrip() {
        let proto = ProtocolType::Iec61850;
        let json = serde_json::to_string(&proto).unwrap();
        let deserialized: ProtocolType = serde_json::from_str(&json).unwrap();
        assert_eq!(proto, deserialized);
    }
}
