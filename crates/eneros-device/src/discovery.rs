use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use parking_lot::RwLock;
use tokio::net::TcpStream;

use crate::adapter::ConnectionConfig;
use crate::protocol::ProtocolType;

#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub address: SocketAddr,
    pub protocol: ProtocolType,
    pub response_time_ms: u64,
    pub metadata: DeviceMetadata,
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
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            subnet: "192.168.1.0/24".to_string(),
            port: 502,
            timeout_ms: 1000,
            max_concurrent: 50,
        }
    }
}

pub struct DeviceDiscovery {
    config: DiscoveryConfig,
    discovered: RwLock<HashMap<String, DiscoveredDevice>>,
    protocols: Vec<ProtocolType>,
}

impl DeviceDiscovery {
    pub fn new(config: DiscoveryConfig) -> Self {
        Self {
            config,
            discovered: RwLock::new(HashMap::new()),
            protocols: vec![
                ProtocolType::Modbus,
                ProtocolType::Iec61850,
                ProtocolType::Iec104,
            ],
        }
    }

    pub fn with_protocols(config: DiscoveryConfig, protocols: Vec<ProtocolType>) -> Self {
        Self {
            config,
            discovered: RwLock::new(HashMap::new()),
            protocols,
        }
    }

    pub async fn scan_network(&self) -> Vec<DiscoveredDevice> {
        let ips = self.expand_subnet();
        let mut handles = Vec::new();
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(
            self.config.max_concurrent,
        ));

        for ip in ips {
            let port = self.config.port;
            let timeout = Duration::from_millis(self.config.timeout_ms);
            let sem = semaphore.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                Self::probe_device(ip, port, timeout).await
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
        ConnectionConfig {
            host: device.address.ip().to_string(),
            port: device.address.port(),
            timeout_ms: self.config.timeout_ms,
            credentials: None,
            protocol_config: crate::adapter::ProtocolConfig::Modbus {
                slave_id: 1,
                baud_rate: None,
            },
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
        };
        let discovery = DeviceDiscovery::new(config);
        let results = discovery.scan_network().await;
        assert!(results.is_empty());
    }
}
