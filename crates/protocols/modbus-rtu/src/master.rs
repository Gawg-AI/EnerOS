//! Modbus RTU 主站 — 请求/响应、重试、统计、批量轮询.

use alloc::vec::Vec;

use eneros_driver_framework::DriverError;

use crate::error::ModbusError;
use crate::frame::ModbusFrame;
use crate::point::{group_by_slave, PointMapping};
use crate::request::{ExceptionCode, ModbusRequest, ModbusResponse};

/// RTU 传输层抽象（D1）。
///
/// 由底层 RS485 驱动实现，提供字节流的发送与接收。
pub trait RtuTransport {
    /// 发送字节流
    fn send(&mut self, data: &[u8]) -> Result<(), DriverError>;
    /// 接收字节流（阻塞直到收到数据或超时）
    fn recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, DriverError>;
}

/// Modbus 主站统计信息
#[derive(Debug, Clone, Default)]
pub struct ModbusStats {
    /// 发送请求总数
    pub request_count: u32,
    /// 收到响应总数（含异常响应）
    pub response_count: u32,
    /// 错误总数（含超时/CRC/地址不匹配等）
    pub error_count: u32,
    /// 超时次数
    pub timeout_count: u32,
    /// CRC 错误次数
    pub crc_error_count: u32,
}

/// Modbus RTU 主站
///
/// 封装请求构建、CRC 校验、响应解析、重试、统计、广播处理。
pub struct ModbusRtuMaster<'a> {
    transport: &'a mut dyn RtuTransport,
    timeout_ms: u32,
    retry_count: u8,
    stats: ModbusStats,
}

impl<'a> ModbusRtuMaster<'a> {
    /// 创建主站实例。
    ///
    /// - `transport`: 传输层实现
    /// - `timeout_ms`: 单次接收超时（毫秒）
    /// - `retry_count`: 失败重试次数（实际请求次数 = retry_count + 1）
    pub fn new(transport: &'a mut dyn RtuTransport, timeout_ms: u32, retry_count: u8) -> Self {
        Self {
            transport,
            timeout_ms,
            retry_count,
            stats: ModbusStats::default(),
        }
    }

    /// 返回统计信息
    pub fn stats(&self) -> &ModbusStats {
        &self.stats
    }

    /// 构建请求帧（D7）。
    fn build_frame(&self, slave_addr: u8, req: &ModbusRequest) -> Vec<u8> {
        let func_code = req.func_code() as u8;
        let data = req.encode_data();
        let frame = ModbusFrame::new(slave_addr, func_code, data);
        frame.encode()
    }

    /// 解析响应帧（D7）。
    fn parse_response(
        &self,
        req: &ModbusRequest,
        frame: &ModbusFrame,
    ) -> Result<ModbusResponse, ModbusError> {
        // 异常响应：功能码最高位置 1
        if frame.func_code & 0x80 != 0 {
            let exc_code = if frame.data.is_empty() {
                ExceptionCode::SlaveDeviceFailure
            } else {
                ExceptionCode::from_u8(frame.data[0]).unwrap_or(ExceptionCode::SlaveDeviceFailure)
            };
            return Ok(ModbusResponse::Error {
                exception_code: exc_code,
            });
        }
        match req {
            ModbusRequest::ReadHoldingRegisters { .. } => {
                // 响应数据：[byte_count(1)][reg values...]
                if frame.data.is_empty() {
                    return Err(ModbusError::UnexpectedResponse);
                }
                let byte_count = frame.data[0] as usize;
                if frame.data.len() < 1 + byte_count {
                    return Err(ModbusError::UnexpectedResponse);
                }
                let mut regs = Vec::new();
                let mut i = 1;
                while i + 1 < frame.data.len() && regs.len() * 2 < byte_count {
                    regs.push(u16::from_be_bytes([frame.data[i], frame.data[i + 1]]));
                    i += 2;
                }
                Ok(ModbusResponse::ReadHoldingRegisters(regs))
            }
            ModbusRequest::WriteSingleRegister { .. } => {
                // 响应：[reg_addr BE(2)][value BE(2)]
                if frame.data.len() < 4 {
                    return Err(ModbusError::UnexpectedResponse);
                }
                let addr = u16::from_be_bytes([frame.data[0], frame.data[1]]);
                let value = u16::from_be_bytes([frame.data[2], frame.data[3]]);
                Ok(ModbusResponse::WriteSingleRegister { addr, value })
            }
            ModbusRequest::WriteMultipleRegisters { .. } => {
                // 响应：[start_addr BE(2)][quantity BE(2)]
                if frame.data.len() < 4 {
                    return Err(ModbusError::UnexpectedResponse);
                }
                let start_addr = u16::from_be_bytes([frame.data[0], frame.data[1]]);
                let quantity = u16::from_be_bytes([frame.data[2], frame.data[3]]);
                Ok(ModbusResponse::WriteMultipleRegisters {
                    start_addr,
                    quantity,
                })
            }
        }
    }

