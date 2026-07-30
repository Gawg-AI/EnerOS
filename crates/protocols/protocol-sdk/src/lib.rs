//! # EnerOS Protocol SDK v1.0
//!
//! 第三方设备协议插件开发框架。
//!
//! ## 快速开始
//!
//! ```ignore
//! use eneros_protocol_sdk::{
//!     register_protocol, DataPoint, PointValue, ProtocolConfig, ProtocolError,
//!     ProtocolPlugin,
//! };
//!
//! struct MyProtocol;
//!
//! impl MyProtocol {
//!     fn new() -> Self { Self }
//! }
//!
//! impl ProtocolPlugin for MyProtocol {
//!     fn name(&self) -> &str { "my-protocol" }
//!     fn version(&self) -> &str { "1.0.0" }
//!     fn connect(&mut self, _config: &ProtocolConfig) -> Result<(), ProtocolError> { Ok(()) }
//!     fn read_point(&mut self, point_id: &str) -> Result<DataPoint, ProtocolError> {
//!         Ok(DataPoint::new(point_id, PointValue::F32(0.0)))
//!     }
//!     fn write_point(&mut self, _point_id: &str, _value: PointValue) -> Result<(), ProtocolError> { Ok(()) }
//!     fn disconnect(&mut self) -> Result<(), ProtocolError> { Ok(()) }
//! }
//!
//! register_protocol!(MyProtocol);
//! ```
//!
//! ## no_std 合规
//! 本 crate 完全 no_std，仅依赖 `alloc` 和 `heapless`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

use heapless::String as HString;

/// Protocol SDK 版本号。
pub const SDK_VERSION: &str = "1.0.0";

/// SDK ABI 版本（插件兼容性检查）。
pub const ABI_VERSION: u32 = 1;

// ============================================================================
// 错误类型
// ============================================================================

/// 协议错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// 连接失败
    ConnectionFailed(HString<128>),
    /// 通信超时
    Timeout,
    /// 点不存在
    PointNotFound(HString<64>),
    /// 写入被拒绝
    WriteRejected(HString<128>),
    /// 协议解析错误
    ParseError(HString<128>),
    /// 传输层错误
    TransportError(HString<128>),
    /// 未连接
    NotConnected,
    /// 不支持的操作
    NotSupported,
}

impl core::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProtocolError::ConnectionFailed(msg) => write!(f, "connection failed: {}", msg),
            ProtocolError::Timeout => write!(f, "timeout"),
            ProtocolError::PointNotFound(id) => write!(f, "point not found: {}", id),
            ProtocolError::WriteRejected(msg) => write!(f, "write rejected: {}", msg),
            ProtocolError::ParseError(msg) => write!(f, "parse error: {}", msg),
            ProtocolError::TransportError(msg) => write!(f, "transport error: {}", msg),
            ProtocolError::NotConnected => write!(f, "not connected"),
            ProtocolError::NotSupported => write!(f, "not supported"),
        }
    }
}

// ============================================================================
// 数据结构
// ============================================================================

/// 传输方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// RS485/RS232 串口
    Serial,
    /// Modbus TCP / 以太网
    Tcp,
    /// CAN 总线
    Can,
    /// 自定义
    Custom,
}

/// 协议配置。
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    /// 传输方式
    pub transport: Transport,
    /// 设备地址（≤64 字节，如 "192.168.1.100" 或 "/dev/ttyS0"）
    pub address: HString<64>,
    /// 端口号
    pub port: u16,
    /// 超时（毫秒）
    pub timeout_ms: u32,
    /// 轮询间隔（毫秒）
    pub poll_interval_ms: u32,
    /// 从站地址（Modbus 等）
    pub slave_id: u8,
    /// 波特率（串口）
    pub baud_rate: u32,
}

impl ProtocolConfig {
    /// 创建 TCP 配置。
    pub fn tcp(address: &str, port: u16) -> Self {
        Self {
            transport: Transport::Tcp,
            address: HString::<64>::try_from(address).unwrap_or_default(),
            port,
            timeout_ms: 1000,
            poll_interval_ms: 100,
            slave_id: 1,
            baud_rate: 0,
        }
    }

    /// 创建串口配置。
    pub fn serial(device: &str, baud_rate: u32) -> Self {
        Self {
            transport: Transport::Serial,
            address: HString::<64>::try_from(device).unwrap_or_default(),
            port: 0,
            timeout_ms: 1000,
            poll_interval_ms: 100,
            slave_id: 1,
            baud_rate,
        }
    }

