# v0.43.0 — 用户态驱动框架 Spec

> 覆盖版本：v0.43.0（用户态驱动框架，P1-F 设备协议栈基石）
> 蓝图依据：`蓝图/phase1.md` §7450-7659
> 前置版本：v0.42.1（System Agent 故障恢复编排 + 本地 HMI，已完成）
> 解锁版本：v0.44.0（RS485 串口驱动）

## Why

v0.42.x 完成了 System Agent 与故障恢复编排，但所有外设驱动（RS485、CAN、网卡等）目前散落在 `crates/drivers/` 各自独立实现，缺少统一的注册/发现/隔离/生命周期抽象。v0.43.0 建立用户态设备驱动框架，定义 `DeviceDriver` trait，使后续所有设备协议版本（v0.44.0~v0.51.0）通过统一接口注册并被 Agent/RTOS 发现使用。

## What Changes

### 新增 crate：eneros-driver-framework
- 新增 `crates/drivers/framework/`（eneros-driver-framework）— 驱动框架核心库
- 新增 `crates/drivers/framework/src/lib.rs` — 模块声明 + re-exports + `DeviceDriver` trait + `DriverType`/`DriverState`/`DriverHealth`/`DriverId` 类型 + `DriverError` 枚举
- 新增 `crates/drivers/framework/src/registry.rs` — `DriverRegistry` 注册表 + `DriverEntry` + `DriverStats`
- 新增 `crates/drivers/framework/src/handle.rs` — `DriverHandle` + `DriverCapability`（自包含能力令牌）
- 新增 `crates/drivers/framework/src/mock.rs` — `MockDriver` 测试桩驱动
- 修改根 `Cargo.toml` — workspace members 新增 `crates/drivers/framework`

### 文档与配置
- 新增 `docs/drivers/driver-framework-design.md` — 驱动框架设计文档（≥10 章，含 mermaid 图）
- 新增 `configs/driver-framework.toml` — 驱动框架配置模板

### 版本同步
- 0.42.1 → 0.43.0（根 `Cargo.toml` / `Makefile` / `ci.yml` / `ci/src/gate.rs` / `crates/agents/agent/src/lib.rs`）

### 集成测试
- 新增 `crates/drivers/framework/tests/driver_framework_test.rs` — 10 个集成测试

## Impact

- **Affected specs**: 无（全新 crate，不修改既有 crate 的公共 API）
- **Affected code**:
  - `crates/drivers/framework/`（全新 crate，4 个源文件 + Cargo.toml）
  - 根 `Cargo.toml`（+1 member）
  - 版本标识文件（5 处）
- **New dependencies**: eneros-driver-framework 零外部依赖（纯 Rust no_std，仅 `alloc`/`core`）
- **Decoupling decision**: 框架不依赖 `eneros-agent`（见偏差 D1），驱动无需传递依赖 agent runtime

## ADDED Requirements

### Requirement: DeviceDriver trait

系统 SHALL 提供统一的设备驱动接口 trait，包含驱动标识、状态、生命周期管理与中断处理方法。

```rust
pub trait DeviceDriver: Send + Sync {
    fn id(&self) -> &DriverId;
    fn name(&self) -> &str;
    fn driver_type(&self) -> DriverType;
    fn state(&self) -> DriverState;
    fn init(&mut self) -> Result<(), DriverError>;
    fn start(&mut self) -> Result<(), DriverError>;
    fn stop(&mut self) -> Result<(), DriverError>;
    fn deinit(&mut self) -> Result<(), DriverError>;
    fn handle_irq(&mut self, irq_id: u32);
    fn health_check(&self) -> DriverHealth;
}
```

#### Scenario: 驱动标识
- **WHEN** 调用 `driver.id()`
- **THEN** 返回该驱动的唯一 `DriverId` 引用

#### Scenario: 状态查询
- **WHEN** 调用 `driver.state()`
- **THEN** 返回当前 `DriverState`（Uninitialized/Ready/Running/Stopped/Error/Dead）

#### Scenario: 生命周期转换
- **WHEN** 调用 `init()` 成功
- **THEN** 驱动状态从 `Uninitialized` 变为 `Ready`
- **WHEN** 调用 `start()` 成功
- **THEN** 驱动状态从 `Ready` 变为 `Running`
- **WHEN** 调用 `stop()` 成功
- **THEN** 驱动状态从 `Running` 变为 `Stopped`
- **WHEN** 调用 `deinit()` 成功
- **THEN** 驱动状态变为 `Dead`

