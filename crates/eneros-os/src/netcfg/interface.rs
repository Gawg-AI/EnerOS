use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterfaceType {
    Ethernet,
    Vlan,
    Bridge,
    Loopback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceConfig {
    pub name: String,
    pub interface_type: InterfaceType,
    pub ipv4_address: Option<String>,
    pub ipv4_netmask: Option<String>,
    pub ipv4_gateway: Option<String>,
    pub ipv6_address: Option<String>,
    pub mtu: Option<u32>,
    pub vlan_id: Option<u16>,
    pub bridge_ports: Vec<String>,
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            interface_type: InterfaceType::Ethernet,
            ipv4_address: None,
            ipv4_netmask: None,
            ipv4_gateway: None,
            ipv6_address: None,
            mtu: Some(1500),
            vlan_id: None,
            bridge_ports: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkInterface {
    pub config: InterfaceConfig,
    pub up: bool,
}

impl NetworkInterface {
    pub fn new(config: InterfaceConfig) -> Self {
        Self {
            config,
            up: false,
        }
    }
}
