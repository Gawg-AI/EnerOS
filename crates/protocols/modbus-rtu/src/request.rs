//! Modbus 功能码、异常码、请求与响应类型.
//!
//! 支持功能码（v0.218.0 B2-01 补全）：
//! - 0x01 读线圈
//! - 0x02 读离散输入
//! - 0x03 读保持寄存器
//! - 0x04 读输入寄存器
//! - 0x05 写单个线圈
//! - 0x06 写单个寄存器
//! - 0x0F 写多个线圈
//! - 0x10 写多个寄存器

use alloc::vec::Vec;

/// Modbus 功能码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionCode {
    /// 读线圈（0x01）
    ReadCoils = 0x01,
    /// 读离散输入（0x02）
    ReadDiscreteInputs = 0x02,
    /// 读保持寄存器（0x03）
    ReadHoldingRegisters = 0x03,
    /// 读输入寄存器（0x04）
    ReadInputRegisters = 0x04,
    /// 写单个线圈（0x05）
    WriteSingleCoil = 0x05,
    /// 写单个寄存器（0x06）
    WriteSingleRegister = 0x06,
    /// 写多个线圈（0x0F）
    WriteMultipleCoils = 0x0F,
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
    /// 读线圈（0x01）
    ReadCoils {
        slave_addr: u8,
        start_addr: u16,
        quantity: u16,
    },
    /// 读离散输入（0x02）
    ReadDiscreteInputs {
        slave_addr: u8,
        start_addr: u16,
        quantity: u16,
    },
    /// 读保持寄存器（0x03）
    ReadHoldingRegisters {
        slave_addr: u8,
        start_addr: u16,
        quantity: u16,
    },
    /// 读输入寄存器（0x04）
    ReadInputRegisters {
        slave_addr: u8,
        start_addr: u16,
        quantity: u16,
    },
    /// 写单个线圈（0x05）
    WriteSingleCoil {
        slave_addr: u8,
        coil_addr: u16,
        value: bool,
    },
    /// 写单个寄存器（0x06）
    WriteSingleRegister {
        slave_addr: u8,
        reg_addr: u16,
        value: u16,
    },
    /// 写多个线圈（0x0F）
    WriteMultipleCoils {
        slave_addr: u8,
        start_addr: u16,
        values: Vec<bool>,
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
            Self::ReadCoils { slave_addr, .. }
            | Self::ReadDiscreteInputs { slave_addr, .. }
            | Self::ReadHoldingRegisters { slave_addr, .. }
            | Self::ReadInputRegisters { slave_addr, .. }
            | Self::WriteSingleCoil { slave_addr, .. }
            | Self::WriteSingleRegister { slave_addr, .. }
            | Self::WriteMultipleCoils { slave_addr, .. }
            | Self::WriteMultipleRegisters { slave_addr, .. } => *slave_addr,
        }
    }

    /// 返回功能码
    pub fn func_code(&self) -> FunctionCode {
        match self {
            Self::ReadCoils { .. } => FunctionCode::ReadCoils,
            Self::ReadDiscreteInputs { .. } => FunctionCode::ReadDiscreteInputs,
            Self::ReadHoldingRegisters { .. } => FunctionCode::ReadHoldingRegisters,
            Self::ReadInputRegisters { .. } => FunctionCode::ReadInputRegisters,
            Self::WriteSingleCoil { .. } => FunctionCode::WriteSingleCoil,
            Self::WriteSingleRegister { .. } => FunctionCode::WriteSingleRegister,
            Self::WriteMultipleCoils { .. } => FunctionCode::WriteMultipleCoils,
            Self::WriteMultipleRegisters { .. } => FunctionCode::WriteMultipleRegisters,
        }
    }

    /// 编码请求帧的数据域（不含从站地址和功能码）。
    ///
    /// - ReadCoils/ReadDiscreteInputs/ReadHoldingRegisters/ReadInputRegisters:
    ///   start_addr BE(2) + quantity BE(2)
    /// - WriteSingleCoil: coil_addr BE(2) + value(0xFF00/0x0000)
    /// - WriteSingleRegister: reg_addr BE(2) + value BE(2)
    /// - WriteMultipleCoils: start_addr BE(2) + quantity BE(2) + byte_count(1) + packed bits
    /// - WriteMultipleRegisters: start_addr BE(2) + quantity BE(2) + byte_count(1) + values BE(...)
    pub fn encode_data(&self) -> Vec<u8> {
        match self {
            Self::ReadCoils {
                start_addr,
                quantity,
                ..
            }
            | Self::ReadDiscreteInputs {
                start_addr,
                quantity,
                ..
            }
            | Self::ReadHoldingRegisters {
                start_addr,
                quantity,
                ..
            }
            | Self::ReadInputRegisters {
                start_addr,
                quantity,
                ..
            } => {
                let mut buf = Vec::with_capacity(4);
                buf.extend_from_slice(&start_addr.to_be_bytes());
                buf.extend_from_slice(&quantity.to_be_bytes());
                buf
            }
            Self::WriteSingleCoil {
                coil_addr, value, ..
            } => {
                let mut buf = Vec::with_capacity(4);
                buf.extend_from_slice(&coil_addr.to_be_bytes());
                // Modbus 协议：ON=0xFF00，OFF=0x0000。
                buf.extend_from_slice(if *value { &[0xFF, 0x00] } else { &[0x00, 0x00] });
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
            Self::WriteMultipleCoils {
                start_addr, values, ..
            } => {
                let quantity = values.len();
                let byte_count = quantity.div_ceil(8);
                let mut buf = Vec::with_capacity(5 + byte_count);
                buf.extend_from_slice(&start_addr.to_be_bytes());
                buf.extend_from_slice(&(quantity as u16).to_be_bytes());
                buf.push(byte_count as u8);
                // 位打包：LSB 在前（线圈 i 在字节 i/8 的位 i%8）。
                for byte_idx in 0..byte_count {
                    let mut byte = 0u8;
                    for bit in 0..8 {
                        let idx = byte_idx * 8 + bit;
                        if idx < quantity && values[idx] {
                            byte |= 1 << bit;
                        }
                    }
                    buf.push(byte);
                }
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
    /// 读线圈响应（位列表）
    ReadCoils(Vec<bool>),
    /// 读离散输入响应（位列表）
    ReadDiscreteInputs(Vec<bool>),
    /// 读保持寄存器响应（寄存器值列表）
    ReadHoldingRegisters(Vec<u16>),
    /// 读输入寄存器响应（寄存器值列表）
    ReadInputRegisters(Vec<u16>),
    /// 写单个线圈响应
    WriteSingleCoil { addr: u16, value: bool },
    /// 写单个寄存器响应
    WriteSingleRegister { addr: u16, value: u16 },
    /// 写多个线圈响应
    WriteMultipleCoils { start_addr: u16, quantity: u16 },
    /// 写多个寄存器响应
    WriteMultipleRegisters { start_addr: u16, quantity: u16 },
    /// 异常响应
    Error { exception_code: ExceptionCode },
    /// 广播（无响应）
    Broadcast,
}

/// 解析读位类响应数据（byte_count + packed bits）。
#[allow(clippy::result_unit_err)] // 结构错误由调用方统一映射为 UnexpectedResponse
pub fn parse_bit_data(data: &[u8], quantity: u16) -> Result<Vec<bool>, ()> {
    if data.is_empty() {
        return Err(());
    }
    let byte_count = data[0] as usize;
    if data.len() < 1 + byte_count {
        return Err(());
    }
    let mut bits = Vec::with_capacity(quantity as usize);
    for i in 0..quantity as usize {
        let byte = data[1 + i / 8];
        bits.push(byte & (1 << (i % 8)) != 0);
    }
    Ok(bits)
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
