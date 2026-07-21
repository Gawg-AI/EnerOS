# v0.73.0 Device Agent Spec

## Why

Phase 1 P1-L MVP 集成第二层：实现 Device Agent（设备管理），负责多设备状态采集和命令执行，作为 RTOS 控制层与 Agent 层之间的桥梁。DeviceAgent 实现 `AgentRuntime` trait（复用 v0.72.0），周期性采集设备状态（SOC/电压/电流/温度/功率）并执行来自 CommandSource 的控制命令。完成本版本后，v0.74.0 MVP 编排器可统一调度 Energy/Market/Device 三个 Agent 完成储能自治端到端场景。

## What Changes

- 新建 crate `eneros-device-agent`（`crates/agents/device-agent/`），含 DeviceAgent 实现
- 新增 `DeviceAdapter` trait + `MockDevice` — 设备适配器抽象（D6：v0.51.0 `PointAccess` 使用 `PointId`/`DataPoint` 类型化 API，过于复杂）
- 新增 `DeviceRegistry` — 多设备注册表，持有 `HashMap<String, Box<dyn DeviceAdapter>>`
- 新增 `DeviceInfo` / `DeviceType` — 设备元信息（D11：简化，仅 device_type + adapter）
- 新增 `DeviceSnapshot` / `DeviceState` — 设备状态快照（D5：替代 `SharedMemoryHandle`）
- 新增 `DeviceCommand` — 设备控制命令（D7：v0.55.0 `ControlCommand` 是 enum，结构不同）
- 新增 `CommandSource` trait + `MockCommandSource` — 命令源抽象（D4：`ControlBusReader` 不存在）
- 新增 `DeviceError` — 设备错误枚举（D8：`AgentError` 缺少 `DeviceNotFound`/`DeviceError` 变体）
- 新增 `DeviceAgent` — 实现 v0.72.0 `AgentRuntime` trait
- **修改** v0.72.0 `AgentRuntimeError` — 添加 `DeviceError(String)` 变体（D8 外科手术式变更，使 DeviceAgent 可复用 trait）
- 根 `Cargo.toml` 版本号 `0.72.0` → `0.73.0`，members 添加 `crates/agents/device-agent`

## Impact

