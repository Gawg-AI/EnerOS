//! 串口设备管理（Serial Device Manager）
//!
//! 提供电力协议串口配置预设、串口独占访问控制与串口故障检测能力。
//! Linux 通过 flock 实现独占访问；非 Linux 平台返回 `UnsupportedPlatform`。
//! 配置预设、健康状态等纯数据结构跨平台可测。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

use crate::hal::{FlowControl, Parity, SerialConfig};

/// 串口管理错误
#[derive(Debug, Error)]
pub enum SerialMgrError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("device locked: {0}")]
    Locked(String),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// 可序列化的串口配置镜像。
///
/// `hal::SerialConfig` 仅派生 `Debug, Clone`（未实现 Serialize/Deserialize），
/// 且 hal 模块冻结不可修改，故此处定义字段一致的镜像结构，
/// 用于 `SerialPreset::Custom` 预设的持久化。通过 `From` 双向转换与 HAL 层互通。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialConfigData {
    pub baud_rate: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: Parity,
    pub flow_control: FlowControl,
    /// 读超时（毫秒）；None 表示阻塞模式
    pub timeout_ms: Option<u32>,
}

impl From<SerialConfig> for SerialConfigData {
    fn from(c: SerialConfig) -> Self {
        Self {
            baud_rate: c.baud_rate,
            data_bits: c.data_bits,
            stop_bits: c.stop_bits,
            parity: c.parity,
            flow_control: c.flow_control,
            timeout_ms: c.timeout_ms,
        }
    }
}

impl From<SerialConfigData> for SerialConfig {
    fn from(d: SerialConfigData) -> Self {
        Self {
            baud_rate: d.baud_rate,
            data_bits: d.data_bits,
            stop_bits: d.stop_bits,
            parity: d.parity,
            flow_control: d.flow_control,
            timeout_ms: d.timeout_ms,
        }
    }
}

/// 串口配置模板（电力协议预设）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "preset")]
pub enum SerialPreset {
    /// IEC 104 FT 1.2：9600/8/N/1/None
    Iec104Ft12,
    /// Modbus RTU：9600/8/E/1/None（偶校验）
    ModbusRtu,
    /// Modbus RTU 高速：115200/8/N/1/None
    ModbusRtuHigh,
    /// 自定义
    Custom(SerialConfigData),
}

impl SerialPreset {
    /// 将预设转换为 HAL 层 [`SerialConfig`]
    pub fn to_config(&self) -> SerialConfig {
        match self {
            SerialPreset::Iec104Ft12 => SerialConfig {
                baud_rate: 9600,
                data_bits: 8,
                stop_bits: 1,
                parity: Parity::None,
                flow_control: FlowControl::None,
                timeout_ms: None,
            },
            SerialPreset::ModbusRtu => SerialConfig {
                baud_rate: 9600,
                data_bits: 8,
                stop_bits: 1,
                parity: Parity::Even,
                flow_control: FlowControl::None,
                timeout_ms: None,
            },
            SerialPreset::ModbusRtuHigh => SerialConfig {
                baud_rate: 115_200,
                data_bits: 8,
                stop_bits: 1,
                parity: Parity::None,
                flow_control: FlowControl::None,
                timeout_ms: None,
            },
            SerialPreset::Custom(data) => data.clone().into(),
        }
    }
}

/// 串口独占访问管理器（Linux 使用 flock LOCK_EX|LOCK_NB）
pub struct SerialAccessControl {
    locked_ports: HashMap<String, std::fs::File>,
}

impl SerialAccessControl {
    pub fn new() -> Self {
        Self {
            locked_ports: HashMap::new(),
        }
    }

