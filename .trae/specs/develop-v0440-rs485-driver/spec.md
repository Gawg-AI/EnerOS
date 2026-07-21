# v0.44.0 RS485 串口驱动 Spec

## Why

RS485 是工业现场最常见的串行通信总线（半双工差分传输，支持多点挂载）。v0.43.0 已交付设备驱动框架（`DeviceDriver` trait + `DriverRegistry`），v0.44.0 需在其之上实现 RS485 串口驱动，提供物理层/链路层数据帧收发能力，为 v0.45.0 Modbus RTU 主站提供底层传输。

蓝图（`蓝图/phase1.md` §7662-7902）假定 HAL 提供 `HalUart` trait（含 `configure()`/`enable_rx_irq()`/`read_byte()`/`wait_tx_done()` 等 UART 专有方法），但实际 HAL 仅提供 `HalSerial`（`write`/`read`/`flush` 三方法）。因此本 spec 在不偏离蓝图意图的前提下，对 UART 硬件抽象层做局部偏差处理（见下文偏差声明 D1）。

## What Changes

### 新增 crate

- **新增** `crates/drivers/rs485/`（crate 名 `eneros-rs485`），实现 RS485 串口驱动
  - `lib.rs` — 模块入口 + re-export
  - `config.rs` — `Rs485Config` 配置结构 + `UartPort`/`StopBits`/`Parity`/`GpioPin` 类型（D2）
  - `uart_hw.rs` — `UartHw` trait 抽象 UART 硬件操作（D1）
  - `driver.rs` — `Rs485Driver` 实现 `DeviceDriver` trait + `Rs485Stats` 统计
  - `ring.rs` — no_std 环形缓冲 `RingBuffer`（D4）
  - `mock.rs` — `MockUartHw` 测试桩，实现 `UartHw` trait
- **新增** `docs/drivers/rs485-driver-design.md` 设计文档

### 修改既有代码

- **修改** `Cargo.toml`（workspace 根）：`members` 增加 `"crates/drivers/rs485"`；`version` 由 `0.43.0` → `0.44.0`
- **修改** `crates/drivers/framework/src/lib.rs`：`DriverError` 枚举增加 `Timeout` 变体 + Display 实现（D5）

### 偏差声明（相对蓝图 §4）

| 偏差 | 蓝图假设 | 实际情况 | 处理方案 |
|------|---------|---------|---------|
| **D1** | `HalUart` trait 提供 UART 配置/IRQ/读字节等 | HAL 仅有 `HalSerial`（write/read/flush），无 UART 专有方法 | 在新 crate 内定义 `UartHw` trait 抽象 UART 硬件操作（configure/enable_rx_irq/read_byte/write_bytes/wait_tx_done/rx_irq_id）；`HalGpio` 直接复用现有 trait 控制 DE/RE 方向 |
| **D2** | `UartPort`/`StopBits`/`Parity`/`GpioPin` 类型已存在 | HAL `types.rs` 中无这些类型 | 在 `config.rs` 内定义上述类型（不污染 HAL crate） |
| **D3** | 使用 `MonotonicTime::now()` 获取当前时间 | no_std 无系统时钟，`MonotonicTime` 不存在 | 沿用 v0.43.0 D3 模式：`send()`/`recv()` 等时间相关方法接受 `now_ns: u64` 参数注入当前时间（HAL `HalClock::now_ns()` 由调用方传入） |
| **D4** | `RingBuffer<u8, 512>` 来自某外部库 | no_std 无该类型 | 在 `ring.rs` 内实现 const generics 环形缓冲 `RingBuffer<T, const N: usize>`（无外部依赖） |
| **D5** | `DriverError::Timeout` 变体已存在 | v0.43.0 `DriverError` 无 `Timeout` 变体 | 向 `crates/drivers/framework/src/lib.rs` 的 `DriverError` 增加 `Timeout` 变体 + Display 实现 |
| **D6** | `AtomicBool`（未指定路径） | no_std 需明确 `core::sync::atomic::AtomicBool` | 使用 `core::sync::atomic::AtomicBool` + `core::sync::atomic::Ordering` |
| **D7** | `de_re: Option<HalGpio>`（GPIO 作为对象） | `HalGpio` 是 trait，方法接受 `pin: u32` 参数 | `Rs485Driver` 持 `de_re_pin: Option<u32>` + `&'static dyn HalGpio`，通过 `gpio.set(pin, val)` 控制 DE/RE |
| **D8** | `Self::delay_us(...)` 阻塞延时 | no_std 无标准延时；蓝图未指定延时来源 | 延时由 `UartHw` trait 的 `pre_send_delay_us`/`post_send_delay_us` 配置驱动，实际延时由 `UartHw` 实现负责（硬件定时器或忙等）；`Rs485Driver` 不直接调用 `delay_us`，而是在 `send()` 流程中通过 `UartHw::write_bytes()` 的阻塞特性隐式处理时序，DE 切换前后由 `UartHw` 实现内部保证 |
| **D9** | `tx_buffer: Vec<u8>` 字段 | 发送路径为同步阻塞，无需发送缓冲 | 移除 `tx_buffer` 字段；`send()` 直接将入参 `&[u8]` 写入 `UartHw` |
| **D10** | `recv()` 返回 `Vec<u8>` | 接受 `alloc::vec::Vec`（no_std + alloc 允许） | 使用 `alloc::vec::Vec<u8>` |

