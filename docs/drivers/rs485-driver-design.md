# RS485 串口驱动设计文档（v0.44.0）

> **版本**：v0.44.0
> **蓝图参考**：`蓝图/phase1.md` §7662-7902
> **前置版本**：v0.43.0（驱动框架）
> **后续版本**：v0.45.0（Modbus RTU 主站）
> **最后更新**：2026-07-15

---

## 1. 版本目标

基于 v0.43.0 驱动框架（`DeviceDriver` trait + `DriverRegistry`）实现 RS485 半双工串口驱动，提供物理层/链路层数据帧收发能力，为 v0.45.0 Modbus RTU 主站提供底层传输。

- **一句话目标**：实现 RS485 串口驱动，支持数据帧收发、DE/RE 方向控制、帧间隔检测、超时重传。
- **架构定位**：P1-F 设备协议栈第二层，为 Modbus RTU 提供物理层/链路层。
- **设计原则关联**：实时性（串口收发延迟 <10ms）、可靠性（CRC16 校验 + 超时重传由上层处理）。

## 2. 前置依赖

| 依赖版本 | 依赖产出 | 用途 |
|---------|---------|------|
| v0.7.0 | HAL Serial + GPIO | 硬件寄存器访问（通过 UartHw 抽象间接使用） |
| v0.43.0 | DeviceDriver trait + DriverRegistry | 驱动注册与发现 |

## 3. 交付物清单

| 类型 | 交付物 | 路径 |
|------|--------|------|
| 代码 crate | `eneros-rs485` | `crates/drivers/rs485/` |
| 驱动实现 | `Rs485Driver` | `crates/drivers/rs485/src/driver.rs` |
| 配置结构 | `Rs485Config` | `crates/drivers/rs485/src/config.rs` |
| 硬件抽象 | `UartHw` trait | `crates/drivers/rs485/src/uart_hw.rs` |
| 环形缓冲 | `RingBuffer` | `crates/drivers/rs485/src/ring.rs` |
| 测试桩 | `MockUartHw` | `crates/drivers/rs485/src/mock.rs` |
| 设计文档 | 本文档 | `docs/drivers/rs485-driver-design.md` |

## 4. 详细设计

### 4.1 模块结构

```
crates/drivers/rs485/
├── Cargo.toml          # crate 清单（依赖 eneros-driver-framework + eneros-hal）
└── src/
    ├── lib.rs          # 模块入口 + re-export
    ├── config.rs       # Rs485Config + UartPort/StopBits/Parity/GpioPin（D2）
    ├── uart_hw.rs      # UartHw trait 抽象（D1）
    ├── ring.rs         # RingBuffer<T, const N: usize>（D4）
    ├── driver.rs       # Rs485Driver + Rs485Stats + DeviceDriver 实现
    └── mock.rs         # MockUartHw 测试桩（#[cfg(test)]）
```

### 4.2 Rs485Config 配置结构

```rust
pub struct Rs485Config {
    pub port: UartPort,              // 串口号
    pub baud_rate: u32,              // 波特率（9600/19200/38400/115200）
    pub data_bits: u8,               // 数据位（7/8）
    pub stop_bits: StopBits,         // 停止位（1/2）
    pub parity: Parity,              // 校验位
    pub local_addr: u8,              // 本机地址（Modbus 从站地址）
    pub response_timeout_ms: u32,    // 响应超时
    pub frame_gap_ms: u32,           // 帧间隔（Modbus RTU 3.5 字符时间）
    pub de_re_pin: Option<GpioPin>,  // DE/RE 方向控制 GPIO 引脚号
    pub pre_send_delay_us: u32,      // 发送前等待时间
    pub post_send_delay_us: u32,      // 发送后等待时间
}
```

默认值：Uart0 / 9600bps / 8N1 / 地址 1 / 超时 1000ms / 帧间隔 4ms / 无 DE_RE / 前后延时 100μs。

### 4.3 UartHw 硬件抽象 trait（D1 偏差）

蓝图假设 HAL 提供 `HalUart` trait，但实际 HAL 仅有 `HalSerial`（write/read/flush）。因此定义本地 `UartHw` trait：

```rust
pub trait UartHw: Send + Sync {
    fn configure(&mut self, baud_rate: u32, data_bits: u8,
                 stop_bits: StopBits, parity: Parity) -> Result<(), DriverError>;
    fn enable_rx_irq(&mut self) -> Result<(), DriverError>;
    fn disable_rx_irq(&mut self) -> Result<(), DriverError>;
    fn read_byte(&mut self) -> Option<u8>;
    fn write_bytes(&mut self, data: &[u8]) -> Result<usize, DriverError>;
    fn wait_tx_done(&mut self, timeout_ms: u32) -> Result<(), DriverError>;
    fn rx_irq_id(&self) -> u32;
    fn now_ns(&self) -> u64;
    fn configure_de_re(&mut self, pin: Option<u32>) -> Result<(), DriverError>;
    fn set_de_re(&mut self, high: bool) -> Result<(), DriverError>;
}
```