    /// 获取串口独占访问（Linux: flock LOCK_EX|LOCK_NB）。
    /// 成功后持有设备文件描述符，drop 时自动释放锁。
    #[cfg(target_os = "linux")]
    pub fn acquire(&mut self, device: &str) -> Result<(), SerialMgrError> {
        use std::os::unix::io::AsRawFd;

        if self.locked_ports.contains_key(device) {
            return Err(SerialMgrError::Locked(device.to_string()));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(device)?;
        // flock 常量：LOCK_EX=2, LOCK_NB=4
        const LOCK_EX: libc::c_int = 2;
        const LOCK_NB: libc::c_int = 4;
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, LOCK_EX | LOCK_NB) };
        if ret < 0 {
            // 已被其它进程锁定或失败
            return Err(SerialMgrError::Locked(device.to_string()));
        }
        self.locked_ports.insert(device.to_string(), file);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn acquire(&mut self, _device: &str) -> Result<(), SerialMgrError> {
        Err(SerialMgrError::UnsupportedPlatform)
    }

    /// 释放串口独占访问（drop 内部 File 自动释放 flock）
    pub fn release(&mut self, device: &str) {
        self.locked_ports.remove(device);
    }

    /// 查询串口是否已被本管理器独占
    pub fn is_locked(&self, device: &str) -> bool {
        self.locked_ports.contains_key(device)
    }
}

impl Default for SerialAccessControl {
    fn default() -> Self {
        Self::new()
    }
}

/// 串口健康状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerialHealth {
    Healthy,
    Degraded,
    Failed,
}

/// 错误计数阈值：达到此值降级为 Degraded
const DEGRADED_THRESHOLD: u32 = 3;
/// 错误计数阈值：达到此值标记为 Failed
const FAILED_THRESHOLD: u32 = 10;

/// 串口故障检测与恢复监控器（跨平台纯状态管理）。
///
/// 错误计数达到 [`DEGRADED_THRESHOLD`] → `Degraded`，
/// 达到 [`FAILED_THRESHOLD`] → `Failed`；一次成功通信重置计数并恢复 `Healthy`。
pub struct SerialMonitor {
    ports: HashMap<String, SerialHealth>,
    error_counts: HashMap<String, u32>,
}

impl SerialMonitor {
    pub fn new() -> Self {
        Self {
            ports: HashMap::new(),
            error_counts: HashMap::new(),
        }
    }

    /// 注册串口设备（初始状态 Healthy）
    pub fn register(&mut self, device: &str) {
        self.ports
            .insert(device.to_string(), SerialHealth::Healthy);
        self.error_counts.insert(device.to_string(), 0);
    }

    /// 记录一次成功通信（重置错误计数，状态恢复 Healthy）
    pub fn record_success(&mut self, device: &str) {
        if let Some(count) = self.error_counts.get_mut(device) {
            *count = 0;
        }
        if self.ports.contains_key(device) {
            self.ports
                .insert(device.to_string(), SerialHealth::Healthy);
        }
    }

    /// 记录一次错误通信，返回更新后的健康状态
    pub fn record_error(&mut self, device: &str) -> SerialHealth {
        let count = self.error_counts.entry(device.to_string()).or_insert(0);
        *count += 1;
        let new_health = if *count >= FAILED_THRESHOLD {
            SerialHealth::Failed
        } else if *count >= DEGRADED_THRESHOLD {
            SerialHealth::Degraded
        } else {
            SerialHealth::Healthy
        };
        self.ports.insert(device.to_string(), new_health.clone());
        new_health
    }

    /// 查询串口当前健康状态（未注册返回 Healthy）
    pub fn health(&self, device: &str) -> SerialHealth {
        self.ports
            .get(device)
            .cloned()
            .unwrap_or(SerialHealth::Healthy)
    }

