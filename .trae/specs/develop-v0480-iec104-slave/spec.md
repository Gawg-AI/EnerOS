# v0.48.0 IEC 104 从站 Spec

## Why

储能系统中的 PCS/BMS 等设备使用 IEC 60870-5-104 协议与调度主站通信（TCP/IP 端口 2404，电力行业标准）。需要 IEC 104 从站协议栈支持 APDU 编解码、ASDU 处理、总召唤/遥控/时钟同步响应。这是 P1-F 设备协议栈第六层，电力行业专用协议，为 v0.49.0 IEC 104 主站提供共享的 APDU/ASDU 类型基础。

## What Changes

- **新增 crate** `eneros-iec104-slave`（`crates/protocols/iec104-slave/`），实现 IEC 104 从站协议栈
- **新增类型**：
  - 帧层：`Apdu`、`ControlField`（I/S/U 三种格式）、`UFormatFunction`
  - 应用层：`Asdu`、`TypeId`（10 变体）、`Cot`（9 变体）、`InformationObject`、`IoValue`、`QualityDescriptor`、`SinglePointValue`、`DoublePointValue`、`Sco`、`Dco`、`TimeTag`（CP56Time2a）
  - 从站：`Iec104Slave`、`SlaveState`、`SlaveConnection`、`Iec104Config`、`SlaveStats`
  - 抽象：`SlaveTransport` trait（D1）、`PointDatabase` trait + `InMemoryPointDatabase`（D2）
  - 测试桩：`MockSlaveTransport`
- **修改根 `Cargo.toml`**：workspace 版本 `0.47.0` → `0.48.0`，`members` 新增 `"crates/protocols/iec104-slave"`
- **新增文档** `docs/protocols/iec104-slave-design.md`
- **零外部依赖**：不依赖 `eneros-net`/smoltcp（D8），不依赖 `eneros-driver-framework`（D9）

## Impact

- **Affected specs**: v0.29.0 Socket 抽象层（概念依赖，不直接依赖 crate）；v0.43.0 驱动框架（可选依赖，不实现 DeviceDriver，D9）
- **Affected code**: 根 `Cargo.toml`（版本 + members）；新增 `crates/protocols/iec104-slave/` crate；新增 `docs/protocols/iec104-slave-design.md`
- **后续解锁**：v0.49.0 IEC 104 主站（复用 APDU/ASDU 类型）

## ADDED Requirements

### Requirement: APDU 帧结构

系统 SHALL 提供 `Apdu` 结构，包含 `control_field: ControlField` 和 `asdu: Option<Asdu>` 字段，支持 I/S/U 三种格式编解码。

#### Scenario: 编码 U 格式 STARTDT_ACT
- **WHEN** 创建 `Apdu::u_format(UFormatFunction::StartDtAct)` 并调用 `encode()`
- **THEN** 返回字节流 `[0x68, 0x04, 0x07, 0x00, 0x00, 0x00]`（起始字节 0x68 + 长度 4 + 控制域 0x07）

#### Scenario: 编码 I 格式帧
- **WHEN** 创建 `Apdu::i_format(send_seq=0, recv_seq=0, asdu=Some(...))` 并调用 `encode()`
- **THEN** 返回字节流以 `0x68` 开头，控制域第一个字节的 bit0=0（I 格式标志）

#### Scenario: 解码 S 格式帧
- **WHEN** 解码字节 `[0x68, 0x04, 0x01, 0x00, 0x02, 0x00]`
- **THEN** 返回 `ControlField::Numbered { recv_seq: 1 }`（recv_seq = 0x02 >> 1 = 1）

### Requirement: ASDU 应用层

系统 SHALL 提供 `Asdu` 结构（`type_id`/`cause_of_tx`/`common_addr`/`ioas` 字段），支持 10 种 `TypeId` 变体的编解码。

#### Scenario: 编码遥测浮点 ASDU
- **WHEN** 创建 `Asdu { type_id: TypeId::MeasuredValueFloat, cot: Cot::Periodic, common_addr: 1, ioas: [InformationObject { ioa: 1, value: IoValue::Float(3.14), quality: QualityDescriptor::good(), time_tag: None }] }`
- **THEN** `encode()` 返回的字节流以类型 ID 13 开头，浮点值以小端序 IEEE 754 编码（D6）

