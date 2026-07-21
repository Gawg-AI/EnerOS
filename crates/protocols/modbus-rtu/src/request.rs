//! Modbus 功能码、异常码、请求与响应类型.
//!
//! 支持功能码：
//! - 0x03 读保持寄存器
//! - 0x06 写单个寄存器
//! - 0x10 写多个寄存器

use alloc::vec::Vec;

/// Modbus 功能码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionCode {
    /// 读线圈（0x01）
    ReadCoils = 0x01,
    /// 读保持寄存器（0x03）
    ReadHoldingRegisters = 0x03,
    /// 读输入寄存器（0x04）
    ReadInputRegisters = 0x04,
    /// 写单个线圈（0x05）
    WriteSingleCoil = 0x05,
    /// 写单个寄存器（0x06）
    WriteSingleRegister = 0x06,
    /// 写多个寄存器（0x10）
    WriteMultipleRegisters = 0x10,
}

/// Modbus 异常码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionCode {
    /// 非法功能码（0x01）
    IllegalFunction = 0x01,
    /// 非法数据地址（0x02）
    IllegalDataAddress = 0x02,
    /// 非法数据值（0x03）
    IllegalDataValue = 0x03,
    /// 从站设备故障（0x04）
    SlaveDeviceFailure = 0x04,
    /// 确认（0x05，处理中需等待）
    Acknowledge = 0x05,
    /// 从站设备忙（0x06）
    SlaveDeviceBusy = 0x06,
}

impl ExceptionCode {
    /// 从 u8 构造异常码，未知值返回 None。
    pub fn from_u8(code: u8) -> Option<Self> {
        match code {
            0x01 => Some(Self::IllegalFunction),
            0x02 => Some(Self::IllegalDataAddress),
            0x03 => Some(Self::IllegalDataValue),
            0x04 => Some(Self::SlaveDeviceFailure),
            0x05 => Some(Self::Acknowledge),
            0x06 => Some(Self::SlaveDeviceBusy),
            _ => None,
        }
    }
}

/// Modbus 请求（主站发起）
#[derive(Debug, Clone)]
pub enum ModbusRequest {
    /// 读保持寄存器（0x03）
    ReadHoldingRegisters {
        slave_addr: u8,
        start_addr: u16,
        quantity: u16,
    },
    /// 写单个寄存器（0x06）
    WriteSingleRegister {
        slave_addr: u8,
        reg_addr: u16,
        value: u16,
    },
    /// 写多个寄存器（0x10）
    WriteMultipleRegisters {
        slave_addr: u8,
        start_addr: u16,
        values: Vec<u16>,
    },
}

impl ModbusRequest {
    /// 返回从站地址
    pub fn slave_addr(&self) -> u8 {
        match self {
            Self::ReadHoldingRegisters { slave_addr, .. }
            | Self::WriteSingleRegister { slave_addr, .. }
            | Self::WriteMultipleRegisters { slave_addr, .. } => *slave_addr,
        }
    }

    /// 返回功能码
    pub fn func_code(&self) -> FunctionCode {
        match self {
            Self::ReadHoldingRegisters { .. } => FunctionCode::ReadHoldingRegisters,
            Self::WriteSingleRegister { .. } => FunctionCode::WriteSingleRegister,
            Self::WriteMultipleRegisters { .. } => FunctionCode::WriteMultipleRegisters,
        }
    }

    /// 编码请求帧的数据域（不含从站地址和功能码）。
    ///
    /// - ReadHoldingRegisters: start_addr BE(2) + quantity BE(2)
    /// - WriteSingleRegister: reg_addr BE(2) + value BE(2)
    /// - WriteMultipleRegisters: start_addr BE(2) + quantity BE(2) + byte_count(1) + values BE(...)
    pub fn encode_data(&self) -> Vec<u8> {
        match self {
            Self::ReadHoldingRegisters {
                start_addr,
                quantity,
                ..
            } => {
                let mut buf = Vec::with_capacity(4);
                buf.extend_from_slice(&start_addr.to_be_bytes());
                buf.extend_from_slice(&quantity.to_be_bytes());
                buf
            }
            Self::WriteSingleRegister {
                reg_addr, value, ..
            } => {
                let mut buf = Vec::with_capacity(4);
                buf.extend_from_slice(&reg_addr.to_be_bytes());
                buf.extend_from_slice(&value.to_be_bytes());
                buf
            }
            Self::WriteMultipleRegisters {
                start_addr, values, ..
            } => {
                let byte_count = (values.len() * 2) as u8;
                let mut buf = Vec::with_capacity(5 + values.len() * 2);
                buf.extend_from_slice(&start_addr.to_be_bytes());
                buf.extend_from_slice(&(values.len() as u16).to_be_bytes());
                buf.push(byte_count);
                for v in values {
                    buf.extend_from_slice(&v.to_be_bytes());
                }
                buf
            }
        }
    }
}

