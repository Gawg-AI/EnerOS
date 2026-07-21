//! Device simulator core types (D11: host-side std).
//!
//! 定义 [`SimConfig`] / [`SimError`] / [`SimHandle`]，封装被模拟设备的
//! 配置、生命周期与请求响应生成。当前为骨架实现（响应为占位回显），
//! 真实协议解码与点表填充由后续版本补全。

use std::fmt;

/// 模拟器配置：描述被模拟设备的基本参数。
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// 模拟协议：`modbus-rtu` / `modbus-tcp` / `iec104` / `can`。
    pub protocol: String,
    /// TCP/UDP 端口（用于 modbus-tcp / iec104）。
    pub port: u16,
    /// 从站/服务器地址（Modbus）或公共地址（IEC 104）。
    pub slave_addr: u8,
    /// 波特率（仅 Modbus RTU）。
    pub baud_rate: u32,
    /// 绑定的 IPv4 地址（仅 TCP 类协议）。
    pub ip: String,
    /// 模拟数据点数量。
    pub point_count: u16,
}

/// 模拟器错误类型。
#[derive(Debug)]
pub enum SimError {
    /// 配置非法或不完整。
    ConfigError(String),
    /// 网络 bind/listen 失败。
    NetworkError(String),
    /// 协议组帧或响应错误。
    ProtocolError(String),
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::ConfigError(msg) => write!(f, "ConfigError: {}", msg),
            SimError::NetworkError(msg) => write!(f, "NetworkError: {}", msg),
            SimError::ProtocolError(msg) => write!(f, "ProtocolError: {}", msg),
        }
    }
}

impl std::error::Error for SimError {}

/// 运行中的设备模拟器实例句柄。
pub struct SimHandle {
    /// 创建模拟器时使用的配置。
    pub config: SimConfig,
    /// 模拟器当前是否处于运行状态。
    pub running: bool,
}

impl SimHandle {
    /// 根据配置创建新的模拟器句柄（尚未启动）。
    pub fn new(config: SimConfig) -> Self {
        Self {
            config,
            running: false,
        }
    }

    /// 启动模拟器。
    ///
    /// 骨架实现仅校验配置不变量并翻转 `running` 标志；真实实现将打开
    /// 串口/TCP 监听并开始接收请求帧。
    pub fn start(&mut self) -> Result<(), SimError> {
        if self.running {
            return Err(SimError::ConfigError("simulator already running".to_string()));
        }
        match self.config.protocol.as_str() {
            "modbus-rtu" | "modbus-tcp" | "iec104" | "can" => {}
            other => {
                return Err(SimError::ConfigError(format!(
                    "unsupported protocol: {}",
                    other
                )));
            }
        }
        if self.config.point_count == 0 {
            return Err(SimError::ConfigError("point_count must be > 0".to_string()));
        }
        self.running = true;
        Ok(())
    }

    /// 停止模拟器。
    pub fn stop(&mut self) -> Result<(), SimError> {
        if !self.running {
            return Err(SimError::ConfigError("simulator not running".to_string()));
        }
        self.running = false;
        Ok(())
    }

    /// 针对入站请求帧生成响应。
    ///
    /// 骨架实现返回占位响应（协议标签 + 请求长度 + 回显请求）。
    /// 真实实现将依据 `self.config.protocol` 解码请求，并从模拟点表
    /// 中产生符合协议规范的应答。
    pub fn generate_response(&self, request: &[u8]) -> Vec<u8> {
        let mut resp = Vec::with_capacity(request.len() + 4);
        resp.extend_from_slice(self.config.protocol.as_bytes());
        resp.push(0x00);
        resp.push(request.len() as u8);
        resp.extend_from_slice(request);
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modbus_rtu_config() -> SimConfig {
        SimConfig {
            protocol: "modbus-rtu".to_string(),
            port: 502,
            slave_addr: 1,
            baud_rate: 9600,
            ip: "127.0.0.1".to_string(),
            point_count: 10,
        }
    }

    #[test]
    fn start_stop_lifecycle() {
        let mut handle = SimHandle::new(modbus_rtu_config());
        assert!(!handle.running);
        assert!(handle.start().is_ok());
        assert!(handle.running);
        // double-start is an error
        assert!(handle.start().is_err());
        assert!(handle.stop().is_ok());
        assert!(!handle.running);
        // double-stop is an error
        assert!(handle.stop().is_err());
    }

    #[test]
    fn invalid_protocol_rejected() {
        let mut handle = SimHandle::new(SimConfig {
            protocol: "unknown".to_string(),
            port: 502,
            slave_addr: 1,
            baud_rate: 9600,
            ip: "127.0.0.1".to_string(),
            point_count: 10,
        });
        let err = handle.start().unwrap_err();
        assert!(matches!(err, SimError::ConfigError(_)));
    }

    #[test]
    fn zero_point_count_rejected() {
        let mut handle = SimHandle::new(SimConfig {
            protocol: "can".to_string(),
            port: 502,
            slave_addr: 1,
            baud_rate: 9600,
            ip: "127.0.0.1".to_string(),
            point_count: 0,
        });
        assert!(handle.start().is_err());
    }

    #[test]
    fn generate_response_nonempty() {
        let handle = SimHandle::new(modbus_rtu_config());
        let resp = handle.generate_response(&[0x01, 0x03]);
        assert!(!resp.is_empty());
        assert!(resp.starts_with(b"modbus-rtu"));
    }
}
