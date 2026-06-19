//! Hardware abstraction layer

use serde::{Deserialize, Serialize};

pub mod linux_hal;
pub mod sensor;

pub use linux_hal::LinuxHal;
#[cfg(target_os = "linux")]
pub use linux_hal::GpioEventMonitor;

/// Hardware abstraction trait
pub trait HardwareAbstraction: Send + Sync {
    fn open_serial(&self, path: &str, baud: u32) -> Result<Box<dyn SerialPort>, HalError>;
    fn list_network_interfaces(&self) -> Result<Vec<String>, HalError>;
    /// 打开 GPIO 引脚
    fn open_gpio(&self, pin: u32) -> Result<Box<dyn GpioPin>, HalError>;
    /// 打开 I2C 设备
    fn open_i2c(&self, bus: u32, addr: u16) -> Result<Box<dyn I2cDevice>, HalError>;
    /// 打开 SPI 设备
    fn open_spi(&self, path: &str, config: &SpiConfig) -> Result<Box<dyn SpiDevice>, HalError>;
}

pub trait SerialPort: Send {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HalError>;
    fn write(&mut self, data: &[u8]) -> Result<usize, HalError>;
    fn configure(&mut self, config: &SerialConfig) -> Result<(), HalError>;
    fn close(&mut self);
}

#[derive(Debug, Clone)]
pub struct SerialConfig {
    pub baud_rate: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: Parity,
    pub flow_control: FlowControl,
    /// 读超时（毫秒）；None 表示阻塞模式
    pub timeout_ms: Option<u32>,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            baud_rate: 9600,
            data_bits: 8,
            stop_bits: 1,
            parity: Parity::None,
            flow_control: FlowControl::None,
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Parity {
    None,
    Even,
    Odd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowControl {
    None,
    Hardware,
    Software,
}

/// GPIO 方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpioDirection {
    Input,
    Output,
}

/// GPIO 中断边沿
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpioEdge {
    None,
    Rising,
    Falling,
    Both,
}

/// GPIO 引脚接口
pub trait GpioPin: Send {
    fn read(&self) -> Result<bool, HalError>;
    fn write(&mut self, value: bool) -> Result<(), HalError>;
    fn set_direction(&mut self, dir: GpioDirection) -> Result<(), HalError>;
    fn set_edge(&mut self, edge: GpioEdge) -> Result<(), HalError>;
}

/// GPIO 事件回调类型（pin 编号, 当前电平）
pub type GpioEventCallback = Box<dyn Fn(u32, bool) + Send + Sync>;

/// GPIO 事件分发器（跨平台）
///
/// 收集多个回调，当 GPIO 事件发生时通过 [`dispatch`](Self::dispatch) 统一触发。
pub struct GpioEventDispatcher {
    callbacks: Vec<GpioEventCallback>,
}

impl GpioEventDispatcher {
    pub fn new() -> Self {
        Self {
            callbacks: Vec::new(),
        }
    }

    /// 注册事件回调
    pub fn on_event<F: Fn(u32, bool) + Send + Sync + 'static>(&mut self, cb: F) {
        self.callbacks.push(Box::new(cb));
    }

    /// 触发所有已注册回调
    pub fn dispatch(&self, pin: u32, value: bool) {
        for cb in &self.callbacks {
            cb(pin, value);
        }
    }
}

impl Default for GpioEventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// I2C 设备接口
pub trait I2cDevice: Send {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HalError>;
    fn write(&mut self, data: &[u8]) -> Result<usize, HalError>;
    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), HalError>;
}

/// SPI 设备接口
pub trait SpiDevice: Send {
    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]) -> Result<(), HalError>;
    fn write(&mut self, data: &[u8]) -> Result<(), HalError>;
    fn read(&mut self, buf: &mut [u8]) -> Result<(), HalError>;
}

/// SPI 配置
#[derive(Debug, Clone)]
pub struct SpiConfig {
    /// SPI 模式（0/1/2/3）
    pub mode: u8,
    /// 时钟频率（Hz）
    pub speed_hz: u32,
    /// 每字位数
    pub bits_per_word: u8,
}

