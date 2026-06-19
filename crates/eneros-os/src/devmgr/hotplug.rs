use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceType {
    Serial,
    Usb,
    Network,
    Block,
    Pci,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_type: DeviceType,
    pub path: String,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub serial_number: Option<String>,
}

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Added(DeviceInfo),
    Removed(String), // path
}
