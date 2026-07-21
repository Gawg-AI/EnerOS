//! Modbus TCP 主站 — MBAP 编帧、响应解析、重试、统计、批量轮询.
//!
//! 与 v0.45.0 RTU 主站的关系（D5）：
//! - 复用 `eneros_modbus_rtu` 的应用层类型（`ModbusRequest`/`ModbusResponse`/...）
//! - TCP 帧结构 = MBAP 头(7B) + PDU(func_code + data)，**无 CRC**
//! - `device.unit_id` 替代 RTU 的 `slave_addr`（D3 语义等价）
//! - 事务 ID 管理（u16 回绕）替代 RTU 的地址匹配

use alloc::vec::Vec;

use eneros_modbus_rtu::{ExceptionCode, ModbusError, ModbusRequest, ModbusResponse, PointMapping};

use crate::device::TcpDevice;
use crate::error::ModbusTcpError;
use crate::mbap::MbapHeader;
use crate::transport::{TcpStats, TcpTransport};

/// Modbus TCP 主站
///
/// 封装 MBAP 编帧、事务 ID 管理、响应解析、超时重试、统计与多设备轮询。
pub struct ModbusTcpMaster<'a> {
    transport: &'a mut dyn TcpTransport,
    next_txn_id: u16,
    stats: TcpStats,
    retry_count: u8,
}

impl<'a> ModbusTcpMaster<'a> {
    /// 创建主站实例。
    ///
    /// - `transport`: TCP 传输层实现
    /// - `retry_count`: 失败重试次数（实际请求次数 = retry_count + 1）
    /// - `next_txn_id` 初始为 0
    pub fn new(transport: &'a mut dyn TcpTransport, retry_count: u8) -> Self {
        Self {
            transport,
            next_txn_id: 0,
            stats: TcpStats::default(),
            retry_count,
        }
    }

    /// 返回统计信息。
    pub fn stats(&self) -> &TcpStats {
        &self.stats
    }

    /// 返回当前事务 ID 并自增（u16 回绕：0 → 1 → ... → 65535 → 0）。
    pub fn next_txn_id(&mut self) -> u16 {
        let id = self.next_txn_id;
        self.next_txn_id = self.next_txn_id.wrapping_add(1);
        id
    }

    /// 构建 PDU（func_code + data），复用 v0.45.0 的 `encode_data()`（D3）。
    pub fn build_pdu(req: &ModbusRequest) -> Vec<u8> {
        let data = req.encode_data();
        let mut pdu = Vec::with_capacity(1 + data.len());
        pdu.push(req.func_code() as u8);
        pdu.extend_from_slice(&data);
        pdu
    }

    /// 构建完整 TCP 帧：MBAP 头(7B) + PDU。
    pub fn build_frame(txn_id: u16, unit_id: u8, pdu: &[u8]) -> Vec<u8> {
        let header = MbapHeader::new(txn_id, unit_id, pdu.len() as u16);
        let mut frame = Vec::with_capacity(7 + pdu.len());
        frame.extend_from_slice(&header.encode());
        frame.extend_from_slice(pdu);
        frame
    }

