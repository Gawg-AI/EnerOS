# v0.47.0 CAN 驱动 Spec

## Why

储能系统中的 BMS/PCS/逆变器等设备大量使用 CAN 总线通信（高可靠性、实时性、抗干扰），需要 CAN 驱动支持帧收发、ID 过滤和基础 CAN 2.0A/B 协议。这是 P1-F 设备协议栈第五层，为后续 CAN 上层协议（如 CANopen/UDS/储能专用协议）提供传输基础。

## What Changes

- **新增 crate** `eneros-can`（`crates/drivers/can/`），实现 CAN 驱动
- **新增类型** `CanDriver`（实现 `DeviceDriver` trait）、`CanConfig`、`CanFrame`、`CanId`、`FrameType`、`CanFilter`、`CanMode`、`CanStats`、`CanController` trait（HAL 抽象）、`MockCanController`（测试桩）
- **复用** v0.43.0 驱动框架的 `DeviceDriver`/`DriverError`/`DriverId`/`DriverState`/`DriverType`/`DriverHealth`
- **修改根 `Cargo.toml`**：workspace 版本 `0.46.0` → `0.47.0`，`members` 新增 `"crates/drivers/can"`
- **新增文档** `docs/drivers/can-driver-design.md`

## Impact

- **Affected specs**: v0.43.0 驱动框架（仅复用，不修改）；v0.7.0 HAL（仅概念依赖，不直接依赖 HAL crate）
- **Affected code**: 根 `Cargo.toml`（版本 + members）；新增 `crates/drivers/can/` crate；新增 `docs/drivers/can-driver-design.md`
- **后续解锁**：CAN 上层协议（Phase 2 CANopen/储能专用 CAN 协议）

## ADDED Requirements

### Requirement: CanFrame 帧结构

系统 SHALL 提供 `CanFrame` 结构，包含 `id: CanId`、`frame_type: FrameType`、`data: Vec<u8>`（0~8 字节）、`dlc: u8` 字段。

#### Scenario: 创建标准数据帧
- **WHEN** 调用 `CanFrame::new_standard(0x123, &[0x01, 0x02])`
- **THEN** 返回帧 `id == CanId::Standard(0x123)`、`frame_type == FrameType::Data`、`data == [0x01, 0x02]`、`dlc == 2`

#### Scenario: 创建扩展数据帧
- **WHEN** 调用 `CanFrame::new_extended(0x1FFFFFFF, &[0xAA])`
- **THEN** 返回帧 `id == CanId::Extended(0x1FFFFFFF)`、`frame_type == FrameType::Data`、`data == [0xAA]`、`dlc == 1`

#### Scenario: 标准 ID 掩码
- **WHEN** 调用 `CanFrame::new_standard(0xFFFF, &[])`（超过 11 位范围）
- **THEN** `id` 被 `& 0x7FF` 截断为 `CanId::Standard(0x7FF)`

### Requirement: CanFilter 过滤器

系统 SHALL 提供 `CanFilter` 结构（`filter_id`/`filter_mask`/`extended` 字段），支持 `accept_all()`/`match_exact()`/`match_prefix()` 构造方法和 `matches()` 匹配方法。

#### Scenario: 接收所有帧
- **WHEN** `CanFilter::accept_all()`
- **THEN** `filter_mask == 0`，任何帧都匹配

#### Scenario: 精确匹配标准帧
- **WHEN** `CanFilter::match_exact(0x123, false)` 匹配 `CanId::Standard(0x123)` 的帧
- **THEN** `matches()` 返回 `true`

#### Scenario: 标准帧与扩展帧不互通
- **WHEN** 标准 ID 过滤器匹配扩展 ID 帧
- **THEN** `matches()` 返回 `false`

### Requirement: CanConfig 配置

系统 SHALL 提供 `CanConfig` 结构，含 `baud_rate: u32`、`mode: CanMode`、`filters: Vec<CanFilter>`、`auto_retransmit: bool` 字段。

### Requirement: CanController HAL 抽象（D1）

