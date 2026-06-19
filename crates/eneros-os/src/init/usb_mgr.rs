//! USB 设备管理（USB Device Manager）
//!
//! 提供 USB 白名单（授权策略）持久化、USB 串口适配器扫描与 sysfs 设备授权能力。
//! Linux 通过 sysfs `/sys/bus/usb/devices/` 枚举与授权；非 Linux 返回 `UnsupportedPlatform`。
//! 白名单配置与适配器信息为跨平台纯数据结构，可序列化测试。

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// USB 管理错误
#[derive(Debug, Error)]
pub enum UsbMgrError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// USB 白名单规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbWhitelistRule {
    pub vendor_id: String,
    pub product_id: String,
    pub description: String,
    pub authorized: bool,
}

/// USB 白名单（授权策略集合，可持久化到 TOML）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbWhitelist {
    #[serde(default)]
    rules: Vec<UsbWhitelistRule>,
}

impl UsbWhitelist {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// 从 TOML 文件加载白名单
    pub fn load(path: &Path) -> Result<Self, UsbMgrError> {
        let content = std::fs::read_to_string(path)?;
        let wl: UsbWhitelist =
            toml::from_str(&content).map_err(|e| UsbMgrError::Config(e.to_string()))?;
        Ok(wl)
    }

    /// 保存白名单到 TOML 文件
    pub fn save(&self, path: &Path) -> Result<(), UsbMgrError> {
        let content =
            toml::to_string_pretty(self).map_err(|e| UsbMgrError::Config(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// 查询指定 VID/PID 是否授权（大小写不敏感）
    pub fn is_authorized(&self, vendor_id: &str, product_id: &str) -> bool {
        self.rules.iter().any(|r| {
            r.authorized
                && r.vendor_id.eq_ignore_ascii_case(vendor_id)
                && r.product_id.eq_ignore_ascii_case(product_id)
        })
    }

    /// 添加白名单规则（VID/PID 大小写不敏感匹配；若已存在则覆盖）
    pub fn add_rule(&mut self, rule: UsbWhitelistRule) {
        if let Some(existing) = self.rules.iter_mut().find(|r| {
            r.vendor_id.eq_ignore_ascii_case(&rule.vendor_id)
                && r.product_id.eq_ignore_ascii_case(&rule.product_id)
        }) {
            *existing = rule;
        } else {
            self.rules.push(rule);
        }
    }

    /// 移除指定 VID/PID 的规则（大小写不敏感）
    pub fn remove_rule(&mut self, vendor_id: &str, product_id: &str) {
        self.rules.retain(|r| {
            !(r.vendor_id.eq_ignore_ascii_case(vendor_id)
                && r.product_id.eq_ignore_ascii_case(product_id))
        });
    }

    /// 获取所有规则
    pub fn rules(&self) -> &[UsbWhitelistRule] {
        &self.rules
    }
}

impl Default for UsbWhitelist {
    fn default() -> Self {
        Self::new()
    }
}

/// USB 串口适配器信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbSerialAdapter {
    pub vendor_id: String,
    pub product_id: String,
    pub tty_device: String,
    pub driver: String,
}

/// 扫描已绑定的 USB 串口适配器。
///
/// Linux 下遍历 `/sys/bus/usb/devices/`，保留含 `tty` 子目录的设备，
/// 并对每个 tty 接口生成一条 [`UsbSerialAdapter`] 记录。
#[cfg(target_os = "linux")]
pub fn list_usb_serial_adapters() -> Result<Vec<UsbSerialAdapter>, UsbMgrError> {
    let mut result = Vec::new();
    let entries = std::fs::read_dir("/sys/bus/usb/devices")?;
    for entry in entries.flatten() {
        let dev_path = entry.path();
        let tty_dir = dev_path.join("tty");
        if !tty_dir.is_dir() {
            continue;
        }
        // 仅保留含 idVendor 的真实设备（与 devmgr::list_usb_devices 一致）
        let vendor_id = match std::fs::read_to_string(dev_path.join("idVendor")) {
            Ok(s) => s.trim().to_string(),
            Err(_) => continue,
        };
        let product_id = std::fs::read_to_string(dev_path.join("idProduct"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        // 遍历 tty 子目录下的实际 tty 设备（如 ttyUSB0）
        let tty_entries = match std::fs::read_dir(&tty_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for tty_entry in tty_entries.flatten() {
            if let Ok(tty_name) = tty_entry.file_name().into_string() {
                let driver = read_driver_link(&tty_entry.path());
                result.push(UsbSerialAdapter {
                    vendor_id: vendor_id.clone(),
                    product_id: product_id.clone(),
                    tty_device: format!("/dev/{}", tty_name),
                    driver,
                });
            }
        }
    }
    Ok(result)
}

#[cfg(not(target_os = "linux"))]
pub fn list_usb_serial_adapters() -> Result<Vec<UsbSerialAdapter>, UsbMgrError> {
    Err(UsbMgrError::UnsupportedPlatform)
}

/// 读取 sysfs tty 设备的驱动名（解析 `driver` 符号链接）
#[cfg(target_os = "linux")]
fn read_driver_link(tty_sysfs_path: &Path) -> String {
    std::fs::read_link(tty_sysfs_path.join("driver"))
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default()
}

/// USB 设备授权（Linux sysfs：写 `authorized` 文件）。
///
/// `sysfs_path` 为设备目录完整路径（如 `/sys/bus/usb/devices/1-2`），
/// 函数向 `{sysfs_path}/authorized` 写入 "1" 或 "0"。
#[cfg(target_os = "linux")]
pub fn authorize_usb_device(sysfs_path: &str, authorized: bool) -> Result<(), UsbMgrError> {
    let path = format!("{}/authorized", sysfs_path.trim_end_matches('/'));
    std::fs::write(&path, if authorized { "1" } else { "0" })?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn authorize_usb_device(_sysfs_path: &str, _authorized: bool) -> Result<(), UsbMgrError> {
    Err(UsbMgrError::UnsupportedPlatform)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usb_whitelist_rule_serialization() {
        let rule = UsbWhitelistRule {
            vendor_id: "10c4".to_string(),
            product_id: "ea60".to_string(),
            description: "CP210x UART".to_string(),
            authorized: true,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let de: UsbWhitelistRule = serde_json::from_str(&json).unwrap();
        assert_eq!(de.vendor_id, "10c4");
        assert_eq!(de.product_id, "ea60");
        assert_eq!(de.description, "CP210x UART");
        assert!(de.authorized);
    }

    #[test]
    fn test_usb_whitelist_new_empty() {
        let wl = UsbWhitelist::new();
        assert!(wl.rules().is_empty());
        assert!(!wl.is_authorized("10c4", "ea60"));
    }

    #[test]
    fn test_usb_whitelist_add_is_authorized_remove() {
        let mut wl = UsbWhitelist::new();
        wl.add_rule(UsbWhitelistRule {
            vendor_id: "10c4".to_string(),
            product_id: "ea60".to_string(),
            description: "CP210x".to_string(),
            authorized: true,
        });
        assert_eq!(wl.rules().len(), 1);
        assert!(wl.is_authorized("10c4", "ea60"));
        assert!(!wl.is_authorized("10c4", "0000"));

        // authorized=false 的规则不视为授权
        wl.add_rule(UsbWhitelistRule {
            vendor_id: "abcd".to_string(),
            product_id: "1234".to_string(),
            description: "denied".to_string(),
            authorized: false,
        });
        assert!(!wl.is_authorized("abcd", "1234"));

        // 移除已授权规则
        wl.remove_rule("10c4", "ea60");
        assert!(!wl.is_authorized("10c4", "ea60"));
        assert_eq!(wl.rules().len(), 1);
    }

    #[test]
    fn test_usb_whitelist_load_save() {
        let mut path = std::env::temp_dir();
        path.push(format!("eneros_usb_wl_test_{}.toml", std::process::id()));
        // 清理可能的历史残留
        let _ = std::fs::remove_file(&path);

        let mut wl = UsbWhitelist::new();
        wl.add_rule(UsbWhitelistRule {
            vendor_id: "10c4".to_string(),
            product_id: "ea60".to_string(),
            description: "CP210x".to_string(),
            authorized: true,
        });

        wl.save(&path).expect("save should succeed");
        let loaded = UsbWhitelist::load(&path).expect("load should succeed");
        assert_eq!(loaded.rules().len(), 1);
        assert_eq!(loaded.rules()[0].vendor_id, "10c4");
        assert_eq!(loaded.rules()[0].product_id, "ea60");
        assert_eq!(loaded.rules()[0].description, "CP210x");
        assert!(loaded.is_authorized("10c4", "ea60"));

        // 空白名单往返
        let empty = UsbWhitelist::new();
        empty.save(&path).expect("save empty");
        let loaded_empty = UsbWhitelist::load(&path).expect("load empty");
        assert!(loaded_empty.rules().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_usb_whitelist_case_insensitive() {
        let mut wl = UsbWhitelist::new();
        wl.add_rule(UsbWhitelistRule {
            vendor_id: "10C4".to_string(),
            product_id: "EA60".to_string(),
            description: "CP210x".to_string(),
            authorized: true,
        });
        assert!(wl.is_authorized("10c4", "ea60"));
        assert!(wl.is_authorized("10C4", "EA60"));
        // 大小写不敏感移除
        wl.remove_rule("10c4", "ea60");
        assert!(wl.rules().is_empty());
    }

    #[test]
    fn test_usb_whitelist_add_overrides_existing() {
        let mut wl = UsbWhitelist::new();
        wl.add_rule(UsbWhitelistRule {
            vendor_id: "10c4".to_string(),
            product_id: "ea60".to_string(),
            description: "v1".to_string(),
            authorized: false,
        });
        assert!(!wl.is_authorized("10c4", "ea60"));

        // 同 VID/PID（大小写不同）覆盖
        wl.add_rule(UsbWhitelistRule {
            vendor_id: "10C4".to_string(),
            product_id: "EA60".to_string(),
            description: "v2".to_string(),
            authorized: true,
        });
        assert_eq!(wl.rules().len(), 1);
        assert!(wl.is_authorized("10c4", "ea60"));
        assert_eq!(wl.rules()[0].description, "v2");
    }

    #[test]
    fn test_usb_serial_adapter_serialization() {
        let adapter = UsbSerialAdapter {
            vendor_id: "067b".to_string(),
            product_id: "2303".to_string(),
            tty_device: "/dev/ttyUSB0".to_string(),
            driver: "pl2303".to_string(),
        };
        let json = serde_json::to_string(&adapter).unwrap();
        let de: UsbSerialAdapter = serde_json::from_str(&json).unwrap();
        assert_eq!(de.vendor_id, "067b");
        assert_eq!(de.product_id, "2303");
        assert_eq!(de.tty_device, "/dev/ttyUSB0");
        assert_eq!(de.driver, "pl2303");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_list_usb_serial_adapters_unsupported() {
        let r = list_usb_serial_adapters();
        assert!(matches!(r, Err(UsbMgrError::UnsupportedPlatform)));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_authorize_usb_device_unsupported() {
        let r = authorize_usb_device("/sys/bus/usb/devices/1-2", true);
        assert!(matches!(r, Err(UsbMgrError::UnsupportedPlatform)));
    }
}
