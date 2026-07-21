# v0.75.0 DDS 中间件集成与 Rust 封装 Spec

> **Skill**: andrej-karpathy-skills-main
> **版本**: v0.75.0（Phase 2 P2-A 起点 / Agent Bus DDS 三层总线之一）
> **蓝图依据**: `蓝图/phase2.md` §v0.75.0（行 42~394）
> **change-id**: `develop-v0750-agent-bus-dds`

---

## Why

Phase 2 多机联邦的起点版本。需要为 Agent Bus（DDS 发布/订阅）提供 Rust 安全抽象 `DdsNode`，解锁多 Agent 跨进程/跨设备发布订阅通信。本版本是联邦协同的数据平面基石，为 v0.97.0 联邦发现、v0.94.0 VPP 聚合提供通信基础。

由于实际部署环境（飞腾/鲲鹏 Edge Box）暂未集成 Cyclone DDS C 库（Eclipse Cyclone DDS，MIT 许可），本版本采用与 v0.59.0（llama.cpp）/ v0.64.0（HiGHS）一致的 **Mock 默认 + feature-gated FFI** 策略：`MockDdsNode` 默认可用（纯 Rust，可交叉编译），`CycloneDdsNode` + FFI 模块通过 `cyclone-dds` feature 门控（启用时需链接 `libddsc.so`）。

---

## What Changes

- **新建 crate** `eneros-agent-bus-dds`（位于 `crates/protocols/agent-bus-dds/`）
- **新增 `DdsNode` 抽象** — trait + Mock 实现 + feature-gated Cyclone DDS FFI 实现
- **新增 `DdsReader` / `DdsWriter` trait** — 发布/订阅统一接口
- **新增 `DdsConfig` / `DiscoveryPolicy`** — 节点配置与发现策略
- **新增 `QosPolicy`** — 基础 QoS（RELIABLE/BEST_EFFORT + KEEP_LAST/KEEP_ALL），为 v0.76.0 扩展预留
- **新增 `DdsSample` / `InstanceHandle` / `ParticipantId` / `ReaderId` / `WriterId`** — 数据样本与句柄类型
- **新增 `DdsError`** — 8 变体错误枚举
- **新增 `MockDdsNode` / `MockReader` / `MockWriter`** — 默认可用的纯 Rust 实现，支持本地发布订阅往返
- **新增 `configs/dds.toml`** — DDS 配置模板（domain id、发现策略、多播地址）
- **版本号 0.74.0 → 0.75.0**（Cargo.toml / Makefile / ci.yml / ci/src/gate.rs）
- **无外科手术式变更**（不修改 v0.74.0 及之前代码，仅新增 crate）

---

## Impact

- **Affected specs**: 无（Phase 2 起点，不依赖 Phase 1 业务 crate）
- **Affected code**: 新增 crate，无现有 crate 修改
- **依赖**: 无（纯 Rust mock，FFI feature 关闭时零外部依赖）
- **解锁**: v0.75.0 完成 → v0.76.0（DDS Topic 设计与 QoS 策略）开发

---

## ADDED Requirements

### Requirement: DdsConfig 与 DiscoveryPolicy

系统 SHALL 提供 `DdsConfig` 结构体与 `DiscoveryPolicy` 枚举，用于配置 DDS 节点的域 ID、发现策略、多播地址、单播对端列表与绑定网卡。

```rust
pub struct DdsConfig {
    pub domain_id: u32,
    pub discovery: DiscoveryPolicy,
    pub interface: Option<alloc::string::String>,
}

pub enum DiscoveryPolicy {
    Multicast,
    Unicast,
    Static,
}
```

- **D5**：简化 `DiscoveryPolicy`：蓝图将多播地址 `Ipv4Addr` 嵌入 enum 变体，本实现改为统一 `Multicast`/`Unicast`/`Static` 三态，具体地址由 `DdsConfig` 持有（no_std 无 `std::net::Ipv4Addr`，`core::net::Ipv4Addr` 在 nightly 可用但增加复杂度）
- **D6**：移除 `multicast_addr: Option<Ipv4Addr>` / `peers: Vec<IpAddr>` 字段（Mock 实现不使用网络地址；真实 FFI 启用时由 `configs/dds.toml` 解析）

#### Scenario: 默认配置

- **WHEN** 调用 `DdsConfig::default()`
- **THEN** 返回 `domain_id = 0` / `discovery = DiscoveryPolicy::Multicast` / `interface = None`

#### Scenario: 自定义配置

- **WHEN** 调用 `DdsConfig { domain_id: 42, discovery: DiscoveryPolicy::Unicast, interface: Some("eth0".into()) }`
- **THEN** 字段正确设置