系统 SHALL 提供 `CanController` trait，抽象 CAN 控制器硬件访问，含 `reset`/`set_baud_rate`/`set_mode`/`set_filter`/`enable_rx_irq`/`disable_rx_irq`/`read_rx_buffer`/`write_tx_buffer`/`now_ns` 方法。

### Requirement: CanDriver 驱动

系统 SHALL 提供 `CanDriver`，实现 `DeviceDriver` trait，支持：
- `send(&mut self, frame: &CanFrame)` — 发送 CAN 帧（数据长度 ≤8）
- `recv(&mut self, timeout_ms: u32)` — 接收 CAN 帧（从 rx_queue 弹出，超时返回 `DriverError::Timeout`）
- `handle_irq(&mut self, irq_id: u32)` — 中断处理（读取 RX 缓冲，应用软件过滤器）
- `health_check(&self)` — 基于 `rx_error_count` 返回健康状态

#### Scenario: 发送成功
- **WHEN** 调用 `driver.send(&frame)` 且硬件 TX 缓冲空闲
- **THEN** 返回 `Ok(())`，`stats.tx_count` 递增

#### Scenario: 发送数据过长
- **WHEN** `frame.data.len() > 8`
- **THEN** 返回 `Err(DriverError::InvalidState)`

#### Scenario: 接收超时
- **WHEN** `rx_queue` 为空且 `timeout_ms` 超时
- **THEN** 返回 `Err(DriverError::Timeout)`，`stats.rx_error_count` 递增

### Requirement: CanStats 统计

系统 SHALL 提供 `CanStats` 结构，含 `tx_count`/`rx_count`/`rx_error_count`/`tx_error_count`/`bus_off_count` 字段。

## MODIFIED Requirements

### Requirement: workspace 版本号

根 `Cargo.toml` 的 `workspace.package.version` 从 `0.46.0` 更新为 `0.47.0`，`members` 数组新增 `"crates/drivers/can"`。

## 偏差声明（D1~D9）

| 偏差 | 说明 | 理由 |
|------|------|------|
| **D1** | 定义本地 `CanController` trait（HAL 仅有 `HalSpi`/`HalGpio`，无 CAN 控制器专有方法） | 类比 v0.44.0 RS485 的 `UartHw`，CAN 控制器寄存器访问需本地抽象 |
| **D2** | `CanControllerType` 枚举仅作配置标识（MCP2515/Internal/SJA1000），不实现具体寄存器级操作 | MVP 不绑定特定硬件；具体寄存器操作由 `CanController` trait 的实现负责 |
| **D3** | `CanFrame` 不含 `timestamp: MonotonicTime` 字段（蓝图引用但 EnerOS 无 `MonotonicTime` 类型） | 时间戳由应用层注入，遵循 RS485 驱动 D3 模式 |
| **D4** | `RingBuffer<T, N>` 本地实现（不依赖 v0.44.0 RS485 的 ring.rs） | 遵循 Surgical Changes 原则：不跨 crate 共享内部实现，避免 crate 间耦合；后续可考虑提取到 driver-framework |
| **D5** | `recv()` 接受 `now_ns: u64` 参数注入时间戳（不使用 `MonotonicTime::now()`） | 与 RS485 驱动一致，便于测试和无 HAL 环境运行 |
| **D6** | `CanController::read_rx_buffer()` 返回 `Option<CanFrame>`（驱动级抽象） | 蓝图的 `CanFrame` 含 `MonotonicTime`，简化为不含时间戳 |
| **D7** | `CanFilter::matches()` 实现 ID+掩码匹配（蓝图定义）+ 标准帧/扩展帧互斥检查 | 蓝图已定义，照实现 |
| **D8** | crate 放入 `crates/drivers/can/`（遵循 §2.3.1 crate 分组规则） | 同属 drivers 子系统，与 rs485/框架同级 |
| **D9** | 不依赖 `eneros-hal` crate（HAL 抽象由本地 `CanController` trait 提供） | 解耦驱动与 HAL 实现，便于 mock 测试 |
