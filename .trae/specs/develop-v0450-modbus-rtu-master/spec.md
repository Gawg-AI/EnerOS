# v0.45.0 Modbus RTU 主站 Spec

## Why

v0.44.0 已交付 RS485 串口驱动（`Rs485Driver::send()/recv()`）。v0.45.0 在其之上实现 Modbus RTU 主站协议栈，支持功能码 03/06/10 与点表映射，为 v0.50.0 统一点表提供数据源。这是 P1-F 设备协议栈的第三层——第一个工业协议实现。

蓝图（`蓝图/phase1.md` §7905-8201）给出了帧结构、功能码枚举、主站对象、点表映射的设计骨架，但多处引用了未定义的类型/方法（`ModbusError`/`ModbusStats`/`AccessMode`/`build_frame()`/`parse_response()`/`word_count()`/`convert()`/`group_by_slave()`）。本 spec 在不偏离蓝图意图的前提下补全这些定义（见偏差声明 D1~D10）。

## What Changes

### 新增 crate

- **新增** `crates/protocols/modbus-rtu/`（crate 名 `eneros-modbus-rtu`），实现 Modbus RTU 主站协议栈
  - `lib.rs` — 模块入口 + re-export
  - `frame.rs` — `ModbusFrame` 帧结构 + CRC16 编解码
  - `crc.rs` — CRC-16/MODBUS 算法实现（多项式 0xA001，初始值 0xFFFF）
  - `request.rs` — `ModbusRequest`/`ModbusResponse`/`FunctionCode`/`ExceptionCode`
  - `master.rs` — `ModbusRtuMaster` 主站对象 + `ModbusStats` + `RtuTransport` trait（D1）
  - `point.rs` — `PointMapping`/`RegToPoint`/`ModbusDataType`/`AccessMode`
  - `error.rs` — `ModbusError` 错误枚举
  - `mock.rs` — `MockRtuTransport` 测试桩
- **新增** `docs/protocols/modbus-rtu-master-design.md` 设计文档

### 修改既有代码

- **修改** `Cargo.toml`（workspace 根）：`members` 增加 `"crates/protocols/modbus-rtu"`；`version` 由 `0.44.0` → `0.45.0`

### 偏差声明（相对蓝图 §4）