/// Modbus 响应（从站返回）
#[derive(Debug, Clone)]
pub enum ModbusResponse {
    /// 读保持寄存器响应（寄存器值列表）
    ReadHoldingRegisters(Vec<u16>),
    /// 写单个寄存器响应
    WriteSingleRegister { addr: u16, value: u16 },
    /// 写多个寄存器响应
    WriteMultipleRegisters { start_addr: u16, quantity: u16 },
    /// 异常响应
    Error { exception_code: ExceptionCode },
    /// 广播（无响应）
    Broadcast,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exception_code_from_u8() {
        assert_eq!(
            ExceptionCode::from_u8(0x01),
            Some(ExceptionCode::IllegalFunction)
        );
        assert_eq!(
            ExceptionCode::from_u8(0x02),
            Some(ExceptionCode::IllegalDataAddress)
        );
        assert_eq!(
            ExceptionCode::from_u8(0x03),
            Some(ExceptionCode::IllegalDataValue)
        );
        assert_eq!(
            ExceptionCode::from_u8(0x04),
            Some(ExceptionCode::SlaveDeviceFailure)
        );
        assert_eq!(
            ExceptionCode::from_u8(0x05),
            Some(ExceptionCode::Acknowledge)
        );
        assert_eq!(
            ExceptionCode::from_u8(0x06),
            Some(ExceptionCode::SlaveDeviceBusy)
        );
        assert_eq!(ExceptionCode::from_u8(0x00), None);
        assert_eq!(ExceptionCode::from_u8(0x07), None);
        assert_eq!(ExceptionCode::from_u8(0xFF), None);
    }

    #[test]
    fn test_func_code_values() {
        assert_eq!(FunctionCode::ReadCoils as u8, 0x01);
        assert_eq!(FunctionCode::ReadHoldingRegisters as u8, 0x03);
        assert_eq!(FunctionCode::ReadInputRegisters as u8, 0x04);
        assert_eq!(FunctionCode::WriteSingleCoil as u8, 0x05);
        assert_eq!(FunctionCode::WriteSingleRegister as u8, 0x06);
        assert_eq!(FunctionCode::WriteMultipleRegisters as u8, 0x10);
    }

    #[test]
    fn test_request_slave_addr() {
        let r = ModbusRequest::ReadHoldingRegisters {
            slave_addr: 5,
            start_addr: 0,
            quantity: 1,
        };
        assert_eq!(r.slave_addr(), 5);

        let w = ModbusRequest::WriteSingleRegister {
            slave_addr: 7,
            reg_addr: 10,
            value: 0x1234,
        };
        assert_eq!(w.slave_addr(), 7);

        let wm = ModbusRequest::WriteMultipleRegisters {
            slave_addr: 9,
            start_addr: 0,
            values: alloc::vec![1, 2],
        };
        assert_eq!(wm.slave_addr(), 9);
    }

    #[test]
    fn test_request_func_code() {
        let r = ModbusRequest::ReadHoldingRegisters {
            slave_addr: 1,
            start_addr: 0,
            quantity: 1,
        };
        assert_eq!(r.func_code(), FunctionCode::ReadHoldingRegisters);

        let w = ModbusRequest::WriteSingleRegister {
            slave_addr: 1,
            reg_addr: 0,
            value: 0,
        };
        assert_eq!(w.func_code(), FunctionCode::WriteSingleRegister);

        let wm = ModbusRequest::WriteMultipleRegisters {
            slave_addr: 1,
            start_addr: 0,
            values: alloc::vec![0],
        };
        assert_eq!(wm.func_code(), FunctionCode::WriteMultipleRegisters);
    }

    #[test]
    fn test_encode_data_read_holding() {
        let r = ModbusRequest::ReadHoldingRegisters {
            slave_addr: 1,
            start_addr: 0x0102,
            quantity: 0x0003,
        };
        // start_addr BE + quantity BE
        assert_eq!(r.encode_data(), vec![0x01, 0x02, 0x00, 0x03]);
    }

    #[test]
    fn test_encode_data_write_single() {
        let r = ModbusRequest::WriteSingleRegister {
            slave_addr: 1,
            reg_addr: 0x0102,
            value: 0x0304,
        };
        assert_eq!(r.encode_data(), vec![0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn test_encode_data_write_multiple() {
        let r = ModbusRequest::WriteMultipleRegisters {
            slave_addr: 1,
            start_addr: 0x0102,
            values: alloc::vec![0x0304, 0x0506],
        };
        // start_addr BE(2) + quantity BE(2) + byte_count(1) + values BE(...)
        assert_eq!(
            r.encode_data(),
            vec![0x01, 0x02, 0x00, 0x02, 0x04, 0x03, 0x04, 0x05, 0x06]
        );
    }
}
