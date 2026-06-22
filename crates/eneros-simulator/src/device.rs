//! 设备模拟器
//!
//! 模拟 RTU/IED/保护装置等电力设备的行为，支持 IEC 104 与 Modbus 协议请求响应，
//! 并提供保护动作（过流、欠压、频率异常）的模拟。
//!
//! ## 功能
//!
//! - 按设备类型（[`DeviceType`]）与协议配置设备模拟器
//! - 响应 IEC 104 / Modbus 协议读写请求（[`ProtocolRequest`] / [`ProtocolResponse`]）
//! - 通过 [`DeviceSimulator::inject_state`] 注入状态变化并触发保护逻辑
//! - 提供保护状态查询与复位接口
//!
//! ## 设计说明
//!
//! 设备数据点以 `point_id -> f64` 形式存储于内部 `HashMap`。协议请求按
//! `iec104_{asdu}_{io}` 或 `modbus_{slave}_{addr}` 的键名约定映射到数据点，
//! 使同一份状态可被多种协议访问。保护逻辑在状态注入时同步触发，便于
//! 场景脚本通过注入异常值验证保护动作。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 设备类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    /// 远动终端单元
    Rtu,
    /// 智能电子设备
    Ied,
    /// 保护装置
    ProtectionRelay,
    /// 开关设备
    Switch,
    /// 变压器
    Transformer,
}

/// 设备模拟器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSimulatorConfig {
    /// 设备 ID
    pub device_id: String,
    /// 设备类型
    pub device_type: DeviceType,
    /// 协议类型（iec104/iec61850/modbus）
    pub protocol: String,
    /// 设备地址
    pub address: String,
    /// 初始数据点
    #[serde(default)]
    pub initial_points: HashMap<String, f64>,
}

/// 设备模拟器
///
/// 模拟单一电力设备的状态与协议响应，并维护保护动作状态。
pub struct DeviceSimulator {
    /// 设备配置
    config: DeviceSimulatorConfig,
    /// 当前数据点（point_id -> value）
    points: HashMap<String, f64>,
    /// 保护装置状态
    protection_state: ProtectionState,
}

/// 保护装置状态
#[derive(Debug, Clone, Default)]
pub struct ProtectionState {
    /// 是否跳闸
    pub tripped: bool,
    /// 跳闸原因
    pub trip_reason: Option<String>,
    /// 过流保护阈值
    pub overcurrent_threshold: f64,
    /// 欠压保护阈值
    pub undervoltage_threshold: f64,
    /// 频率异常阈值
    pub frequency_deviation_threshold: f64,
}

/// 协议请求
#[derive(Debug, Clone)]
pub enum ProtocolRequest {
    /// IEC 104 读请求
    Iec104Read { asdu_addr: u16, io_addr: u16 },
    /// IEC 104 写请求
    Iec104Write {
        asdu_addr: u16,
        io_addr: u16,
        value: f64,
    },
    /// Modbus 读保持寄存器
    ModbusReadHolding {
        slave_id: u8,
        addr: u16,
        count: u16,
    },
    /// Modbus 写单个寄存器
    ModbusWriteSingle {
        slave_id: u8,
        addr: u16,
        value: u16,
    },
}

/// 协议响应
#[derive(Debug, Clone)]
pub enum ProtocolResponse {
    /// 读成功
    ReadOk { values: Vec<f64> },
    /// 写成功
    WriteOk,
    /// 错误
    Error { code: u8, message: String },
}

impl DeviceSimulator {
    /// 创建设备模拟器
    ///
    /// 以配置中的初始数据点初始化内部状态，并设置默认保护阈值：
    /// - 过流：1000 A
    /// - 欠压：0.85 p.u.
    /// - 频率偏差：0.5 Hz
    pub fn new(config: DeviceSimulatorConfig) -> Self {
        let points = config.initial_points.clone();
        Self {
            config,
            points,
            protection_state: ProtectionState {
                overcurrent_threshold: 1000.0, // A
                undervoltage_threshold: 0.85,  // p.u.
                frequency_deviation_threshold: 0.5, // Hz
                ..Default::default()
            },
        }
    }

