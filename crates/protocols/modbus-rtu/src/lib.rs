//! EnerOS Modbus RTU 主站协议栈（v0.45.0）.
//!
//! 基于 v0.44.0 RS485 驱动提供 Modbus RTU 主站协议实现，支持：
//! - 功能码 0x03（读保持寄存器）/ 0x06（写单个寄存器）/ 0x10（写多个寄存器）
//! - CRC-16/MODBUS 校验
//! - 点表映射（寄存器 ↔ 测点）与批量轮询
//! - 带重试的请求/响应、超时统计、广播处理
//!
//! # 核心类型
//! - [`master::ModbusRtuMaster`] — RTU 主站，封装请求/响应、重试、统计
//! - [`master::RtuTransport`] — RTU 传输层抽象（D1）
//! - [`frame::ModbusFrame`] — RTU 帧（地址 + 功能码 + 数据 + CRC）
//! - [`request::ModbusRequest`] / [`request::ModbusResponse`] — 请求/响应枚举
//! - [`point::PointMapping`] / [`point::RegToPoint`] — 寄存器到测点的映射
//! - [`error::ModbusError`] — 协议错误类型
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，外部依赖仅 `eneros-driver-framework`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod crc;
pub mod error;
pub mod frame;
pub mod master;
pub mod point;
pub mod request;

#[cfg(test)]
pub mod mock;

