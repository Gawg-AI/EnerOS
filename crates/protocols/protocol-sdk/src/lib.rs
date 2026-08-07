//! # EnerOS Protocol SDK v1.0
//!
//! 协议插件 IPC wire codec 客户端开发框架（ADR-0025 / ARCH-S01）。
//!
//! ## 快速开始
//!
//! 插件以**签名静态 ELF PIE → 受限 EL0 进程 → 版本化 IPC wire** 形态执行
//! （MASTER §10.1 / ADR-0025）。SDK 提供 `wire` 模块对跨进程请求/响应做
//! 二进制编解码（帧 + 语义消息），不再导出 cross-binary trait-object：
//!
//! ```text
//! use eneros_protocol_sdk::wire::{
//!     encode_request, decode_request, ProtocolRequest,
//! };
//!
//! let mut buf = [0u8; eneros_protocol_sdk::wire::MAX_WIRE_SIZE];
//! let n = encode_request(&ProtocolRequest::ReadPoint { point_id: 42 }, &mut buf).unwrap();
//! let req = decode_request(&buf[..n]).unwrap();
//! ```
//!
//! 宿主进程内的插件实现接口 `ProtocolPlugin` 仅供编译期组合（同一地址空间），
//! 跨进程通信一律走 `wire`。
//!
//! ## no_std 合规
//! 帧层纯 `core`（`#![cfg_attr(not(test), no_std)]`）；语义消息层依赖 `alloc` 与
//! `heapless`。

#![cfg_attr(not(test), no_std)]
#![allow(clippy::result_large_err)]

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

// ============================================================================
// 点类型（ADR-0027 / ARCH-S02）：canonical 定义来自 upa-model
// ============================================================================

/// 测点值类型（canonical 定义：`eneros-upa-model::PointValue`，ADR-0027）。
pub use eneros_upa_model::{DataPoint, PointId, PointQuality, PointValue};

/// 数据品质（canonical 定义：`eneros-upa-model::PointQuality`，7 位标志，ADR-0027）。
/// 旧版 3 态 `Quality`（Good/Uncertain/Bad）已删除，语义映射：
/// `Good → PointQuality::good()`、`Uncertain → { questionable: true }`、
/// `Bad → PointQuality::invalid()`（见 ADR-0027 §3 映射表）。
pub type Quality = PointQuality;

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

    /// 读取测点（按 upa-model `PointId`，ADR-0027）。
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError>;

    /// 写入测点（按 upa-model `PointId`，ADR-0027）。
    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError>;

    /// 断开连接。
    fn disconnect(&mut self) -> Result<(), ProtocolError>;

    /// 批量读取（可选，默认逐个调用 read_point）。
    fn read_batch(&mut self, point_ids: &[PointId]) -> Result<Vec<DataPoint>, ProtocolError> {
        let mut results = Vec::with_capacity(point_ids.len());
        for id in point_ids {
            results.push(self.read_point(*id)?);
        }
        Ok(results)
    }

    /// 检查连接状态（可选）。
    fn is_connected(&self) -> bool {
        false
    }
}

// ============================================================================
// IPC wire codec（ADR-0025 / ARCH-S01）
// ============================================================================

/// IPC wire codec — 协议插件跨进程消息编解码客户端。
///
/// 插件与 EL0 协议服务进程之间的请求/响应经版本化二进制帧交换
/// （固定头 + 有界长度 + CRC32，对齐 agent-sdk `wire.rs` EIPC 规范）。
/// 本 SDK 不再导出 `extern "C" -> *mut dyn` 跨二进制 trait-object（AGT-003）。
pub mod wire;

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
        fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
            if !self.connected {
                return Err(ProtocolError::NotConnected);
            }
            Ok(DataPoint {
                point_id,
                device_id: 1,
                name: alloc::string::String::from("mock"),
                description: None,
                point_type: eneros_upa_model::PointType::Analog,
                value: PointValue::Float(42.0),
                quality: PointQuality::good(),
                timestamp_ms: 0,
                source: eneros_upa_model::DataSource::Internal,
                unit: None,
            })
        }
        fn write_point(
            &mut self,
            _point_id: PointId,
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

        let dp = proto.read_point(42).expect("读取应成功");
        assert_eq!(dp.point_id, 42);
        assert_eq!(dp.value, PointValue::Float(42.0));
        assert_eq!(dp.quality, PointQuality::good());

        proto
            .write_point(100, PointValue::Float(100.0))
            .expect("写入应成功");

        proto.disconnect().expect("断开应成功");
        assert!(!proto.is_connected());
    }

    /// PRT2：未连接读取拒绝。
    #[test]
    fn prt2_read_without_connect() {
        let mut proto = MockProtocol::new();
        let err = proto.read_point(1).expect_err("应拒绝");
        assert_eq!(err, ProtocolError::NotConnected);
    }

    /// PRT3：批量读取。
    #[test]
    fn prt3_batch_read() {
        let mut proto = MockProtocol::new();
        let config = ProtocolConfig::tcp("192.168.1.1", 502);
        proto.connect(&config).expect("连接应成功");

        let points = proto.read_batch(&[1, 2, 3]).expect("批量读取应成功");
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].point_id, 1);
        assert_eq!(points[1].point_id, 2);
        assert_eq!(points[2].point_id, 3);
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

    /// PRT6：点类型转换（ADR-0027）——旧 3 态 Quality 语义 → upa-model PointQuality
    /// round-trip：Good→good()、Uncertain→{questionable}、Bad→invalid()，双向一致。
    #[test]
    fn prt6_quality_mapping_roundtrip() {
        // 旧 3 态 → 7 位标志（ADR-0027 §3 映射表）
        let good = Quality::good();
        let mut uncertain = PointQuality::good();
        uncertain.questionable = true;
        let mut bad = PointQuality::good();
        bad.invalid = true;

        // round-trip 属性：编码（encode_quality）→ 解码（decode_quality）后语义不变
        for q in [good, uncertain, bad] {
            let decoded = crate::wire::decode_quality(crate::wire::encode_quality(q));
            assert_eq!(decoded, q);
        }

        // 值类型 round-trip 属性：全部 upa-model 变体经 wire 往返一致
        let values = [
            PointValue::Float(123.456),
            PointValue::Int(-7),
            PointValue::Bool(true),
            PointValue::Enum(3),
            PointValue::String(alloc::string::String::from("P_total")),
            PointValue::Null,
        ];
        for v in values.iter() {
            let mut payload: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            crate::wire::encode_point_value(v, &mut payload).unwrap();
            let (decoded, consumed) = crate::wire::decode_point_value(&payload).unwrap();
            assert_eq!(consumed, payload.len());
            assert_eq!(&decoded, v);
        }
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