    /// 响应协议请求
    ///
    /// 根据 IEC 104 / Modbus 协议约定映射数据点键名，执行读写操作。
    /// 读请求未命中的数据点返回 0.0。
    pub fn respond(&mut self, request: &ProtocolRequest) -> ProtocolResponse {
        match request {
            ProtocolRequest::Iec104Read { asdu_addr, io_addr } => {
                let key = format!("iec104_{}_{}", asdu_addr, io_addr);
                let value = self.points.get(&key).copied().unwrap_or(0.0);
                ProtocolResponse::ReadOk {
                    values: vec![value],
                }
            }
            ProtocolRequest::Iec104Write {
                asdu_addr,
                io_addr,
                value,
            } => {
                let key = format!("iec104_{}_{}", asdu_addr, io_addr);
                self.points.insert(key, *value);
                ProtocolResponse::WriteOk
            }
            ProtocolRequest::ModbusReadHolding {
                slave_id,
                addr,
                count,
            } => {
                let mut values = Vec::with_capacity(*count as usize);
                for i in 0..*count {
                    // 使用 u32 计算地址，避免 addr + i 在 u16 域溢出
                    // （当 addr + count > 65535 时，u16 加法会 panic 或 wrap-around）
                    let key = format!("modbus_{}_{}", slave_id, *addr as u32 + i as u32);
                    values.push(self.points.get(&key).copied().unwrap_or(0.0));
                }
                ProtocolResponse::ReadOk { values }
            }
            ProtocolRequest::ModbusWriteSingle {
                slave_id,
                addr,
                value,
            } => {
                let key = format!("modbus_{}_{}", slave_id, addr);
                self.points.insert(key, *value as f64);
                ProtocolResponse::WriteOk
            }
        }
    }

    /// 注入状态变化
    ///
    /// 更新指定数据点，并按数据点名称（current/voltage/frequency）触发保护检查。
    pub fn inject_state(&mut self, point_id: &str, value: f64) {
        self.points.insert(point_id.to_string(), value);
        // 检查保护动作
        self.check_protection(point_id, value);
    }

    /// 检查保护动作
    ///
    /// 根据数据点名称关键字匹配保护类型：
    /// - `current`：过流保护（值 > 阈值）
    /// - `voltage`：欠压保护（值 < 阈值）
    /// - `frequency`：频率异常（|值 - 50| > 阈值）
    fn check_protection(&mut self, point_id: &str, value: f64) {
        // 过流保护
        if point_id.contains("current") && value > self.protection_state.overcurrent_threshold {
            self.protection_state.tripped = true;
            self.protection_state.trip_reason = Some(format!(
                "过流保护：{} > {}",
                value, self.protection_state.overcurrent_threshold
            ));
        }
        // 欠压保护
        if point_id.contains("voltage") && value < self.protection_state.undervoltage_threshold {
            self.protection_state.tripped = true;
            self.protection_state.trip_reason = Some(format!(
                "欠压保护：{} < {}",
                value, self.protection_state.undervoltage_threshold
            ));
        }
        // 频率异常
        if point_id.contains("frequency")
            && (value - 50.0).abs() > self.protection_state.frequency_deviation_threshold
        {
            self.protection_state.tripped = true;
            self.protection_state.trip_reason = Some(format!("频率异常：{} Hz", value));
        }
    }

    /// 获取保护状态
    pub fn protection_state(&self) -> &ProtectionState {
        &self.protection_state
    }

    /// 复位保护
    ///
    /// 清除跳闸状态与跳闸原因，保留保护阈值配置。
    pub fn reset_protection(&mut self) {
        self.protection_state.tripped = false;
        self.protection_state.trip_reason = None;
    }

    /// 获取所有数据点
    pub fn points(&self) -> &HashMap<String, f64> {
        &self.points
    }