---

### Requirement: DdsNode trait

系统 SHALL 提供 `DdsNode` trait，定义 DDS 节点的生命周期与资源管理接口：

```rust
pub trait DdsNode {
    fn create_participant(&mut self) -> Result<ParticipantId, DdsError>;
    fn create_reader(
        &mut self,
        p: ParticipantId,
        topic: &str,
        qos: QosPolicy,
    ) -> Result<ReaderId, DdsError>;
    fn create_writer(
        &mut self,
        p: ParticipantId,
        topic: &str,
        qos: QosPolicy,
    ) -> Result<WriterId, DdsError>;
    fn read(&mut self, reader: ReaderId, max_samples: usize) -> Result<alloc::vec::Vec<DdsSample>, DdsError>;
    fn take(&mut self, reader: ReaderId, max_samples: usize) -> Result<alloc::vec::Vec<DdsSample>, DdsError>;
    fn write(&mut self, writer: WriterId, data: &[u8]) -> Result<(), DdsError>;
    fn shutdown(&mut self) -> Result<(), DdsError>;
    fn is_shutdown(&self) -> bool;
}
```

- **D2**：trait **不要求** `Send + Sync`（与 v0.59/v0.63/v0.71/v0.72 一致；`*mut c_void` 非 `Send`）
- **D7**：合并 `DdsReader`/`DdsWriter` trait 为 `DdsNode` 统一接口（蓝图分离 reader/writer trait，但 Mock 实现单一结构体更简单；read/take/write 方法集中管理避免句柄跨结构体同步）
- **D8**：`create_reader`/`create_writer` 接受 `&str` topic（no_std 兼容；FFI 实现内部用 `alloc::ffi::CString` 转换）

#### Scenario: 创建参与者

- **WHEN** 调用 `node.create_participant()`
- **THEN** 返回 `Ok(ParticipantId)`，后续可用该 ID 创建 reader/writer

#### Scenario: 创建写入器

- **WHEN** 调用 `node.create_writer(p, "topic1", QosPolicy::default())`
- **THEN** 返回 `Ok(WriterId)`，后续可用该 ID 写入数据

#### Scenario: 创建读取器

- **WHEN** 调用 `node.create_reader(p, "topic1", QosPolicy::default())`
- **THEN** 返回 `Ok(ReaderId)`，后续可用该 ID 读取数据

#### Scenario: 写入与读取往返

- **WHEN** 在同一 `MockDdsNode` 中 writer 写入 `[0x01, 0x02, 0x03]`，再用 reader 调用 `take(10)`
- **THEN** 返回 `Vec` 含一个 `DdsSample`，其 `payload == [0x01, 0x02, 0x03]`

#### Scenario: 关闭后再操作

- **WHEN** 调用 `node.shutdown()` 后再调用 `node.create_participant()`
- **THEN** 返回 `Err(DdsError::Closed)`

#### Scenario: 查询关闭状态

- **WHEN** 调用 `node.is_shutdown()`
- **THEN** shutdown 前返回 `false`，shutdown 后返回 `true`

---

### Requirement: QosPolicy

系统 SHALL 提供 `QosPolicy` 结构体，定义 DDS 服务质量策略：

```rust
pub struct QosPolicy {
    pub reliability: Reliability,
    pub durability: Durability,
    pub history: History,
    pub history_depth: i32,
}

pub enum Reliability { BestEffort, Reliable }
pub enum Durability { Volatile, TransientLocal }
pub enum History { KeepAll, KeepLast }
```

- **D9**：`QosPolicy` 为简单结构体（非 builder 模式）；v0.76.0 将扩展为完整 builder + Topic 注册表
- `history_depth`：`KeepLast` 时为深度，`KeepAll` 时忽略

#### Scenario: 默认 QoS

- **WHEN** 调用 `QosPolicy::default()`
- **THEN** 返回 `Reliable` + `Volatile` + `KeepLast` + `history_depth = 10`

#### Scenario: 状态类 QoS

- **WHEN** 调用 `QosPolicy::state_default()`
- **THEN** 返回 `BestEffort` + `Volatile` + `KeepLast(1)`

---

### Requirement: DdsSample 与 InstanceHandle

系统 SHALL 提供 `DdsSample` 结构体与 `InstanceHandle` 类型：

```rust
pub struct DdsSample {
    pub payload: alloc::vec::Vec<u8>,
    pub instance_handle: InstanceHandle,
    pub source_timestamp: u64,
}

pub type InstanceHandle = u64;
```