| 偏差 | 蓝图假设 | 实际情况 | 处理方案 |
|------|---------|---------|---------|
| **D1** | `ModbusRtuMaster` 持有 `rs485: DriverHandle` | `DriverHandle`（v0.43.0）是能力令牌，无 `send()`/`recv()` 方法 | 定义 `RtuTransport` trait（`send(&mut self, &[u8]) -> Result<(), DriverError>` + `recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, DriverError>`），`Rs485Driver` 自动满足此接口；主站持 `&mut dyn RtuTransport`，便于 mock 测试 |
| **D2** | `ModbusError` 枚举被引用但未定义 | 蓝图未给出完整定义 | 定义 `ModbusError` 枚举：`FrameTooShort`/`CrcMismatch`/`AddrMismatch`/`UnexpectedResponse`/`Exception(ExceptionCode)`/`Driver(DriverError)`/`MaxRetryExceeded`/`InvalidSlaveAddr`/`InvalidQuantity`/`InvalidRegisterAddr` |
| **D3** | `ModbusStats` 被引用但未定义 | 蓝图未给出定义 | 定义 `ModbusStats`：`request_count`/`response_count`/`error_count`/`timeout_count`/`crc_error_count` + `Default` |
| **D4** | `AccessMode` 被引用但未定义 | 蓝图未给出定义 | 定义 `AccessMode` 枚举：`ReadOnly`/`WriteOnly`/`ReadWrite` |
| **D5** | `RegToPoint::word_count()` / `convert()` 被引用但未定义 | 蓝图未给出实现 | `word_count()` 根据 `data_type` 返回所需寄存器数（U16/I16=1, U32/F32=2, Bit=1）；`convert(regs: &[u16]) -> Result<f64, ModbusError>` 按 `data_type`+`scale`+`offset` 转换 |
| **D6** | `group_by_slave()` 被引用但未定义 | 蓝图未给出实现 | 在 `point.rs` 内实现 `group_by_slave()` 辅助函数，返回 `Vec<(u8, Vec<&RegToPoint>)>` |
| **D7** | `build_frame()` / `parse_response()` 被引用但未定义 | 蓝图未给出实现 | 在 `master.rs` 内实现 `build_frame(slave_addr, &request) -> Vec<u8>`（编码请求帧+CRC）和 `parse_response(&request, &frame) -> Result<ModbusResponse, ModbusError>`（解码响应帧+校验） |
| **D8** | crate 名 `modbus-rtu-master` 放于不确定位置 | §2.3.1 要求所有 crate 放 `crates/<subsystem>/` | 放入 `crates/protocols/modbus-rtu/`（crate 名 `eneros-modbus-rtu`）；`crates/protocols/` 为设备协议栈子系统 |
| **D9** | `FunctionCode` 枚举列 6 个功能码（01/03/04/05/06/10），但版本目标仅要求 03/06/10 | 部分功能码非本版本必需 | 枚举包含全部 6 个变体（9.7 可扩展），但 `build_frame()`/`parse_response()` 仅实现 03/06/10 的编解码；01/04/05 返回 `ModbusError::UnsupportedFunction` |
| **D10** | 蓝图未明确广播地址 0 的处理 | 风险 §8.4 提及"广播地址 0 的写操作无响应" | `send_request_with_retry()` 中：若 `slave_addr == 0`，发送后不等待响应，直接返回 `Ok(ModbusResponse::Broadcast)` |

## Impact

- **Affected specs**: 无（新 crate，不修改既有 API）
- **Affected code**:
  - `Cargo.toml`（workspace 根）— 版本号 + members
  - `crates/protocols/modbus-rtu/` — 全新 crate
  - `docs/protocols/modbus-rtu-master-design.md` — 全新设计文档
- **后续影响**：v0.46.0 Modbus TCP 复用本版本的应用层逻辑（功能码/请求响应/点表映射）；v0.50.0 统一点表使用本版本的 `PointMapping` 作为数据源

## ADDED Requirements

### Requirement: CRC-16/MODBUS 校验

系统 SHALL 在 `crc.rs` 内实现 CRC-16/MODBUS 算法：多项式 `0xA001`，初始值 `0xFFFF`，低字节在前（LE）输出。

#### Scenario: 已知测试向量

- **WHEN** 输入 `[0x01, 0x03, 0x00, 0x00, 0x00, 0x01]`（读保持寄存器请求帧）
- **THEN** CRC16 结果为 `0x840A`（低字节 `0x0A` 在前，高字节 `0x84` 在后）

- **WHEN** 输入空切片 `&[]`
- **THEN** CRC16 结果为 `0xFFFF`（初始值）

### Requirement: Modbus RTU 帧结构

系统 SHALL 提供 `ModbusFrame` 结构，字段：`slave_addr: u8`、`func_code: u8`、`data: Vec<u8>`、`crc: u16`。

系统 SHALL 提供 `encode(&self) -> Vec<u8>` 方法：将帧编码为 `[SlaveAddr(1)][FuncCode(1)][Data(N)][CRC16(2 LE)]` 字节流。

系统 SHALL 提供 `decode(buf: &[u8]) -> Result<Self, ModbusError>` 方法：从字节流解码，校验最小长度（≥4）与 CRC16。

#### Scenario: 编码帧

- **WHEN** 构造 `ModbusFrame { slave_addr: 1, func_code: 3, data: vec![0x00, 0x00, 0x00, 0x01], crc: 0 }` 并调用 `encode()`
- **THEN** 返回 8 字节字节流，末尾 2 字节为 CRC16（LE）