    /// 获取设备配置
    pub fn config(&self) -> &DeviceSimulatorConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用设备模拟器配置
    fn make_config() -> DeviceSimulatorConfig {
        let mut initial_points = HashMap::new();
        initial_points.insert("iec104_1_10".to_string(), 220.0);
        initial_points.insert("modbus_2_5".to_string(), 100.0);
        DeviceSimulatorConfig {
            device_id: "dev-001".to_string(),
            device_type: DeviceType::Rtu,
            protocol: "iec104".to_string(),
            address: "127.0.0.1:2404".to_string(),
            initial_points,
        }
    }

    #[test]
    fn test_device_simulator_new() {
        let config = make_config();
        let sim = DeviceSimulator::new(config);
        // 初始数据点应包含配置中的两个点
        assert_eq!(sim.points().len(), 2);
        assert_eq!(sim.points().get("iec104_1_10"), Some(&220.0));
        assert_eq!(sim.points().get("modbus_2_5"), Some(&100.0));
        // 配置应正确保存
        assert_eq!(sim.config().device_id, "dev-001");
        assert_eq!(sim.config().device_type, DeviceType::Rtu);
        // 保护状态默认未跳闸，阈值已初始化
        assert!(!sim.protection_state().tripped);
        assert!(sim.protection_state().trip_reason.is_none());
        assert!((sim.protection_state().overcurrent_threshold - 1000.0).abs() < 1e-9);
        assert!((sim.protection_state().undervoltage_threshold - 0.85).abs() < 1e-9);
        assert!(
            (sim.protection_state().frequency_deviation_threshold - 0.5).abs() < 1e-9
        );
    }

    #[test]
    fn test_iec104_read_response() {
        let config = make_config();
        let mut sim = DeviceSimulator::new(config);
        // 读取已存在的点 iec104_1_10 = 220.0
        let resp = sim.respond(&ProtocolRequest::Iec104Read {
            asdu_addr: 1,
            io_addr: 10,
        });
        match resp {
            ProtocolResponse::ReadOk { values } => {
                assert_eq!(values.len(), 1);
                assert!((values[0] - 220.0).abs() < 1e-9);
            }
            _ => panic!("期望 ReadOk 响应"),
        }
        // 读取不存在的点应返回 0.0
        let resp = sim.respond(&ProtocolRequest::Iec104Read {
            asdu_addr: 2,
            io_addr: 20,
        });
        match resp {
            ProtocolResponse::ReadOk { values } => {
                assert_eq!(values.len(), 1);
                assert!((values[0] - 0.0).abs() < 1e-9);
            }
            _ => panic!("期望 ReadOk 响应"),
        }
        // 写入后应能读到新值
        let _ = sim.respond(&ProtocolRequest::Iec104Write {
            asdu_addr: 3,
            io_addr: 30,
            value: 110.0,
        });
        let resp = sim.respond(&ProtocolRequest::Iec104Read {
            asdu_addr: 3,
            io_addr: 30,
        });
        match resp {
            ProtocolResponse::ReadOk { values } => {
                assert!((values[0] - 110.0).abs() < 1e-9);
            }
            _ => panic!("期望 ReadOk 响应"),
        }
    }

    #[test]
    fn test_modbus_read_response() {
        let config = make_config();
        let mut sim = DeviceSimulator::new(config);
        // 读取 modbus_2_5 = 100.0，count=3 应返回 [100.0, 0.0, 0.0]
        let resp = sim.respond(&ProtocolRequest::ModbusReadHolding {
            slave_id: 2,
            addr: 5,
            count: 3,
        });
        match resp {
            ProtocolResponse::ReadOk { values } => {
                assert_eq!(values.len(), 3);
                assert!((values[0] - 100.0).abs() < 1e-9);
                assert!((values[1] - 0.0).abs() < 1e-9);
                assert!((values[2] - 0.0).abs() < 1e-9);
            }
            _ => panic!("期望 ReadOk 响应"),
        }
        // 写入单个寄存器后读取应返回新值
        let _ = sim.respond(&ProtocolRequest::ModbusWriteSingle {
            slave_id: 2,
            addr: 6,
            value: 500,
        });
        let resp = sim.respond(&ProtocolRequest::ModbusReadHolding {
            slave_id: 2,
            addr: 6,
            count: 1,
        });
        match resp {
            ProtocolResponse::ReadOk { values } => {
                assert_eq!(values.len(), 1);
                assert!((values[0] - 500.0).abs() < 1e-9);
            }
            _ => panic!("期望 ReadOk 响应"),
        }
    }