## Impact

- **Affected specs**: `develop-v0430-driver-framework`（向其 `DriverError` 增加 `Timeout` 变体，向后兼容，不破坏既有 API）
- **Affected code**:
  - `Cargo.toml`（workspace 根）— 版本号 + members
  - `crates/drivers/framework/src/lib.rs` — `DriverError` 增加 `Timeout`
  - `crates/drivers/rs485/` — 全新 crate
  - `docs/drivers/rs485-driver-design.md` — 全新设计文档
- **后续影响**：v0.45.0 Modbus RTU 主站依赖本版本的 `Rs485Driver::send()`/`recv()` 作为底层传输

## ADDED Requirements

### Requirement: RS485 配置结构

系统 SHALL 提供 `Rs485Config` 结构，包含以下字段：`port: UartPort`、`baud_rate: u32`、`data_bits: u8`、`stop_bits: StopBits`、`parity: Parity`、`local_addr: u8`、`response_timeout_ms: u32`、`frame_gap_ms: u32`、`de_re_pin: Option<u32>`、`pre_send_delay_us: u32`、`post_send_delay_us: u32`。

系统 SHALL 为 `Rs485Config` 提供 `Default` 实现，默认值：Uart0/9600bps/8 数据位/1 停止位/无校验/地址 1/超时 1000ms/帧间隔 4ms/无 DE_RE GPIO/前后延时 100μs。

#### Scenario: 默认配置

- **WHEN** 调用 `Rs485Config::default()`
- **THEN** 返回的配置波特率为 9600，数据位为 8，停止位为 `StopBits::One`，校验为 `Parity::None`，帧间隔为 4ms

#### Scenario: 自定义配置

- **WHEN** 构造 `Rs485Config { baud_rate: 115200, ..Default::default() }`
- **THEN** 波特率为 115200，其余字段为默认值

### Requirement: UartHw 硬件抽象 trait

系统 SHALL 在新 crate 内定义 `UartHw` trait，抽象 UART 硬件操作（D1 偏差）。trait 方法：

- `fn configure(&mut self, baud_rate: u32, data_bits: u8, stop_bits: StopBits, parity: Parity) -> Result<(), DriverError>`
- `fn enable_rx_irq(&mut self) -> Result<(), DriverError>`
- `fn disable_rx_irq(&mut self) -> Result<(), DriverError>`
- `fn read_byte(&mut self) -> Option<u8>`
- `fn write_bytes(&mut self, data: &[u8]) -> Result<usize, DriverError>`
- `fn wait_tx_done(&mut self, timeout_ms: u32) -> Result<(), DriverError>`
- `fn rx_irq_id(&self) -> u32`

#### Scenario: 配置 UART 硬件

- **WHEN** 调用 `uart_hw.configure(9600, 8, StopBits::One, Parity::None)`
- **THEN** 返回 `Ok(())`，UART 硬件按指定参数配置

#### Scenario: 读取单字节

- **WHEN** UART 接收寄存器有数据时调用 `uart_hw.read_byte()`
- **THEN** 返回 `Some(byte)`

- **WHEN** UART 接收寄存器无数据时调用 `uart_hw.read_byte()`
- **THEN** 返回 `None`

### Requirement: RS485 驱动实现

系统 SHALL 提供 `Rs485Driver` 结构，实现 `DeviceDriver` trait（来自 v0.43.0 框架）。字段：`id: DriverId`、`config: Rs485Config`、`state: DriverState`、`uart: &dyn UartHw`（D1）、`gpio: &'static dyn HalGpio`（D7）、`de_re_pin: Option<u32>`、`rx_buffer: RingBuffer<u8, 512>`（D4）、`stats: Rs485Stats`、`irq_rx: AtomicBool`（D6）。

#### Scenario: 初始化驱动

- **WHEN** 调用 `driver.init()` 且 UART 硬件可用
- **THEN** UART 按 `config` 参数配置；若 `de_re_pin` 存在，GPIO 配置为输出并置低（接收模式）；驱动状态变为 `Ready`

#### Scenario: 启动驱动

- **WHEN** 调用 `driver.start()` 且当前状态为 `Ready`
- **THEN** 调用 `uart.enable_rx_irq()`；驱动状态变为 `Running`

#### Scenario: 停止驱动

- **WHEN** 调用 `driver.stop()` 且当前状态为 `Running`
- **THEN** 调用 `uart.disable_rx_irq()`；驱动状态变为 `Stopped`

#### Scenario: 中断处理

- **WHEN** 调用 `driver.handle_irq(irq_id)` 且 `irq_id == uart.rx_irq_id()`
- **THEN** 从 UART 读取所有可用字节推入 `rx_buffer`；`irq_rx` 标志置为 `true`

