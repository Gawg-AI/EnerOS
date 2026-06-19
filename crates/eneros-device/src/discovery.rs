use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::adapter::ConnectionConfig;
use crate::protocol::ProtocolType;

#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub address: SocketAddr,
    pub protocol: ProtocolType,
    pub response_time_ms: u64,
    pub metadata: DeviceMetadata,
    /// v0.7.0: Confidence score (0-100) for protocol identification
    pub confidence: u8,
    /// v0.7.0: All protocols detected on this device (multi-protocol devices)
    pub detected_protocols: Vec<ProtocolType>,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceMetadata {
    pub manufacturer: String,
    pub model: String,
    pub firmware_version: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub subnet: String,
    pub port: u16,
    pub timeout_ms: u64,
    pub max_concurrent: usize,
    /// v0.7.0: Protocols to probe (empty = auto-detect all)
    pub protocols: Vec<ProtocolType>,
    /// v0.7.0: Whether to perform handshake identification
    pub handshake_identify: bool,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            subnet: "192.168.1.0/24".to_string(),
            port: 502,
            timeout_ms: 1000,
            max_concurrent: 50,
            protocols: Vec::new(),
            handshake_identify: true,
        }
    }
}

/// v0.7.0: Protocol probe result
#[derive(Debug, Clone)]
pub struct ProtocolProbe {
    pub protocol: ProtocolType,
    pub port: u16,
    pub confidence: u8,
    pub banner: String,
}

/// v0.7.0: Protocol signature for identification
struct ProtocolSignature {
    protocol: ProtocolType,
    port: u16,
    /// Probe data to send (None = just connect)
    probe: Option<&'static [u8]>,
    /// Expected response prefix (None = any response)
    expected_response: Option<&'static [u8]>,
    /// Minimum confidence if port is open
    port_open_confidence: u8,
    /// Confidence if probe response matches
    response_match_confidence: u8,
}

/// Known protocol signatures for identification
fn protocol_signatures() -> Vec<ProtocolSignature> {
    vec![
        ProtocolSignature {
            protocol: ProtocolType::Modbus,
            port: 502,
            probe: Some(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x06, 0x01, 0x03, 0x00, 0x00, 0x00, 0x01]),
            expected_response: Some(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0x01, 0x03, 0x02]),
            port_open_confidence: 60,
            response_match_confidence: 95,
        },
        ProtocolSignature {
            protocol: ProtocolType::Iec104,
            port: 2404,
            probe: Some(&[0x68, 0x04, 0x07, 0x00, 0x00, 0x00]), // STARTDT_ACT
            expected_response: Some(&[0x68, 0x04, 0x0B, 0x00, 0x00, 0x00]), // STARTDT_CON
            port_open_confidence: 70,
            response_match_confidence: 95,
        },
        ProtocolSignature {
            protocol: ProtocolType::Iec61850,
            port: 102,
            probe: None, // COTP CR requires complex encoding; port-based only
            expected_response: None,
            port_open_confidence: 65,
            response_match_confidence: 65,
        },
        ProtocolSignature {
            protocol: ProtocolType::OpcUa,
            port: 4840,
            probe: Some(&[0x4D, 0x53, 0x47, 0x46]), // "MSGF" OPC UA Hello
            expected_response: Some(&[0x41, 0x43, 0x4B, 0x46]), // "ACKF"
            port_open_confidence: 60,
            response_match_confidence: 90,
        },
        ProtocolSignature {
            protocol: ProtocolType::Dnp3,
            port: 20000,
            probe: Some(&[0x05, 0x64, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]), // DNP3 link reset
            expected_response: Some(&[0x05, 0x64, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00]),
            port_open_confidence: 70,
            response_match_confidence: 95,
        },
        ProtocolSignature {
            protocol: ProtocolType::Mqtt,
            port: 1883,
            probe: Some(&[0x10, 0x10, 0x00, 0x04, 0x4D, 0x51, 0x54, 0x54, 0x04, 0x02, 0x00, 0x3C, 0x00, 0x04, 0x74, 0x65, 0x73, 0x74]), // CONNECT
            expected_response: Some(&[0x20, 0x02, 0x00, 0x00]), // CONNACK
            port_open_confidence: 50,
            response_match_confidence: 90,
        },
    ]
}

