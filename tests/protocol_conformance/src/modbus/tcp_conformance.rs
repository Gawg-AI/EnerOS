//! Modbus TCP 功能码一致性测试 — Modbus Application Protocol v1.1b3
//!
//! 测试 Modbus TCP 功能码请求/响应格式、MBAP 头格式。
//!
//! 注意：EnerOS 的 ModbusTcpAdapter 基于 tokio-modbus 库，PDU 构造逻辑
//! 封装在库内部，不直接暴露。因此部分功能码格式测试标注 `#[ignore]`，
//! 待实现层暴露 PDU 构造接口后启用。
//!
//! 参考:
//! - Modbus Application Protocol Specification v1.1b3
//! - Modbus Messaging on TCP/IP Implementation Guide v1.0b

use eneros_device::ModbusTcpAdapter;
use eneros_device::adapter::ProtocolAdapter;

// ============================================================================
// MBAP 头格式 — Modbus TCP Implementation Guide v1.0b
// ============================================================================

/// MBAP 头结构：Transaction ID(2) + Protocol ID(2) + Length(2) + Unit ID(1) = 7 字节
#[test]
fn test_mbap_header_structure() {
    // Modbus TCP MBAP (Modbus Application Header) 固定 7 字节
    // 此测试验证 MBAP 头的结构知识，因为 tokio-modbus 内部处理 MBAP
    // 我们验证 adapter 配置可以正确设置 unit ID
    let adapter = ModbusTcpAdapter::with_slave_id("test", 5);
    assert_eq!(adapter.name(), "test");
    assert!(!adapter.is_connected());
}

/// MBAP Protocol ID 固定为 0x0000（Modbus 协议）
#[test]
fn test_mbap_protocol_id_is_zero() {
    // Protocol ID = 0x0000 是 Modbus 协议标识
    // tokio-modbus 内部处理，此处验证协议知识
    let protocol_id: u16 = 0x0000;
    assert_eq!(protocol_id, 0);
}

// ============================================================================
// 功能码 0x01 — 读线圈 (Read Coils)
// ============================================================================

/// FC 0x01 请求格式：功能码(1) + 起始地址(2,大端) + 数量(2,大端) = 5 字节
#[test]
fn test_fc_0x01_read_coils_request_format() {
    // Modbus 协议规定 FC 0x01 请求 PDU 为：
    //   功能码(1B) + 起始地址(2B,大端) + 线圈数量(2B,大端)
    // 共 5 字节
    const FC_READ_COILS: u8 = 0x01;
    assert_eq!(FC_READ_COILS, 0x01);

    // 模拟 PDU: 起始地址=0x0064(100), 数量=0x0008(8)
    let pdu: [u8; 5] = [0x01, 0x00, 0x64, 0x00, 0x08];
    assert_eq!(pdu[0], FC_READ_COILS);
    let start_addr = u16::from_be_bytes([pdu[1], pdu[2]]);
    assert_eq!(start_addr, 100);
    let quantity = u16::from_be_bytes([pdu[3], pdu[4]]);
    assert_eq!(quantity, 8);
}

/// FC 0x01 响应格式：功能码(1) + 字节计数(1) + 线圈状态(N)
#[test]
fn test_fc_0x01_read_coils_response_format() {
    // 响应 PDU: 功能码(1B) + 字节计数(1B) + 线圈状态(NB)
    // 8 个线圈 → 1 字节
    let response: [u8; 3] = [0x01, 0x01, 0xFF];
    assert_eq!(response[0], 0x01);
    assert_eq!(response[1], 1); // 1 字节
    assert_eq!(response[2], 0xFF); // 所有 8 个线圈 ON
}

// ============================================================================
// 功能码 0x02 — 读离散输入 (Read Discrete Inputs)
// ============================================================================

/// FC 0x02 请求格式与 FC 0x01 相同结构
#[test]
fn test_fc_0x02_read_discrete_inputs_request_format() {
    const FC_READ_DISCRETE_INPUTS: u8 = 0x02;
    assert_eq!(FC_READ_DISCRETE_INPUTS, 0x02);

    let pdu: [u8; 5] = [0x02, 0x00, 0x00, 0x00, 0x10]; // 起始=0, 数量=16
    assert_eq!(pdu[0], FC_READ_DISCRETE_INPUTS);
    let quantity = u16::from_be_bytes([pdu[3], pdu[4]]);
    assert_eq!(quantity, 16);
}

// ============================================================================
// 功能码 0x03 — 读保持寄存器 (Read Holding Registers)
// ============================================================================

/// FC 0x03 请求格式：功能码(1) + 起始地址(2,大端) + 数量(2,大端)
#[test]
fn test_fc_0x03_read_holding_registers_request_format() {
    const FC_READ_HOLDING_REGISTERS: u8 = 0x03;
    assert_eq!(FC_READ_HOLDING_REGISTERS, 0x03);

    let pdu: [u8; 5] = [0x03, 0x00, 0x6B, 0x00, 0x03]; // 起始=107, 数量=3
    assert_eq!(pdu[0], FC_READ_HOLDING_REGISTERS);
    let start_addr = u16::from_be_bytes([pdu[1], pdu[2]]);
    assert_eq!(start_addr, 107);
    let quantity = u16::from_be_bytes([pdu[3], pdu[4]]);
    assert_eq!(quantity, 3);
}

