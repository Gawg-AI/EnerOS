//! 设备管理（Device Manager）
//!
//! 提供 uevent 热插拔监听和网络接口插拔自动配置能力。
//! Linux 通过 NETLINK_KOBJECT_UEVENT socket 监听内核设备事件。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// 设备管理错误
#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("uevent monitor failed: {0}")]
    UeventMonitorFailed(String),
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("unsupported platform")]
    UnsupportedPlatform,
    #[error("config error: {0}")]
    ConfigError(String),
}

/// 设备类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Net,
    Block,
    Usb,
    Serial,
    Gpio,
    I2c,
    Spi,
    Unknown,
}

/// 设备状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Online,
    Offline,
    Error,
}

/// 热插拔动作
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotplugAction {
    Add,
    Remove,
    Change,
}

/// 热插拔事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotplugEvent {
    pub action: HotplugAction,
    pub device_type: DeviceType,
    pub subsystem: String,
    pub device_name: String,
    pub properties: HashMap<String, String>,
}

/// 设备信息（跟踪设备运行时状态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub device_type: DeviceType,
    pub status: DeviceStatus,
    pub path: String,
    pub driver: String,
    pub last_seen: Option<String>,
}

/// 设备配置规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRule {
    pub name: String,
    pub device_type: DeviceType,
    pub path: String,
    pub enabled: bool,
    pub config: HashMap<String, String>,
}

/// 设备配置（持久化到 TOML）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceConfig {
    pub devices: Vec<DeviceRule>,
}

/// 热插拔事件处理器类型
type HotplugHandler = Box<dyn Fn(&HotplugEvent) + Send + Sync>;

/// 设备管理器
pub struct DeviceManager {
    event_handlers: Vec<HotplugHandler>,
    devices: HashMap<String, DeviceInfo>,
}

impl DeviceManager {
    pub fn new() -> Self {
        Self {
            event_handlers: Vec::new(),
            devices: HashMap::new(),
        }
    }

    /// 注册热插拔事件处理器
    pub fn on_event<F>(&mut self, handler: F)
    where
        F: Fn(&HotplugEvent) + Send + Sync + 'static,
    {
        self.event_handlers.push(Box::new(handler));
    }

