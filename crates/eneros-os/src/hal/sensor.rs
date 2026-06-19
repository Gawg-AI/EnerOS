//! 传感器驱动框架
//!
//! 提供统一的传感器读数接口与示例驱动：
//! - [`Lm75Sensor`]：LM75 I2C 温度传感器（地址 0x48，分辨率 0.5°C）
//! - [`Mcp3008Adc`]：MCP3008 SPI ADC（8 通道 10 位，参考电压 3.3V）

use super::{HalError, I2cDevice, SpiDevice};
use serde::{Deserialize, Serialize};

/// 传感器类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensorType {
    /// 温度
    Temperature,
    /// 湿度
    Humidity,
    /// 电压
    Voltage,
    /// 电流
    Current,
    /// 功率
    Power,
    /// 自定义类型
    Custom(String),
}

/// 传感器读数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub sensor_type: SensorType,
    pub value: f64,
    /// 单位，如 "°C"、"%"、"V"、"A"、"W"
    pub unit: String,
    /// ISO 8601 时间戳
    pub timestamp: String,
}

/// 传感器驱动 trait
pub trait SensorDriver: Send {
    /// 读取传感器数据
    fn read(&mut self) -> Result<SensorReading, HalError>;
    /// 传感器标识
    fn name(&self) -> &str;
    /// 传感器类型
    fn sensor_type(&self) -> SensorType;
}

/// 传感器管理器
pub struct SensorManager {
    sensors: Vec<Box<dyn SensorDriver>>,
}

impl SensorManager {
    pub fn new() -> Self {
        Self {
            sensors: Vec::new(),
        }
    }

    /// 添加传感器驱动
    pub fn add_sensor(&mut self, sensor: Box<dyn SensorDriver>) {
        self.sensors.push(sensor);
    }

    /// 读取所有传感器，按注册顺序返回结果
    pub fn read_all(&mut self) -> Vec<Result<SensorReading, HalError>> {
        self.sensors.iter_mut().map(|s| s.read()).collect()
    }

    /// 列出已注册传感器（名称, 类型）
    pub fn list_sensors(&self) -> Vec<(&str, SensorType)> {
        self.sensors
            .iter()
            .map(|s| (s.name(), s.sensor_type()))
            .collect()
    }
}

impl Default for SensorManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// LM75 — I2C 温度传感器
// ============================================================================

/// LM75 I2C 温度传感器驱动
///
/// I2C 地址 0x48，温度寄存器 0x00，16 位数据中高 9 位为有符号温度，
/// 分辨率 0.5°C。
pub struct Lm75Sensor {
    name: String,
    i2c: Box<dyn I2cDevice>,
}

impl Lm75Sensor {
    pub fn new(name: impl Into<String>, i2c: Box<dyn I2cDevice>) -> Self {
        Self {
            name: name.into(),
            i2c,
        }
    }
}