    /// 解析响应帧（MBAP + PDU），无 CRC 校验（D5：TCP 无 CRC）。
    ///
    /// 1. 解码 MBAP 头（长度 < 7 → FrameTooShort，protocol_id != 0 → InvalidProtocolId）
    /// 2. 校验事务 ID（不匹配 → TransactionMismatch）
    /// 3. 解析 PDU（异常响应 func_code & 0x80）
    pub fn parse_response(
        req: &ModbusRequest,
        txn_id: u16,
        resp: &[u8],
    ) -> Result<ModbusResponse, ModbusTcpError> {
        let header = MbapHeader::decode(resp)?;
        if header.transaction_id != txn_id {
            return Err(ModbusTcpError::TransactionMismatch);
        }

        // PDU = resp[7..]（MBAP 头之后）
        if resp.len() < 8 {
            return Err(ModbusTcpError::Modbus(ModbusError::UnexpectedResponse));
        }
        let pdu = &resp[7..];
        let func_code = pdu[0];
        let data = &pdu[1..];

        // 异常响应
        if func_code & 0x80 != 0 {
            let exc_code = if data.is_empty() {
                ExceptionCode::SlaveDeviceFailure
            } else {
                ExceptionCode::from_u8(data[0]).unwrap_or(ExceptionCode::SlaveDeviceFailure)
            };
            return Ok(ModbusResponse::Error {
                exception_code: exc_code,
            });
        }

        // 正常响应：按请求类型解析
        match req {
            ModbusRequest::ReadHoldingRegisters { .. } => {
                // [byte_count(1)][reg values...]
                if data.is_empty() {
                    return Err(ModbusTcpError::Modbus(ModbusError::UnexpectedResponse));
                }
                let byte_count = data[0] as usize;
                if data.len() < 1 + byte_count {
                    return Err(ModbusTcpError::Modbus(ModbusError::UnexpectedResponse));
                }
                let mut regs = Vec::new();
                let mut i = 1;
                while i + 1 < data.len() && regs.len() * 2 < byte_count {
                    regs.push(u16::from_be_bytes([data[i], data[i + 1]]));
                    i += 2;
                }
                Ok(ModbusResponse::ReadHoldingRegisters(regs))
            }
            ModbusRequest::WriteSingleRegister { .. } => {
                // [reg_addr BE(2)][value BE(2)]
                if data.len() < 4 {
                    return Err(ModbusTcpError::Modbus(ModbusError::UnexpectedResponse));
                }
                let addr = u16::from_be_bytes([data[0], data[1]]);
                let value = u16::from_be_bytes([data[2], data[3]]);
                Ok(ModbusResponse::WriteSingleRegister { addr, value })
            }
            ModbusRequest::WriteMultipleRegisters { .. } => {
                // [start_addr BE(2)][quantity BE(2)]
                if data.len() < 4 {
                    return Err(ModbusTcpError::Modbus(ModbusError::UnexpectedResponse));
                }
                let start_addr = u16::from_be_bytes([data[0], data[1]]);
                let quantity = u16::from_be_bytes([data[2], data[3]]);
                Ok(ModbusResponse::WriteMultipleRegisters {
                    start_addr,
                    quantity,
                })
            }
        }
    }

    /// 发送请求并等待响应（带超时重试）。
    ///
    /// 流程：connect → (send → recv)* → parse。
    /// 仅 `Timeout` 触发重试；其他错误立即返回。
    /// 每次调用 `connect` 递增 `reconnect_count`。
    pub fn send_request_with_retry(
        &mut self,
        device: &TcpDevice,
        req: &ModbusRequest,
    ) -> Result<ModbusResponse, ModbusTcpError> {
        // 建立连接（D7：每次请求建立连接，计数重连）
        self.transport
            .connect(device)
            .map_err(|_| ModbusTcpError::ConnectionFailed)?;
        self.stats.reconnect_count += 1;

        // 构建请求帧（事务 ID + MBAP + PDU）
        let txn_id = self.next_txn_id();
        let unit_id = device.unit_id;
        let pdu = Self::build_pdu(req);
        let frame = Self::build_frame(txn_id, unit_id, &pdu);

        for _attempt in 0..=self.retry_count {
            self.stats.request_count += 1;
            if let Err(e) = self.transport.send(&frame) {
                self.stats.error_count += 1;
                return Err(e);
            }

            match self.transport.recv(device.timeout_ms) {
                Ok(resp_bytes) => {
                    self.stats.response_count += 1;
                    return Self::parse_response(req, txn_id, &resp_bytes);
                }
                Err(ModbusTcpError::Timeout) => {
                    self.stats.timeout_count += 1;
                    self.stats.error_count += 1;
                    continue; // 超时重试
                }
                Err(e) => {
                    self.stats.error_count += 1;
                    return Err(e);
                }
            }
        }
        Err(ModbusTcpError::Modbus(ModbusError::MaxRetryExceeded))
    }