pub struct DeviceDiscovery {
    config: DiscoveryConfig,
    discovered: RwLock<HashMap<String, DiscoveredDevice>>,
}

impl DeviceDiscovery {
    pub fn new(config: DiscoveryConfig) -> Self {
        Self {
            config,
            discovered: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_protocols(config: DiscoveryConfig, protocols: Vec<ProtocolType>) -> Self {
        Self {
            config: DiscoveryConfig {
                protocols,
                ..config
            },
            discovered: RwLock::new(HashMap::new()),
        }
    }

    pub async fn scan_network(&self) -> Vec<DiscoveredDevice> {
        let ips = self.expand_subnet();
        let mut handles = Vec::new();
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(
            self.config.max_concurrent,
        ));

        for ip in ips {
            let timeout = Duration::from_millis(self.config.timeout_ms);
            let sem = semaphore.clone();
            let protocols = self.config.protocols.clone();
            let handshake = self.config.handshake_identify;

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                Self::probe_device_smart(ip, timeout, &protocols, handshake).await
            });
            handles.push(handle);
        }

        let mut results = Vec::new();
        for handle in handles {
            if let Ok(Some(device)) = handle.await {
                let key = format!("{}:{}", device.address.ip(), device.address.port());
                self.discovered.write().insert(key, device.clone());
                results.push(device);
            }
        }