- Affected specs: P1-L MVP 集成第二层（v0.73.0~v0.74.0）
- Affected code:
  - 新建 `crates/agents/device-agent/`（6 源文件 + Cargo.toml）
  - **修改** `crates/agents/energy-market-agent/src/error.rs`（添加 `DeviceError(String)` 变体）
  - 根 `Cargo.toml`（版本 + members）
  - `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
  - 新建设计文档 `docs/agents/device-agent-design.md`

## ADDED Requirements

### Requirement: DeviceAdapter trait

系统 SHALL 提供 `DeviceAdapter` trait，定义设备数据访问接口（字符串点名读写）：

```rust
pub trait DeviceAdapter {
    fn read_point(&mut self, name: &str) -> Result<f64, DeviceError>;
    fn write_point(&mut self, name: &str, value: f64) -> Result<(), DeviceError>;
    fn device_type(&self) -> DeviceType;
    fn is_online(&self) -> bool;
}
```

- D6：v0.51.0 `PointAccess` 使用 `PointId`/`DataPoint` 类型化 API，需 PointMap 映射；MVP 阶段用字符串点名更简单
- 不派生 `Send + Sync`（与 v0.59/v0.63/v0.71/v0.72 一致）

#### Scenario: 读取设备点
- **WHEN** `device.read_point("soc")`
- **THEN** 返回 `Ok(f64)` 或 `Err(DeviceError::PointNotFound)`

#### Scenario: 写入设备点
- **WHEN** `device.write_point("power_setpoint", 50.0)`
- **THEN** 返回 `Ok(())` 或 `Err`

### Requirement: MockDevice

```rust
pub struct MockDevice {
    device_type: DeviceType,
    points: BTreeMap<String, f64>,
    online: bool,
}
```

- `new(device_type: DeviceType) -> Self` — 创建空设备（无点位）
- `with_point(name: &str, value: f64) -> Self` — 链式添加点位
- `set_point(&mut self, name: &str, value: f64)` — 设置点位值
- `set_online(&mut self, online: bool)` — 设置在线状态
- 实现 `DeviceAdapter` trait

D10：`PcsPointMap::default()` / `BatteryPointMap::default()` 等不存在；`MockDevice` 预设点位值即可。

#### Scenario: Mock 设备构造
- **WHEN** `MockDevice::new(DeviceType::Battery).with_point("soc", 0.65)`
- **THEN** `read_point("soc")` 返回 `Ok(0.65)`

#### Scenario: Mock 设备离线
- **WHEN** `device.set_online(false)`，调用 `read_point("soc")`
- **THEN** 返回 `Err(DeviceError::DeviceOffline)`

### Requirement: DeviceType 枚举

```rust
pub enum DeviceType {
    Pcs,
    Battery,
    Bms,
    Meter,
    Temperature,
}
```

派生 `Debug + Clone + Copy + PartialEq + Eq + Hash`。

### Requirement: DeviceInfo

```rust
pub struct DeviceInfo {
    pub device_type: DeviceType,
    pub adapter: Box<dyn DeviceAdapter>,
}
```

D11：简化，仅含 `device_type` + `adapter`（蓝图 `protocol`/`address`/`point_map` 字段不适用于 MVP Mock 设备）。

### Requirement: DeviceRegistry

```rust
pub struct DeviceRegistry {
    devices: BTreeMap<String, DeviceInfo>,
}
```

- `new() -> Self` — 创建空注册表
- `register(&mut self, name: &str, device_type: DeviceType, adapter: Box<dyn DeviceAdapter>)` — 注册设备
- `get_mut(&mut self, name: &str) -> Option<&mut DeviceInfo>` — 获取可变引用
- `len(&self) -> usize` — 设备数量
- `is_empty(&self) -> bool`
- `iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut DeviceInfo)>` — 可变迭代

使用 `BTreeMap`（no_std `alloc::collections::BTreeMap`，非 `HashMap`）。

#### Scenario: 注册并查找设备
- **WHEN** `registry.register("pcs", DeviceType::Pcs, Box::new(MockDevice::new(DeviceType::Pcs)))`
- **THEN** `registry.len() == 1`，`get_mut("pcs")` 返回 `Some`

#### Scenario: 查找不存在的设备
- **WHEN** `get_mut("unknown")`
- **THEN** 返回 `None`

### Requirement: DeviceState + DeviceSnapshot

```rust
pub struct DeviceState {
    pub soc: f64,
    pub voltage: f64,
    pub current: f64,
    pub temperature: f64,
    pub power: f64,
    pub online: bool,
    pub last_update_ms: u64,
}