    /// 读保持寄存器（功能码 0x03）。
    ///
    /// - `quantity` 范围 1..=125
    pub fn read_holding_registers(
        &mut self,
        device: &TcpDevice,
        start_addr: u16,
        quantity: u16,
    ) -> Result<Vec<u16>, ModbusTcpError> {
        if quantity == 0 || quantity > 125 {
            return Err(ModbusTcpError::Modbus(ModbusError::InvalidQuantity));
        }
        let req = ModbusRequest::ReadHoldingRegisters {
            slave_addr: device.unit_id,
            start_addr,
            quantity,
        };
        match self.send_request_with_retry(device, &req)? {
            ModbusResponse::ReadHoldingRegisters(regs) => Ok(regs),
            ModbusResponse::Error { exception_code } => Err(ModbusTcpError::Modbus(
                ModbusError::Exception(exception_code),
            )),
            _ => Err(ModbusTcpError::Modbus(ModbusError::UnexpectedResponse)),
        }
    }

    /// 写单个寄存器（功能码 0x06）。
    pub fn write_single_register(
        &mut self,
        device: &TcpDevice,
        reg_addr: u16,
        value: u16,
    ) -> Result<(), ModbusTcpError> {
        let req = ModbusRequest::WriteSingleRegister {
            slave_addr: device.unit_id,
            reg_addr,
            value,
        };
        match self.send_request_with_retry(device, &req)? {
            ModbusResponse::WriteSingleRegister { .. } => Ok(()),
            ModbusResponse::Error { exception_code } => Err(ModbusTcpError::Modbus(
                ModbusError::Exception(exception_code),
            )),
            _ => Err(ModbusTcpError::Modbus(ModbusError::UnexpectedResponse)),
        }
    }

    /// 写多个寄存器（功能码 0x10）。
    ///
    /// - `values` 长度范围 1..=123
    pub fn write_multiple_registers(
        &mut self,
        device: &TcpDevice,
        start_addr: u16,
        values: &[u16],
    ) -> Result<(), ModbusTcpError> {
        if values.is_empty() || values.len() > 123 {
            return Err(ModbusTcpError::Modbus(ModbusError::InvalidQuantity));
        }
        let req = ModbusRequest::WriteMultipleRegisters {
            slave_addr: device.unit_id,
            start_addr,
            values: values.to_vec(),
        };
        match self.send_request_with_retry(device, &req)? {
            ModbusResponse::WriteMultipleRegisters { .. } => Ok(()),
            ModbusResponse::Error { exception_code } => Err(ModbusTcpError::Modbus(
                ModbusError::Exception(exception_code),
            )),
            _ => Err(ModbusTcpError::Modbus(ModbusError::UnexpectedResponse)),
        }
    }