        tracing::info!("Device discovery found {} devices", results.len());
        results
    }

    /// v0.7.0: Smart probe — tries multiple protocol signatures
    async fn probe_device_smart(
        ip: std::net::Ipv4Addr,
        timeout: Duration,
        target_protocols: &[ProtocolType],
        handshake: bool,
    ) -> Option<DiscoveredDevice> {
        let signatures = protocol_signatures();
        let mut probes = Vec::new();

        for sig in &signatures {
            // Filter by target protocols if specified
            if !target_protocols.is_empty() && !target_protocols.contains(&sig.protocol) {
                continue;
            }
            probes.push(sig);
        }

        let mut detected = Vec::new();
        let mut best_protocol = ProtocolType::Modbus;
        let mut best_confidence: u8 = 0;
        let mut best_port: u16 = 0;
        let mut best_response_time: u64 = 0;

        for sig in &probes {
            let addr = SocketAddr::new(ip.into(), sig.port);
            let start = std::time::Instant::now();

            let result = tokio::time::timeout(timeout, Self::probe_protocol(addr, sig, handshake)).await;
            let response_time = start.elapsed().as_millis() as u64;

            if let Ok(Ok(confidence)) = result {
                if confidence > 0 {
                    detected.push(ProtocolProbe {
                        protocol: sig.protocol.clone(),
                        port: sig.port,
                        confidence,
                        banner: String::new(),
                    });
                    if confidence > best_confidence {
                        best_confidence = confidence;
                        best_protocol = sig.protocol.clone();
                        best_port = sig.port;
                        best_response_time = response_time;
                    }
                }
            }
        }

        if detected.is_empty() {
            return None;
        }

        let detected_protocols: Vec<ProtocolType> = detected.iter().map(|p| p.protocol.clone()).collect();

        Some(DiscoveredDevice {
            address: SocketAddr::new(ip.into(), best_port),
            protocol: best_protocol,
            response_time_ms: best_response_time,
            metadata: DeviceMetadata {
                manufacturer: String::new(),
                model: String::new(),
                firmware_version: String::new(),
                capabilities: detected.iter().map(|p| format!("{:?}", p.protocol)).collect(),
            },
            confidence: best_confidence,
            detected_protocols,
        })
    }

    /// v0.7.0: Probe a single protocol on a single port
    async fn probe_protocol(
        addr: SocketAddr,
        sig: &ProtocolSignature,
        handshake: bool,
    ) -> std::io::Result<u8> {
        let mut stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;

        if !handshake || sig.probe.is_none() {
            return Ok(sig.port_open_confidence);
        }

        // Send probe
        let probe = sig.probe.unwrap();
        stream.write_all(probe).await?;

        // Read response (up to 256 bytes)
        let mut buf = [0u8; 256];
        let n = tokio::time::timeout(Duration::from_millis(500), stream.read(&mut buf)).await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "read timeout"))??;

        if n == 0 {
            return Ok(sig.port_open_confidence / 2); // Port open but no response
        }

        // Check expected response
        if let Some(expected) = sig.expected_response {
            if n >= expected.len() && &buf[..expected.len()] == expected {
                return Ok(sig.response_match_confidence);
            }
            // Partial match — lower confidence
            let match_count = expected.iter().zip(buf.iter()).take_while(|(a, b)| a == b).count();
            if match_count > 0 {
                let partial_confidence = (sig.response_match_confidence as usize * match_count / expected.len()) as u8;
                return Ok(partial_confidence.max(sig.port_open_confidence));
            }
        } else {
            // No expected response — any response is a match
            return Ok(sig.response_match_confidence);
        }

        Ok(sig.port_open_confidence)
    }

    async fn probe_device(
        ip: std::net::Ipv4Addr,
        port: u16,
        timeout: Duration,
    ) -> Option<DiscoveredDevice> {
        let addr = SocketAddr::new(ip.into(), port);
        let start = std::time::Instant::now();

        match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
            Ok(Ok(_)) => {
                let response_time = start.elapsed().as_millis() as u64;
                Some(DiscoveredDevice {
                    address: addr,
                    protocol: ProtocolType::Modbus,
                    response_time_ms: response_time,
                    metadata: DeviceMetadata::default(),
                    confidence: 50,
                    detected_protocols: vec![ProtocolType::Modbus],
                })
            }
            _ => None,
        }
    }

    fn expand_subnet(&self) -> Vec<std::net::Ipv4Addr> {
        let parts: Vec<&str> = self.config.subnet.split('/').collect();
        if parts.len() != 2 {
            return Vec::new();
        }

        let ip_parts: Vec<u8> = parts[0]
            .split('.')
            .filter_map(|s| s.parse().ok())
            .collect();

        if ip_parts.len() != 4 {
            return Vec::new();
        }

        let prefix_len: u32 = parts[1].parse().unwrap_or(24);
        let host_bits = 32 - prefix_len;

        let base_ip = ((ip_parts[0] as u32) << 24)
            | ((ip_parts[1] as u32) << 16)
            | ((ip_parts[2] as u32) << 8)
            | (ip_parts[3] as u32);

        let mask = !((1u32 << host_bits) - 1);
        let network = base_ip & mask;
        let broadcast = network | ((1u32 << host_bits) - 1);

        let mut ips = Vec::new();
        for ip in (network + 1)..broadcast {
            ips.push(std::net::Ipv4Addr::new(
                ((ip >> 24) & 0xFF) as u8,
                ((ip >> 16) & 0xFF) as u8,
                ((ip >> 8) & 0xFF) as u8,
                (ip & 0xFF) as u8,
            ));
        }
        ips
    }

    pub fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        self.discovered.read().values().cloned().collect()
    }

    pub fn discovered_count(&self) -> usize {
        self.discovered.read().len()
    }

    pub fn get_device(&self, key: &str) -> Option<DiscoveredDevice> {
        self.discovered.read().get(key).cloned()
    }

    pub fn clear(&self) {
        self.discovered.write().clear();
    }

    pub fn create_connection_config(&self, device: &DiscoveredDevice) -> ConnectionConfig {
        let protocol_config = match device.protocol {
            ProtocolType::Modbus => crate::adapter::ProtocolConfig::Modbus {
                slave_id: 1,
                baud_rate: None,
            },
            ProtocolType::Iec104 => crate::adapter::ProtocolConfig::Iec104 {
                common_address: 1,
                ioa_size: 3,
            },
            ProtocolType::Iec61850 => crate::adapter::ProtocolConfig::Iec61850 {
                logical_devices: vec!["LD0".to_string()],
            },
            ProtocolType::Mqtt => crate::adapter::ProtocolConfig::Mqtt {
                client_id: format!("eneros-{}", device.address.port()),
                topics: vec!["#".to_string()],
            },
            ProtocolType::OpcUa => crate::adapter::ProtocolConfig::OpcUa {
                namespace_url: format!("opc.tcp://{}:{}/", device.address.ip(), device.address.port()),
                security_policy: "None".to_string(),
            },
            ProtocolType::Dnp3 => crate::adapter::ProtocolConfig::Dnp3 {
                master_address: 1,
                slave_address: 1024,
            },
            _ => crate::adapter::ProtocolConfig::Modbus {
                slave_id: 1,
                baud_rate: None,
            },
        };

        ConnectionConfig {
            host: device.address.ip().to_string(),
            port: device.address.port(),
            timeout_ms: self.config.timeout_ms,
            credentials: None,
            protocol_config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subnet_expansion() {
        let config = DiscoveryConfig {
            subnet: "192.168.1.0/30".to_string(),
            ..Default::default()
        };
        let discovery = DeviceDiscovery::new(config);
        let ips = discovery.expand_subnet();
        assert_eq!(ips.len(), 2);
        assert_eq!(ips[0], "192.168.1.1".parse::<std::net::Ipv4Addr>().unwrap());
        assert_eq!(ips[1], "192.168.1.2".parse::<std::net::Ipv4Addr>().unwrap());
    }

    #[test]
    fn test_subnet_expansion_24() {
        let config = DiscoveryConfig {
            subnet: "10.0.0.0/30".to_string(),
            ..Default::default()
        };
        let discovery = DeviceDiscovery::new(config);
        let ips = discovery.expand_subnet();
        assert_eq!(ips.len(), 2);
    }

    #[test]
    fn test_device_discovery_new() {
        let config = DiscoveryConfig::default();
        let discovery = DeviceDiscovery::new(config);
        assert_eq!(discovery.discovered_count(), 0);
    }

    #[tokio::test]
    async fn test_scan_network() {
        let config = DiscoveryConfig {
            subnet: "127.0.0.0/30".to_string(),
            port: 1,
            timeout_ms: 100,
            max_concurrent: 10,
            handshake_identify: false,
            ..Default::default()
        };
        let discovery = DeviceDiscovery::new(config);
        let results = discovery.scan_network().await;
        assert!(results.is_empty());
    }

    // ---- v0.7.0 tests ----

    #[test]
    fn test_protocol_signatures_count() {
        let sigs = protocol_signatures();
        assert!(sigs.len() >= 6); // Modbus, IEC104, IEC61850, OPC UA, DNP3, MQTT
    }

    #[test]
    fn test_protocol_signatures_ports() {
        let sigs = protocol_signatures();
        let ports: Vec<u16> = sigs.iter().map(|s| s.port).collect();
        assert!(ports.contains(&502));   // Modbus
        assert!(ports.contains(&2404));  // IEC 104
        assert!(ports.contains(&102));   // IEC 61850
        assert!(ports.contains(&4840));  // OPC UA
        assert!(ports.contains(&20000)); // DNP3
        assert!(ports.contains(&1883));  // MQTT
    }

    #[test]
    fn test_discovery_config_with_protocols() {
        let config = DiscoveryConfig {
            protocols: vec![ProtocolType::Iec104, ProtocolType::Modbus],
            handshake_identify: true,
            ..Default::default()
        };
        assert_eq!(config.protocols.len(), 2);
        assert!(config.handshake_identify);
    }

    #[test]
    fn test_discovered_device_confidence() {
        let device = DiscoveredDevice {
            address: "127.0.0.1:502".parse().unwrap(),
            protocol: ProtocolType::Modbus,
            response_time_ms: 10,
            metadata: DeviceMetadata::default(),
            confidence: 95,
            detected_protocols: vec![ProtocolType::Modbus],
        };
        assert_eq!(device.confidence, 95);
        assert_eq!(device.detected_protocols.len(), 1);
    }

    #[test]
    fn test_create_connection_config_modbus() {
        let config = DiscoveryConfig::default();
        let discovery = DeviceDiscovery::new(config);
        let device = DiscoveredDevice {
            address: "127.0.0.1:502".parse().unwrap(),
            protocol: ProtocolType::Modbus,
            response_time_ms: 10,
            metadata: DeviceMetadata::default(),
            confidence: 95,
            detected_protocols: vec![ProtocolType::Modbus],
        };
        let conn = discovery.create_connection_config(&device);
        assert_eq!(conn.port, 502);
        assert!(matches!(conn.protocol_config, crate::adapter::ProtocolConfig::Modbus { .. }));
    }

    #[test]
    fn test_create_connection_config_iec104() {
        let config = DiscoveryConfig::default();
        let discovery = DeviceDiscovery::new(config);
        let device = DiscoveredDevice {
            address: "127.0.0.1:2404".parse().unwrap(),
            protocol: ProtocolType::Iec104,
            response_time_ms: 10,
            metadata: DeviceMetadata::default(),
            confidence: 95,
            detected_protocols: vec![ProtocolType::Iec104],
        };
        let conn = discovery.create_connection_config(&device);
        assert_eq!(conn.port, 2404);
        assert!(matches!(conn.protocol_config, crate::adapter::ProtocolConfig::Iec104 { .. }));
    }

    #[test]
    fn test_create_connection_config_dnp3() {
        let config = DiscoveryConfig::default();
        let discovery = DeviceDiscovery::new(config);
        let device = DiscoveredDevice {
            address: "127.0.0.1:20000".parse().unwrap(),
            protocol: ProtocolType::Dnp3,
            response_time_ms: 10,
            metadata: DeviceMetadata::default(),
            confidence: 95,
            detected_protocols: vec![ProtocolType::Dnp3],
        };
        let conn = discovery.create_connection_config(&device);
        assert_eq!(conn.port, 20000);
        assert!(matches!(conn.protocol_config, crate::adapter::ProtocolConfig::Dnp3 { .. }));
    }

    #[test]
    fn test_create_connection_config_opcua() {
        let config = DiscoveryConfig::default();
        let discovery = DeviceDiscovery::new(config);
        let device = DiscoveredDevice {
            address: "127.0.0.1:4840".parse().unwrap(),
            protocol: ProtocolType::OpcUa,
            response_time_ms: 10,
            metadata: DeviceMetadata::default(),
            confidence: 90,
            detected_protocols: vec![ProtocolType::OpcUa],
        };
        let conn = discovery.create_connection_config(&device);
        assert_eq!(conn.port, 4840);
        assert!(matches!(conn.protocol_config, crate::adapter::ProtocolConfig::OpcUa { .. }));
    }

    #[test]
    fn test_create_connection_config_mqtt() {
        let config = DiscoveryConfig::default();
        let discovery = DeviceDiscovery::new(config);
        let device = DiscoveredDevice {
            address: "127.0.0.1:1883".parse().unwrap(),
            protocol: ProtocolType::Mqtt,
            response_time_ms: 10,
            metadata: DeviceMetadata::default(),
            confidence: 90,
            detected_protocols: vec![ProtocolType::Mqtt],
        };
        let conn = discovery.create_connection_config(&device);
        assert_eq!(conn.port, 1883);
        assert!(matches!(conn.protocol_config, crate::adapter::ProtocolConfig::Mqtt { .. }));
    }

    #[test]
    fn test_create_connection_config_iec61850() {
        let config = DiscoveryConfig::default();
        let discovery = DeviceDiscovery::new(config);
        let device = DiscoveredDevice {
            address: "127.0.0.1:102".parse().unwrap(),
            protocol: ProtocolType::Iec61850,
            response_time_ms: 10,
            metadata: DeviceMetadata::default(),
            confidence: 65,
            detected_protocols: vec![ProtocolType::Iec61850],
        };
        let conn = discovery.create_connection_config(&device);
        assert_eq!(conn.port, 102);
        assert!(matches!(conn.protocol_config, crate::adapter::ProtocolConfig::Iec61850 { .. }));
    }

    #[tokio::test]
    async fn test_with_protocols_filter() {
        let config = DiscoveryConfig {
            subnet: "127.0.0.0/30".to_string(),
            port: 1,
            timeout_ms: 100,
            max_concurrent: 10,
            handshake_identify: false,
            ..Default::default()
        };
        let discovery = DeviceDiscovery::with_protocols(config, vec![ProtocolType::Iec104]);
        assert_eq!(discovery.config.protocols.len(), 1);
        let results = discovery.scan_network().await;
        assert!(results.is_empty());
    }
}
