//! Modbus 协议错误类型.

use core::fmt;

use eneros_driver_framework::DriverError;

use crate::request::ExceptionCode;

/// Modbus 协议错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModbusError {
    /// 帧过短（少于 4 字节）
    FrameTooShort,
    /// CRC 校验失败
    CrcMismatch,
    /// 响应地址与请求不匹配
    AddrMismatch,
    /// 非预期响应（结构不符）
    UnexpectedResponse,
    /// 从站返回异常码
    Exception(ExceptionCode),
    /// 底层驱动错误
    Driver(DriverError),
    /// 超过最大重试次数
    MaxRetryExceeded,
    /// 非法从站地址（>247）
    InvalidSlaveAddr,
    /// 非法数量（0 或超过协议上限）
    InvalidQuantity,
    /// 非法寄存器地址
    InvalidRegisterAddr,
    /// 不支持的功能码
    UnsupportedFunction,
}

impl fmt::Display for ModbusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooShort => write!(f, "modbus frame too short"),
            Self::CrcMismatch => write!(f, "modbus CRC mismatch"),
            Self::AddrMismatch => write!(f, "modbus response address mismatch"),
            Self::UnexpectedResponse => write!(f, "modbus unexpected response"),
            Self::Exception(code) => write!(f, "modbus exception: {:?}", code),
            Self::Driver(e) => write!(f, "modbus driver error: {}", e),
            Self::MaxRetryExceeded => write!(f, "modbus max retry exceeded"),
            Self::InvalidSlaveAddr => write!(f, "modbus invalid slave address"),
            Self::InvalidQuantity => write!(f, "modbus invalid quantity"),
            Self::InvalidRegisterAddr => write!(f, "modbus invalid register address"),
            Self::UnsupportedFunction => write!(f, "modbus unsupported function code"),
        }
    }
}

impl core::error::Error for ModbusError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Driver(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DriverError> for ModbusError {
    fn from(e: DriverError) -> Self {
        Self::Driver(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_all_variants() {
        assert_eq!(
            format!("{}", ModbusError::FrameTooShort),
            "modbus frame too short"
        );
        assert_eq!(
            format!("{}", ModbusError::CrcMismatch),
            "modbus CRC mismatch"
        );
        assert_eq!(
            format!("{}", ModbusError::AddrMismatch),
            "modbus response address mismatch"
        );
        assert_eq!(
            format!("{}", ModbusError::UnexpectedResponse),
            "modbus unexpected response"
        );
        assert_eq!(
            format!("{}", ModbusError::Exception(ExceptionCode::IllegalFunction)),
            "modbus exception: IllegalFunction"
        );
        assert_eq!(
            format!("{}", ModbusError::Driver(DriverError::Timeout)),
            "modbus driver error: operation timed out"
        );
        assert_eq!(
            format!("{}", ModbusError::MaxRetryExceeded),
            "modbus max retry exceeded"
        );
        assert_eq!(
            format!("{}", ModbusError::InvalidSlaveAddr),
            "modbus invalid slave address"
        );
        assert_eq!(
            format!("{}", ModbusError::InvalidQuantity),
            "modbus invalid quantity"
        );
        assert_eq!(
            format!("{}", ModbusError::InvalidRegisterAddr),
            "modbus invalid register address"
        );
        assert_eq!(
            format!("{}", ModbusError::UnsupportedFunction),
            "modbus unsupported function code"
        );
    }

    #[test]
    fn test_from_driver_error() {
        let e: ModbusError = DriverError::Timeout.into();
        assert_eq!(e, ModbusError::Driver(DriverError::Timeout));
    }

    #[test]
    fn test_eq() {
        assert_eq!(ModbusError::FrameTooShort, ModbusError::FrameTooShort);
        assert_ne!(ModbusError::FrameTooShort, ModbusError::CrcMismatch);
        assert_eq!(
            ModbusError::Exception(ExceptionCode::IllegalFunction),
            ModbusError::Exception(ExceptionCode::IllegalFunction)
        );
        assert_ne!(
            ModbusError::Exception(ExceptionCode::IllegalFunction),
            ModbusError::Exception(ExceptionCode::IllegalDataAddress)
        );
    }
}