#### Scenario: 解码帧 — CRC 校验失败

- **WHEN** 调用 `ModbusFrame::decode(&[0x01, 0x03, 0x02, 0x00, 0x00, 0xFF, 0xFF])`（CRC 不匹配）
- **THEN** 返回 `Err(ModbusError::CrcMismatch)`

#### Scenario: 解码帧 — 帧过短

- **WHEN** 调用 `ModbusFrame::decode(&[0x01, 0x03])`（长度 < 4）
- **THEN** 返回 `Err(ModbusError::FrameTooShort)`

### Requirement: Modbus 功能码与请求/响应

系统 SHALL 定义 `FunctionCode` 枚举：`ReadCoils=0x01`、`ReadHoldingRegisters=0x03`、`ReadInputRegisters=0x04`、`WriteSingleCoil=0x05`、`WriteSingleRegister=0x06`、`WriteMultipleRegisters=0x10`。

系统 SHALL 定义 `ModbusRequest` 枚举：
- `ReadHoldingRegisters { slave_addr: u8, start_addr: u16, quantity: u16 }`（功能码 03，quantity ≤ 125）
- `WriteSingleRegister { slave_addr: u8, reg_addr: u16, value: u16 }`（功能码 06）
- `WriteMultipleRegisters { slave_addr: u8, start_addr: u16, values: Vec<u16> }`（功能码 10，values.len() ≤ 123）

系统 SHALL 定义 `ModbusResponse` 枚举：
- `ReadHoldingRegisters(Vec<u16>)`
- `WriteSingleRegister { addr: u16, value: u16 }`
- `WriteMultipleRegisters { start_addr: u16, quantity: u16 }`
- `Error { exception_code: ExceptionCode }`
- `Broadcast`（D10：广播地址响应）

系统 SHALL 定义 `ExceptionCode` 枚举：`IllegalFunction=0x01`、`IllegalDataAddress=0x02`、`IllegalDataValue=0x03`、`SlaveDeviceFailure=0x04`、`Acknowledge=0x05`、`SlaveDeviceBusy=0x06`。

#### Scenario: 读保持寄存器请求

- **WHEN** 构造 `ModbusRequest::ReadHoldingRegisters { slave_addr: 1, start_addr: 0, quantity: 10 }`
- **THEN** `build_frame()` 生成帧数据域为 `[0x00, 0x00, 0x00, 0x0A]`（起始地址 BE + 数量 BE）

#### Scenario: 写单个寄存器请求

- **WHEN** 构造 `ModbusRequest::WriteSingleRegister { slave_addr: 2, reg_addr: 100, value: 0xABCD }`
- **THEN** `build_frame()` 生成帧数据域为 `[0x00, 0x64, 0xAB, 0xCD]`（寄存器地址 BE + 值 BE）

### Requirement: ModbusRtuMaster 主站

系统 SHALL 提供 `ModbusRtuMaster` 结构，字段：`transport: &mut dyn RtuTransport`（D1）、`timeout_ms: u32`、`retry_count: u8`、`stats: ModbusStats`。

系统 SHALL 定义 `RtuTransport` trait（D1）：
- `fn send(&mut self, data: &[u8]) -> Result<(), DriverError>`
- `fn recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, DriverError>`

`Rs485Driver` 自动满足 `RtuTransport`（方法签名一致）。

#### Scenario: 读取保持寄存器

- **WHEN** 调用 `master.read_holding_registers(1, 0, 5)` 且从站正常响应
- **THEN** 返回 `Ok(vec![u16; 5])`；`stats.request_count` 与 `stats.response_count` 各递增 1

#### Scenario: 写多个寄存器

- **WHEN** 调用 `master.write_multiple_registers(1, 100, &[0x0001, 0x0002])` 且从站正常响应
- **THEN** 返回 `Ok(())`；`stats.request_count` 与 `stats.response_count` 各递增 1