#### Scenario: 解码单点遥信 ASDU
- **WHEN** 解码 ASDU 字节流中 type_id=1 的数据
- **THEN** 返回 `Asdu` 中 `ioas[0].value` 为 `IoValue::SinglePoint(SinglePointValue::On)`

### Requirement: 总召唤流程

系统 SHALL 在收到总召唤命令时，按"激活确认 → 数据 → 激活终止"三步流程响应。

#### Scenario: 总召唤完整流程
- **WHEN** 主站发送 `TypeId::InterrogationCommand`（COT=Activation）
- **THEN** 从站依次发送：1) 激活确认（COT=ActivationConfirm）；2) 全部点数据（COT=InterrogatedByStation）；3) 激活终止（COT=ActivationConfirm）

### Requirement: 遥控命令响应

系统 SHALL 处理 `SingleCommand`（TypeId 45）和 `DoubleCommand`（TypeId 46），执行后回复确认。

#### Scenario: 单点遥控执行
- **WHEN** 主站发送 `SingleCommand`（COT=Activation）
- **THEN** 从站调用 `PointDatabase::execute_single_command(ioa, &sco)` 执行，回复 `SingleCommand`（COT=ActivationConfirm）

### Requirement: 时钟同步

系统 SHALL 处理 `ClockSyncCommand`（TypeId 103），更新系统时间并回复确认。

#### Scenario: 时钟同步
- **WHEN** 主站发送 `ClockSyncCommand`（COT=Activation，含 CP56Time2a 时标）
- **THEN** 从站更新内部时间，回复 `ClockSyncCommand`（COT=ActivationConfirm）

### Requirement: 传输层抽象（D1）

系统 SHALL 提供 `SlaveTransport` trait，抽象 TCP 传输层访问（accept/send/recv/close/now_ms），使从站独立于 smoltcp。

### Requirement: 点数据库（D2）

系统 SHALL 提供 `PointDatabase` trait + `InMemoryPointDatabase` 实现，存储遥测/遥信/遥控点数据。

## MODIFIED Requirements

### Requirement: workspace 版本号

根 `Cargo.toml` 的 `workspace.package.version` 从 `0.47.0` 更新为 `0.48.0`，`members` 数组新增 `"crates/protocols/iec104-slave"`。

## 偏差声明（D1~D10）

| 偏差 | 说明 | 理由 |
|------|------|------|
| **D1** | 定义本地 `SlaveTransport` trait（accept/send/recv/close/now_ms） | 蓝图直接使用 `SocketHandle::connect()`；类比 v0.46.0 Modbus TCP 的 `TcpTransport`，解耦 smoltcp 便于 mock 测试 |
| **D2** | `PointDatabase` trait + `InMemoryPointDatabase`（蓝图引用 `PointDatabase` 但未定义） | 定义为 trait 使应用可插入自定义存储；提供内存实现供测试参考 |
| **D3** | 时间通过 `now_ms: u64` 参数注入（蓝图使用 `MonotonicTime`） | EnerOS 无 `MonotonicTime` 类型；与 RS485/CAN 驱动 D3/D5 模式一致 |
| **D4** | 单活动连接 MVP（蓝图 `Vec<Iec104Connection>`） | 储能场景下从站通常服务单一本地主站；多连接可后置扩展；遵循 Simplicity First |
| **D5** | 超时使用 `u32` 毫秒（蓝图使用 `Duration`） | EnerOS 无 `core::time::Duration` 在 no_std 下需 alloc；直接用 u32 ms 更简洁 |
| **D6** | 浮点值显式小端序编解码 | IEC 104 浮点为 LE IEEE 754（蓝图 §8.5 坑点）；非网络大端序，需显式处理 |
| **D7** | crate 放入 `crates/protocols/iec104-slave/`（遵循 §2.3.1） | 同属 protocols 子系统，与 modbus-rtu/modbus-tcp 同级 |
| **D8** | 不依赖 `eneros-net`/smoltcp | 传输层由 `SlaveTransport` trait 抽象；slave 监听端口，连接以 `ConnId` 标识 |
| **D9** | 不实现 `DeviceDriver` trait | IEC 104 从站是协议栈而非设备驱动；蓝图标注 v0.43.0 依赖为"可选"；与 v0.46.0 Modbus TCP 一致 |
| **D10** | CP56Time2a 7 字节时标本地实现 | 蓝图使用 `TimeTag` 但未定义；实现 encode/decode 支持 `Option<TimeTag>` |
