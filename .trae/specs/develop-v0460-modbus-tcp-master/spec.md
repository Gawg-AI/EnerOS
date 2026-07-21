# v0.46.0 Modbus TCP 主站 Spec

## Why

v0.45.0 实现了 Modbus RTU 主站（基于 RS485 串口），但储能系统中大量设备（BMS/PCS/电表/逆变器）通过 TCP/IP 网络通信，需要 Modbus TCP 协议支持。Modbus TCP 用 MBAP 头替代 RTU 的从站地址 + CRC，基于 TCP 可靠传输，支持多设备并发轮询，是 P1-F 设备协议栈的第四层。

## What Changes

- **新增 crate** `eneros-modbus-tcp`（`crates/protocols/modbus-tcp/`），实现 Modbus TCP 主站
- **新增类型** `MbapHeader`（7 字节 MBAP 头）、`TcpDevice`（设备描述）、`ModbusTcpMaster`（主站）、`TcpTransport` trait（传输抽象）、`ModbusTcpError`、`TcpStats`、`MockTcpTransport`
- **复用 v0.45.0 应用层**：`ModbusRequest`/`ModbusResponse`/`ExceptionCode`/`FunctionCode`/`PointMapping`/`RegToPoint`/`ModbusDataType`/`AccessMode`
- **修改根 `Cargo.toml`**：workspace 版本 `0.45.0` → `0.46.0`，`members` 新增 `"crates/protocols/modbus-tcp"`
- **新增文档** `docs/protocols/modbus-tcp-master-design.md`

## Impact

- **Affected specs**: v0.45.0 Modbus RTU 主站（复用其应用层类型，不修改其代码）
- **Affected code**: 根 `Cargo.toml`（版本 + members）；新增 `crates/protocols/modbus-tcp/` crate；新增 `docs/protocols/modbus-tcp-master-design.md`
- **后续解锁**：v0.48.0 IEC 104、Phase 2 协议抽象层（v0.51.0）

## ADDED Requirements

### Requirement: MBAP 头编解码

系统 SHALL 提供 `MbapHeader` 结构，包含 `transaction_id`/`protocol_id`/`length`/`unit_id` 四个字段，支持 7 字节大端编解码。

#### Scenario: 编码后解码环回
- **WHEN** 创建 `MbapHeader::new(txn_id=0x1234, unit_id=0x05, data_len=5)`
- **THEN** `encode()` 返回 7 字节 `[0x12,0x34, 0x00,0x00, 0x00,0x06, 0x05]`（protocol_id=0，length=data_len+1=6）
- **AND** `decode(&encoded)` 返回原始 header

#### Scenario: 解码帧过短
- **WHEN** `decode(&[0x01,0x02,0x03])`（仅 3 字节）
- **THEN** 返回 `Err(ModbusTcpError::FrameTooShort)`

### Requirement: TcpDevice 设备描述

系统 SHALL 提供 `TcpDevice` 结构，包含 IPv4 地址（4 字节）、端口（默认 502）、单元标识 `unit_id`、超时 `timeout_ms`。

### Requirement: TcpTransport 传输抽象（D1）

系统 SHALL 提供 `TcpTransport` trait，含 `send`/`recv`/`connect` 三个方法，抽象 TCP 传输层，使 `ModbusTcpMaster` 与底层 socket 实现解耦，支持 mock 测试。

#### Scenario: Mock 传输回环
- **WHEN** `MockTcpTransport` 预置响应帧
- **AND** 主站调用 `read_holding_registers()`
- **THEN** 传输层收到正确的 MBAP+PDU 请求帧，主站收到解析后的寄存器值

### Requirement: ModbusTcpMaster 主站

系统 SHALL 提供 `ModbusTcpMaster`，支持：
- `read_holding_registers(device, start_addr, quantity)` — 功能码 0x03
- `write_single_register(device, reg_addr, value)` — 功能码 0x06
- `write_multiple_registers(device, start_addr, values)` — 功能码 0x10
- `poll_devices(devices, mapping)` — 多设备批量轮询点表

#### Scenario: 读保持寄存器成功
- **WHEN** 主站向设备发送读请求（quantity=2）
- **AND** 从站返回正确 MBAP+PDU 响应（含 2 个寄存器值）
- **THEN** 返回 `Ok(Vec<u16>)` 含 2 个寄存器值
- **AND** 事务 ID 匹配验证通过

#### Scenario: 事务 ID 不匹配
- **WHEN** 响应的 `transaction_id` 与请求不符
- **THEN** 返回 `Err(ModbusTcpError::TransactionMismatch)`

#### Scenario: 多设备轮询
- **WHEN** `poll_devices()` 收到 3 个设备 + 点表映射
- **THEN** 依次连接每个设备，读取并转换点表值
- **AND** 返回 `Vec<(TcpDevice, Vec<(u32, Result<f64, ModbusTcpError>)>)>`

### Requirement: ModbusTcpError 错误类型（D2）

系统 SHALL 提供 `ModbusTcpError` 枚举，包含应用层错误（`Modbus(ModbusError)`）和 TCP 特有错误（`TransactionMismatch`/`ConnectionFailed`/`NotConnected`/`Timeout`/`Closed`/`FrameTooShort`）。

## MODIFIED Requirements

### Requirement: workspace 版本号

根 `Cargo.toml` 的 `workspace.package.version` 从 `0.45.0` 更新为 `0.46.0`，`members` 数组新增 `"crates/protocols/modbus-tcp"`。

## 偏差声明（D1~D8）

| 偏差 | 说明 | 理由 |
|------|------|------|
| **D1** | 定义 `TcpTransport` trait（`send`/`recv`/`connect`），抽象 TCP 传输层 | 解耦主站与 smoltcp socket 实现，支持 mock 测试（类比 v0.45.0 的 `RtuTransport`） |
| **D2** | 定义 `ModbusTcpError` 枚举，包装 `ModbusError` + TCP 特有变体 | 不修改 v0.45.0 的 `ModbusError`（遵循 Surgical Changes 原则）；TCP 需 `TransactionMismatch` 等新变体 |
| **D3** | 复用 v0.45.0 的 `ModbusRequest`/`ModbusResponse`，`slave_addr` 字段语义复用为 `unit_id` | RTU 从站地址与 TCP 单元标识语义等价（均为设备标识），PDU 格式完全相同，避免重复定义类型 |
| **D4** | `TcpDevice` 使用 `[u8; 4]` 表示 IPv4 地址，不依赖 smoltcp 的 `Ipv4Addr` | 避免 modbus-tcp crate 直接依赖 smoltcp，保持协议层与网络栈解耦 |
| **D5** | 定义 `TcpStats`（request/response/error/timeout/reconnect 计数） | 蓝图引用但未定义，TCP 需独立统计（含重连次数） |
| **D6** | 连接管理委托给 `TcpTransport::connect()`，主站不持有连接池 | 遵循 Simplicity First：主站只负责协议逻辑，连接池/重连策略由传输层实现负责 |
| **D7** | `poll_devices()` 串行遍历设备（非真并发） | MVP 简化：Modbus 是请求-响应模式，单连接不支持并发；真并发需多连接+多线程，后置 |
| **D8** | crate 放入 `crates/protocols/modbus-tcp/` | 遵循 §2.3.1 crate 分组规则，与 modbus-rtu 同属 protocols 子系统 |