pub struct DeviceSnapshot {
    pub states: BTreeMap<String, DeviceState>,
}
```

- `DeviceState::default()` — 全零 + `online: false`
- `DeviceSnapshot::new() -> Self` — 空快照
- `DeviceSnapshot::set(&mut self, name: &str, state: DeviceState)` — 设置设备状态

D5：`SharedMemoryHandle` 不存在；`poll_devices()` 返回 `DeviceSnapshot`，调用方直接访问。

### Requirement: DeviceCommand

```rust
pub struct DeviceCommand {
    pub target_device: String,
    pub power_kw: f64,
    pub ttl_ms: u64,
    pub timestamp_ms: u64,
}
```

D7：v0.55.0 `ControlCommand` 是 enum（`Single(SingleCommand)`/`Double(DoubleCommand)`），结构不同；本地定义匹配蓝图语义。

### Requirement: CommandSource trait + MockCommandSource

```rust
pub trait CommandSource {
    fn try_read(&mut self) -> Option<DeviceCommand>;
}
```

- `MockCommandSource` — 预加载命令队列的 Mock 实现
  - `new() -> Self` — 创建空 source
  - `with_commands(commands: Vec<DeviceCommand>) -> Self` — 预加载
  - `push(&mut self, cmd: DeviceCommand)` — 追加命令
  - 实现 `CommandSource` trait — `try_read()` 弹出队首

D4：`ControlBusReader` 不存在；本地定义（与 v0.72.0 `MarketDataSource` 模式一致）。

#### Scenario: 命令源读取
- **WHEN** source 有命令，`try_read()`
- **THEN** 返回 `Some(DeviceCommand)`

#### Scenario: 命令源空
- **WHEN** source 无命令，`try_read()`
- **THEN** 返回 `None`

### Requirement: DeviceError

```rust
pub enum DeviceError {
    DeviceNotFound(String),
    PointNotFound(String),
    DeviceOffline(String),
    WriteFailed(String),
    ReadFailed(String),
}
```

派生 `Debug`（D8：`AgentError` 缺少 `DeviceNotFound`/`DeviceError` 变体）。

实现 `From<DeviceError> for AgentRuntimeError`（映射到 `AgentRuntimeError::DeviceError(String)`）。

### Requirement: DeviceAgent

```rust
pub struct DeviceAgent {
    descriptor: AgentDescriptor,
    devices: DeviceRegistry,
    command_source: Box<dyn CommandSource>,
    last_snapshot: DeviceSnapshot,
    state: AgentState,
    tick_count: u64,
}
```

- `new(name: &str, command_source: Box<dyn CommandSource>, now_ms: u64) -> Self`
  - `descriptor = AgentDescriptor::new(AgentType::Device, name, now_ms)`（D3）
  - `devices = DeviceRegistry::new()`，`last_snapshot = DeviceSnapshot::new()`
  - `state = AgentState::Created`，`tick_count = 0`
- `new_default(now_ms: u64) -> Self` — 使用 `MockCommandSource::new()` 构造，预注册 3 个 Mock 设备（pcs/battery/meter）
- `registry_mut(&mut self) -> &mut DeviceRegistry` — 获取设备注册表（供测试注册设备）
- `last_snapshot(&self) -> &DeviceSnapshot` — 获取最近状态快照

实现 `AgentRuntime` trait（复用 v0.72.0）：
- `descriptor()` — 返回 `&AgentDescriptor`
- `on_start(now_ms)` — `state = Running`，`Ok(())`
- `on_tick(now_ms)` — 执行：
  1. `poll_devices(now_ms)` — 遍历所有设备，通过 `DeviceAdapter::read_point` 采集 soc/voltage/current/temperature/power，更新 `last_snapshot`
  2. `execute_commands(now_ms)` — 从 `command_source.try_read()` 读取命令，查找目标设备，调用 `DeviceAdapter::write_point("power_setpoint", power_kw)`，循环直到无命令
  3. `tick_count += 1`，返回 `Ok(())`
- `on_stop(now_ms)` — `state = Dead`，`Ok(())`
- `on_heartbeat(now_ms)` — `Running` → `Alive` / 否则 `Dead`

`poll_devices` 错误处理：`read_point` 失败时标记 `online: false`，继续采集下一个设备（不中断）。

`execute_commands` 错误处理：
- 设备未找到：跳过该命令，继续下一条（不中断）
- 写入失败：记录错误，继续下一条命令（不中断）

D14（v0.72.0）延续：Device Agent 安全默认为跳过失败命令，不 panic。

#### Scenario: Device Agent 启动
- **WHEN** `on_start(1000)`
- **THEN** `state == Running`，`descriptor.agent_type == Device`

#### Scenario: Device Agent 采集设备状态
- **WHEN** 注册了 1 个设备（battery，soc=0.65），`on_tick(2000)`
- **THEN** `last_snapshot` 含 battery 状态，`soc == 0.65`，`online == true`

#### Scenario: Device Agent 执行命令
- **WHEN** command_source 有命令（target="pcs", power_kw=50.0），注册了 pcs 设备，`on_tick(2000)`
- **THEN** 命令被执行，`write_point("power_setpoint", 50.0)` 被调用

#### Scenario: Device Agent 设备离线
- **WHEN** 设备 `is_online() == false`，`on_tick(2000)`
- **THEN** 该设备状态标记 `online: false`，其他设备正常采集

#### Scenario: Device Agent 命令目标设备不存在
- **WHEN** 命令 target="unknown"，`on_tick(2000)`
- **THEN** 命令被跳过，返回 `Ok(())`，其他命令正常执行

#### Scenario: Device Agent 心跳
- **WHEN** `state == Running`，`on_heartbeat`
- **THEN** 返回 `HeartbeatStatus::Alive`

### Requirement: no_std 合规

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 仅使用 `alloc::*` / `core::*`
- 无 `Instant::now()` / `SystemTime::now()` / `uuid::Uuid::new_v4()`（D2）
- 无 `log::warn!` / `log::info!` / `log::error!`（D1）
- 无 `std::collections::HashMap` / `std::sync::Mutex`（使用 `BTreeMap` / `spin::Mutex` 或不用 Mutex）
- 子模块不重复 `#![cfg_attr(not(test), no_std)]`