`Send + Sync` 超级 trait 确保 `Rs485Driver` 可实现 `DeviceDriver: Send + Sync`，从而可注册到 `DriverRegistry`。

### 4.4 Rs485Driver 驱动实现

```rust
pub struct Rs485Driver {
    id: DriverId,
    name: String,
    config: Rs485Config,
    state: DriverState,
    uart: Box<dyn UartHw>,           // D1: UART 硬件抽象
    rx_buffer: RingBuffer<u8, 512>,  // D4: 接收环形缓冲
    stats: Rs485Stats,
    irq_rx: AtomicBool,              // D6: 接收中断标志
}
```

实现 `DeviceDriver` trait 的生命周期方法：

| 方法 | 状态转换 | 操作 |
|------|---------|------|
| `init()` | Uninitialized → Ready | 配置 UART + 配置 DE/RE GPIO + 设置接收模式 |
| `start()` | Ready → Running | 启用 RX 中断 |
| `stop()` | Running → Stopped | 禁用 RX 中断 |
| `deinit()` | Stopped → Dead | 标记销毁 |
| `handle_irq(irq_id)` | — | 读取 UART 数据到 rx_buffer + 设置 irq_rx 标志 |
| `health_check()` | — | 基于 rx_error_count 返回 Healthy/Degraded/Unhealthy |

### 4.5 发送流程

```
send(data) →
  1. uart.set_de_re(true)           // DE=1, 发送模式
  2. uart.write_bytes(data)          // 写入数据
  3. uart.wait_tx_done(timeout)      // 等待发送完成
     ├─ Ok → uart.set_de_re(false)   // DE=0, 接收模式
     │       stats.tx_count++
     │       return Ok(())
     └─ Err(Timeout) → uart.set_de_re(false)  // 恢复接收模式
                        stats.rx_error_count++
                        return Err(Timeout)
```

### 4.6 接收流程

```
recv(timeout_ms) →
  1. start_ns = uart.now_ns()
  2. deadline = start_ns + timeout_ms * 1_000_000
  3. frame_gap = config.frame_gap_ms * 1_000_000
  4. loop:
     - now = uart.now_ns()
     - if now >= deadline: break
     - if rx_buffer.pop() → Some(byte):
         frame.push(byte); last_byte = now
     - elif last_byte != None and now - last_byte >= frame_gap:
         break  // 帧间隔超时，帧结束
  5. if frame empty: return Err(Timeout)
  6. stats.rx_count++; return Ok(frame)
```

### 4.7 RingBuffer 环形缓冲（D4 偏差）

使用 const generics 实现固定容量环形缓冲，无外部依赖：

```rust
pub struct RingBuffer<T, const N: usize> {
    buffer: [MaybeUninit<T>; N],
    read: usize,
    write: usize,
    count: usize,
}
```

方法：`push`（满时返回 Err）、`pop`、`len`、`is_empty`、`is_full`、`capacity`、`clear`。

### 4.8 MockUartHw 测试桩

`MockUartHw` 实现 `UartHw` trait，支持：
- 预填充接收数据（`push_rx` / `push_rx_slice`）
- 记录已发送数据（`written()`）
- 可配置 TX 超时（`set_tx_timeout`）
- 模拟时间推进（`now_ns()` 每次调用自动推进 `time_step_ns`）
- DE/RE 状态跟踪（`de_re_high()`）
- configure / enable_rx_irq / disable_rx_irq 调用记录

## 5. 偏差声明

| 偏差 | 蓝图假设 | 实际情况 | 处理方案 |
|------|---------|---------|---------|
| **D1** | `HalUart` trait 提供 UART 配置/IRQ/读字节等 | HAL 仅有 `HalSerial`（write/read/flush） | 定义本地 `UartHw` trait 抽象 UART 硬件操作 |
| **D2** | `UartPort`/`StopBits`/`Parity`/`GpioPin` 已存在 | HAL 中无这些类型 | 在 `config.rs` 内定义 |
| **D3 修正** | `MonotonicTime::now()` 获取时间 | no_std 无系统时钟 | `UartHw` trait 新增 `now_ns()` 方法，`recv()` 通过此方法获取时间 |
| **D4** | `RingBuffer` 来自外部库 | no_std 无该类型 | 本地实现 `RingBuffer<T, const N: usize>` |
| **D5** | `DriverError::Timeout` 已存在 | v0.43.0 无此变体 | 向框架 `DriverError` 增加变体 |
| **D6** | `AtomicBool`（未指定路径） | no_std 需明确路径 | 使用 `core::sync::atomic::AtomicBool` |
| **D7 修正** | `&'static dyn HalGpio` 控制 DE/RE | `HalGpio` 无 `Send + Sync` 超级 trait | 将 DE/RE 控制方法（`configure_de_re`/`set_de_re`）合并到 `UartHw` trait，`Rs485Driver` 持 `Box<dyn UartHw>` 而非 `HalGpio` 引用 |
| **D8** | `Self::delay_us()` 阻塞延时 | no_std 无标准延时 | 延时由 `UartHw` 实现负责，`Rs485Driver` 不直接调用 |
| **D9** | `tx_buffer: Vec<u8>` 字段 | 同步发送无需缓冲 | 移除 `tx_buffer` 字段 |
| **D10** | `recv()` 返回 `Vec<u8>` | no_std + alloc 允许 | 使用 `alloc::vec::Vec<u8>` |