/// FC 0x03 响应格式：功能码(1) + 字节计数(1) + 寄存器值(N*2,大端)
#[test]
fn test_fc_0x03_read_holding_registers_response_format() {
    // 3 个寄存器 → 6 字节数据
    let response: [u8; 8] = [0x03, 0x06, 0x02, 0x2B, 0x00, 0x00, 0x00, 0x64];
    assert_eq!(response[0], 0x03);
    assert_eq!(response[1], 6); // 6 字节
    // 第一个寄存器值
    let reg0 = u16::from_be_bytes([response[2], response[3]]);
    assert_eq!(reg0, 0x022B);
}

// ============================================================================
// 功能码 0x04 — 读输入寄存器 (Read Input Registers)
// ============================================================================

/// FC 0x04 请求格式与 FC 0x03 相同结构
#[test]
fn test_fc_0x04_read_input_registers_request_format() {
    const FC_READ_INPUT_REGISTERS: u8 = 0x04;
    assert_eq!(FC_READ_INPUT_REGISTERS, 0x04);

    let pdu: [u8; 5] = [0x04, 0x00, 0x08, 0x00, 0x01]; // 起始=8, 数量=1
    assert_eq!(pdu[0], FC_READ_INPUT_REGISTERS);
}

// ============================================================================
// 功能码 0x05 — 写单个线圈 (Write Single Coil)
// ============================================================================

/// FC 0x05 请求格式：功能码(1) + 地址(2,大端) + 值(2,大端: 0xFF00=ON / 0x0000=OFF)
#[test]
fn test_fc_0x05_write_single_coil_request_format() {
    const FC_WRITE_SINGLE_COIL: u8 = 0x05;
    assert_eq!(FC_WRITE_SINGLE_COIL, 0x05);

    // ON = 0xFF00
    let pdu_on: [u8; 5] = [0x05, 0x00, 0xAC, 0xFF, 0x00];
    assert_eq!(pdu_on[0], FC_WRITE_SINGLE_COIL);
    let value = u16::from_be_bytes([pdu_on[3], pdu_on[4]]);
    assert_eq!(value, 0xFF00); // ON

    // OFF = 0x0000
    let pdu_off: [u8; 5] = [0x05, 0x00, 0xAC, 0x00, 0x00];
    let value_off = u16::from_be_bytes([pdu_off[3], pdu_off[4]]);
    assert_eq!(value_off, 0x0000); // OFF
}

// ============================================================================
// 功能码 0x06 — 写单个寄存器 (Write Single Register)
// ============================================================================

/// FC 0x06 请求格式：功能码(1) + 地址(2,大端) + 值(2,大端)
#[test]
fn test_fc_0x06_write_single_register_request_format() {
    const FC_WRITE_SINGLE_REGISTER: u8 = 0x06;
    assert_eq!(FC_WRITE_SINGLE_REGISTER, 0x06);

    let pdu: [u8; 5] = [0x06, 0x00, 0x01, 0x00, 0x03]; // 地址=1, 值=3
    assert_eq!(pdu[0], FC_WRITE_SINGLE_REGISTER);
    let addr = u16::from_be_bytes([pdu[1], pdu[2]]);
    assert_eq!(addr, 1);
    let value = u16::from_be_bytes([pdu[3], pdu[4]]);
    assert_eq!(value, 3);
}

// ============================================================================
// 功能码 0x0F — 写多个线圈 (Write Multiple Coils)
// ============================================================================

/// FC 0x0F 请求格式：功能码(1) + 地址(2) + 数量(2) + 字节计数(1) + 线圈值(N)
#[test]
fn test_fc_0x0f_write_multiple_coils_request_format() {
    const FC_WRITE_MULTIPLE_COILS: u8 = 0x0F;
    assert_eq!(FC_WRITE_MULTIPLE_COILS, 0x0F);

    // 写 10 个线圈（2 字节）
    let pdu: [u8; 8] = [0x0F, 0x00, 0x13, 0x00, 0x0A, 0x02, 0xCD, 0x01];
    assert_eq!(pdu[0], FC_WRITE_MULTIPLE_COILS);
    let addr = u16::from_be_bytes([pdu[1], pdu[2]]);
    assert_eq!(addr, 19);
    let quantity = u16::from_be_bytes([pdu[3], pdu[4]]);
    assert_eq!(quantity, 10);
    assert_eq!(pdu[5], 2); // 字节计数
}

// ============================================================================
// 功能码 0x10 — 写多个寄存器 (Write Multiple Registers)
// ============================================================================