#### Scenario: 超时重试

- **WHEN** 调用 `master.read_holding_registers(1, 0, 5)` 且从站 3 次均超时（`retry_count=2`）
- **THEN** 返回 `Err(ModbusError::MaxRetryExceeded)`；`stats.timeout_count` 递增 3

#### Scenario: 广播写（D10）

- **WHEN** 调用 `master.write_multiple_registers(0, 100, &[0x0001])`（广播地址 0）
- **THEN** 发送帧后不等待响应；返回 `Ok(ModbusResponse::Broadcast)`；`stats.request_count` 递增 1，`response_count` 不递增

#### Scenario: 异常码响应

- **WHEN** 从站返回异常码 `0x02`（IllegalDataAddress）
- **THEN** 返回 `Err(ModbusError::Exception(ExceptionCode::IllegalDataAddress))`

### Requirement: 点表映射

系统 SHALL 提供 `PointMapping` 结构：`mappings: Vec<RegToPoint>`。

系统 SHALL 提供 `RegToPoint` 结构：`point_id: u32`、`point_name: String`、`slave_addr: u8`、`reg_addr: u16`、`data_type: ModbusDataType`、`scale: f64`、`offset: f64`、`access: AccessMode`。

系统 SHALL 提供 `ModbusDataType` 枚举：`U16`/`I16`/`U32`/`F32`/`Bit(u8)`。

系统 SHALL 提供 `AccessMode` 枚举（D4）：`ReadOnly`/`WriteOnly`/`ReadWrite`。

`RegToPoint` SHALL 实现：
- `word_count(&self) -> u16`（D5）：U16/I16/Bit=1, U32/F32=2
- `convert(&self, regs: &[u16]) -> Result<f64, ModbusError>`（D5）：按 data_type 解码原始寄存器值，应用 `value * scale + offset`

#### Scenario: U16 转换

- **WHEN** `RegToPoint { data_type: U16, scale: 0.1, offset: 0.0, .. }` 调用 `convert(&[0x0064])`
- **THEN** 返回 `Ok(10.0)`（100 * 0.1 = 10.0）

#### Scenario: F32 转换（大端）

- **WHEN** `RegToPoint { data_type: F32, scale: 1.0, offset: 0.0, .. }` 调用 `convert(&[0x41A0, 0x0000])`
- **THEN** 返回 `Ok(20.0)`（IEEE 754 单精度：0x41A00000 = 20.0）

#### Scenario: 轮询点表

- **WHEN** 调用 `master.poll_points(&mapping)` 且映射表含 2 个点位（同一从站）
- **THEN** 返回 `Vec<(u32, Result<f64, ModbusError>)>`，长度 2

### Requirement: ModbusError 错误枚举

系统 SHALL 定义 `ModbusError` 枚举（D2）：`FrameTooShort`/`CrcMismatch`/`AddrMismatch`/`UnexpectedResponse`/`Exception(ExceptionCode)`/`Driver(DriverError)`/`MaxRetryExceeded`/`InvalidSlaveAddr`/`InvalidQuantity`/`InvalidRegisterAddr`/`UnsupportedFunction`。

### Requirement: ModbusStats 统计

系统 SHALL 定义 `ModbusStats` 结构（D3）：`request_count: u32`/`response_count: u32`/`error_count: u32`/`timeout_count: u32`/`crc_error_count: u32` + `Default`。

### Requirement: MockRtuTransport 测试桩

系统 SHALL 提供 `MockRtuTransport` 结构，实现 `RtuTransport` trait，用于单元测试。支持：
- 预填充接收帧（`push_response(frame: Vec<u8>)`）
- 记录发送数据（`sent_frames() -> &[Vec<u8>]`）
- 可配置 recv 是否超时（`set_recv_timeout(true)`）

## MODIFIED Requirements

无。本版本不修改既有 API。

## REMOVED Requirements

无。