### Requirement: DriverType 枚举

系统 SHALL 定义驱动类型枚举，用于按类型发现驱动。

```rust
pub enum DriverType {
    Serial, Network, Can, Storage, Gpio, I2c, Spi, Custom(u16),
}
```

#### Scenario: 类型索引
- **WHEN** 注册一个 `DriverType::Serial` 驱动
- **THEN** `find_by_type(Serial)` 返回包含该驱动 ID 的列表

### Requirement: DriverState 状态机

系统 SHALL 定义驱动生命周期状态机，包含 6 个状态：Uninitialized / Ready / Running / Stopped / Error / Dead。

#### Scenario: 合法转换
- **WHEN** 状态为 `Uninitialized` 并调用 `init()`
- **THEN** 状态转为 `Ready`
- **WHEN** 状态为 `Ready` 并调用 `start()`
- **THEN** 状态转为 `Running`
- **WHEN** 状态为 `Running` 并调用 `stop()`
- **THEN** 状态转为 `Stopped`

#### Scenario: 非法转换
- **WHEN** 状态为 `Uninitialized` 并调用 `start()`
- **THEN** 返回 `Err(DriverError::InvalidState)`

### Requirement: DriverRegistry 注册表

系统 SHALL 提供全局驱动注册表，支持按 ID/类型/名称注册与发现，并通过能力校验控制访问。

#### Scenario: 注册驱动
- **WHEN** 调用 `register(driver)`
- **THEN** 驱动被加入注册表
- **AND** 返回该驱动的 `DriverId`

#### Scenario: 重复注册
- **WHEN** 注册已存在 ID 的驱动
- **THEN** 返回 `Err(DriverError::AlreadyRegistered)`

#### Scenario: 按 ID 查找
- **WHEN** 调用 `find_by_id(id)`
- **THEN** 返回 `Some(DriverId)` 或 `None`

#### Scenario: 按类型查找
- **WHEN** 调用 `find_by_type(dtype)`
- **THEN** 返回该类型所有驱动 ID 的 `Vec`

#### Scenario: 按名称查找
- **WHEN** 调用 `find_by_name(name)`
- **THEN** 返回 `Some(DriverId)` 或 `None`

#### Scenario: 打开驱动（能力校验）
- **WHEN** 调用 `open(id, cap)`
- **AND** `cap` 不具备该驱动所需权限
- **THEN** 返回 `Err(DriverError::PermissionDenied)`

#### Scenario: 打开不存在驱动
- **WHEN** 调用 `open(unknown_id, cap)`
- **THEN** 返回 `Err(DriverError::NotFound)`

#### Scenario: 注销驱动
- **WHEN** 调用 `unregister(id)`
- **THEN** 驱动从注册表移除

### Requirement: DriverHandle 句柄

系统 SHALL 提供驱动句柄，持有驱动 ID 与能力令牌，作为驱动访问的凭证。

#### Scenario: 句柄创建
- **WHEN** `open()` 成功
- **THEN** 返回 `DriverHandle { id, cap }`

#### Scenario: 句柄身份
- **WHEN** 调用 `handle.id()`
- **THEN** 返回所持有驱动的 `DriverId`

### Requirement: DriverCapability 能力令牌

系统 SHALL 提供自包含的驱动访问能力令牌，包含所有者 ID 与权限位集，用于 `open()` 时的访问控制。

#### Scenario: 授权令牌
- **WHEN** 调用 `DriverCapability::new(owner, permissions)`
- **THEN** 创建包含所有者与权限的令牌

#### Scenario: 权限校验
- **WHEN** 调用 `cap.can_access(required)`
- **AND** `cap.permissions` 包含 `required` 的所有位
- **THEN** 返回 `true`

### Requirement: MockDriver 测试桩

系统 SHALL 提供测试桩驱动 `MockDriver`，实现 `DeviceDriver` trait，支持可配置的状态转换与调用记录。

#### Scenario: 模拟初始化
- **WHEN** 创建 `MockDriver::new(id, name, dtype)`
- **THEN** 初始状态为 `Uninitialized`
- **WHEN** 调用 `init()`
- **THEN** 状态转为 `Ready`