    /// 列出所有非 Healthy 的串口
    pub fn list_unhealthy(&self) -> Vec<(String, SerialHealth)> {
        self.ports
            .iter()
            .filter(|(_, h)| **h != SerialHealth::Healthy)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

impl Default for SerialMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_preset_iec104_ft12() {
        let cfg = SerialPreset::Iec104Ft12.to_config();
        assert_eq!(cfg.baud_rate, 9600);
        assert_eq!(cfg.data_bits, 8);
        assert_eq!(cfg.stop_bits, 1);
        assert_eq!(cfg.parity, Parity::None);
        assert_eq!(cfg.flow_control, FlowControl::None);
        assert_eq!(cfg.timeout_ms, None);
    }

    #[test]
    fn test_serial_preset_modbus_rtu() {
        let cfg = SerialPreset::ModbusRtu.to_config();
        assert_eq!(cfg.baud_rate, 9600);
        assert_eq!(cfg.data_bits, 8);
        assert_eq!(cfg.stop_bits, 1);
        assert_eq!(cfg.parity, Parity::Even);
        assert_eq!(cfg.flow_control, FlowControl::None);
        assert_eq!(cfg.timeout_ms, None);
    }

    #[test]
    fn test_serial_preset_modbus_rtu_high() {
        let cfg = SerialPreset::ModbusRtuHigh.to_config();
        assert_eq!(cfg.baud_rate, 115_200);
        assert_eq!(cfg.data_bits, 8);
        assert_eq!(cfg.stop_bits, 1);
        assert_eq!(cfg.parity, Parity::None);
        assert_eq!(cfg.flow_control, FlowControl::None);
        assert_eq!(cfg.timeout_ms, None);
    }

    #[test]
    fn test_serial_preset_custom() {
        let data = SerialConfigData {
            baud_rate: 19_200,
            data_bits: 7,
            stop_bits: 2,
            parity: Parity::Odd,
            flow_control: FlowControl::Hardware,
            timeout_ms: Some(500),
        };
        let preset = SerialPreset::Custom(data.clone());
        let cfg = preset.to_config();
        assert_eq!(cfg.baud_rate, 19_200);
        assert_eq!(cfg.data_bits, 7);
        assert_eq!(cfg.stop_bits, 2);
        assert_eq!(cfg.parity, Parity::Odd);
        assert_eq!(cfg.flow_control, FlowControl::Hardware);
        assert_eq!(cfg.timeout_ms, Some(500));
        // SerialConfigData -> SerialConfig -> SerialConfigData 往返
        let back: SerialConfigData = cfg.into();
        assert_eq!(back, data);
    }

    #[test]
    fn test_serial_access_control_new_and_is_locked() {
        let ac = SerialAccessControl::new();
        assert!(!ac.is_locked("/dev/ttyS0"));
        // release 对未持有设备是 no-op
        let mut ac = ac;
        ac.release("/dev/ttyS0");
        assert!(!ac.is_locked("/dev/ttyS0"));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_serial_access_control_acquire_unsupported() {
        let mut ac = SerialAccessControl::new();
        let r = ac.acquire("/dev/ttyS0");
        assert!(matches!(r, Err(SerialMgrError::UnsupportedPlatform)));
        // acquire 失败不应标记为锁定
        assert!(!ac.is_locked("/dev/ttyS0"));
    }

    #[test]
    fn test_serial_monitor_register_and_health() {
        let mut m = SerialMonitor::new();
        assert_eq!(m.health("/dev/ttyS0"), SerialHealth::Healthy);
        m.register("/dev/ttyS0");
        assert_eq!(m.health("/dev/ttyS0"), SerialHealth::Healthy);
        assert!(m.list_unhealthy().is_empty());
    }

    #[test]
    fn test_serial_monitor_error_thresholds() {
        let mut m = SerialMonitor::new();
        m.register("/dev/ttyS0");
        // 1-2 次：Healthy
        assert_eq!(m.record_error("/dev/ttyS0"), SerialHealth::Healthy);
        assert_eq!(m.record_error("/dev/ttyS0"), SerialHealth::Healthy);
        // 3-9 次：Degraded
        for _ in 3..=9 {
            assert_eq!(m.record_error("/dev/ttyS0"), SerialHealth::Degraded);
        }
        // 10 次：Failed
        assert_eq!(m.record_error("/dev/ttyS0"), SerialHealth::Failed);
        assert_eq!(m.health("/dev/ttyS0"), SerialHealth::Failed);
    }

    #[test]
    fn test_serial_monitor_record_success_resets() {
        let mut m = SerialMonitor::new();
        m.register("/dev/ttyS0");
        // 制造 Degraded
        for _ in 0..3 {
            m.record_error("/dev/ttyS0");
        }
        assert_eq!(m.health("/dev/ttyS0"), SerialHealth::Degraded);
        // 一次成功 → 重置为 Healthy
        m.record_success("/dev/ttyS0");
        assert_eq!(m.health("/dev/ttyS0"), SerialHealth::Healthy);
        // 再次错误从 0 开始计数，2 次仍 Healthy
        m.record_error("/dev/ttyS0");
        m.record_error("/dev/ttyS0");
        assert_eq!(m.health("/dev/ttyS0"), SerialHealth::Healthy);
    }

    #[test]
    fn test_serial_monitor_list_unhealthy() {
        let mut m = SerialMonitor::new();
        m.register("/dev/ttyS0");
        m.register("/dev/ttyS1");
        // ttyS0 制造 Degraded
        for _ in 0..3 {
            m.record_error("/dev/ttyS0");
        }
        let unhealthy = m.list_unhealthy();
        assert_eq!(unhealthy.len(), 1);
        assert_eq!(unhealthy[0].0, "/dev/ttyS0");
        assert_eq!(unhealthy[0].1, SerialHealth::Degraded);
    }

    #[test]
    fn test_serial_health_serialization() {
        assert_eq!(
            serde_json::to_string(&SerialHealth::Healthy).unwrap(),
            "\"healthy\""
        );
        assert_eq!(
            serde_json::to_string(&SerialHealth::Degraded).unwrap(),
            "\"degraded\""
        );
        assert_eq!(
            serde_json::to_string(&SerialHealth::Failed).unwrap(),
            "\"failed\""
        );
        for h in [
            SerialHealth::Healthy,
            SerialHealth::Degraded,
            SerialHealth::Failed,
        ] {
            let json = serde_json::to_string(&h).unwrap();
            let de: SerialHealth = serde_json::from_str(&json).unwrap();
            assert_eq!(de, h);
        }
    }

    #[test]
    fn test_serial_preset_serialization_roundtrip() {
        let presets = vec![
            SerialPreset::Iec104Ft12,
            SerialPreset::ModbusRtu,
            SerialPreset::ModbusRtuHigh,
            SerialPreset::Custom(SerialConfigData {
                baud_rate: 38_400,
                data_bits: 8,
                stop_bits: 1,
                parity: Parity::None,
                flow_control: FlowControl::None,
                timeout_ms: Some(100),
            }),
        ];
        for p in presets {
            let json = serde_json::to_string(&p).unwrap();
            let de: SerialPreset = serde_json::from_str(&json).unwrap();
            // SerialConfig 未派生 PartialEq，经镜像结构比较
            let orig: SerialConfigData = p.to_config().into();
            let restored: SerialConfigData = de.to_config().into();
            assert_eq!(restored, orig);
        }
    }

    #[test]
    fn test_serial_config_data_roundtrip() {
        let cfg = SerialConfig {
            baud_rate: 57_600,
            data_bits: 8,
            stop_bits: 1,
            parity: Parity::Even,
            flow_control: FlowControl::Software,
            timeout_ms: Some(250),
        };
        let data: SerialConfigData = cfg.clone().into();
        let cfg2: SerialConfig = data.into();
        assert_eq!(cfg2.baud_rate, cfg.baud_rate);
        assert_eq!(cfg2.data_bits, cfg.data_bits);
        assert_eq!(cfg2.stop_bits, cfg.stop_bits);
        assert_eq!(cfg2.parity, cfg.parity);
        assert_eq!(cfg2.flow_control, cfg.flow_control);
        assert_eq!(cfg2.timeout_ms, cfg.timeout_ms);
    }
}