- `source_timestamp`：发布方时间戳（ns），由 Mock 实现填充 `now_ns` 参数

#### Scenario: 样本构造

- **WHEN** 从 reader `take` 返回样本
- **THEN** `payload` / `instance_handle` / `source_timestamp` 字段可访问

---

### Requirement: DdsError

系统 SHALL 提供 `DdsError` 错误枚举，8 变体：

```rust
pub enum DdsError {
    Ffi(i32),
    InvalidHandle,
    Closed,
    InconsistentQos(alloc::string::String),
    Serialization(alloc::string::String),
    TopicNotFound(alloc::string::String),
    ParticipantNotFound,
    Timeout,
}
```

- 派生 `Debug`
- 实现 `core::fmt::Display`
- 实现 `core::error::Error`（nightly `no_std` 支持）

#### Scenario: FFI 错误码

- **WHEN** FFI 返回 `-1`
- **THEN** 映射为 `DdsError::Ffi(-1)`

#### Scenario: 关闭后操作

- **WHEN** shutdown 后调用 create
- **THEN** 返回 `Err(DdsError::Closed)`

---

### Requirement: MockDdsNode

系统 SHALL 提供 `MockDdsNode`，默认可用的纯 Rust 实现，支持本地发布/订阅往返：

```rust
pub struct MockDdsNode {
    config: DdsConfig,
    participants: SlotMap<ParticipantId, MockParticipant>,
    shutdown: bool,
    now_ns: u64,
}
```

- **D3**：`MockDdsNode` 默认可用；`CycloneDdsNode` + `ffi` 模块通过 `#[cfg(feature = "cyclone-dds")]` 门控
- **D4**：Mock 使用 `slotmap::SlotMap` 管理句柄（与蓝图一致；`slotmap` no_std 兼容）
- **D10**：Mock 内部用 `alloc::collections::BTreeMap` 按 topic 路由消息（writer 写入 topic → 存入对应 topic 的 ring buffer；reader take 时从该 topic 读取）
- **D11**：Mock 支持 `set_now_ns(now_ns)` 注入时间戳（避免 `SystemTime::now()`，no_std 兼容）

#### Scenario: 单节点往返

- **WHEN** 同一 `MockDdsNode` 中 writer 写入 `topic1`，reader 从 `topic1` 读取
- **THEN** reader `take(10)` 返回写入的样本

#### Scenario: 跨 topic 隔离

- **WHEN** writer 写入 `topic1`，reader 监听 `topic2`
- **THEN** reader `take(10)` 返回空 `Vec`

#### Scenario: QoS 过滤

- **WHEN** writer 用 `Reliable` 写入，reader 用 `BestEffort` 读取
- **THEN** 仍能收到（Mock 不强制 QoS 兼容性校验；真实 FFI 启用时由 C 库校验）

#### Scenario: 多 reader 共享 topic

- **WHEN** 同一 topic 创建 2 个 reader，writer 写入 1 条
- **THEN** 每个 reader `take(1)` 各返回 1 条（广播语义）

#### Scenario: KeepLast 深度限制

- **WHEN** reader QoS `KeepLast(2)`，writer 写入 3 条
- **THEN** reader `take(10)` 最多返回 2 条（Mock 实现 ring buffer 截断）

---

### Requirement: CycloneDdsNode（feature-gated）

系统 SHALL 在 `feature = "cyclone-dds"` 启用时提供 `CycloneDdsNode` 与 `ffi` 模块，封装 Cyclone DDS C API：

- **D3**：`CycloneDdsNode` + `ffi` 模块通过 `#[cfg(feature = "cyclone-dds")]` 门控
- **D10**：FFI 集中封装于 `ffi` 模块；每个 `unsafe` 块附 SAFETY 注释；指针所有权明确（`dds_create_*` 返回值由 `CycloneDdsNode` 持有，`Drop` 调用 `dds_delete`）
- 默认 feature 关闭，不引入任何 `std::*` / `unsafe` / C 库依赖

#### Scenario: feature 未启用时编译

- **WHEN** 默认 `cargo build -p eneros-agent-bus-dds`
- **THEN** 成功编译，`CycloneDdsNode` / `ffi` 模块不参与编译

#### Scenario: feature 启用时编译

- **WHEN** `cargo build -p eneros-agent-bus-dds --features cyclone-dds`
- **THEN** `ffi` 模块参与编译（但需 `libddsc.so` 链接；CI 默认不启用此 feature）

---

### Requirement: 配置文件

系统 SHALL 提供 `configs/dds.toml` 配置模板：

