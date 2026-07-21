//! EnerOS Modbus TCP 主站协议栈（v0.46.0）.
//!
//! 基于 v0.45.0 `eneros-modbus-rtu` 的应用层类型，提供 Modbus TCP 主站实现：
//! - 功能码 0x03（读保持寄存器）/ 0x06（写单个寄存器）/ 0x10（写多个寄存器）
//! - MBAP 头部编解码（无 CRC，TCP 帧结构）
//! - 事务 ID 管理（u16 回绕）
//! - 带重试的请求/响应、超时统计、重连统计
//! - 多设备批量轮询与点表映射
//!
//! # 核心类型
//! - [`master::ModbusTcpMaster`] — TCP 主站，封装 MBAP 编帧、响应解析、重试、统计
//! - [`transport::TcpTransport`] — TCP 传输层抽象
//! - [`mbap::MbapHeader`] — MBAP 头部（事务 ID + 协议 ID + 长度 + 单元 ID）
//! - [`device::TcpDevice`] — TCP 从设备描述（IP/端口/单元 ID/超时）
//! - [`transport::TcpStats`] — TCP 主站统计
//!
//! # 与 v0.45.0 的关系
//!
//! 复用 `eneros-modbus-rtu` 的应用层类型：`ModbusRequest` / `ModbusResponse` /
//! `FunctionCode` / `ExceptionCode` / `PointMapping` / `RegToPoint` /
//! `ModbusDataType` / `AccessMode` / `ModbusError`。
//! TCP 与 RTU 的差异在于传输层：TCP 用 MBAP 头替代 RTU 的从站地址 + CRC16。
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，外部依赖仅 `eneros-modbus-rtu`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod device;
pub mod error;
pub mod master;
pub mod mbap;
pub mod transport;

#[cfg(test)]
pub mod mock;

pub use device::TcpDevice;
// 重导出 v0.45.0 应用层类型，便于上游统一从本 crate 引用
pub use eneros_modbus_rtu::{
    AccessMode, ExceptionCode, FunctionCode, ModbusDataType, ModbusError, ModbusRequest,
    ModbusResponse, PointMapping, RegToPoint,
};
pub use error::ModbusTcpError;
pub use master::ModbusTcpMaster;
pub use mbap::MbapHeader;
pub use transport::{TcpStats, TcpTransport};

#[cfg(test)]
mod tests {
    //! 跨模块集成测试 — 端到端验证公共 API（mbap + error + master + transport + device）.

    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_modbus_rtu::{AccessMode, ExceptionCode, ModbusDataType, ModbusError, RegToPoint};

    use crate::mock::MockTcpTransport;
    use crate::{MbapHeader, ModbusTcpError, ModbusTcpMaster, PointMapping, TcpDevice};

    // ===== 测试辅助函数（仅使用公共 API 构帧）=====

    /// 构建读保持寄存器响应帧（MBAP + PDU），通过公共 `build_frame` 编帧.
    fn build_read_response(txn_id: u16, unit_id: u8, regs: &[u16]) -> Vec<u8> {
        let mut pdu = vec![0x03u8]; // func_code
        pdu.push((regs.len() * 2) as u8); // byte_count
        for r in regs {
            pdu.extend_from_slice(&r.to_be_bytes());
        }
        ModbusTcpMaster::build_frame(txn_id, unit_id, &pdu)
    }

    /// 构建写单寄存器响应帧.
    fn build_write_single_response(txn_id: u16, unit_id: u8, addr: u16, value: u16) -> Vec<u8> {
        let mut pdu = vec![0x06u8];
        pdu.extend_from_slice(&addr.to_be_bytes());
        pdu.extend_from_slice(&value.to_be_bytes());
        ModbusTcpMaster::build_frame(txn_id, unit_id, &pdu)
    }

    /// 构建异常响应帧（func_code | 0x80）.
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

    // ===== 1. MBAP 头部编解码往返（mbap + error 模块）=====
    #[test]
    fn test_mbap_roundtrip_integration() {
        let header = MbapHeader::new(0xABCD, 0x10, 8);
        let bytes = header.encode();
        assert_eq!(bytes.len(), 7);

        let decoded = MbapHeader::decode(&bytes).expect("decode should succeed");
        // 全字段匹配
        assert_eq!(decoded.transaction_id, 0xABCD);
        assert_eq!(decoded.protocol_id, 0);
        assert_eq!(decoded.length, 9); // data_len(8) + 1
        assert_eq!(decoded.unit_id, 0x10);
        assert_eq!(decoded, header);

        // 二次编码应完全一致（编解码确定性）
        let bytes2 = decoded.encode();
        assert_eq!(bytes, bytes2);

        // 跨模块：缓冲不足触发 error 模块的 FrameTooShort
        let short = [0u8; 5];
        assert_eq!(
            MbapHeader::decode(&short),
            Err(ModbusTcpError::FrameTooShort)
        );
    }