## 6. 测试计划

| 测试类型 | 测试内容 | 状态 |
|---------|---------|------|
| 单元测试 | RingBuffer（空/满/环绕/容量 0） | ✅ 10 个 |
| 单元测试 | Rs485Config 默认值/自定义/枚举变体 | ✅ 6 个 |
| 集成测试 | 状态转换（init→Ready→Running→Stopped→Dead） | ✅ |
| 集成测试 | send() 成功（数据写入 + tx_count 递增） | ✅ |
| 集成测试 | send() 超时（Timeout → DE 恢复 → Err） | ✅ |
| 集成测试 | recv() 成功（帧间隔检测 → 返回帧） | ✅ |
| 集成测试 | recv() 超时（空缓冲 → Timeout） | ✅ |
| 集成测试 | handle_irq() 匹配/不匹配 | ✅ |
| 集成测试 | health_check() 三档（Healthy/Degraded/Unhealthy） | ✅ |
| 集成测试 | trait object 兼容性（Box<dyn DeviceDriver>） | ✅ |
| 集成测试 | name() 按 port 生成 | ✅ |

## 7. 验收标准

- [x] RS485 驱动实现 DeviceDriver trait
- [x] 支持 9600/19200/38400/115200 波特率（配置参数化）
- [x] DE/RE 方向控制正确（发送时 DE=1，接收时 DE=0）
- [x] 收发逻辑完整（send + recv 路径）
- [x] 帧间隔检测逻辑实现（frame_gap_ms 静默判定）

## 8. 风险与注意事项

| # | 风险 | 缓解措施 |
|---|------|---------|
| 1 | RS485 总线冲突（多节点同时发送） | 由上层 Modbus 主从轮询保证 |
| 2 | 波特率误差（晶振偏差 >3%） | 配置参数化，支持动态调整 |
| 3 | 中断延迟（seL4 IRQ 路由） | rx_buffer 容量 512 字节，减少溢出风险 |
| 4 | DE 切换时序错误导致丢字节 | `wait_tx_done()` 确保发送完成后才切 DE |
| 5 | GPIO 初始状态误发送 | `init()` 中设置 DE=0（接收模式） |

## 9. 多角度要求

| 维度 | 要求 | 实现 |
|------|------|------|
| 功能 | 配置/收发/方向控制/帧检测 | ✅ Rs485Config + send/recv + DE/RE + frame_gap |
| 性能 | 收发延迟 <10ms@9600bps | 配置参数化，wait_tx_done 阻塞等待 |
| 安全 | 驱动隔离，硬件置于安全状态 | ✅ init 默认接收模式（DE=0） |
| 可靠 | 超时重传、错误统计 | ✅ Timeout + Rs485Stats + health_check |
| 可维护 | 配置参数化 | ✅ Rs485Config 11 个参数 |
| 可观测 | 收发统计、错误计数 | ✅ Rs485Stats（tx/rx/error_count） |
| 可扩展 | 支持多串口实例 | ✅ UartPort 枚举 + UartHw trait 抽象 |

## 10. 架构图

```
┌──────────────────────────────────────────────────┐
│              v0.45.0 Modbus RTU                   │
│              (ModbusMaster)                       │
└──────────────────┬───────────────────────────────┘
                   │ send() / recv()
                   ▼
┌──────────────────────────────────────────────────┐
│              v0.44.0 RS485 Driver                 │
│  ┌─────────────────────────────────────────────┐ │
│  │ Rs485Driver (DeviceDriver)                  │ │
│  │  ├── Rs485Config (端口/波特率/帧间隔/DE_RE) │ │
│  │  ├── RingBuffer<u8, 512> (接收缓冲)         │ │
│  │  ├── Rs485Stats (收发统计)                  │ │
│  │  └── AtomicBool (IRQ 标志)                  │ │
│  └─────────────┬───────────────────────────────┘ │
│                │ UartHw trait (D1)                 │
└────────────────┼─────────────────────────────────┘
                 ▼
┌──────────────────────────────────────────────────┐
│          UartHw 实现（BSP / MockUartHw）           │
│  ├── configure() / enable_rx_irq() / read_byte() │
│  ├── write_bytes() / wait_tx_done()              │
│  ├── now_ns() (时间源)                           │
│  └── configure_de_re() / set_de_re() (D7)        │
└──────────────────────────────────────────────────┘
```