    /// 发送请求并等待响应（带重试 + 广播处理 D10）。
    fn send_request_with_retry(
        &mut self,
        req: &ModbusRequest,
    ) -> Result<ModbusResponse, ModbusError> {
        let slave_addr = req.slave_addr();

        // 广播（地址 0）：发送后不等待响应
        if slave_addr == 0 {
            let frame = self.build_frame(0, req);
            self.transport.send(&frame).map_err(ModbusError::Driver)?;
            self.stats.request_count += 1;
            return Ok(ModbusResponse::Broadcast);
        }

        // 校验从站地址
        if slave_addr > 247 {
            return Err(ModbusError::InvalidSlaveAddr);
        }

        for _attempt in 0..=self.retry_count {
            self.stats.request_count += 1;
            let frame = self.build_frame(slave_addr, req);
            self.transport.send(&frame).map_err(ModbusError::Driver)?;

            match self.transport.recv(self.timeout_ms) {
                Ok(resp_bytes) => match ModbusFrame::decode(&resp_bytes) {
                    Ok(resp_frame) => {
                        if resp_frame.slave_addr != slave_addr {
                            self.stats.error_count += 1;
                            return Err(ModbusError::AddrMismatch);
                        }
                        self.stats.response_count += 1;
                        return self.parse_response(req, &resp_frame);
                    }
                    Err(ModbusError::CrcMismatch) => {
                        self.stats.crc_error_count += 1;
                        self.stats.error_count += 1;
                        continue; // CRC 错误时重试
                    }
                    Err(e) => {
                        self.stats.error_count += 1;
                        return Err(e);
                    }
                },
                Err(DriverError::Timeout) => {
                    self.stats.timeout_count += 1;
                    self.stats.error_count += 1;
                    continue; // 超时重试
                }
                Err(e) => {
                    self.stats.error_count += 1;
                    return Err(ModbusError::Driver(e));
                }
            }
        }
        Err(ModbusError::MaxRetryExceeded)
    }

    /// 读保持寄存器（功能码 0x03）。
    ///
    /// - `quantity` 范围 1..=125
    pub fn read_holding_registers(
        &mut self,
        slave_addr: u8,
        start_addr: u16,
        quantity: u16,
    ) -> Result<Vec<u16>, ModbusError> {
        if quantity == 0 || quantity > 125 {
            return Err(ModbusError::InvalidQuantity);
        }
        let req = ModbusRequest::ReadHoldingRegisters {
            slave_addr,
            start_addr,
            quantity,
        };
        match self.send_request_with_retry(&req)? {
            ModbusResponse::ReadHoldingRegisters(regs) => Ok(regs),
            ModbusResponse::Error { exception_code } => Err(ModbusError::Exception(exception_code)),
            _ => Err(ModbusError::UnexpectedResponse),
        }
    }

    /// 写单个寄存器（功能码 0x06）。
    pub fn write_single_register(
        &mut self,
        slave_addr: u8,
        reg_addr: u16,
        value: u16,
    ) -> Result<(), ModbusError> {
        let req = ModbusRequest::WriteSingleRegister {
            slave_addr,
            reg_addr,
            value,
        };
        match self.send_request_with_retry(&req)? {
            ModbusResponse::WriteSingleRegister { .. } => Ok(()),
            ModbusResponse::Broadcast => Ok(()), // 广播写
            ModbusResponse::Error { exception_code } => Err(ModbusError::Exception(exception_code)),
            _ => Err(ModbusError::UnexpectedResponse),
        }
    }