```toml
# DDS 节点配置模板
domain_id = 0
discovery = "multicast"  # multicast | unicast | static
interface = ""            # 绑定网卡，空字符串表示自动选择

# 单播发现对端列表（discovery = "unicast" 时生效）
[[peers]]
address = "192.168.1.100"
port = 7400
```

- **D6**：蓝图将多播地址嵌入 `DiscoveryPolicy` enum，本实现改为配置文件解析（`configs/dds.toml`），运行时由 `DdsConfig` 加载

#### Scenario: 配置文件存在

- **WHEN** 检查 `configs/dds.toml`
- **THEN** 文件存在，包含 `domain_id` / `discovery` / `interface` / `peers` 字段

---

## MODIFIED Requirements

### Requirement: Workspace 版本同步

- 根 `Cargo.toml` `version = "0.74.0"` → `"0.75.0"`
- 根 `Cargo.toml` `members` 列表添加 `"crates/protocols/agent-bus-dds"`（置于 `crates/protocols/mvp-scenario` 不存在时，置于 `crates/protocols/soe-engine` 之后）
- `Makefile` `VERSION` 变量与 header 注释更新为 `0.75.0`
- `.github/workflows/ci.yml` 版本注释更新为 `0.75.0`
- `ci/src/gate.rs` clippy 段与 test 段注释补充 `eneros-agent-bus-dds`

---

## REMOVED Requirements

无移除需求。

---

## 偏差声明（D1~D12）

| 偏差 | 说明 |
|------|------|
| **D1** | no_std 合规：`alloc::string::String` / `alloc::vec::Vec` / `alloc::collections::BTreeMap` 替代 `std::*`；`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明 |
| **D2** | `DdsNode` trait **不要求** `Send + Sync`（与 v0.59/v0.63/v0.71/v0.72 一致；`*mut c_void` 非 `Send`） |
| **D3** | `MockDdsNode` 默认可用；`CycloneDdsNode` + `ffi` 模块通过 `#[cfg(feature = "cyclone-dds")]` 门控；`Cargo.toml` 声明 `[features] cyclone-dds = []`（默认关闭） |
| **D4** | 使用 `slotmap::SlotMap` 管理句柄（与蓝图一致；`slotmap` no_std 兼容，`default-features = false`） |
| **D5** | `DiscoveryPolicy` 简化为三态 enum（`Multicast`/`Unicast`/`Static`），不嵌入 `Ipv4Addr`（no_std 无 `std::net::Ipv4Addr`，`core::net::Ipv4Addr` 增加复杂度） |
| **D6** | 移除 `DdsConfig` 的 `multicast_addr` / `peers` 字段（Mock 不使用网络地址；真实 FFI 由 `configs/dds.toml` 解析） |
| **D7** | 合并 `DdsReader`/`DdsWriter` trait 为 `DdsNode` 统一接口（Mock 单结构体更简单；read/take/write 集中管理避免句柄跨结构体同步） |
| **D8** | `create_reader`/`create_writer` 接受 `&str` topic（no_std 兼容；FFI 实现内部用 `alloc::ffi::CString` 转换） |
| **D9** | `QosPolicy` 为简单结构体（非 builder 模式）；v0.76.0 将扩展为完整 builder + Topic 注册表 |
| **D10** | FFI 集中封装于 `ffi` 模块；每个 `unsafe` 块附 SAFETY 注释；指针所有权明确（`dds_create_*` 返回值由 `CycloneDdsNode` 持有，`Drop` 调用 `dds_delete`） |
| **D11** | Mock 支持 `set_now_ns(now_ns)` 注入时间戳（避免 `SystemTime::now()`，no_std 兼容） |
| **D12** | crate 位置 `crates/protocols/agent-bus-dds/`（protocols 子系统；DDS 是通信协议中间件，与 mqtt/modbus 同类；项目规则 §2.3.1） |

---

## 简化设计验证（Karpathy 原则）

- ✅ Mock 默认可用（不依赖 C 库，可交叉编译）
- ✅ 合并 reader/writer trait 为 DdsNode（单一结构体更简单）
- ✅ 简化 DiscoveryPolicy（三态 enum，不嵌入 IP 地址）
- ✅ 简化 QosPolicy（结构体非 builder，v0.76.0 扩展）
- ✅ 无 panic hook 安装（蓝图 install_panic_hook_once 在 no_std 下由 #[panic_handler] 处理，本 crate 不涉及）
- ✅ 无 valgrind 集成（Mock 实现无 FFI 内存泄漏；真实 FFI 启用时由 CI 集成层验证）
- ✅ 无跨机集成测试（Mock 仅验证单节点往返；跨机测试由集成测试层 + Cyclone DDS C 库启用时验证）