impl SensorDriver for Lm75Sensor {
    fn read(&mut self) -> Result<SensorReading, HalError> {
        let mut buf = [0u8; 2];
        // 写寄存器地址 0x00，读 2 字节
        self.i2c.transfer(&[0x00], &mut buf)?;
        // 高 9 位为有符号温度值，i16 算术右移做符号扩展
        let raw = ((buf[0] as i16) << 8) | (buf[1] as i16);
        let temp = (raw >> 7) as f64 * 0.5;
        Ok(SensorReading {
            sensor_type: SensorType::Temperature,
            value: temp,
            unit: "°C".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn sensor_type(&self) -> SensorType {
        SensorType::Temperature
    }
}

// ============================================================================
// MCP3008 — SPI ADC
// ============================================================================

/// MCP3008 SPI ADC 驱动（8 通道 10 位 ADC，参考电压 3.3V）
pub struct Mcp3008Adc {
    name: String,
    spi: Box<dyn SpiDevice>,
    channel: u8,
}

impl Mcp3008Adc {
    pub fn new(name: impl Into<String>, spi: Box<dyn SpiDevice>, channel: u8) -> Self {
        Self {
            name: name.into(),
            spi,
            channel,
        }
    }
}

impl SensorDriver for Mcp3008Adc {
    fn read(&mut self) -> Result<SensorReading, HalError> {
        // MCP3008 读取时序：发送 3 字节，接收 3 字节，低 10 位为 ADC 值
        let tx: [u8; 3] = [0x01, 0x80 | (self.channel << 4), 0x00];
        let mut rx = [0u8; 3];
        self.spi.transfer(&tx, &mut rx)?;
        let adc = ((rx[1] as u16 & 0x03) << 8) | rx[2] as u16;
        let voltage = adc as f64 * 3.3 / 1024.0;
        Ok(SensorReading {
            sensor_type: SensorType::Voltage,
            value: voltage,
            unit: "V".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn sensor_type(&self) -> SensorType {
        SensorType::Voltage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 I2C mock 设备
    struct MockI2cDevice {
        read_data: Vec<u8>,
    }

    impl I2cDevice for MockI2cDevice {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, HalError> {
            let n = buf.len().min(self.read_data.len());
            buf[..n].copy_from_slice(&self.read_data[..n]);
            Ok(n)
        }
        fn write(&mut self, _data: &[u8]) -> Result<usize, HalError> {
            Ok(0)
        }
        fn transfer(&mut self, _write: &[u8], read: &mut [u8]) -> Result<(), HalError> {
            let n = read.len().min(self.read_data.len());
            read[..n].copy_from_slice(&self.read_data[..n]);
            Ok(())
        }
    }

    /// 测试用 SPI mock 设备
    struct MockSpiDevice {
        rx_data: Vec<u8>,
    }

    impl SpiDevice for MockSpiDevice {
        fn transfer(&mut self, _tx: &[u8], rx: &mut [u8]) -> Result<(), HalError> {
            let n = rx.len().min(self.rx_data.len());
            rx[..n].copy_from_slice(&self.rx_data[..n]);
            Ok(())
        }
        fn write(&mut self, _data: &[u8]) -> Result<(), HalError> {
            Ok(())
        }
        fn read(&mut self, _buf: &mut [u8]) -> Result<(), HalError> {
            Ok(())
        }
    }

    /// 测试用通用传感器 mock
    struct MockSensor {
        name: String,
        sensor_type: SensorType,
        value: f64,
        unit: String,
    }

    impl SensorDriver for MockSensor {
        fn read(&mut self) -> Result<SensorReading, HalError> {
            Ok(SensorReading {
                sensor_type: self.sensor_type.clone(),
                value: self.value,
                unit: self.unit.clone(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            })
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn sensor_type(&self) -> SensorType {
            self.sensor_type.clone()
        }
    }

    #[test]
    fn test_sensor_type_serialization() {
        let types = [
            SensorType::Temperature,
            SensorType::Humidity,
            SensorType::Voltage,
            SensorType::Current,
            SensorType::Power,
            SensorType::Custom("pressure".to_string()),
        ];
        for t in &types {
            let json = serde_json::to_string(t).unwrap();
            let de: SensorType = serde_json::from_str(&json).unwrap();
            assert_eq!(de, *t);
        }
        // 自定义类型与预定义类型不相等
        assert_ne!(
            SensorType::Custom("voltage".to_string()),
            SensorType::Voltage
        );
    }

    #[test]
    fn test_sensor_reading_serialization() {
        let reading = SensorReading {
            sensor_type: SensorType::Temperature,
            value: 25.5,
            unit: "°C".to_string(),
            timestamp: "2026-06-19T12:00:00+08:00".to_string(),
        };
        let json = serde_json::to_string(&reading).unwrap();
        let de: SensorReading = serde_json::from_str(&json).unwrap();
        assert_eq!(de.sensor_type, SensorType::Temperature);
        assert!((de.value - 25.5).abs() < f64::EPSILON);
        assert_eq!(de.unit, "°C");
        assert_eq!(de.timestamp, "2026-06-19T12:00:00+08:00");
    }

    #[test]
    fn test_sensor_manager_empty() {
        let mut manager = SensorManager::new();
        assert!(manager.list_sensors().is_empty());
        assert!(manager.read_all().is_empty());
    }

    #[test]
    fn test_sensor_manager_add_and_read_all() {
        let mut manager = SensorManager::new();
        manager.add_sensor(Box::new(MockSensor {
            name: "temp1".to_string(),
            sensor_type: SensorType::Temperature,
            value: 23.0,
            unit: "°C".to_string(),
        }));
        manager.add_sensor(Box::new(MockSensor {
            name: "vbus".to_string(),
            sensor_type: SensorType::Voltage,
            value: 12.0,
            unit: "V".to_string(),
        }));

        // list_sensors
        let list = manager.list_sensors();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], ("temp1", SensorType::Temperature));
        assert_eq!(list[1], ("vbus", SensorType::Voltage));

        // read_all
        let results = manager.read_all();
        assert_eq!(results.len(), 2);
        let r0 = results[0].as_ref().unwrap();
        assert!((r0.value - 23.0).abs() < f64::EPSILON);
        assert_eq!(r0.unit, "°C");
        let r1 = results[1].as_ref().unwrap();
        assert!((r1.value - 12.0).abs() < f64::EPSILON);
        assert_eq!(r1.unit, "V");
    }

    #[test]
    fn test_lm75_sensor_read_positive() {
        // 25°C: raw9 = 50, 编码后 0x1900
        let mock = MockI2cDevice {
            read_data: vec![0x19, 0x00],
        };
        let mut sensor = Lm75Sensor::new("lm75-1", Box::new(mock));
        assert_eq!(sensor.name(), "lm75-1");
        assert_eq!(sensor.sensor_type(), SensorType::Temperature);

        let reading = sensor.read().unwrap();
        assert_eq!(reading.sensor_type, SensorType::Temperature);
        assert!((reading.value - 25.0).abs() < 1e-9, "got {}", reading.value);
        assert_eq!(reading.unit, "°C");
    }

    #[test]
    fn test_lm75_sensor_read_negative() {
        // -25°C: raw9 = -50, 9 位补码 = 462 = 0b111001110, 左移 7 位 = 0xE700
        let mock = MockI2cDevice {
            read_data: vec![0xE7, 0x00],
        };
        let mut sensor = Lm75Sensor::new("lm75-2", Box::new(mock));
        let reading = sensor.read().unwrap();
        assert!(
            (reading.value - (-25.0)).abs() < 1e-9,
            "got {}",
            reading.value
        );
    }

    #[test]
    fn test_mcp3008_adc_read() {
        // ADC = 512: rx[1] & 0x03 = 2, rx[2] = 0 → 电压 = 512 * 3.3 / 1024 = 1.65V
        let mock = MockSpiDevice {
            rx_data: vec![0x00, 0x02, 0x00],
        };
        let mut adc = Mcp3008Adc::new("adc-0", Box::new(mock), 0);
        assert_eq!(adc.name(), "adc-0");
        assert_eq!(adc.sensor_type(), SensorType::Voltage);

        let reading = adc.read().unwrap();
        assert_eq!(reading.sensor_type, SensorType::Voltage);
        assert!(
            (reading.value - 1.65).abs() < 1e-9,
            "got {}",
            reading.value
        );
        assert_eq!(reading.unit, "V");
    }

    #[test]
    fn test_mcp3008_adc_max_value() {
        // ADC = 1023 (满量程): rx[1] & 0x03 = 3, rx[2] = 0xFF → 3.3V
        let mock = MockSpiDevice {
            rx_data: vec![0x00, 0x03, 0xFF],
        };
        let mut adc = Mcp3008Adc::new("adc-max", Box::new(mock), 7);
        let reading = adc.read().unwrap();
        let expected = 1023.0 * 3.3 / 1024.0;
        assert!(
            (reading.value - expected).abs() < 1e-9,
            "got {} expected {}",
            reading.value,
            expected
        );
    }
}