impl Default for SpiConfig {
    fn default() -> Self {
        Self {
            mode: 0,
            speed_hz: 1_000_000,
            bits_per_word: 8,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HalError {
    #[error("device not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_config_default() {
        let config = SerialConfig::default();
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.data_bits, 8);
        assert_eq!(config.stop_bits, 1);
        assert_eq!(config.parity, Parity::None);
        assert_eq!(config.flow_control, FlowControl::None);
        assert_eq!(config.timeout_ms, None);
    }

    #[test]
    fn test_parity_serialization() {
        for p in [Parity::None, Parity::Even, Parity::Odd] {
            let json = serde_json::to_string(&p).unwrap();
            let de: Parity = serde_json::from_str(&json).unwrap();
            assert_eq!(de, p);
        }
        assert_ne!(Parity::None, Parity::Even);
        assert_ne!(Parity::Even, Parity::Odd);
    }

    #[test]
    fn test_flow_control_serialization() {
        for f in [FlowControl::None, FlowControl::Hardware, FlowControl::Software] {
            let json = serde_json::to_string(&f).unwrap();
            let de: FlowControl = serde_json::from_str(&json).unwrap();
            assert_eq!(de, f);
        }
        assert_ne!(FlowControl::None, FlowControl::Hardware);
    }

    #[test]
    fn test_gpio_direction_serialization() {
        for d in [GpioDirection::Input, GpioDirection::Output] {
            let json = serde_json::to_string(&d).unwrap();
            let de: GpioDirection = serde_json::from_str(&json).unwrap();
            assert_eq!(de, d);
        }
        assert_ne!(GpioDirection::Input, GpioDirection::Output);
    }

    #[test]
    fn test_gpio_edge_serialization() {
        let edges = [
            GpioEdge::None,
            GpioEdge::Rising,
            GpioEdge::Falling,
            GpioEdge::Both,
        ];
        for e in edges {
            let json = serde_json::to_string(&e).unwrap();
            let de: GpioEdge = serde_json::from_str(&json).unwrap();
            assert_eq!(de, e);
        }
        // 所有变体互不相等
        for i in 0..edges.len() {
            for j in (i + 1)..edges.len() {
                assert_ne!(edges[i], edges[j], "edges {} and {} collide", i, j);
            }
        }
    }

    #[test]
    fn test_spi_config_default() {
        let config = SpiConfig::default();
        assert_eq!(config.mode, 0);
        assert_eq!(config.speed_hz, 1_000_000);
        assert_eq!(config.bits_per_word, 8);
    }

    #[test]
    fn test_hal_error_display() {
        assert_eq!(
            HalError::NotFound("ttyS0".to_string()).to_string(),
            "device not found: ttyS0"
        );
        assert_eq!(
            HalError::PermissionDenied("denied".to_string()).to_string(),
            "permission denied: denied"
        );
        assert_eq!(
            HalError::Timeout("read".to_string()).to_string(),
            "timeout: read"
        );
        assert_eq!(
            HalError::InvalidConfig("bad baud".to_string()).to_string(),
            "invalid config: bad baud"
        );
    }

    #[test]
    fn test_hal_error_io_from() {
        let io_err = std::io::Error::other("test");
        let hal_err: HalError = io_err.into();
        assert!(hal_err.to_string().contains("io error"));
    }

    #[test]
    fn test_gpio_event_dispatcher() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let mut dispatcher = GpioEventDispatcher::new();
        assert!(dispatcher.callbacks.is_empty());

        // 记录高电平事件次数
        let high_count = Arc::new(AtomicU32::new(0));
        let low_count = Arc::new(AtomicU32::new(0));
        let high_clone = high_count.clone();
        let low_clone = low_count.clone();
        dispatcher.on_event(move |_pin, value| {
            if value {
                high_clone.fetch_add(1, Ordering::SeqCst);
            } else {
                low_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        // 第二个回调记录最后收到的 pin
        let last_pin = Arc::new(AtomicU32::new(0));
        let last_pin_clone = last_pin.clone();
        dispatcher.on_event(move |pin, _value| {
            last_pin_clone.store(pin, Ordering::SeqCst);
        });

        dispatcher.dispatch(17, true);
        dispatcher.dispatch(17, false);
        dispatcher.dispatch(23, true);

        assert_eq!(high_count.load(Ordering::SeqCst), 2);
        assert_eq!(low_count.load(Ordering::SeqCst), 1);
        assert_eq!(last_pin.load(Ordering::SeqCst), 23);
    }
}