    /// 获取当前跟踪的设备快照
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.values().cloned().collect()
    }

    /// 启动 uevent 监听（阻塞当前线程）
    #[cfg(target_os = "linux")]
    pub fn run(&mut self) -> Result<(), DeviceError> {
        use std::os::raw::{c_int, c_uint};
        // 创建 NETLINK_KOBJECT_UEVENT socket
        const AF_NETLINK: c_int = 16;
        const SOCK_RAW: c_int = 3;
        const NETLINK_KOBJECT_UEVENT: c_int = 15;
        const SOL_NETLINK: c_int = 270;
        const NETLINK_ADD_MEMBERSHIP: c_int = 1;

        let sock = unsafe { libc::socket(AF_NETLINK, SOCK_RAW, NETLINK_KOBJECT_UEVENT) };
        if sock < 0 {
            return Err(DeviceError::UeventMonitorFailed(
                "socket creation failed".to_string(),
            ));
        }

        // 绑定到 netlink
        #[repr(C)]
        struct SockaddrNl {
            nl_family: u16,
            nl_pad: u16,
            nl_pid: u32,
            nl_groups: u32,
        }
        let addr = SockaddrNl {
            nl_family: AF_NETLINK as u16,
            nl_pad: 0,
            nl_pid: 0,
            nl_groups: 1, // 加入组播
        };
        let ret = unsafe {
            libc::bind(
                sock,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<SockaddrNl>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            unsafe { libc::close(sock) };
            return Err(DeviceError::UeventMonitorFailed("bind failed".to_string()));
        }

        // 接收循环
        let mut buf = vec![0u8; 8192];
        loop {
            let len = unsafe {
                libc::recv(sock, buf.as_mut_ptr() as *mut _, buf.len() as libc::size_t, 0)
            };
            if len < 0 {
                continue;
            }
            let data = &buf[..len as usize];
            if let Some(event) = parse_uevent(data) {
                // 更新设备状态跟踪
                self.update_device_state(&event);
                for handler in &self.event_handlers {
                    handler(&event);
                }
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn run(&mut self) -> Result<(), DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// 根据 uevent 事件更新内部设备状态表
    #[allow(dead_code)] // Linux run() 调用，跨平台测试也调用
    fn update_device_state(&mut self, event: &HotplugEvent) {
        let now = chrono::Utc::now().to_rfc3339();
        match event.action {
            HotplugAction::Add | HotplugAction::Change => {
                let path = event
                    .properties
                    .get("DEVPATH")
                    .cloned()
                    .unwrap_or_default();
                let info = DeviceInfo {
                    name: event.device_name.clone(),
                    device_type: event.device_type.clone(),
                    status: DeviceStatus::Online,
                    path,
                    driver: String::new(),
                    last_seen: Some(now),
                };
                self.devices.insert(event.device_name.clone(), info);
            }
            HotplugAction::Remove => {
                if let Some(info) = self.devices.get_mut(&event.device_name) {
                    info.status = DeviceStatus::Offline;
                    info.last_seen = Some(now);
                }
            }
        }
    }

    /// 枚举当前网络接口
    #[cfg(target_os = "linux")]
    pub fn list_net_interfaces() -> Result<Vec<String>, DeviceError> {
        let entries = std::fs::read_dir("/sys/class/net")?;
        let mut names = Vec::new();
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                names.push(name);
            }
        }
        Ok(names)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list_net_interfaces() -> Result<Vec<String>, DeviceError> {
        Ok(Vec::new())
    }

    /// 检查设备是否存在
    #[cfg(target_os = "linux")]
    pub fn device_exists(device_name: &str) -> bool {
        std::path::Path::new(&format!("/sys/class/net/{}", device_name)).exists()
    }

    #[cfg(not(target_os = "linux"))]
    pub fn device_exists(_device_name: &str) -> bool {
        false
    }

    /// 枚举串口设备（ttyS*/ttyUSB*/ttyACM*）
    #[cfg(target_os = "linux")]
    pub fn list_serial_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut result = Vec::new();
        let entries = std::fs::read_dir("/sys/class/tty")?;
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                if name.starts_with("ttyS")
                    || name.starts_with("ttyUSB")
                    || name.starts_with("ttyACM")
                {
                    result.push(DeviceInfo {
                        name: name.clone(),
                        device_type: DeviceType::Serial,
                        status: DeviceStatus::Online,
                        path: format!("/dev/{}", name),
                        driver: read_sysfs_driver(&format!("/sys/class/tty/{}", name)),
                        last_seen: Some(now.clone()),
                    });
                }
            }
        }
        Ok(result)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list_serial_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// 枚举 USB 设备（读取 idVendor/idProduct/product）
    #[cfg(target_os = "linux")]
    pub fn list_usb_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut result = Vec::new();
        let entries = std::fs::read_dir("/sys/bus/usb/devices")?;
        for entry in entries.flatten() {
            let path = entry.path();
            // 仅保留含有 idVendor 的条目（真实设备，非 hub/root）
            let id_vendor = std::fs::read_to_string(path.join("idVendor"));
            if id_vendor.is_err() {
                continue;
            }
            let name = std::fs::read_to_string(path.join("product"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| {
                    entry
                        .file_name()
                        .into_string()
                        .unwrap_or_default()
                });
            result.push(DeviceInfo {
                name,
                device_type: DeviceType::Usb,
                status: DeviceStatus::Online,
                path: path.to_string_lossy().to_string(),
                driver: read_sysfs_driver(&path.to_string_lossy()),
                last_seen: Some(now.clone()),
            });
        }
        Ok(result)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list_usb_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// 枚举 GPIO 设备（/dev/gpiochip*）
    #[cfg(target_os = "linux")]
    pub fn list_gpio_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        scan_dev_devices("gpiochip", DeviceType::Gpio)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list_gpio_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// 枚举 I2C 设备（/dev/i2c-*）
    #[cfg(target_os = "linux")]
    pub fn list_i2c_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        scan_dev_devices("i2c-", DeviceType::I2c)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list_i2c_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// 枚举 SPI 设备（/dev/spidev*）
    #[cfg(target_os = "linux")]
    pub fn list_spi_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        scan_dev_devices("spidev", DeviceType::Spi)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list_spi_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// 汇总枚举所有类型设备
    #[cfg(target_os = "linux")]
    pub fn list_all_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        let mut all = Vec::new();
        all.extend(Self::list_serial_devices()?);
        all.extend(Self::list_usb_devices()?);
        all.extend(Self::list_gpio_devices()?);
        all.extend(Self::list_i2c_devices()?);
        all.extend(Self::list_spi_devices()?);
        Ok(all)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn list_all_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// 从 TOML 文件加载设备配置（跨平台可测试）
    pub fn load_config(path: &str) -> Result<DeviceConfig, DeviceError> {
        let content = std::fs::read_to_string(path)?;
        let config: DeviceConfig =
            toml::from_str(&content).map_err(|e| DeviceError::ConfigError(e.to_string()))?;
        Ok(config)
    }

    /// 保存设备配置到 TOML 文件（跨平台可测试）
    pub fn save_config(path: &str, config: &DeviceConfig) -> Result<(), DeviceError> {
        let content = toml::to_string_pretty(config)
            .map_err(|e| DeviceError::ConfigError(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// 应用设备配置（Linux 下设置设备权限）
    #[cfg(target_os = "linux")]
    pub fn apply_config(&self, config: &DeviceConfig) -> Result<(), DeviceError> {
        use std::os::unix::fs::PermissionsExt;
        for rule in &config.devices {
            if !rule.enabled {
                continue;
            }
            // 设置设备文件权限为 0660（owner/group 读写）
            if std::path::Path::new(&rule.path).exists() {
                let perms = std::fs::Permissions::from_mode(0o660);
                std::fs::set_permissions(&rule.path, perms)?;
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply_config(&self, _config: &DeviceConfig) -> Result<(), DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 扫描 /dev/ 目录下匹配指定前缀的设备文件（gpio/i2c/spi 共用）
#[cfg(target_os = "linux")]
fn scan_dev_devices(prefix: &str, device_type: DeviceType) -> Result<Vec<DeviceInfo>, DeviceError> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = Vec::new();
    let entries = std::fs::read_dir("/dev")?;
    for entry in entries.flatten() {
        if let Ok(name) = entry.file_name().into_string() {
            if name.starts_with(prefix) {
                result.push(DeviceInfo {
                    name: name.clone(),
                    device_type: device_type.clone(),
                    status: DeviceStatus::Online,
                    path: format!("/dev/{}", name),
                    driver: String::new(),
                    last_seen: Some(now.clone()),
                });
            }
        }
    }
    Ok(result)
}

/// 读取 sysfs 设备的驱动名（解析 device/driver 符号链接）
#[cfg(target_os = "linux")]
fn read_sysfs_driver(sysfs_path: &str) -> String {
    let driver_link = format!("{}/device/driver", sysfs_path);
    std::fs::read_link(&driver_link)
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default()
}

/// 解析 uevent 消息（NULL 分隔的 KEY=VALUE 格式，首行为 ACTION@DEVPATH）
#[cfg(target_os = "linux")]
fn parse_uevent(data: &[u8]) -> Option<HotplugEvent> {
    let mut parts = data.split(|&b| b == 0);
    // 首行：ACTION@/dev/path
    let header = parts.next()?;
    let header_str = std::str::from_utf8(header).ok()?;
    let at_pos = header_str.find('@')?;
    let action_str = &header_str[..at_pos];
    let devpath = &header_str[at_pos + 1..];

    let action = match action_str {
        "add" => HotplugAction::Add,
        "remove" => HotplugAction::Remove,
        "change" => HotplugAction::Change,
        _ => return None,
    };

    let mut props = HashMap::new();
    let mut subsystem = String::new();
    let mut device_name = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if let Ok(s) = std::str::from_utf8(part) {
            if let Some(eq) = s.find('=') {
                let key = &s[..eq];
                let value = &s[eq + 1..];
                if key == "SUBSYSTEM" {
                    subsystem = value.to_string();
                } else if key == "DEVNAME" {
                    device_name = value.to_string();
                }
                props.insert(key.to_string(), value.to_string());
            }
        }
    }

    let device_type = match subsystem.as_str() {
        "net" => DeviceType::Net,
        "block" => DeviceType::Block,
        "usb" => DeviceType::Usb,
        "tty" => DeviceType::Serial,
        "gpio" => DeviceType::Gpio,
        "i2c" => DeviceType::I2c,
        "spi" => DeviceType::Spi,
        _ => DeviceType::Unknown,
    };

    if device_name.is_empty() {
        // 从 devpath 提取设备名
        if let Some(name) = devpath.rsplit('/').next() {
            device_name = name.to_string();
        }
    }

    Some(HotplugEvent {
        action,
        device_type,
        subsystem,
        device_name,
        properties: props,
    })
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotplug_event_serialization() {
        let event = HotplugEvent {
            action: HotplugAction::Add,
            device_type: DeviceType::Net,
            subsystem: "net".to_string(),
            device_name: "eth1".to_string(),
            properties: HashMap::new(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: HotplugEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.action, HotplugAction::Add);
        assert_eq!(deserialized.device_type, DeviceType::Net);
        assert_eq!(deserialized.device_name, "eth1");
    }

    #[test]
    fn test_device_type_serialization() {
        let json = serde_json::to_string(&DeviceType::Net).unwrap();
        assert_eq!(json, "\"net\"");
        let json = serde_json::to_string(&DeviceType::Usb).unwrap();
        assert_eq!(json, "\"usb\"");
    }

    #[test]
    fn test_hotplug_action_serialization() {
        let json = serde_json::to_string(&HotplugAction::Add).unwrap();
        assert_eq!(json, "\"add\"");
        let json = serde_json::to_string(&HotplugAction::Remove).unwrap();
        assert_eq!(json, "\"remove\"");
    }

    #[test]
    fn test_list_net_interfaces_returns_vec() {
        let result = DeviceManager::list_net_interfaces();
        assert!(result.is_ok());
    }

    #[test]
    fn test_device_exists_returns_bool() {
        let result = DeviceManager::device_exists("eth0");
        // 非 Linux 返回 false，Linux 可能 true 或 false
        let _ = result;
    }

    #[test]
    fn test_device_manager_new() {
        let mgr = DeviceManager::new();
        assert!(mgr.event_handlers.is_empty());
    }

    #[test]
    fn test_device_manager_on_event() {
        let mut mgr = DeviceManager::new();
        mgr.on_event(|_event| {
            // 处理器
        });
        assert_eq!(mgr.event_handlers.len(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_uevent_add_net() {
        let msg = b"add@/devices/pci/net/eth1\0ACTION=add\0SUBSYSTEM=net\0DEVNAME=eth1\0";
        let event = parse_uevent(msg).unwrap();
        assert_eq!(event.action, HotplugAction::Add);
        assert_eq!(event.device_type, DeviceType::Net);
        assert_eq!(event.subsystem, "net");
        assert_eq!(event.device_name, "eth1");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_uevent_remove() {
        let msg = b"remove@/devices/usb/usb1\0ACTION=remove\0SUBSYSTEM=usb\0";
        let event = parse_uevent(msg).unwrap();
        assert_eq!(event.action, HotplugAction::Remove);
        assert_eq!(event.device_type, DeviceType::Usb);
    }

    #[test]
    fn test_device_type_new_variants_serialization() {
        assert_eq!(serde_json::to_string(&DeviceType::Serial).unwrap(), "\"serial\"");
        assert_eq!(serde_json::to_string(&DeviceType::Gpio).unwrap(), "\"gpio\"");
        assert_eq!(serde_json::to_string(&DeviceType::I2c).unwrap(), "\"i2c\"");
        assert_eq!(serde_json::to_string(&DeviceType::Spi).unwrap(), "\"spi\"");
        // 反序列化往返
        let dt: DeviceType = serde_json::from_str("\"serial\"").unwrap();
        assert_eq!(dt, DeviceType::Serial);
    }

    #[test]
    fn test_device_status_serialization() {
        assert_eq!(serde_json::to_string(&DeviceStatus::Online).unwrap(), "\"online\"");
        assert_eq!(serde_json::to_string(&DeviceStatus::Offline).unwrap(), "\"offline\"");
        assert_eq!(serde_json::to_string(&DeviceStatus::Error).unwrap(), "\"error\"");
        let s: DeviceStatus = serde_json::from_str("\"offline\"").unwrap();
        assert_eq!(s, DeviceStatus::Offline);
    }

    #[test]
    fn test_device_info_serialization() {
        let info = DeviceInfo {
            name: "ttyS0".to_string(),
            device_type: DeviceType::Serial,
            status: DeviceStatus::Online,
            path: "/dev/ttyS0".to_string(),
            driver: "8250".to_string(),
            last_seen: Some("2026-01-01T00:00:00+00:00".to_string()),
        };
        let json = serde_json::to_string(&info).unwrap();
        let de: DeviceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(de.name, "ttyS0");
        assert_eq!(de.device_type, DeviceType::Serial);
        assert_eq!(de.status, DeviceStatus::Online);
        assert_eq!(de.path, "/dev/ttyS0");
        assert_eq!(de.driver, "8250");
        assert!(de.last_seen.is_some());
    }

    #[test]
    fn test_device_config_default() {
        let config = DeviceConfig::default();
        assert!(config.devices.is_empty());
    }

    #[test]
    fn test_device_config_parse() {
        let toml_str = r#"
[[devices]]
name = "ttyS0"
device_type = "serial"
path = "/dev/ttyS0"
enabled = true

[devices.config]
baud_rate = "115200"
parity = "none"
"#;
        let config: DeviceConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.devices.len(), 1);
        let rule = &config.devices[0];
        assert_eq!(rule.name, "ttyS0");
        assert_eq!(rule.device_type, DeviceType::Serial);
        assert_eq!(rule.path, "/dev/ttyS0");
        assert!(rule.enabled);
        assert_eq!(rule.config.get("baud_rate"), Some(&"115200".to_string()));
        assert_eq!(rule.config.get("parity"), Some(&"none".to_string()));
    }

    #[test]
    fn test_device_rule_serialization() {
        let mut config_map = HashMap::new();
        config_map.insert("baud_rate".to_string(), "9600".to_string());
        let rule = DeviceRule {
            name: "ttyUSB0".to_string(),
            device_type: DeviceType::Serial,
            path: "/dev/ttyUSB0".to_string(),
            enabled: false,
            config: config_map,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let de: DeviceRule = serde_json::from_str(&json).unwrap();
        assert_eq!(de.name, "ttyUSB0");
        assert!(!de.enabled);
        assert_eq!(de.config.get("baud_rate"), Some(&"9600".to_string()));
    }

    #[test]
    fn test_list_serial_devices_returns_vec() {
        let result = DeviceManager::list_serial_devices();
        #[cfg(target_os = "linux")]
        {
            assert!(result.is_ok());
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(matches!(result, Err(DeviceError::UnsupportedPlatform)));
        }
    }

    #[test]
    fn test_list_all_devices_returns_vec() {
        let result = DeviceManager::list_all_devices();
        #[cfg(target_os = "linux")]
        {
            assert!(result.is_ok());
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(matches!(result, Err(DeviceError::UnsupportedPlatform)));
        }
    }

    #[test]
    fn test_device_manager_devices_empty() {
        let mgr = DeviceManager::new();
        assert!(mgr.devices().is_empty());
    }

    #[test]
    fn test_update_device_state_add_remove() {
        let mut mgr = DeviceManager::new();
        let event = HotplugEvent {
            action: HotplugAction::Add,
            device_type: DeviceType::Serial,
            subsystem: "tty".to_string(),
            device_name: "ttyS0".to_string(),
            properties: {
                let mut m = HashMap::new();
                m.insert("DEVPATH".to_string(), "/devices/tty/ttyS0".to_string());
                m
            },
        };
        mgr.update_device_state(&event);
        assert_eq!(mgr.devices().len(), 1);
        let info = mgr.devices().into_iter().next().unwrap();
        assert_eq!(info.name, "ttyS0");
        assert_eq!(info.status, DeviceStatus::Online);

        // Remove 事件将状态置为 Offline
        let remove_event = HotplugEvent {
            action: HotplugAction::Remove,
            device_type: DeviceType::Serial,
            subsystem: "tty".to_string(),
            device_name: "ttyS0".to_string(),
            properties: HashMap::new(),
        };
        mgr.update_device_state(&remove_event);
        let info = mgr.devices().into_iter().next().unwrap();
        assert_eq!(info.status, DeviceStatus::Offline);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_uevent_tty_serial() {
        let msg = b"add@/devices/tty/ttyS0\0ACTION=add\0SUBSYSTEM=tty\0DEVNAME=ttyS0\0";
        let event = parse_uevent(msg).unwrap();
        assert_eq!(event.device_type, DeviceType::Serial);
        assert_eq!(event.subsystem, "tty");
    }
}