#### Scenario: 中断记录
- **WHEN** 调用 `handle_irq(irq_id)`
- **THEN** `irq_id` 被记录到调用历史

## MODIFIED Requirements

### Requirement: 版本标识

将以下文件的版本标识从 `0.42.1` 同步为 `0.43.0`：
- 根 `Cargo.toml`（`[workspace.package] version`）
- `Makefile`（`VERSION` + 头部注释）
- `.github/workflows/ci.yml`（版本注释）
- `ci/src/gate.rs`（clippy + test 注释，描述新增 eneros-driver-framework）
- `crates/agents/agent/src/lib.rs`（`VERSION` 常量 + 模块文档）

## 偏差声明

| 偏差 | 蓝图设计 | 实际实现 | 理由 |
|------|---------|---------|------|
| **D1** | `DriverEntry` 使用 `CapabilityToken`（来自 eneros-agent），`open()` 调用 `cap.can_access(entry.cap_token.owner())` + `CapabilityToken::new_owner_only()` | 自包含 `DriverCapability`（owner_id + permissions 位集），`open()` 调用 `cap.can_access(required_perms)` | (1) 真实 `CapabilityToken` API 无 `new_owner_only()`/`can_access(owner())`/`owner()` 方法（蓝图假设的接口不存在）；(2) 驱动框架是基础库，所有驱动将依赖它，若依赖 eneros-agent 则所有驱动传递依赖 agent runtime + eneros-crypto（SM2 签名验证），架构耦合过重；(3) 自包含令牌满足蓝图核心意图"能力令牌控制访问"且零外部依赖 |
| **D2** | `DriverRegistry` 使用 `HashMap`/`HashSet` + `String` 名称索引 | `BTreeMap`/`BTreeSet` + `String`（alloc） | no_std 无 `std::collections::HashMap`（需 `hashbrown` 依赖）；BTreeMap 零依赖且有序，遵循 v0.42.0 D1 先例 |
| **D3** | `DriverEntry.created_at: MonotonicTime` + `MonotonicTime::now()` | `created_at: u64`，时间戳由 `register()` 调用方传入 `now: u64` 参数 | no_std 无系统时钟；遵循 v0.41.0 `tick(now)` 注入时间模式 |
| **D4** | seL4 CNode 隔离 + notification IRQ 路由 + 分区级 panic 隔离 | 框架提供 `handle_irq()` trait 方法签名与状态机；seL4 真实隔离在 Phase 3（v0.127.0+）定制时集成 | Phase 1 阶段 seL4 尚未定制（ADR-0001）；框架是 trait 抽象层，硬件隔离由运行时注入。蓝图 §8.1 风险"seL4 能力传递复杂"在 Phase 3 解决 |
| **D5** | 蓝图 `DriverStats` 被引用但未定义 | 在框架内定义 `DriverStats { open_count: u32, error_count: u32, last_error: Option<DriverError>, irq_count: u32 }` | 蓝图交付物清单引用但缺定义；自包含类型供 `health_check` 与可观测性使用 |
| **D6** | 蓝图未明确 `DriverId` 来源（隐含复用 agent `DeviceId`） | `pub struct DriverId(pub u64)` 在框架内定义 | 框架不依赖 eneros-agent（D1）；DriverId 自包含，避免传递依赖 |
| **D7** | 蓝图 §5 "每个驱动独占一个线程" | 框架为单线程注册表；线程模型由调用方（RTOS/Agent）决定 | 线程管理依赖 v0.19.0 调度器；框架聚焦 trait/注册/发现，不绑定线程策略 |
| **D8** | 蓝图 `DriverHandle` 持 `cap: CapabilityToken`（Clone） | `DriverHandle` 持 `cap: DriverCapability`（Copy，因内部仅 u64+u32） | D1 一致性；DriverCapability 为 POD 类型，Copy 语义更自然 |
| **D9** | 蓝图 §8.4 "DMA 缓冲区需 seL4 SharedMemory 授权" | 框架不涉及 DMA 缓冲区管理（由具体驱动如 v0.44.0 RS485 处理） | 框架是 trait 层，DMA 是驱动实现细节；遵循"Surgical Changes"不引入未要求的抽象 |