    #[test]
    fn test_modbus_read_holding_overflow() {
        // 验证 addr + count > 65535 时不会触发 u16 溢出 panic
        // 场景：addr=65530, count=10，应正确读取 65530-65539 共 10 个寄存器
        let config = make_config();
        let mut sim = DeviceSimulator::new(config);
        // 向 65530-65539 注入递增值，便于校验地址映射正确
        for offset in 0..10u32 {
            let key = format!("modbus_2_{}", 65530u32 + offset);
            sim.inject_state(&key, (offset as f64) + 1.0);
        }
        // 触发可能溢出的读取请求（修复前在 debug 模式下会 panic）
        let resp = sim.respond(&ProtocolRequest::ModbusReadHolding {
            slave_id: 2,
            addr: 65530,
            count: 10,
        });
        match resp {
            ProtocolResponse::ReadOk { values } => {
                // 应返回 10 个寄存器值，且地址映射正确（无 wrap-around）
                assert_eq!(values.len(), 10, "应返回 10 个寄存器值");
                for (i, v) in values.iter().enumerate() {
                    let expected = (i as f64) + 1.0;
                    assert!(
                        (*v - expected).abs() < 1e-9,
                        "寄存器 655{} 期望 {}，实际 {}",
                        30 + i,
                        expected,
                        v
                    );
                }
            }
            _ => panic!("期望 ReadOk 响应"),
        }
    }

    #[test]
    fn test_inject_state() {
        let config = make_config();
        let mut sim = DeviceSimulator::new(config);
        // 注入新数据点
        sim.inject_state("active_power", 50.0);
        assert_eq!(sim.points().get("active_power"), Some(&50.0));
        // 注入已存在的点应覆盖
        sim.inject_state("iec104_1_10", 230.0);
        assert_eq!(sim.points().get("iec104_1_10"), Some(&230.0));
        // 注入正常电压不应触发保护
        sim.inject_state("voltage_bus1", 1.0);
        assert!(!sim.protection_state().tripped);
    }

    #[test]
    fn test_protection_overcurrent() {
        let config = make_config();
        let mut sim = DeviceSimulator::new(config);
        // 注入超过过流阈值的电流（阈值 1000 A）
        sim.inject_state("current_phase_a", 1200.0);
        assert!(sim.protection_state().tripped, "过流应触发跳闸");
        let reason = sim
            .protection_state()
            .trip_reason
            .as_ref()
            .expect("跳闸原因应存在");
        assert!(
            reason.contains("过流保护"),
            "跳闸原因应包含'过流保护'，实际：{}",
            reason
        );
        assert!(
            reason.contains("1200"),
            "跳闸原因应包含电流值，实际：{}",
            reason
        );
    }

    #[test]
    fn test_protection_reset() {
        let config = make_config();
        let mut sim = DeviceSimulator::new(config);
        // 触发过流跳闸
        sim.inject_state("current_phase_a", 1500.0);
        assert!(sim.protection_state().tripped);
        assert!(sim.protection_state().trip_reason.is_some());
        // 复位保护
        sim.reset_protection();
        assert!(!sim.protection_state().tripped, "复位后应未跳闸");
        assert!(
            sim.protection_state().trip_reason.is_none(),
            "复位后跳闸原因应清除"
        );
        // 阈值应保留
        assert!(
            (sim.protection_state().overcurrent_threshold - 1000.0).abs() < 1e-9,
            "复位不应改变阈值"
        );
    }
}