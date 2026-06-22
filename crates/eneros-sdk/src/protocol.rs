//! 协议适配器开发 SDK — ProtocolAdapterBuilder 与 ProtocolAdapterSdk
//!
//! 提供构造器模式构建 [`ProtocolAdapterConfig`]，封装协议适配器开发
//! 所需的常用参数（协议类型、设备地址、扫描周期、通信超时）。
//! 第三方开发者通过本模块可以快速组装协议适配器配置，再交由
//! `eneros-device` 的具体 adapter 实现进行连接与数据交换。

use crate::common::SdkResult;
use eneros_device::ProtocolType;

/// 默认扫描周期（1 秒）
const DEFAULT_SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// 默认通信超时（5 秒）
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 协议适配器配置
///
/// 封装协议适配器开发所需的常用参数。具体协议的细粒度配置
/// （如 Modbus slave_id、IEC 61850 logical_devices）由各 adapter
/// 自身的 `ConnectionConfig` / `ProtocolConfig` 承载，本配置仅提供
/// 顶层通用字段。
#[derive(Debug, Clone)]
pub struct ProtocolAdapterConfig {
    /// 协议类型
    pub protocol_type: ProtocolType,
    /// 设备地址（如 `127.0.0.1:502`）
    pub address: String,
    /// 扫描周期
    pub scan_interval: std::time::Duration,
    /// 通信超时
    pub timeout: std::time::Duration,
}

/// 协议适配器构造器 — 使用构造器模式构建 [`ProtocolAdapterConfig`]
///
/// # 示例
/// ```no_run
/// use eneros_sdk::protocol::ProtocolAdapterBuilder;
/// use eneros_device::ProtocolType;
///
/// let config = ProtocolAdapterBuilder::new(ProtocolType::Modbus, "127.0.0.1:502")
///     .scan_interval(std::time::Duration::from_millis(500))
///     .timeout(std::time::Duration::from_secs(2))
///     .build()
/// .unwrap();
/// ```
pub struct ProtocolAdapterBuilder {
    protocol_type: ProtocolType,
    address: String,
    scan_interval: std::time::Duration,
    timeout: std::time::Duration,
}

impl ProtocolAdapterBuilder {
    /// 创建新的协议适配器构造器
    ///
    /// 默认值：
    /// - scan_interval: 1 秒
    /// - timeout: 5 秒
    pub fn new(protocol_type: ProtocolType, address: impl Into<String>) -> Self {
        Self {
            protocol_type,
            address: address.into(),
            scan_interval: DEFAULT_SCAN_INTERVAL,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// 设置扫描周期
    pub fn scan_interval(mut self, d: std::time::Duration) -> Self {
        self.scan_interval = d;
        self
    }

    /// 设置通信超时
    pub fn timeout(mut self, d: std::time::Duration) -> Self {
        self.timeout = d;
        self
    }

    /// 构建 [`ProtocolAdapterConfig`]
    pub fn build(self) -> SdkResult<ProtocolAdapterConfig> {
        Ok(ProtocolAdapterConfig {
            protocol_type: self.protocol_type,
            address: self.address,
            scan_interval: self.scan_interval,
            timeout: self.timeout,
        })
    }
}

/// 协议适配器 SDK 封装
///
/// 持有 [`ProtocolAdapterConfig`]，供协议适配器开发者在运行时
/// 读取配置参数。具体的连接与数据交换由 `eneros-device` 的
/// adapter 实现负责。
pub struct ProtocolAdapterSdk {
    /// 协议适配器配置
    pub config: ProtocolAdapterConfig,
}

impl ProtocolAdapterSdk {
    /// 创建协议适配器 SDK
    pub fn new(config: ProtocolAdapterConfig) -> Self {
        Self { config }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_adapter_builder_new() {
        let builder = ProtocolAdapterBuilder::new(ProtocolType::Modbus, "127.0.0.1:502");
        assert_eq!(builder.protocol_type, ProtocolType::Modbus);
        assert_eq!(builder.address, "127.0.0.1:502");
        assert_eq!(builder.scan_interval, DEFAULT_SCAN_INTERVAL);
        assert_eq!(builder.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn test_protocol_adapter_builder_with_scan_interval() {
        let builder = ProtocolAdapterBuilder::new(ProtocolType::Iec104, "127.0.0.1:2404")
            .scan_interval(std::time::Duration::from_millis(500));
        assert_eq!(builder.scan_interval, std::time::Duration::from_millis(500));
    }

    #[test]
    fn test_protocol_adapter_builder_build() {
        let config = ProtocolAdapterBuilder::new(ProtocolType::Mqtt, "127.0.0.1:1883")
            .scan_interval(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("build should succeed");
        assert_eq!(config.protocol_type, ProtocolType::Mqtt);
        assert_eq!(config.address, "127.0.0.1:1883");
        assert_eq!(config.scan_interval, std::time::Duration::from_secs(2));
        assert_eq!(config.timeout, std::time::Duration::from_secs(10));
    }
}