    /// 设置超时。
    pub fn with_timeout(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// 设置轮询间隔。
    pub fn with_poll_interval(mut self, ms: u32) -> Self {
        self.poll_interval_ms = ms;
        self
    }

    /// 设置从站地址。
    pub fn with_slave_id(mut self, id: u8) -> Self {
        self.slave_id = id;
        self
    }
}

/// 测点值类型。
#[derive(Debug, Clone, PartialEq)]
pub enum PointValue {
    /// 布尔
    Bool(bool),
    /// 有符号 16 位
    I16(i16),
    /// 无符号 16 位
    U16(u16),
    /// 有符号 32 位
    I32(i32),
    /// 无符号 32 位
    U32(u32),
    /// 单精度浮点
    F32(f32),
    /// 双精度浮点
    F64(f64),
    /// 字节数组（≤256 字节）
    Bytes(Vec<u8>),
}

/// 数据质量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// 良好
    Good,
    /// 不确定
    Uncertain,
    /// 坏
    Bad,
}

/// 数据点（测点读数）。
#[derive(Debug, Clone)]
pub struct DataPoint {
    /// 测点 ID（≤64 字节）
    pub point_id: HString<64>,
    /// 值
    pub value: PointValue,
    /// 质量
    pub quality: Quality,
    /// 时间戳（毫秒）
    pub timestamp_ms: u64,
}

impl DataPoint {
    /// 创建数据点（Good 质量，时间戳 0）。
    pub fn new(point_id: &str, value: PointValue) -> Self {
        Self {
            point_id: HString::<64>::try_from(point_id).unwrap_or_default(),
            value,
            quality: Quality::Good,
            timestamp_ms: 0,
        }
    }

    /// 设置时间戳。
    pub fn with_timestamp(mut self, ms: u64) -> Self {
        self.timestamp_ms = ms;
        self
    }

    /// 设置质量。
    pub fn with_quality(mut self, q: Quality) -> Self {
        self.quality = q;
        self
    }
}

// ============================================================================
// ProtocolPlugin trait
// ============================================================================

/// 协议插件 trait（SDK v1.0 稳定 API）。
///
/// 第三方开发者实现此接口开发自定义设备协议插件。
///
/// # 生命周期
/// ```text
/// new() → connect → read_point/write_point (循环) → disconnect
/// ```
pub trait ProtocolPlugin: Send + Sync {
    /// 协议名称。
    fn name(&self) -> &str;

    /// 协议版本。
    fn version(&self) -> &str;

    /// 连接到设备。
    fn connect(&mut self, config: &ProtocolConfig) -> Result<(), ProtocolError>;

    /// 读取测点。
    fn read_point(&mut self, point_id: &str) -> Result<DataPoint, ProtocolError>;

    /// 写入测点。
    fn write_point(&mut self, point_id: &str, value: PointValue) -> Result<(), ProtocolError>;

    /// 断开连接。
    fn disconnect(&mut self) -> Result<(), ProtocolError>;

    /// 批量读取（可选，默认逐个调用 read_point）。
    fn read_batch(&mut self, point_ids: &[&str]) -> Result<Vec<DataPoint>, ProtocolError> {
        let mut results = Vec::with_capacity(point_ids.len());
        for id in point_ids {
            results.push(self.read_point(id)?);
        }
        Ok(results)
    }

    /// 检查连接状态（可选）。
    fn is_connected(&self) -> bool {
        false
    }
}

// ============================================================================
// 注册宏
// ============================================================================

