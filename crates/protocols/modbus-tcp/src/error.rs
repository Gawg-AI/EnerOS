//! Modbus TCP 协议错误类型.

use core::fmt;

use eneros_modbus_rtu::ModbusError;

/// Modbus TCP 协议错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModbusTcpError {
    /// 复用底层 Modbus 应用层错误
    Modbus(ModbusError),
    /// 事务 ID 不匹配（响应 txn_id != 请求 txn_id）
    TransactionMismatch,
    /// 连接失败
    ConnectionFailed,
    /// 未连接（在 connect 之前调用 send/recv）
    NotConnected,
    /// 接收超时
    Timeout,
    /// 连接已关闭
    Closed,
    /// 帧过短（MBAP 头部不足 7 字节）
    FrameTooShort,
    /// MBAP 协议 ID 非 0（仅 Modbus 协议 = 0 合法）
    InvalidProtocolId,
}

impl fmt::Display for ModbusTcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Modbus(e) => write!(f, "modbus tcp application error: {}", e),
            Self::TransactionMismatch => write!(f, "modbus tcp transaction id mismatch"),
            Self::ConnectionFailed => write!(f, "modbus tcp connection failed"),
            Self::NotConnected => write!(f, "modbus tcp not connected"),
            Self::Timeout => write!(f, "modbus tcp timeout"),
            Self::Closed => write!(f, "modbus tcp connection closed"),
            Self::FrameTooShort => write!(f, "modbus tcp frame too short"),
            Self::InvalidProtocolId => write!(f, "modbus tcp invalid protocol id"),
        }
    }
}

impl core::error::Error for ModbusTcpError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Modbus(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ModbusError> for ModbusTcpError {
    fn from(e: ModbusError) -> Self {
        Self::Modbus(e)
    }
}

#[cfg(test)]
mod tests {
    use eneros_modbus_rtu::ExceptionCode;

    use super::*;

    #[test]
    fn test_display_all_variants() {
        assert_eq!(
            format!("{}", ModbusTcpError::TransactionMismatch),
            "modbus tcp transaction id mismatch"
        );
        assert_eq!(
            format!("{}", ModbusTcpError::ConnectionFailed),
            "modbus tcp connection failed"
        );
        assert_eq!(
            format!("{}", ModbusTcpError::NotConnected),
            "modbus tcp not connected"
        );
        assert_eq!(format!("{}", ModbusTcpError::Timeout), "modbus tcp timeout");
        assert_eq!(
            format!("{}", ModbusTcpError::Closed),
            "modbus tcp connection closed"
        );
        assert_eq!(
            format!("{}", ModbusTcpError::FrameTooShort),
            "modbus tcp frame too short"
        );
        assert_eq!(
            format!("{}", ModbusTcpError::InvalidProtocolId),
            "modbus tcp invalid protocol id"
        );
    }

    #[test]
    fn test_display_modbus_variant() {
        let e = ModbusTcpError::Modbus(ModbusError::Exception(ExceptionCode::IllegalFunction));
        assert_eq!(
            format!("{}", e),
            "modbus tcp application error: modbus exception: IllegalFunction"
        );
    }

    #[test]
    fn test_from_modbus_error() {
        let e: ModbusTcpError = ModbusError::FrameTooShort.into();
        assert_eq!(e, ModbusTcpError::Modbus(ModbusError::FrameTooShort));
    }

    #[test]
    fn test_eq() {
        assert_eq!(ModbusTcpError::Timeout, ModbusTcpError::Timeout);
        assert_ne!(ModbusTcpError::Timeout, ModbusTcpError::Closed);
        assert_eq!(
            ModbusTcpError::Modbus(ModbusError::FrameTooShort),
            ModbusTcpError::Modbus(ModbusError::FrameTooShort)
        );
    }
}