## MODIFIED Requirements

### Requirement: AgentRuntimeError（v0.72.0 外科手术式变更）

在 v0.72.0 `eneros-energy-market-agent` crate 的 `AgentRuntimeError` 枚举中添加 `DeviceError(String)` 变体：

```rust
pub enum AgentRuntimeError {
    DualBrainError(DualBrainError),
    ChannelError(String),
    MarketDataError(String),
    AgentError(AgentError),
    NotRunning,
    DeviceError(String),  // ← 新增（D8）
}
```

- 这是对 v0.72.0 的外科手术式变更（Karpathy "Surgical Changes"）
- 理由：DeviceAgent 复用 `AgentRuntime` trait，需要将 `DeviceError` 映射到 `AgentRuntimeError`
- 向后兼容：仅新增变体，不影响现有 v0.72.0 代码

### Requirement: Workspace 版本同步

- 根 `Cargo.toml` 版本号 `0.72.0` → `0.73.0`
- members 列表添加 `"crates/agents/device-agent"`（置于 `"crates/agents/energy-market-agent"` 之后）
- `Makefile` 版本号 `0.73.0`（header + VERSION 变量）
- `.github/workflows/ci.yml` 版本号 `0.73.0`
- `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-device-agent`

## 偏差声明（D1~D12，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `log::info!("执行命令: ...")` / `log::info!("Device Agent 启动")` | 移除日志；状态/错误通过返回值传递 | no_std 无 `log` crate；与 v0.57/v0.64/v0.70/v0.71/v0.72 一致 |
| **D2** | `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()` | `now_ms: u64` 参数 | no_std 合规：`SystemTime` 不可用；与 v0.57~v0.72 一致 |
| **D3** | `AgentDescriptor { id: "device-agent".into(), agent_type: AgentType::Device, priority: 1, capabilities: vec!["device.read", ...], trust_level: TrustLevel::Trusted, ..Default::default() }` | `AgentDescriptor::new(AgentType::Device, name, now_ms)` | v0.33.0 `AgentDescriptor` 13 字段 + 构造器 `new(type, name, now)` 自动设置；蓝图 `..Default::default()` 与 `capabilities: Vec<&str>` 类型不匹配（实际 `Vec<CapabilityRef>`）；与 v0.72.0 D7 一致 |
| **D4** | `ControlBusReader::new()` / `self.control_bus_rx.try_read()` | 本地 `CommandSource` trait + `MockCommandSource`（VecDeque-backed） | `ControlBusReader` 不存在；本地简单实现保持 crate 自包含可测试（与 v0.72.0 D4 `MarketDataSource` 模式一致） |
| **D5** | `SharedMemoryHandle::new()` / `self.shared_memory.write_snapshot(&snapshot)` | `poll_devices()` 返回 `DeviceSnapshot`，存入 `last_snapshot` 字段 | `SharedMemoryHandle` 不存在；MVP 阶段直接返回快照，调用方直接访问（Karpathy 简化原则） |
| **D6** | `device.read_point("soc").unwrap_or(0.0)` / `device.write_point("power_setpoint", power_kw)` on `Box<dyn PointAccess>` | 本地 `DeviceAdapter` trait with `read_point(name: &str) -> Result<f64, DeviceError>` + `MockDevice` | v0.51.0 `PointAccess::read_point(PointId) -> Result<DataPoint, ProtocolError>` 使用类型化 `PointId`/`DataPoint`，需 `PointMap` 映射字符串→ID，MVP 过于复杂；本地 `DeviceAdapter` 字符串点名更简单（与 v0.72.0 D6 `MarketDataSource` 模式一致） |
| **D7** | `command.target_device` / `command.power_kw` / `command.ttl_ms` on `ControlCommand` | 本地 `DeviceCommand` 结构体（target_device/power_kw/ttl_ms/timestamp_ms） | v0.55.0 `ControlCommand` 是 enum（`Single(SingleCommand)`/`Double(DoubleCommand)`），无 `target_device`/`power_kw`/`ttl_ms` 字段；本地定义匹配蓝图语义 |
| **D8** | `AgentError::DeviceNotFound(command.target_device.clone())` / `AgentError::DeviceError(e.to_string())` | 本地 `DeviceError` 枚举 + 在 v0.72.0 `AgentRuntimeError` 添加 `DeviceError(String)` 变体 | v0.33.0 `AgentError` 缺少 `DeviceNotFound`/`DeviceError` 变体（有 `AgentNotFound` 但语义不同）；在 `AgentRuntimeError` 添加变体是外科手术式变更，使 DeviceAgent 可复用 `AgentRuntime` trait |
| **D9** | `impl AgentRuntime for DeviceAgent`（蓝图 trait 无 `now_ms` 参数） | 复用 v0.72.0 `AgentRuntime` trait（含 `now_ms: u64` 参数） | v0.72.0 已定义 `AgentRuntime` trait + `HeartbeatStatus`；复用而非重定义，使 v0.74.0 MVP 编排器可统一调度 Energy/Market/Device 三种 Agent（trait 相同） |
| **D10** | `PcsPointMap::default()` / `BatteryPointMap::default()` / `MeterPointMap::default()` | `MockDevice::new(DeviceType::X).with_point("soc", 0.65)` 链式构造 | `PointMap`/`PcsPointMap`/`BatteryPointMap`/`MeterPointMap` 类型不存在；MockDevice 预设点位即可（Karpathy 简化原则） |
| **D11** | `DeviceInfo { device_type, protocol: String, address: String, point_map: PointMap }` | `DeviceInfo { device_type: DeviceType, adapter: Box<dyn DeviceAdapter> }` | 蓝图 `protocol`/`address`/`point_map` 不适用于 Mock 设备（MVP 无真实协议栈）；简化为 device_type + adapter |
| **D12** | `HashMap<String, Box<dyn PointAccess>>` / `HashMap<String, DeviceInfo>` | `BTreeMap<String, DeviceInfo>` | no_std `alloc::collections::BTreeMap`（`HashMap` 需哈希器配置或 `hashbrown`）；`BTreeMap` 是 no_std 标准选择，有序遍历便于测试 |

## 依赖复用清单

| 复用版本 | 复用类型 | 用途 |
|---------|---------|------|
| v0.72.0 | `AgentRuntime` / `HeartbeatStatus` / `AgentRuntimeError`（+新增 `DeviceError` 变体） | Agent 运行时 trait + 心跳 + 错误 |
| v0.33.0 | `AgentDescriptor` / `AgentType` / `AgentState` / `TrustLevel` / `AgentError` / `AgentId` | Agent 框架类型 |