#### Scenario: 健康检查

- **WHEN** `stats.rx_error_count > 100`
- **THEN** `health_check()` 返回 `Unhealthy`

- **WHEN** `stats.rx_error_count > 10` 且 `<= 100`
- **THEN** `health_check()` 返回 `Degraded`

- **WHEN** `stats.rx_error_count <= 10`
- **THEN** `health_check()` 返回 `Healthy`

### Requirement: RS485 数据帧发送

系统 SHALL 提供 `Rs485Driver::send(&mut self, data: &[u8]) -> Result<(), DriverError>` 方法。

发送流程：
1. 若 `de_re_pin` 存在：`gpio.set(pin, true)` 拉高 DE（发送模式）
2. 调用 `uart.write_bytes(data)` 写入数据
3. 调用 `uart.wait_tx_done(config.response_timeout_ms)` 等待发送完成，超时返回 `DriverError::Timeout`
4. 若 `de_re_pin` 存在：`gpio.set(pin, false)` 拉低 DE（接收模式）
5. `stats.tx_count += 1`

#### Scenario: 成功发送

- **WHEN** 调用 `driver.send(&[0x01, 0x02, 0x03])` 且 UART 硬件正常
- **THEN** DE 拉高 → 数据写入 UART → 等待 TX 完成 → DE 拉低；`stats.tx_count` 递增；返回 `Ok(())`

#### Scenario: 发送超时

- **WHEN** 调用 `driver.send(data)` 且 `uart.wait_tx_done()` 返回 `Err(Timeout)`
- **THEN** DE 拉低（恢复接收模式）；返回 `Err(DriverError::Timeout)`

### Requirement: RS485 数据帧接收

系统 SHALL 提供 `Rs485Driver::recv(&mut self, timeout_ms: u32, now_ns: u64) -> Result<Vec<u8>, DriverError>` 方法（D3 偏差：注入 `now_ns` 时间戳）。

接收流程：
1. 计算 deadline = `now_ns + timeout_ms * 1_000_000`
2. 循环从 `rx_buffer` 弹出字节；记录 `last_byte_ns`
3. 当 `rx_buffer` 为空且距 `last_byte_ns` 超过 `frame_gap_ms` 时，帧结束
4. 超过 deadline 时返回 `DriverError::Timeout`
5. 帧为空（超时且无数据）返回 `DriverError::Timeout`
6. `stats.rx_count += 1`；返回帧数据

#### Scenario: 成功接收一帧

- **WHEN** `rx_buffer` 中有 `[0x01, 0x02, 0x03]`，调用 `recv(100, now)` 后帧间隔超时
- **THEN** 返回 `Ok(vec![0x01, 0x02, 0x03])`；`stats.rx_count` 递增

#### Scenario: 接收超时

- **WHEN** `rx_buffer` 为空，调用 `recv(100, now)` 且超过 100ms
- **THEN** 返回 `Err(DriverError::Timeout)`

### Requirement: RS485 统计信息

系统 SHALL 提供 `Rs485Stats` 结构，字段：`tx_count: u32`、`rx_count: u32`、`rx_error_count: u32`、`last_rx_error: Option<DriverError>`。提供 `Default` 实现。

### Requirement: no_std 环形缓冲

系统 SHALL 在 `ring.rs` 内实现 `RingBuffer<T, const N: usize>` 泛型环形缓冲（D4 偏差）。方法：`push(&mut self, item: T) -> Result<(), T>`（满时返回 Err）、`pop(&mut self) -> Option<T>`、`len(&self) -> usize`、`is_empty(&self) -> bool`、`is_full(&self) -> bool`、`clear(&mut self)`。

#### Scenario: 推入并弹出

- **WHEN** 向空缓冲 `push(0x42)` 后 `pop()`
- **THEN** `pop()` 返回 `Some(0x42)`

#### Scenario: 缓冲满

- **WHEN** 向容量 4 的缓冲推入 5 个字节
- **THEN** 第 5 个 `push()` 返回 `Err(0x05)`（被拒绝的字节）

### Requirement: MockUartHw 测试桩

系统 SHALL 提供 `MockUartHw` 结构，实现 `UartHw` trait，用于单元测试。支持：
- 预填充接收缓冲（`push_rx(byte)`）
- 记录发送数据（`written()` 返回 `&[u8]`）
- 可配置 `wait_tx_done` 是否超时（`set_tx_timeout(true)`）
- 记录 `configure`/`enable_rx_irq`/`disable_rx_irq` 调用

## MODIFIED Requirements

### Requirement: DriverError 增加 Timeout 变体

v0.43.0 的 `DriverError` 枚举缺少 `Timeout` 变体。本版本向 `crates/drivers/framework/src/lib.rs` 的 `DriverError` 增加：

```rust
/// 操作超时
Timeout,
```

并在 `Display` 实现中增加 `"operation timed out"`。

此变更为向后兼容的枚举扩展（新增变体），不破坏既有 API。

## REMOVED Requirements

无。