    /// 写多个寄存器（功能码 0x10）。
    ///
    /// - `values` 长度范围 1..=123
    pub fn write_multiple_registers(
        &mut self,
        slave_addr: u8,
        start_addr: u16,
        values: &[u16],
    ) -> Result<(), ModbusError> {
        if values.is_empty() || values.len() > 123 {
            return Err(ModbusError::InvalidQuantity);
        }
        let req = ModbusRequest::WriteMultipleRegisters {
            slave_addr,
            start_addr,
            values: values.to_vec(),
        };
        match self.send_request_with_retry(&req)? {
            ModbusResponse::WriteMultipleRegisters { .. } => Ok(()),
            ModbusResponse::Broadcast => Ok(()),
            ModbusResponse::Error { exception_code } => Err(ModbusError::Exception(exception_code)),
            _ => Err(ModbusError::UnexpectedResponse),
        }
    }

    /// 批量轮询点表（D6 分组）。
    ///
    /// 按从站地址分组，逐点读取并转换为工程值。
    /// 返回 `[(point_id, Result<f64, ModbusError>)]`。
    pub fn poll_points(&mut self, mapping: &PointMapping) -> Vec<(u32, Result<f64, ModbusError>)> {
        let mut results = Vec::new();
        let grouped = group_by_slave(&mapping.mappings);
        for (slave_addr, regs) in grouped {
            for reg in regs {
                let word_count = reg.word_count();
                let raw = self.read_holding_registers(slave_addr, reg.reg_addr, word_count);
                let value = raw.and_then(|v| reg.convert(&v));
                results.push((reg.point_id, value));
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use super::*;
    use crate::mock::MockRtuTransport;
    use crate::point::{AccessMode, ModbusDataType, RegToPoint};

    /// 构建一个读保持寄存器响应帧
    fn build_read_response(slave_addr: u8, regs: &[u16]) -> Vec<u8> {
        let byte_count = (regs.len() * 2) as u8;
        let mut data = vec![byte_count];
        for r in regs {
            data.extend_from_slice(&r.to_be_bytes());
        }
        let frame = ModbusFrame::new(slave_addr, 0x03, data);
        frame.encode()
    }

    /// 构建一个写单寄存器响应帧
    fn build_write_single_response(slave_addr: u8, addr: u16, value: u16) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&addr.to_be_bytes());
        data.extend_from_slice(&value.to_be_bytes());
        let frame = ModbusFrame::new(slave_addr, 0x06, data);
        frame.encode()
    }

    /// 构建一个写多寄存器响应帧
    fn build_write_multiple_response(slave_addr: u8, start_addr: u16, quantity: u16) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&start_addr.to_be_bytes());
        data.extend_from_slice(&quantity.to_be_bytes());
        let frame = ModbusFrame::new(slave_addr, 0x10, data);
        frame.encode()
    }

    /// 构建异常响应帧
    fn build_exception_response(slave_addr: u8, func_code: u8, exc: ExceptionCode) -> Vec<u8> {
        let frame = ModbusFrame::new(slave_addr, func_code | 0x80, vec![exc as u8]);
        frame.encode()
    }

    #[test]
    fn test_read_holding_registers_success() {
        let mut mock = MockRtuTransport::new();
        // 响应：2 个寄存器 [0x1234, 0x5678]
        mock.push_response(build_read_response(1, &[0x1234, 0x5678]));

        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let regs = master
            .read_holding_registers(1, 0, 2)
            .expect("read should succeed");
        assert_eq!(regs, vec![0x1234, 0x5678]);
        assert_eq!(master.stats().request_count, 1);
        assert_eq!(master.stats().response_count, 1);
        assert_eq!(master.stats().error_count, 0);
    }

    #[test]
    fn test_read_holding_registers_single_reg() {
        let mut mock = MockRtuTransport::new();
        mock.push_response(build_read_response(2, &[0xABCD]));
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let regs = master
            .read_holding_registers(2, 10, 1)
            .expect("read should succeed");
        assert_eq!(regs, vec![0xABCD]);
    }

    #[test]
    fn test_read_holding_registers_invalid_quantity() {
        let mut mock = MockRtuTransport::new();
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        // quantity = 0
        assert_eq!(
            master.read_holding_registers(1, 0, 0),
            Err(ModbusError::InvalidQuantity)
        );
        // quantity = 126 > 125
        assert_eq!(
            master.read_holding_registers(1, 0, 126),
            Err(ModbusError::InvalidQuantity)
        );
        // 不应发送任何请求
        assert_eq!(master.stats().request_count, 0);
        assert!(mock.sent_frames().is_empty());
    }

    #[test]
    fn test_write_single_register_success() {
        let mut mock = MockRtuTransport::new();
        mock.push_response(build_write_single_response(1, 0x0102, 0x0304));
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        master
            .write_single_register(1, 0x0102, 0x0304)
            .expect("write should succeed");
        assert_eq!(master.stats().request_count, 1);
        assert_eq!(master.stats().response_count, 1);
    }

    #[test]
    fn test_write_multiple_registers_success() {
        let mut mock = MockRtuTransport::new();
        mock.push_response(build_write_multiple_response(1, 0, 2));
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        master
            .write_multiple_registers(1, 0, &[0x1111, 0x2222])
            .expect("write multiple should succeed");
        assert_eq!(master.stats().request_count, 1);
        assert_eq!(master.stats().response_count, 1);
    }

    #[test]
    fn test_write_multiple_registers_invalid_quantity() {
        let mut mock = MockRtuTransport::new();
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        assert_eq!(
            master.write_multiple_registers(1, 0, &[]),
            Err(ModbusError::InvalidQuantity)
        );
        let too_many = vec![0u16; 124];
        assert_eq!(
            master.write_multiple_registers(1, 0, &too_many),
            Err(ModbusError::InvalidQuantity)
        );
    }

    #[test]
    fn test_broadcast_write_single() {
        let mut mock = MockRtuTransport::new();
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        // 广播地址 0，不应等待响应
        master
            .write_single_register(0, 0, 0x1234)
            .expect("broadcast should succeed");
        assert_eq!(master.stats().request_count, 1);
        assert_eq!(master.stats().response_count, 0); // 广播不计响应
                                                      // 应已发送一帧
        assert_eq!(mock.sent_frames().len(), 1);
    }

    #[test]
    fn test_broadcast_write_multiple() {
        let mut mock = MockRtuTransport::new();
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        master
            .write_multiple_registers(0, 0, &[0x0001, 0x0002])
            .expect("broadcast should succeed");
        assert_eq!(master.stats().request_count, 1);
        assert_eq!(mock.sent_frames().len(), 1);
    }

    #[test]
    fn test_invalid_slave_addr() {
        let mut mock = MockRtuTransport::new();
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        // 地址 248 > 247
        assert_eq!(
            master.read_holding_registers(248, 0, 1),
            Err(ModbusError::InvalidSlaveAddr)
        );
        assert_eq!(master.stats().request_count, 0);
    }

    #[test]
    fn test_exception_response() {
        let mut mock = MockRtuTransport::new();
        mock.push_response(build_exception_response(
            1,
            0x03,
            ExceptionCode::IllegalDataAddress,
        ));
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let result = master.read_holding_registers(1, 0, 1);
        assert_eq!(
            result,
            Err(ModbusError::Exception(ExceptionCode::IllegalDataAddress))
        );
        assert_eq!(master.stats().response_count, 1); // 异常响应仍计入
    }

    #[test]
    fn test_crc_error_retry_then_success() {
        let mut mock = MockRtuTransport::new();
        // 第一帧 CRC 错误
        mock.push_response(vec![0x01, 0x03, 0x02, 0x12, 0x34, 0xFF, 0xFF]);
        // 第二帧正确
        mock.push_response(build_read_response(1, &[0x1234]));
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 2);
        let regs = master
            .read_holding_registers(1, 0, 1)
            .expect("retry should succeed");
        assert_eq!(regs, vec![0x1234]);
        assert_eq!(master.stats().request_count, 2);
        assert_eq!(master.stats().response_count, 1);
        assert_eq!(master.stats().crc_error_count, 1);
        assert_eq!(master.stats().error_count, 1);
    }

    #[test]
    fn test_timeout_retry_exhausted() {
        let mut mock = MockRtuTransport::new();
        mock.set_recv_timeout(true);
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 1);
        // retry_count=1 -> 共 2 次尝试均超时
        let result = master.read_holding_registers(1, 0, 1);
        assert_eq!(result, Err(ModbusError::MaxRetryExceeded));
        assert_eq!(master.stats().request_count, 2);
        assert_eq!(master.stats().timeout_count, 2);
        assert_eq!(master.stats().error_count, 2);
    }

    #[test]
    fn test_addr_mismatch() {
        let mut mock = MockRtuTransport::new();
        // 请求 slave 1，响应 slave 2
        mock.push_response(build_read_response(2, &[0x1234]));
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let result = master.read_holding_registers(1, 0, 1);
        assert_eq!(result, Err(ModbusError::AddrMismatch));
        assert_eq!(master.stats().error_count, 1);
    }

    #[test]
    fn test_poll_points() {
        let mut mock = MockRtuTransport::new();
        // 两个测点，分属两个从站
        // slave 1: U16 raw=100, scale=0.1 -> 10.0
        mock.push_response(build_read_response(1, &[100u16]));
        // slave 2: U16 raw=200, scale=0.1 -> 20.0
        mock.push_response(build_read_response(2, &[200u16]));

        let mapping = PointMapping {
            mappings: vec![
                RegToPoint {
                    point_id: 1,
                    point_name: String::from("voltage"),
                    slave_addr: 1,
                    reg_addr: 0,
                    data_type: ModbusDataType::U16,
                    scale: 0.1,
                    offset: 0.0,
                    access: AccessMode::ReadOnly,
                },
                RegToPoint {
                    point_id: 2,
                    point_name: String::from("current"),
                    slave_addr: 2,
                    reg_addr: 0,
                    data_type: ModbusDataType::U16,
                    scale: 0.1,
                    offset: 0.0,
                    access: AccessMode::ReadOnly,
                },
            ],
        };

        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let results = master.poll_points(&mapping);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 1);
        assert!((results[0].1.as_ref().unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(results[1].0, 2);
        assert!((results[1].1.as_ref().unwrap() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_poll_points_with_error() {
        let mut mock = MockRtuTransport::new();
        // 第一个测点正常，第二个超时
        mock.push_response(build_read_response(1, &[100u16]));
        // 第二次 recv 超时（队列空 -> Timeout）
        // retry_count=0，单次超时即失败
        let mapping = PointMapping {
            mappings: vec![
                RegToPoint {
                    point_id: 1,
                    point_name: String::from("ok"),
                    slave_addr: 1,
                    reg_addr: 0,
                    data_type: ModbusDataType::U16,
                    scale: 0.1,
                    offset: 0.0,
                    access: AccessMode::ReadOnly,
                },
                RegToPoint {
                    point_id: 2,
                    point_name: String::from("fail"),
                    slave_addr: 2,
                    reg_addr: 0,
                    data_type: ModbusDataType::U16,
                    scale: 0.1,
                    offset: 0.0,
                    access: AccessMode::ReadOnly,
                },
            ],
        };
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let results = master.poll_points(&mapping);
        assert_eq!(results.len(), 2);
        assert!(results[0].1.is_ok());
        assert!(results[1].1.is_err());
        // 第二个测点应因超时而 MaxRetryExceeded
        assert_eq!(results[1].1, Err(ModbusError::MaxRetryExceeded));
    }

    #[test]
    fn test_stats_default() {
        let stats = ModbusStats::default();
        assert_eq!(stats.request_count, 0);
        assert_eq!(stats.response_count, 0);
        assert_eq!(stats.error_count, 0);
        assert_eq!(stats.timeout_count, 0);
        assert_eq!(stats.crc_error_count, 0);
    }

    #[test]
    fn test_sent_frame_content() {
        let mut mock = MockRtuTransport::new();
        mock.push_response(build_read_response(1, &[0x1234]));
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let _ = master.read_holding_registers(1, 0, 1);
        // 验证发送的帧：01 03 00 00 00 01 + CRC(低字节在前 84 0A)
        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0],
            vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A]
        );
    }
}