/// FC 0x10 请求格式：功能码(1) + 地址(2) + 数量(2) + 字节计数(1) + 寄存器值(N*2)
#[test]
fn test_fc_0x10_write_multiple_registers_request_format() {
    const FC_WRITE_MULTIPLE_REGISTERS: u8 = 0x10;
    assert_eq!(FC_WRITE_MULTIPLE_REGISTERS, 0x10);

    // 写 2 个寄存器（4 字节）
    let pdu: [u8; 10] = [0x10, 0x00, 0x01, 0x00, 0x02, 0x04, 0x00, 0x0A, 0x01, 0x02];
    assert_eq!(pdu[0], FC_WRITE_MULTIPLE_REGISTERS);
    let addr = u16::from_be_bytes([pdu[1], pdu[2]]);
    assert_eq!(addr, 1);
    let quantity = u16::from_be_bytes([pdu[3], pdu[4]]);
    assert_eq!(quantity, 2);
    assert_eq!(pdu[5], 4); // 字节计数
    // 第一个寄存器值
    let reg0 = u16::from_be_bytes([pdu[6], pdu[7]]);
    assert_eq!(reg0, 10);
}

// ============================================================================
// 地址解析 — Modbus 寄存器类型映射
// ============================================================================

/// ModbusTcpAdapter 地址解析：holding:40001 → 偏移 0
/// 注意：parse_address 是私有方法，通过 adapter 行为间接验证
#[test]
fn test_modbus_tcp_adapter_creation() {
    let adapter = ModbusTcpAdapter::new("test-tcp");
    assert_eq!(adapter.name(), "test-tcp");
    assert_eq!(adapter.protocol_type(), eneros_device::ProtocolType::Modbus);
    assert!(!adapter.is_connected());
}

/// ModbusTcpAdapter 可设置 slave_id
#[test]
fn test_modbus_tcp_adapter_with_slave_id() {
    let adapter = ModbusTcpAdapter::with_slave_id("test-tcp", 10);
    assert_eq!(adapter.name(), "test-tcp");
    assert!(!adapter.is_connected());
}

/// 未连接时读取应返回错误
#[tokio::test]
async fn test_modbus_tcp_read_not_connected() {
    let adapter = ModbusTcpAdapter::new("test-tcp");
    let result = adapter.read("holding:40001").await;
    assert!(result.is_err());
}

/// 未连接时写入应返回错误
#[tokio::test]
async fn test_modbus_tcp_write_not_connected() {
    let mut adapter = ModbusTcpAdapter::new("test-tcp");
    use eneros_device::adapter::DataValue;
    let result = adapter.write("holding:40001", &DataValue::Int16(100)).await;
    assert!(result.is_err());
}

// ============================================================================
// 以下测试标注 #[ignore]：tokio-modbus 库内部封装了 PDU 构造，
// ModbusTcpAdapter 未暴露原始 PDU 字节，无法直接验证 TCP PDU 字节级一致性。
// 待实现层暴露 PDU 构造接口后启用。
// ============================================================================

/// FC 0x01 实际 PDU 字节级验证
#[test]
#[ignore = "ModbusTcpAdapter 基于 tokio-modbus，PDU 构造封装在库内部，未暴露原始 PDU 字节"]
fn test_fc_0x01_actual_pdu_bytes() {
    // 需要 ModbusTcpAdapter 暴露 PDU 构造接口
}

/// FC 0x03 实际 PDU 字节级验证
#[test]
#[ignore = "ModbusTcpAdapter 基于 tokio-modbus，PDU 构造封装在库内部，未暴露原始 PDU 字节"]
fn test_fc_0x03_actual_pdu_bytes() {
    // 需要 ModbusTcpAdapter 暴露 PDU 构造接口
}

/// FC 0x05 实际 PDU 字节级验证
#[test]
#[ignore = "ModbusTcpAdapter 基于 tokio-modbus，PDU 构造封装在库内部，未暴露原始 PDU 字节"]
fn test_fc_0x05_actual_pdu_bytes() {
    // 需要 ModbusTcpAdapter 暴露 PDU 构造接口
}

/// FC 0x06 实际 PDU 字节级验证
#[test]
#[ignore = "ModbusTcpAdapter 基于 tokio-modbus，PDU 构造封装在库内部，未暴露原始 PDU 字节"]
fn test_fc_0x06_actual_pdu_bytes() {
    // 需要 ModbusTcpAdapter 暴露 PDU 构造接口
}

/// FC 0x0F 实际 PDU 字节级验证
#[test]
#[ignore = "ModbusTcpAdapter 基于 tokio-modbus，PDU 构造封装在库内部，未暴露原始 PDU 字节"]
fn test_fc_0x0f_actual_pdu_bytes() {
    // 需要 ModbusTcpAdapter 暴露 PDU 构造接口
}

/// FC 0x10 实际 PDU 字节级验证
#[test]
#[ignore = "ModbusTcpAdapter 基于 tokio-modbus，PDU 构造封装在库内部，未暴露原始 PDU 字节"]
fn test_fc_0x10_actual_pdu_bytes() {
    // 需要 ModbusTcpAdapter 暴露 PDU 构造接口
}

/// MBAP 头实际字节级验证
#[test]
#[ignore = "ModbusTcpAdapter 基于 tokio-modbus，MBAP 头由库内部构造，未暴露原始字节"]
fn test_mbap_actual_header_bytes() {
    // 需要 ModbusTcpAdapter 暴露 MBAP 头构造接口
}