/// 协议插件注册宏（C ABI 导出）。
///
/// # 导出函数
/// - `eneros_protocol_create` — 创建插件实例
/// - `eneros_protocol_version` — 获取 ABI 版本
/// - `eneros_protocol_destroy` — 销毁插件实例
#[macro_export]
macro_rules! register_protocol {
    ($type:ty) => {
        /// 创建协议插件实例（C ABI）。
        #[no_mangle]
        pub extern "C" fn eneros_protocol_create() -> *mut dyn $crate::ProtocolPlugin {
            let p = <$type>::new();
            Box::into_raw(Box::new(p))
        }

        /// 获取 ABI 版本（C ABI）。
        #[no_mangle]
        pub extern "C" fn eneros_protocol_version() -> u32 {
            $crate::ABI_VERSION
        }

        /// 销毁协议插件实例（C ABI）。
        ///
        /// # Safety
        /// 调用者必须确保 `ptr` 是由 `eneros_protocol_create` 返回的有效指针。
        #[no_mangle]
        pub unsafe extern "C" fn eneros_protocol_destroy(ptr: *mut dyn $crate::ProtocolPlugin) {
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }
    };
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock 协议插件。
    struct MockProtocol {
        connected: bool,
    }

    impl MockProtocol {
        fn new() -> Self {
            Self { connected: false }
        }
    }

    impl ProtocolPlugin for MockProtocol {
        fn name(&self) -> &str {
            "mock-protocol"
        }
        fn version(&self) -> &str {
            "1.0.0"
        }
        fn connect(&mut self, _config: &ProtocolConfig) -> Result<(), ProtocolError> {
            self.connected = true;
            Ok(())
        }
        fn read_point(&mut self, point_id: &str) -> Result<DataPoint, ProtocolError> {
            if !self.connected {
                return Err(ProtocolError::NotConnected);
            }
            Ok(DataPoint::new(point_id, PointValue::F32(42.0)))
        }
        fn write_point(
            &mut self,
            _point_id: &str,
            _value: PointValue,
        ) -> Result<(), ProtocolError> {
            if !self.connected {
                return Err(ProtocolError::NotConnected);
            }
            Ok(())
        }
        fn disconnect(&mut self) -> Result<(), ProtocolError> {
            self.connected = false;
            Ok(())
        }
        fn is_connected(&self) -> bool {
            self.connected
        }
    }

    /// PRT1：完整生命周期。
    #[test]
    fn prt1_full_lifecycle() {
        let mut proto = MockProtocol::new();
        let config = ProtocolConfig::tcp("192.168.1.1", 502);

        assert!(!proto.is_connected());
        proto.connect(&config).expect("连接应成功");
        assert!(proto.is_connected());

        let dp = proto.read_point("U_a").expect("读取应成功");
        assert_eq!(dp.point_id.as_str(), "U_a");
        assert_eq!(dp.value, PointValue::F32(42.0));
        assert_eq!(dp.quality, Quality::Good);

        proto
            .write_point("setpoint", PointValue::F32(100.0))
            .expect("写入应成功");

        proto.disconnect().expect("断开应成功");
        assert!(!proto.is_connected());
    }

    /// PRT2：未连接读取拒绝。
    #[test]
    fn prt2_read_without_connect() {
        let mut proto = MockProtocol::new();
        let err = proto.read_point("U_a").expect_err("应拒绝");
        assert_eq!(err, ProtocolError::NotConnected);
    }

    /// PRT3：批量读取。
    #[test]
    fn prt3_batch_read() {
        let mut proto = MockProtocol::new();
        let config = ProtocolConfig::tcp("192.168.1.1", 502);
        proto.connect(&config).expect("连接应成功");

        let points = proto
            .read_batch(&["U_a", "U_b", "U_c"])
            .expect("批量读取应成功");
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].point_id.as_str(), "U_a");
        assert_eq!(points[1].point_id.as_str(), "U_b");
        assert_eq!(points[2].point_id.as_str(), "U_c");
    }

    /// PRT4：TCP 配置构建器。
    #[test]
    fn prt4_tcp_config() {
        let config = ProtocolConfig::tcp("10.0.0.1", 502)
            .with_timeout(2000)
            .with_poll_interval(50)
            .with_slave_id(3);

        assert_eq!(config.transport, Transport::Tcp);
        assert_eq!(config.address.as_str(), "10.0.0.1");
        assert_eq!(config.port, 502);
        assert_eq!(config.timeout_ms, 2000);
        assert_eq!(config.poll_interval_ms, 50);
        assert_eq!(config.slave_id, 3);
    }

    /// PRT5：串口配置构建器。
    #[test]
    fn prt5_serial_config() {
        let config = ProtocolConfig::serial("/dev/ttyS0", 9600).with_slave_id(1);

        assert_eq!(config.transport, Transport::Serial);
        assert_eq!(config.address.as_str(), "/dev/ttyS0");
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.slave_id, 1);
    }

    /// PRT6：数据点构建器。
    #[test]
    fn prt6_data_point_builder() {
        let dp = DataPoint::new("P_total", PointValue::F64(123.456))
            .with_timestamp(1_000_000)
            .with_quality(Quality::Uncertain);

        assert_eq!(dp.point_id.as_str(), "P_total");
        assert_eq!(dp.value, PointValue::F64(123.456));
        assert_eq!(dp.timestamp_ms, 1_000_000);
        assert_eq!(dp.quality, Quality::Uncertain);
    }

    /// PRT7：错误类型 Display。
    #[test]
    fn prt7_error_display() {
        let e = ProtocolError::ConnectionFailed(
            HString::<128>::try_from("refused").unwrap_or_default(),
        );
        assert_eq!(format!("{}", e), "connection failed: refused");
        assert_eq!(format!("{}", ProtocolError::Timeout), "timeout");
        assert_eq!(format!("{}", ProtocolError::NotConnected), "not connected");
    }

    /// PRT8：trait object safety。
    #[test]
    fn prt8_object_safe() {
        let proto: Box<dyn ProtocolPlugin> = Box::new(MockProtocol::new());
        assert_eq!(proto.name(), "mock-protocol");
        assert_eq!(proto.version(), "1.0.0");
    }

    /// PRT9：SDK 版本常量。
    #[test]
    fn prt9_sdk_version() {
        assert_eq!(SDK_VERSION, "1.0.0");
        assert_eq!(ABI_VERSION, 1);
    }
}