    /// 批量轮询多设备点表（D7：串行迭代）。
    ///
    /// 对每个设备，匹配 `mapping` 中 `slave_addr == device.unit_id` 的测点，
    /// 逐点读取寄存器并经 `RegToPoint::convert` 转换为工程值。
    /// 返回 `[(device, [(point_id, Result<f64, ModbusTcpError>)])]`。
    #[allow(clippy::type_complexity)]
    pub fn poll_devices(
        &mut self,
        devices: &[TcpDevice],
        mapping: &PointMapping,
    ) -> Vec<(TcpDevice, Vec<(u32, Result<f64, ModbusTcpError>)>)> {
        let mut results = Vec::new();
        for device in devices {
            let mut device_results = Vec::new();
            for point in &mapping.mappings {
                if point.slave_addr != device.unit_id {
                    continue;
                }
                let word_count = point.word_count();
                let raw = self.read_holding_registers(device, point.reg_addr, word_count);
                let value = raw.and_then(|v| point.convert(&v).map_err(ModbusTcpError::from));
                device_results.push((point.point_id, value));
            }
            results.push((*device, device_results));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use eneros_modbus_rtu::{AccessMode, ModbusDataType, RegToPoint};

    use super::*;
    use crate::mock::MockTcpTransport;

    // ===== 测试辅助函数 =====

    /// 构建读保持寄存器响应帧（MBAP + PDU）
    fn build_read_response(txn_id: u16, unit_id: u8, regs: &[u16]) -> Vec<u8> {
        let mut pdu = vec![0x03]; // func_code
        pdu.push((regs.len() * 2) as u8); // byte_count
        for r in regs {
            pdu.extend_from_slice(&r.to_be_bytes());
        }
        ModbusTcpMaster::build_frame(txn_id, unit_id, &pdu)
    }

    /// 构建写单寄存器响应帧
    fn build_write_single_response(txn_id: u16, unit_id: u8, addr: u16, value: u16) -> Vec<u8> {
        let mut pdu = vec![0x06];
        pdu.extend_from_slice(&addr.to_be_bytes());
        pdu.extend_from_slice(&value.to_be_bytes());
        ModbusTcpMaster::build_frame(txn_id, unit_id, &pdu)
    }

    /// 构建写多寄存器响应帧
    fn build_write_multiple_response(
        txn_id: u16,
        unit_id: u8,
        start_addr: u16,
        quantity: u16,
    ) -> Vec<u8> {
        let mut pdu = vec![0x10];
        pdu.extend_from_slice(&start_addr.to_be_bytes());
        pdu.extend_from_slice(&quantity.to_be_bytes());
        ModbusTcpMaster::build_frame(txn_id, unit_id, &pdu)
    }

    /// 构建异常响应帧
    fn build_exception_response(
        txn_id: u16,
        unit_id: u8,
        func_code: u8,
        exc: ExceptionCode,
    ) -> Vec<u8> {
        let pdu = vec![func_code | 0x80, exc as u8];
        ModbusTcpMaster::build_frame(txn_id, unit_id, &pdu)
    }

    fn make_device(unit_id: u8) -> TcpDevice {
        TcpDevice::new([192, 168, 1, 10], 502, unit_id)
    }

    // ===== 1. 读保持寄存器成功 =====
    #[test]
    fn test_read_holding_registers_success() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_read_response(0, 1, &[0x1234, 0x5678]));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        let regs = master
            .read_holding_registers(&dev, 0, 2)
            .expect("read should succeed");
        assert_eq!(regs, vec![0x1234, 0x5678]);

        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1);
        assert_eq!(stats.error_count, 0);
        assert_eq!(stats.timeout_count, 0);
        assert_eq!(stats.reconnect_count, 1);
    }

    // ===== 2. 读单个寄存器 =====
    #[test]
    fn test_read_holding_registers_single_reg() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_read_response(0, 2, &[0xABCD]));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = TcpDevice::new([10, 0, 0, 1], 502, 2);
        let regs = master
            .read_holding_registers(&dev, 10, 1)
            .expect("read should succeed");
        assert_eq!(regs, vec![0xABCD]);
    }

    // ===== 3. 非法数量校验 =====
    #[test]
    fn test_read_holding_registers_invalid_quantity() {
        let mut mock = MockTcpTransport::new();
        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);

        // quantity = 0
        assert_eq!(
            master.read_holding_registers(&dev, 0, 0),
            Err(ModbusTcpError::Modbus(ModbusError::InvalidQuantity))
        );
        // quantity = 126 > 125
        assert_eq!(
            master.read_holding_registers(&dev, 0, 126),
            Err(ModbusTcpError::Modbus(ModbusError::InvalidQuantity))
        );
        // 不应发送任何请求
        assert_eq!(master.stats().request_count, 0);
        assert!(mock.sent_frames().is_empty());
    }

    // ===== 4. 写单寄存器成功 =====
    #[test]
    fn test_write_single_register_success() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_write_single_response(0, 1, 0x0102, 0x0304));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        master
            .write_single_register(&dev, 0x0102, 0x0304)
            .expect("write should succeed");

        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1);
        assert_eq!(stats.reconnect_count, 1);
    }

    // ===== 5. 写多寄存器成功 =====
    #[test]
    fn test_write_multiple_registers_success() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_write_multiple_response(0, 1, 0, 2));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        master
            .write_multiple_registers(&dev, 0, &[0x1111, 0x2222])
            .expect("write multiple should succeed");

        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1);
    }

    // ===== 6. 写多寄存器非法数量 =====
    #[test]
    fn test_write_multiple_registers_invalid_quantity() {
        let mut mock = MockTcpTransport::new();
        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);

        assert_eq!(
            master.write_multiple_registers(&dev, 0, &[]),
            Err(ModbusTcpError::Modbus(ModbusError::InvalidQuantity))
        );
        let too_many = vec![0u16; 124];
        assert_eq!(
            master.write_multiple_registers(&dev, 0, &too_many),
            Err(ModbusTcpError::Modbus(ModbusError::InvalidQuantity))
        );
        assert_eq!(master.stats().request_count, 0);
    }

    // ===== 7. 事务 ID 不匹配 =====
    #[test]
    fn test_transaction_mismatch() {
        let mut mock = MockTcpTransport::new();
        // 请求用 txn_id=0，响应用 txn_id=999
        mock.push_response(build_read_response(999, 1, &[0x1234]));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        let result = master.read_holding_registers(&dev, 0, 1);
        assert_eq!(result, Err(ModbusTcpError::TransactionMismatch));

        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1); // 收到响应但解析失败
        assert_eq!(stats.error_count, 0); // TransactionMismatch 不计入 error_count
    }

    // ===== 8. 超时重试耗尽 =====
    #[test]
    fn test_timeout_retry_exhausted() {
        let mut mock = MockTcpTransport::new();
        mock.set_recv_timeout(true);

        let mut master = ModbusTcpMaster::new(&mut mock, 1);
        let dev = make_device(1);
        let result = master.read_holding_registers(&dev, 0, 1);
        assert_eq!(
            result,
            Err(ModbusTcpError::Modbus(ModbusError::MaxRetryExceeded))
        );

        let stats = master.stats().clone();
        // retry_count=1 → 共 2 次尝试
        assert_eq!(stats.request_count, 2);
        assert_eq!(stats.response_count, 0);
        assert_eq!(stats.timeout_count, 2);
        assert_eq!(stats.error_count, 2);
        assert_eq!(stats.reconnect_count, 1); // 连接只调用一次
    }

    // ===== 9. 异常响应 =====
    #[test]
    fn test_exception_response() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_exception_response(
            0,
            1,
            0x03,
            ExceptionCode::IllegalDataAddress,
        ));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        let result = master.read_holding_registers(&dev, 0, 1);
        assert_eq!(
            result,
            Err(ModbusTcpError::Modbus(ModbusError::Exception(
                ExceptionCode::IllegalDataAddress
            )))
        );

        let stats = master.stats().clone();
        assert_eq!(stats.response_count, 1); // 异常响应仍计入
        assert_eq!(stats.error_count, 0);
    }

    // ===== 10. 批量轮询成功 =====
    #[test]
    fn test_poll_devices_success() {
        let mut mock = MockTcpTransport::new();
        // 设备 1 (unit_id=1): txn_id=0, raw=100 → 10.0
        mock.push_response(build_read_response(0, 1, &[100u16]));
        // 设备 2 (unit_id=2): txn_id=1, raw=200 → 20.0
        mock.push_response(build_read_response(1, 2, &[200u16]));

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

        let devices = vec![
            TcpDevice::new([192, 168, 1, 1], 502, 1),
            TcpDevice::new([192, 168, 1, 2], 502, 2),
        ];

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let results = master.poll_devices(&devices, &mapping);

        assert_eq!(results.len(), 2);
        // 设备 1
        assert_eq!(results[0].0, devices[0]);
        assert_eq!(results[0].1.len(), 1);
        assert_eq!(results[0].1[0].0, 1);
        assert!((results[0].1[0].1.as_ref().unwrap() - 10.0).abs() < 1e-9);
        // 设备 2
        assert_eq!(results[1].0, devices[1]);
        assert_eq!(results[1].1.len(), 1);
        assert_eq!(results[1].1[0].0, 2);
        assert!((results[1].1[0].1.as_ref().unwrap() - 20.0).abs() < 1e-9);

        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 2);
        assert_eq!(stats.response_count, 2);
        assert_eq!(stats.reconnect_count, 2);
    }

    // ===== 11. 批量轮询含错误 =====
    #[test]
    fn test_poll_devices_with_error() {
        let mut mock = MockTcpTransport::new();
        // 设备 1 成功
        mock.push_response(build_read_response(0, 1, &[100u16]));
        // 设备 2 无响应 → 超时

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

        let devices = vec![
            TcpDevice::new([192, 168, 1, 1], 502, 1),
            TcpDevice::new([192, 168, 1, 2], 502, 2),
        ];

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let results = master.poll_devices(&devices, &mapping);

        assert_eq!(results.len(), 2);
        assert!(results[0].1[0].1.is_ok());
        assert!(results[1].1[0].1.is_err());
        assert_eq!(
            results[1].1[0].1,
            Err(ModbusTcpError::Modbus(ModbusError::MaxRetryExceeded))
        );
    }

    // ===== 12. 事务 ID 初始值为 0 =====
    #[test]
    fn test_txn_id_initial_zero() {
        let mut mock = MockTcpTransport::new();
        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        assert_eq!(master.next_txn_id(), 0);
        assert_eq!(master.next_txn_id(), 1);
        assert_eq!(master.next_txn_id(), 2);
    }

    // ===== 13. 事务 ID 回绕（65535 → 0）=====
    #[test]
    fn test_txn_id_wraparound() {
        let mut mock = MockTcpTransport::new();
        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        // 推进 65536 次：返回 0..=65535，next_txn_id 回到 0
        for _ in 0..0x1_0000 {
            let _ = master.next_txn_id();
        }
        // 下一次应返回 0（回绕）
        assert_eq!(master.next_txn_id(), 0);
        assert_eq!(master.next_txn_id(), 1);
    }

    // ===== 14. build_pdu 内容验证 =====
    #[test]
    fn test_build_pdu_content() {
        let req = ModbusRequest::ReadHoldingRegisters {
            slave_addr: 1,
            start_addr: 0x0102,
            quantity: 0x0003,
        };
        let pdu = ModbusTcpMaster::build_pdu(&req);
        // func_code(0x03) + start_addr BE + quantity BE
        assert_eq!(pdu, vec![0x03, 0x01, 0x02, 0x00, 0x03]);

        let req2 = ModbusRequest::WriteSingleRegister {
            slave_addr: 1,
            reg_addr: 0x0102,
            value: 0x0304,
        };
        let pdu2 = ModbusTcpMaster::build_pdu(&req2);
        assert_eq!(pdu2, vec![0x06, 0x01, 0x02, 0x03, 0x04]);

        let req3 = ModbusRequest::WriteMultipleRegisters {
            slave_addr: 1,
            start_addr: 0x0102,
            values: vec![0x0304, 0x0506],
        };
        let pdu3 = ModbusTcpMaster::build_pdu(&req3);
        // func_code + start_addr BE + quantity BE + byte_count + values BE
        assert_eq!(
            pdu3,
            vec![0x10, 0x01, 0x02, 0x00, 0x02, 0x04, 0x03, 0x04, 0x05, 0x06]
        );
    }

    // ===== 15. build_frame 内容验证（MBAP + PDU）=====
    #[test]
    fn test_build_frame_content() {
        let pdu = [0x03, 0x00, 0x00, 0x00, 0x01]; // read 1 reg at addr 0
        let frame = ModbusTcpMaster::build_frame(0x1234, 0x05, &pdu);
        // MBAP: txn=1234, proto=0000, length=0006 (5+1), unit=05
        // PDU: 03 00 00 00 01
        assert_eq!(
            frame,
            vec![
                0x12, 0x34, // transaction_id
                0x00, 0x00, // protocol_id
                0x00, 0x06, // length (pdu_len 5 + 1)
                0x05, // unit_id
                0x03, 0x00, 0x00, 0x00, 0x01, // PDU
            ]
        );
    }

    // ===== 16. parse_response 帧过短 =====
    #[test]
    fn test_parse_response_frame_too_short() {
        let req = ModbusRequest::ReadHoldingRegisters {
            slave_addr: 1,
            start_addr: 0,
            quantity: 1,
        };
        let buf = [0u8; 6]; // < 7
        let result = ModbusTcpMaster::parse_response(&req, 0, &buf);
        // ModbusResponse 未实现 PartialEq，用 unwrap_err 比较 Err 变体
        assert_eq!(result.unwrap_err(), ModbusTcpError::FrameTooShort);
    }

    // ===== 17. parse_response 协议 ID 非 0 =====
    #[test]
    fn test_parse_response_invalid_protocol_id() {
        let req = ModbusRequest::ReadHoldingRegisters {
            slave_addr: 1,
            start_addr: 0,
            quantity: 1,
        };
        // protocol_id = 1
        let buf = [0x00, 0x01, 0x00, 0x01, 0x00, 0x05, 0xFF, 0x03];
        let result = ModbusTcpMaster::parse_response(&req, 1, &buf);
        assert_eq!(result.unwrap_err(), ModbusTcpError::InvalidProtocolId);
    }

    // ===== 18. 连接递增 reconnect_count =====
    #[test]
    fn test_connect_increments_reconnect_count() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_read_response(0, 1, &[0x1234]));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        let _ = master.read_holding_registers(&dev, 0, 1);

        assert_eq!(master.stats().reconnect_count, 1);
        assert_eq!(mock.connect_calls().len(), 1);
        assert_eq!(mock.connect_calls()[0], dev);
    }

    // ===== 19. 发送帧结构验证（MBAP 头 + PDU）=====
    #[test]
    fn test_sent_frame_structure() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_read_response(0, 1, &[0x1234]));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        let _ = master.read_holding_registers(&dev, 0, 1);

        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        // MBAP: txn=0000, proto=0000, length=0006 (5+1), unit=01
        // PDU: 03 0000 0001 (func=read, start_addr=0, qty=1)
        assert_eq!(
            sent[0],
            vec![
                0x00, 0x00, // transaction_id = 0
                0x00, 0x00, // protocol_id = 0
                0x00, 0x06, // length = 6
                0x01, // unit_id = 1
                0x03, // func_code = read holding registers
                0x00, 0x00, // start_addr = 0
                0x00, 0x01, // quantity = 1
            ]
        );
    }

    // ===== 20. 统计综合验证 =====
    #[test]
    fn test_stats_comprehensive() {
        let mut mock = MockTcpTransport::new();
        // 第一次成功
        mock.push_response(build_read_response(0, 1, &[0x1234]));
        // 第二次超时（队列将空）

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);

        // 第一次：成功
        let _ = master.read_holding_registers(&dev, 0, 1);
        // 第二次：超时 → MaxRetryExceeded（retry_count=0，单次尝试）
        let _ = master.read_holding_registers(&dev, 0, 1);

        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 2);
        assert_eq!(stats.response_count, 1); // 第一次成功
        assert_eq!(stats.timeout_count, 1); // 第二次超时
        assert_eq!(stats.error_count, 1);
        assert_eq!(stats.reconnect_count, 2); // 每次请求都连接
    }

    // ===== 21. 写多寄存器发送帧验证 =====
    #[test]
    fn test_write_multiple_sent_frame() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_write_multiple_response(0, 1, 0x0010, 2));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        master
            .write_multiple_registers(&dev, 0x0010, &[0x0001, 0x0002])
            .expect("write should succeed");

        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        // MBAP: txn=0000, proto=0000, length=000b (pdu 10 + 1), unit=01
        // PDU: 10 0010 0002 04 0001 0002 (10 bytes)
        assert_eq!(
            sent[0],
            vec![
                0x00, 0x00, // transaction_id
                0x00, 0x00, // protocol_id
                0x00, 0x0B, // length = 11 (pdu 10 + 1)
                0x01, // unit_id
                0x10, // func_code = write multiple
                0x00, 0x10, // start_addr
                0x00, 0x02, // quantity
                0x04, // byte_count
                0x00, 0x01, // value 1
                0x00, 0x02, // value 2
            ]
        );
    }
}