    // ===== 2. 读保持寄存器端到端（master + mbap + transport + error）=====
    #[test]
    fn test_master_read_holding_registers_integration() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_read_response(0, 1, &[0x1234, 0x5678]));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        let regs = master
            .read_holding_registers(&dev, 0x0100, 2)
            .expect("read should succeed");
        assert_eq!(regs, vec![0x1234, 0x5678]);

        // 先取统计（master 最后一次使用），释放对 mock 的可变借用
        let stats = master.stats().clone();

        // 验证发送帧结构（MBAP 头 + PDU）
        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0],
            vec![
                0x00, 0x00, // transaction_id = 0
                0x00, 0x00, // protocol_id = 0
                0x00, 0x06, // length = 6 (pdu 5 + 1)
                0x01, // unit_id = 1
                0x03, // func_code = read holding registers
                0x01, 0x00, // start_addr = 0x0100
                0x00, 0x02, // quantity = 2
            ]
        );

        // 验证统计
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1);
        assert_eq!(stats.error_count, 0);
        assert_eq!(stats.timeout_count, 0);
        assert_eq!(stats.reconnect_count, 1);
    }

    // ===== 3. 写单寄存器端到端（含发送帧结构）=====
    #[test]
    fn test_master_write_single_register_integration() {
        let mut mock = MockTcpTransport::new();
        mock.push_response(build_write_single_response(0, 1, 0x0102, 0x0304));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        master
            .write_single_register(&dev, 0x0102, 0x0304)
            .expect("write should succeed");

        // 先取统计（master 最后一次使用），释放对 mock 的可变借用
        let stats = master.stats().clone();

        // 验证发送帧结构
        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0],
            vec![
                0x00, 0x00, // transaction_id = 0
                0x00, 0x00, // protocol_id = 0
                0x00, 0x06, // length = 6 (pdu 5 + 1)
                0x01, // unit_id = 1
                0x06, // func_code = write single register
                0x01, 0x02, // reg_addr = 0x0102
                0x03, 0x04, // value = 0x0304
            ]
        );

        // 验证统计
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1);
        assert_eq!(stats.error_count, 0);
        assert_eq!(stats.reconnect_count, 1);
    }

    // ===== 4. 事务 ID 不匹配端到端 =====
    #[test]
    fn test_transaction_mismatch_integration() {
        let mut mock = MockTcpTransport::new();
        // 请求用 txn_id=0，响应用 txn_id=999（不匹配）
        mock.push_response(build_read_response(999, 1, &[0x1234]));

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let dev = make_device(1);
        let result = master.read_holding_registers(&dev, 0, 1);
        assert_eq!(result, Err(ModbusTcpError::TransactionMismatch));

        // 先取统计（master 最后一次使用），释放对 mock 的可变借用
        let stats = master.stats().clone();

        // 即使事务不匹配，请求帧仍应已发送
        assert_eq!(mock.sent_frames().len(), 1);
        // 收到响应但事务 ID 校验失败
        assert_eq!(stats.request_count, 1);
        assert_eq!(stats.response_count, 1);
    }

    // ===== 5. 超时重试端到端（retry_count=2，共 3 次尝试）=====
    #[test]
    fn test_timeout_retry_integration() {
        let mut mock = MockTcpTransport::new();
        mock.set_recv_timeout(true);

        let mut master = ModbusTcpMaster::new(&mut mock, 2);
        let dev = make_device(1);
        let result = master.read_holding_registers(&dev, 0, 1);
        // 重试耗尽 → MaxRetryExceeded
        assert_eq!(
            result,
            Err(ModbusTcpError::Modbus(ModbusError::MaxRetryExceeded))
        );

        let stats = master.stats().clone();
        // retry_count=2 → 共 3 次尝试
        assert_eq!(stats.request_count, 3);
        assert_eq!(stats.response_count, 0);
        assert_eq!(stats.timeout_count, 3);
        assert_eq!(stats.error_count, 3);
        // 连接只在请求开始时建立一次
        assert_eq!(stats.reconnect_count, 1);
    }

    // ===== 6. 异常响应端到端（func_code | 0x80）=====
    #[test]
    fn test_exception_response_integration() {
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
        // ModbusResponse 未实现 PartialEq，用 matches! 验证异常模式
        assert!(matches!(
            result,
            Err(ModbusTcpError::Modbus(ModbusError::Exception(_)))
        ));
        // 进一步验证具体异常码
        assert_eq!(
            result,
            Err(ModbusTcpError::Modbus(ModbusError::Exception(
                ExceptionCode::IllegalDataAddress
            )))
        );
    }

    // ===== 7. 多设备批量轮询端到端（3 设备，全点转换）=====
    #[test]
    fn test_poll_devices_integration() {
        let mut mock = MockTcpTransport::new();
        // 设备 1 (unit_id=1): txn_id=0, raw=100 → 10.0
        mock.push_response(build_read_response(0, 1, &[100u16]));
        // 设备 2 (unit_id=2): txn_id=1, raw=200 → 20.0
        mock.push_response(build_read_response(1, 2, &[200u16]));
        // 设备 3 (unit_id=3): txn_id=2, raw=300 → 30.0
        mock.push_response(build_read_response(2, 3, &[300u16]));

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
                RegToPoint {
                    point_id: 3,
                    point_name: String::from("power"),
                    slave_addr: 3,
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
            TcpDevice::new([192, 168, 1, 3], 502, 3),
        ];

        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        let results = master.poll_devices(&devices, &mapping);

        // 结果数组应有 3 项
        assert_eq!(results.len(), 3);

        // 设备 1：raw=100 × 0.1 = 10.0
        assert_eq!(results[0].0, devices[0]);
        assert_eq!(results[0].1.len(), 1);
        assert_eq!(results[0].1[0].0, 1);
        assert!((results[0].1[0].1.as_ref().unwrap() - 10.0).abs() < 1e-9);

        // 设备 2：raw=200 × 0.1 = 20.0
        assert_eq!(results[1].0, devices[1]);
        assert_eq!(results[1].1.len(), 1);
        assert_eq!(results[1].1[0].0, 2);
        assert!((results[1].1[0].1.as_ref().unwrap() - 20.0).abs() < 1e-9);

        // 设备 3：raw=300 × 0.1 = 30.0
        assert_eq!(results[2].0, devices[2]);
        assert_eq!(results[2].1.len(), 1);
        assert_eq!(results[2].1[0].0, 3);
        assert!((results[2].1[0].1.as_ref().unwrap() - 30.0).abs() < 1e-9);

        // 统计：3 次请求、3 次响应、3 次重连（每设备各 1 次）
        let stats = master.stats().clone();
        assert_eq!(stats.request_count, 3);
        assert_eq!(stats.response_count, 3);
        assert_eq!(stats.reconnect_count, 3);
    }

    // ===== 8. 事务 ID 回绕端到端（65535 → 0，通过真实请求验证）=====
    #[test]
    fn test_txn_id_wraparound_integration() {
        let mut mock = MockTcpTransport::new();
        // 预置响应（txn_id=0xFFFF，将与 65535 次 next_txn_id 后的请求配对）
        mock.push_response(build_read_response(0xFFFF, 1, &[0x1234]));

        // 用块作用域限定 master 生命周期，便于块外访问 mock
        let (first_after, second_after) = {
            let mut master = ModbusTcpMaster::new(&mut mock, 0);
            // 推进 65535 次：next_txn_id 变为 65535（下一次调用返回 65535）
            for _ in 0..0xFFFF {
                let _ = master.next_txn_id();
            }
            // 真实请求应使用 txn_id=65535（send_request_with_retry 内部调用 next_txn_id）
            let dev = make_device(1);
            let regs = master
                .read_holding_registers(&dev, 0, 1)
                .expect("read should succeed");
            assert_eq!(regs, vec![0x1234]);
            // 回绕后连续两次 next_txn_id 应为 0 与 1
            (master.next_txn_id(), master.next_txn_id())
        };

        // master 离开作用域后，mock 可借用：验证发送帧 txn_id = 0xFFFF（大端序）
        let sent = mock.sent_frames();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0][0], 0xFF);
        assert_eq!(sent[0][1], 0xFF);

        // 验证回绕值
        assert_eq!(first_after, 0);
        assert_eq!(second_after, 1);
    }
}