pub use crc::crc16_modbus;
pub use error::ModbusError;
pub use frame::ModbusFrame;
pub use master::{ModbusRtuMaster, ModbusStats, RtuTransport};
pub use point::{AccessMode, ModbusDataType, PointMapping, RegToPoint};
pub use request::{ExceptionCode, FunctionCode, ModbusRequest, ModbusResponse};

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use crate::mock::MockRtuTransport;
    use crate::{
        crc16_modbus, AccessMode, FunctionCode, ModbusDataType, ModbusFrame, ModbusRequest,
        ModbusRtuMaster, PointMapping, RegToPoint,
    };

    /// 构建读保持寄存器响应帧（功能码 0x03），经公共 ModbusFrame API 编码。
    fn build_read_response(slave_addr: u8, regs: &[u16]) -> Vec<u8> {
        let byte_count = (regs.len() * 2) as u8;
        let mut data = vec![byte_count];
        for r in regs {
            data.extend_from_slice(&r.to_be_bytes());
        }
        ModbusFrame::new(slave_addr, 0x03, data).encode()
    }

    /// 构建写单寄存器响应帧（功能码 0x06）。
    fn build_write_single_response(slave_addr: u8, addr: u16, value: u16) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&addr.to_be_bytes());
        data.extend_from_slice(&value.to_be_bytes());
        ModbusFrame::new(slave_addr, 0x06, data).encode()
    }

    /// 1. CRC-16/MODBUS 权威校验向量（exercise 公共 crc16_modbus 重导出）。
    #[test]
    fn test_crc_known_vector() {
        // CRC-16/MODBUS of ASCII "123456789" == 0x4B37（标准 check value）
        assert_eq!(crc16_modbus(b"123456789"), 0x4B37);
    }

    /// 2. 帧编解码往返集成（crc + frame 协同，经公共 ModbusFrame API）。
    #[test]
    fn test_frame_roundtrip_integration() {
        let original = ModbusFrame::new(0x02, 0x03, vec![0x10, 0x20, 0x30, 0x40]);
        let encoded = original.encode();
        // 线上格式：addr(1) + func(1) + data(4) + crc_le(2) = 8 字节
        assert_eq!(encoded.len(), 8);

        let decoded = ModbusFrame::decode(&encoded).expect("decode should succeed");
        // 验证全部字段一致
        assert_eq!(decoded.slave_addr, original.slave_addr);
        assert_eq!(decoded.func_code, original.func_code);
        assert_eq!(decoded.data, original.data);
        assert_eq!(decoded.crc, original.crc);
        // 解码后重新编码应与原线上字节流完全一致
        assert_eq!(decoded.encode(), encoded);
    }

    /// 3. 主站读保持寄存器端到端集成（master + frame + mock + crc）。
    #[test]
    fn test_master_read_holding_registers_integration() {
        let mut mock = MockRtuTransport::new();
        // 预置响应：从站 1 返回 2 个寄存器 [0x1234, 0x5678]
        mock.push_response(build_read_response(1, &[0x1234, 0x5678]));

        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let regs = master
            .read_holding_registers(1, 0x0102, 2)
            .expect("read should succeed");

        // 验证返回的寄存器值
        assert_eq!(regs, vec![0x1234, 0x5678]);

        // 先取统计副本（此后 master 不再使用，释放对 mock 的可变借用）
        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1);
        assert_eq!(stats.error_count, 0);
        assert_eq!(stats.timeout_count, 0);
        assert_eq!(stats.crc_error_count, 0);

        // 验证发送帧内容：用公共 ModbusRequest + ModbusFrame 重建期望帧
        let expected_req = ModbusRequest::ReadHoldingRegisters {
            slave_addr: 1,
            start_addr: 0x0102,
            quantity: 2,
        };
        assert_eq!(expected_req.func_code(), FunctionCode::ReadHoldingRegisters);
        let expected_frame = ModbusFrame::new(
            1,
            expected_req.func_code() as u8,
            expected_req.encode_data(),
        )
        .encode();
        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], expected_frame);
        // 结构性校验：addr=01, func=03, start_addr=0102, quantity=0002
        assert_eq!(sent[0][0], 0x01);
        assert_eq!(sent[0][1], 0x03);
        assert_eq!(&sent[0][2..6], &[0x01, 0x02, 0x00, 0x02]);
    }

    /// 4. 主站写单个寄存器端到端集成。
    #[test]
    fn test_master_write_single_register_integration() {
        let mut mock = MockRtuTransport::new();
        // 预置写单寄存器响应：回显地址与值
        mock.push_response(build_write_single_response(1, 0x0102, 0x0304));

        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        master
            .write_single_register(1, 0x0102, 0x0304)
            .expect("write should succeed");

        // 先取统计副本（此后 master 不再使用，释放对 mock 的可变借用）
        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1);

        // 验证发送帧内容
        let expected_req = ModbusRequest::WriteSingleRegister {
            slave_addr: 1,
            reg_addr: 0x0102,
            value: 0x0304,
        };
        assert_eq!(expected_req.func_code(), FunctionCode::WriteSingleRegister);
        let expected_frame = ModbusFrame::new(
            1,
            expected_req.func_code() as u8,
            expected_req.encode_data(),
        )
        .encode();
        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], expected_frame);
        // 结构性校验：addr=01, func=06, reg_addr=0102, value=0304
        assert_eq!(sent[0][0], 0x01);
        assert_eq!(sent[0][1], 0x06);
        assert_eq!(&sent[0][2..6], &[0x01, 0x02, 0x03, 0x04]);
    }

    /// 5. 广播写多寄存器集成（地址 0，无需响应）。
    #[test]
    fn test_master_broadcast_write_integration() {
        let mut mock = MockRtuTransport::new();
        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        // 广播地址 0：内部 send_request_with_retry 直接返回 ModbusResponse::Broadcast，
        // write_multiple_registers 将其映射为 Ok(())，且不消费任何响应。
        master
            .write_multiple_registers(0, 0x0010, &[0x0001, 0x0002])
            .expect("broadcast write should succeed");

        // 广播路径无响应：response_count 应为 0
        // 先取统计副本（此后 master 不再使用，释放对 mock 的可变借用）
        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 0);
        assert_eq!(stats.error_count, 0);

        // 用公共 API 重建期望帧进行整体比对
        let expected_req = ModbusRequest::WriteMultipleRegisters {
            slave_addr: 0,
            start_addr: 0x0010,
            values: vec![0x0001, 0x0002],
        };
        assert_eq!(
            expected_req.func_code(),
            FunctionCode::WriteMultipleRegisters
        );
        let expected_frame = ModbusFrame::new(
            0,
            expected_req.func_code() as u8,
            expected_req.encode_data(),
        )
        .encode();
        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], expected_frame);
        // 关键广播语义校验：首字节为广播地址 0，功能码 0x10
        assert_eq!(sent[0][0], 0x00);
        assert_eq!(sent[0][1], 0x10);
    }

    /// 6. 测点转换集成（U32 大端字序，经公共 RegToPoint/ModbusDataType API）。
    #[test]
    fn test_point_conversion_integration() {
        // U32 大端：高字 0x0001，低字 0x0002 -> 0x00010002 = 65538
        let point = RegToPoint {
            point_id: 100,
            point_name: String::from("energy_total"),
            slave_addr: 1,
            reg_addr: 0,
            data_type: ModbusDataType::U32,
            scale: 1.0,
            offset: 0.0,
            access: AccessMode::ReadOnly,
        };
        assert_eq!(point.word_count(), 2);
        let value = point
            .convert(&[0x0001u16, 0x0002u16])
            .expect("convert should succeed");
        assert!((value - 65538.0).abs() < 1e-9);

        // 带 scale/offset 的 U32 转换：65538 * 0.001 + 1.0 = 66.538
        let point_scaled = RegToPoint {
            point_id: 101,
            point_name: String::from("energy_scaled"),
            slave_addr: 1,
            reg_addr: 0,
            data_type: ModbusDataType::U32,
            scale: 0.001,
            offset: 1.0,
            access: AccessMode::ReadWrite,
        };
        let v2 = point_scaled
            .convert(&[0x0001u16, 0x0002u16])
            .expect("convert should succeed");
        assert!((v2 - 66.538).abs() < 1e-9);
    }

    /// 7. 批量轮询点表端到端集成（多从站、U16+U32 混合类型、多响应）。
    #[test]
    fn test_poll_points_integration() {
        let mut mock = MockRtuTransport::new();
        // 点表（group_by_slave 顺序：slave 1 -> [point1, point2], slave 2 -> [point3]）：
        // - point 1: slave 1, U16, scale 0.1 -> 1 reg, raw=100 -> 10.0
        // - point 2: slave 1, U32, scale 1.0 -> 2 regs, raw=0x00010002 -> 65538.0
        // - point 3: slave 2, U16, scale 2.0 -> 1 reg, raw=50 -> 100.0
        mock.push_response(build_read_response(1, &[100u16]));
        mock.push_response(build_read_response(1, &[0x0001u16, 0x0002u16]));
        mock.push_response(build_read_response(2, &[50u16]));

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
                    point_name: String::from("energy"),
                    slave_addr: 1,
                    reg_addr: 1,
                    data_type: ModbusDataType::U32,
                    scale: 1.0,
                    offset: 0.0,
                    access: AccessMode::ReadOnly,
                },
                RegToPoint {
                    point_id: 3,
                    point_name: String::from("current"),
                    slave_addr: 2,
                    reg_addr: 0,
                    data_type: ModbusDataType::U16,
                    scale: 2.0,
                    offset: 0.0,
                    access: AccessMode::ReadOnly,
                },
            ],
        };

        let mut master = ModbusRtuMaster::new(&mut mock, 100, 0);
        let results = master.poll_points(&mapping);

        // 验证结果数量与顺序
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, 1);
        assert!((results[0].1.as_ref().unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(results[1].0, 2);
        assert!((results[1].1.as_ref().unwrap() - 65538.0).abs() < 1e-9);
        assert_eq!(results[2].0, 3);
        assert!((results[2].1.as_ref().unwrap() - 100.0).abs() < 1e-9);

        // 验证统计：3 次请求全部成功
        let stats = master.stats();
        assert_eq!(stats.request_count, 3);
        assert_eq!(stats.response_count, 3);
        assert_eq!(stats.error_count, 0);
    }
}
