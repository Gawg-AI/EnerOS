# v0.30.0 + v0.30.1 + v0.30.2 — 网络安全 + 蜂窝通信 + 双网冗余 Spec

## Why

v0.29.0 Socket 抽象层完成后，网络栈具备了基础通信能力，但缺乏安全防护（防火墙、连接限制、DDoS 防护）和性能基准。同时，储能终端部署场景多样（偏远电站、移动储能），有线网络不可达时需蜂窝网络作为备份通道，并要求双网冗余保障通信高可用。

本 spec 一次性开发 v0.30.x 系列全部 3 个版本（工作区规则 §记忆.md 要求）：
- **v0.30.0**：网络栈安全与性能（防火墙 + 连接限制 + DDoS 防护 + 基准测试）
- **v0.30.1**：蜂窝通信模块（AT 命令 + PPP 拨号 + modem 驱动）— 刚性子版本 R2
- **v0.30.2**：双网冗余与切换（心跳监测 + 故障切换 + 防抖回切）— 刚性子版本 R2

## What Changes

### v0.30.0 — 网络栈安全与性能

- **新增 `security/` 子模块**（4 文件，添加到 eneros-net crate）：
  - `security/mod.rs` — 模块声明 + re-exports
  - `security/firewall.rs` — 防火墙规则引擎（Firewall + FirewallRule + FirewallAction + FirewallPolicy）
  - `security/rate_limit.rs` — 连接数/速率限制（ConnectionTracker + RateLimit）
  - `security/ddos.rs` — DDoS 防护（DdosProtector，SYN Flood 检测）
- **新增 `perf/` 子模块**（2 文件）：
  - `perf/mod.rs` — 模块声明
  - `perf/benchmark.rs` — 吞吐基准测试框架
- **修改 lib.rs**：添加 `pub mod security; pub mod perf;` + VERSION 升至 "0.30.0"

### v0.30.1 — 蜂窝通信模块（新 crate）

- **新增 `crates/drivers/cellular/` crate**（eneros-cellular）：
  - `src/lib.rs` — 模块声明 + re-exports + VERSION
  - `src/error.rs` — CellularError（5 变体）
  - `src/at_command.rs` — AT 命令封装（AtCommand + AtParser + AtResponse）
  - `src/ppp.rs` — PPP 拨号协议（PppState 状态机 + PppFrame 帧 + PppDevice smoltcp 适配器）
  - `src/modem.rs` — CellularModem 驱动 + CellularDriver trait + SignalStrength
- **依赖**：`eneros-hal`（HalSerial trait）、`smoltcp`（Device trait）、`alloc`
- **修改根 Cargo.toml**：members 添加 `"crates/drivers/cellular"`

### v0.30.2 — 双网冗余与切换

- **在 cellular crate 中新增 3 文件**：
  - `src/redundancy.rs` — RedundancyManager + LinkState + LinkType
  - `src/heartbeat.rs` — HeartbeatMonitor（心跳发送/超时检测）
  - `src/failover.rs` — FailoverManager（切换状态机 + 防抖回切 + 事件回调）
- **修改 cellular/src/lib.rs**：添加新模块声明

### 共通变更

- **版本标识更新**：根 Cargo.toml → "0.30.0"、Makefile、ci.yml、gate.rs
- **新增文档**：3 个设计文档（docs/drivers/）
- **新增配置**：3 个配置模板（configs/）
- **BREAKING**：无（v0.30.0 纯新增模块；v0.30.1/v0.30.2 新 crate）

## Impact

- **Affected specs**: 解锁 v0.31.0（国密算法）、v0.57.0（降级规则联动）、Phase 2 mTLS
- **Affected code**:
  - v0.30.0: 修改 `crates/drivers/net/src/lib.rs`、`Cargo.toml`；新增 `security/` + `perf/` 子模块
  - v0.30.1: 新增 `crates/drivers/cellular/` crate；修改根 `Cargo.toml`
  - v0.30.2: 在 cellular crate 中新增 3 文件
  - **不修改**：v0.27.0~v0.29.0 的现有源文件（Surgical Changes）

## 设计决策（Karpathy 原则应用）

### 1. Think Before Coding — 硬件依赖与测试策略

**问题**：v0.30.1 蜂窝驱动和 v0.30.2 双网冗余都涉及真实硬件（4G/5G modem、双网链路）。

**决策**：
- **软件可实现 + Mock 测试**：AT 命令解析、PPP 状态机、信号解析、心跳超时判定、切换状态机、防抖逻辑
- **集成测试延后**：真实 modem 拨号、断网重连、长时间保活、拔网线触发切换 — 需真实硬件
- **理由**：蓝图 §6 测试计划已明确区分单元测试和集成测试，单元测试用 Mock，集成测试需硬件

### 2. Simplicity First — 最小实现

**决策**：
- **PacketInfo 简化结构体**：防火墙只需检查包元数据（src_ip/dst_ip/protocol/ports），不引入 smoltcp 完整包解析
- **BTreeMap 替代 HashMap**：no_std 合规（蓝图代码用了 HashMap，需修正）
- **PPP 最小状态机**：实现 LCP/IPCP 状态迁移 + HDLC 基础帧，不实现完整 PPP 协议栈（PAP/CHAP/MP 等）
- **PppDevice smoltcp 适配器**：定义接口，实际网络数据通道需硬件验证
- **双网冗余状态机**：4 状态（PrimaryActive / BackupActive / Switching / Recovering），不引入复杂事件系统

### 3. Surgical Changes — 不修改现有源文件

- v0.30.0: 仅修改 `lib.rs` 和 `Cargo.toml`，不修改 v0.27.0~v0.29.0 的 18 个源文件
- v0.30.1: 新建 cellular crate，不修改现有 crate
- v0.30.2: 在 cellular crate 中添加文件，不修改 v0.30.1 已有文件（仅修改 lib.rs 添加模块声明）

### 4. Goal-Driven Execution — 测试覆盖

**验证标准**：
1. `cargo test -p eneros-net` 通过（v0.29.0 的 370 + v0.30.0 新增 60+ = 430+ tests）
2. `cargo test -p eneros-cellular` 通过（v0.30.1 + v0.30.2 新增 80+ tests）
3. aarch64 交叉编译通过（eneros-net + eneros-cellular）
4. workspace 回归测试 PASS
5. cargo fmt / clippy 无 warning

## ADDED Requirements

### Requirement: v0.30.0 防火墙规则引擎

系统 SHALL 提供 `Firewall` 结构体管理防火墙规则和默认策略。

```rust
pub struct Firewall {
    rules: Vec<FirewallRule>,
    default_policy: FirewallPolicy,
    conn_tracker: ConnectionTracker,
}

pub struct FirewallRule {
    pub action: FirewallAction,
    pub src_ip: Option<Ipv4Cidr>,
    pub dst_port: Option<u16>,
    pub protocol: Option<IpProtocol>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FirewallAction { Allow, Drop, Reject }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FirewallPolicy { AllowAll, DropAll }
```

**注**：`IpProtocol` 使用 `smoltcp::wire::IpProtocol` 类型别名。`Vec` 来自 `alloc::vec::Vec`。

#### Scenario: 规则匹配放行
- **WHEN** 防火墙有 Allow 规则匹配源 IP 192.168.1.0/24
- **AND** check_connection(192.168.1.100) 被调用
- **THEN** 返回 FirewallAction::Allow

#### Scenario: 默认策略拒绝
- **WHEN** 防火墙无匹配规则且默认策略为 DropAll
- **AND** check_connection(10.0.0.1) 被调用
- **THEN** 返回 FirewallAction::Drop

### Requirement: v0.30.0 连接跟踪与速率限制

系统 SHALL 提供 `ConnectionTracker` 跟踪每 IP 连接数和总连接数。

```rust
pub struct ConnectionTracker {
    connections: BTreeMap<Ipv4Addr, ConnInfo>,
    max_per_ip: u32,
    max_total: u32,
    total: u32,
}

pub struct ConnInfo {
    pub count: u32,
    pub last_connect: u64,
    pub rate_window: u64,
    pub rate_count: u32,
}

pub struct RateLimit {
    pub max_connections: u32,
    pub max_rate_per_sec: u32,
}
```

**注**：使用 `BTreeMap` 替代蓝图的 `HashMap`（no_std 合规）。

#### Scenario: 超过单 IP 连接限制
- **WHEN** max_per_ip=10 且某 IP 已有 10 个连接
- **AND** try_connect(ip, now) 被调用
- **THEN** 返回 false（拒绝新连接）

#### Scenario: 速率限制
- **WHEN** 某 IP 在 1 秒内发起超过 max_rate_per_sec 次连接
- **THEN** is_rate_limited(ip) 返回 true

### Requirement: v0.30.0 DDoS 防护

系统 SHALL 提供 `DdosProtector` 检测 SYN Flood 攻击。

```rust
pub struct DdosProtector {
    syn_tracker: BTreeMap<Ipv4Addr, SynInfo>,
    syn_rate_threshold: u32,
    window_ms: u64,
}

pub struct SynInfo {
    pub syn_count: u32,
    pub window_start: u64,
}

impl DdosProtector {
    pub fn check_syn(&mut self, src: Ipv4Addr, now: u64) -> bool;  // true=允许, false=疑似攻击
    pub fn is_under_attack(&self) -> bool;
}
```

#### Scenario: SYN Flood 检测
- **WHEN** 某 IP 在 window_ms 内发送超过 syn_rate_threshold 个 SYN
- **THEN** check_syn 返回 false（拒绝后续 SYN）

### Requirement: v0.30.0 性能基准测试

系统 SHALL 提供 `BenchmarkSuite` 框架用于网络性能测试。

```rust
pub struct BenchmarkResult {
    pub throughput_kbps: u32,
    pub latency_us: u32,
    pub packets_per_sec: u32,
}

pub struct BenchmarkSuite {
    results: Vec<(alloc::string::String, BenchmarkResult)>,
}

impl BenchmarkSuite {
    pub fn new() -> Self;
    pub fn run_firewall_benchmark(&mut self, iterations: u32) -> BenchmarkResult;
    pub fn run_connection_benchmark(&mut self, iterations: u32) -> BenchmarkResult;
    pub fn results(&self) -> &[(alloc::string::String, BenchmarkResult)];
}
```

#### Scenario: 防火墙检查延迟
- **WHEN** 运行防火墙基准测试
- **THEN** 返回 BenchmarkResult 含 latency_us（目标 < 1μs，需实机验证）

### Requirement: v0.30.1 AT 命令封装

系统 SHALL 提供 AT 命令编码/解码器。

```rust
pub struct AtCommand {
    pub cmd: alloc::string::String,
    pub args: alloc::vec::Vec<alloc::string::String>,
    pub timeout_ms: u32,
}

pub enum AtResponse {
    Ok(alloc::string::String),
    Error(alloc::string::String),
    Timeout,
}

pub struct AtParser;

impl AtParser {
    pub fn parse_response(raw: &str) -> Result<AtResponse, CellularError>;
    pub fn parse_signal(raw: &str) -> Result<SignalStrength, CellularError>;
    pub fn encode(cmd: &AtCommand) -> alloc::string::String;
}
```

#### Scenario: AT 命令编码
- **WHEN** 创建 AtCommand{cmd:"AT+CSQ", args:[], timeout_ms:1000}
- **THEN** AtParser::encode 返回 "AT+CSQ\r\n"

#### Scenario: 信号解析
- **WHEN** 解析 "+CSQ: 23,0"
- **THEN** 返回 SignalStrength{rssi:23, ber:0, network_type:NetworkType::Unknown}

### Requirement: v0.30.1 PPP 拨号协议

系统 SHALL 提供 PPP 状态机和基础帧结构。

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PppState {
    Closed,
    Establishing,   // LCP 协商中
    Authenticating, // PAP/CHAP 认证
    Networking,     // IPCP 协商中
    Connected,      // IP 已获取
    Terminating,
}

pub struct PppFrame {
    pub protocol: u16,
    pub data: alloc::vec::Vec<u8>,
}

pub struct PppStateMachine {
    state: PppState,
    retry_count: u32,
    max_retries: u32,
}

impl PppStateMachine {
    pub fn new() -> Self;
    pub fn state(&self) -> PppState;
    pub fn on_lcp_config_ack(&mut self) -> Result<(), CellularError>;
    pub fn on_auth_success(&mut self) -> Result<(), CellularError>;
    pub fn on_ipcp_config_ack(&mut self, ip: Ipv4Addr) -> Result<(), CellularError>;
    pub fn on_error(&mut self) -> Result<(), CellularError>;
    pub fn terminate(&mut self);
}
```

#### Scenario: PPP 状态迁移
- **WHEN** PppStateMachine 在 Establishing 状态
- **AND** on_lcp_config_ack() 被调用
- **THEN** 状态迁移到 Authenticating

### Requirement: v0.30.1 CellularModem 驱动

系统 SHALL 提供 CellularModem 驱动封装 AT 命令和 PPP 拨号。

```rust
pub trait CellularDriver {
    fn send_at(&mut self, cmd: &AtCommand) -> Result<AtResponse, CellularError>;
    fn dial(&mut self, apn: &str) -> Result<Ipv4Addr, CellularError>;
    fn hang_up(&mut self) -> Result<(), CellularError>;
    fn signal(&mut self) -> Result<SignalStrength, CellularError>;
}

pub struct CellularModem<S: HalSerial> {
    serial: S,
    at_parser: AtParser,
    ppp: PppStateMachine,
    apn: alloc::string::String,
    retry_config: RetryConfig,
}

pub struct SignalStrength {
    pub rssi: i8,
    pub ber: u8,
    pub network_type: NetworkType,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NetworkType { Unknown, Gsm, Wcdma, Lte, Nr5g }

pub struct RetryConfig {
    pub max_retries: u32,
    pub retry_interval_ms: u64,
}
```

#### Scenario: 拨号连接
- **WHEN** modem.dial("internet") 被调用
- **THEN** 依次执行 AT 命令初始化 → PPP 拨号 → 返回分配的 IP

### Requirement: v0.30.2 心跳监测

系统 SHALL 提供 HeartbeatMonitor 监测链路活性。

```rust
pub struct HeartbeatMonitor {
    interval_ms: u64,
    timeout_ms: u64,
    last_heartbeat: u64,
    missed_count: u32,
    max_missed: u32,
}

impl HeartbeatMonitor {
    pub fn new(interval_ms: u64, timeout_ms: u64, max_missed: u32) -> Self;
    pub fn on_heartbeat(&mut self, now: u64);
    pub fn check_timeout(&mut self, now: u64) -> bool;  // true=超时
    pub fn is_alive(&self) -> bool;
}
```

#### Scenario: 心跳超时
- **WHEN** max_missed=3 且连续 3 次心跳超时
- **THEN** check_timeout 返回 true 且 is_alive 返回 false

### Requirement: v0.30.2 故障切换管理

系统 SHALL 提供 FailoverManager 管理主备链路切换。

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LinkType { Ethernet, Cellular }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FailoverState {
    PrimaryActive,
    BackupActive,
    Switching,
    Recovering,
}

pub enum FailoverEvent {
    PrimaryDown,
    PrimaryUp,
    SwitchCompleted,
    RecoveryCompleted,
}

pub struct FailoverManager {
    state: FailoverState,
    active: LinkType,
    heartbeat_primary: HeartbeatMonitor,
    heartbeat_backup: HeartbeatMonitor,
    failover_count: u32,
    recovery_delay_ms: u64,
    last_failover_time: u64,
    callback: Option<fn(FailoverEvent)>,
}

impl FailoverManager {
    pub fn new(recovery_delay_ms: u64) -> Self;
    pub fn on_event(&mut self, event: FailoverEvent, now: u64) -> Result<LinkType, FailoverError>;
    pub fn current_active(&self) -> LinkType;
    pub fn state(&self) -> FailoverState;
    pub fn force_switch(&mut self, target: LinkType, now: u64) -> Result<(), FailoverError>;
    pub fn register_callback(&mut self, cb: fn(FailoverEvent));
}
```

#### Scenario: 主链路故障切换
- **WHEN** state=PrimaryActive 且收到 PrimaryDown 事件
- **THEN** 状态迁移到 Switching，active 变为 Cellular

#### Scenario: 防抖回切
- **WHEN** state=BackupActive 且收到 PrimaryUp 事件
- **THEN** 状态迁移到 Recovering，等待 recovery_delay_ms 后才回切

### Requirement: v0.30.2 双网冗余管理器

系统 SHALL 提供 RedundancyManager 统一管理主备链路。

```rust
pub struct RedundancyManager {
    primary_link: LinkState,
    backup_link: LinkState,
    active: LinkType,
    failover_mgr: FailoverManager,
}

pub struct LinkState {
    pub link_type: LinkType,
    pub is_up: bool,
    pub ipv4_addr: Option<Ipv4Addr>,
}

impl RedundancyManager {
    pub fn new() -> Self;
    pub fn set_primary_status(&mut self, up: bool, now: u64);
    pub fn set_backup_status(&mut self, up: bool, now: u64);
    pub fn current_active(&self) -> LinkType;
    pub fn failover_count(&self) -> u32;
}
```

## 错误类型

### SecurityError (v0.30.0)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityError {
    BlockedByFirewall,
    RateLimited,
    ConnectionLimitExceeded,
    SuspiciousActivity,
}
```

### CellularError (v0.30.1)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellularError {
    NoSimCard,
    NoSignal,
    DialFailed,
    AtCommandTimeout,
    PppNegotiationFailed,
}
```

### FailoverError (v0.30.2)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverError {
    NoBackupAvailable,
    SwitchInProgress,
    HeartbeatTimeout,
    InvalidState,
}
```

## 性能目标（蓝图 §6.3）

| 指标 | 目标 | 验证方式 |
|------|------|---------|
| 防火墙检查延迟 | < 1μs | QEMU/实机验证 |
| 连接数限制 | ≥ 64 | Mock 测试 |
| 蜂窝拨号时间 | < 30s | 实机验证 |
| 双网切换时间 | < 5s | 实机验证 |
| 心跳误判率 | < 0.1% | Mock 测试 |

**注**：性能目标需真实硬件/QEMU 验证，v0.30.x 仅交付软件实现 + mock 测试，性能验证延后。

## no_std 合规

- 所有新文件遵循 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 使用 `alloc::collections::BTreeMap`（不用 HashMap）
- 使用 `alloc::vec::Vec` / `alloc::string::String`
- 无 `use std::*`
- cellular crate 依赖 `eneros-hal`（HalSerial）和 `smoltcp`（Device trait）

## 内存预算声明（§5.6）

| 组件 | 预估内存 | 说明 |
|------|---------|------|
| Firewall 规则表 | ~4 KB | 64 条规则 × 64 字节 |
| ConnectionTracker | ~8 KB | 64 连接 × 128 字节 |
| DdosProtector | ~4 KB | 64 IP × 64 字节 |
| CellularModem | ~2 KB | AT 缓冲 + PPP 状态 |
| RedundancyManager | ~1 KB | 状态 + 心跳监测 |
| **总计** | **≤ 20 KB** | 不含 TCP 缓冲（v0.29.0 已计） |

**OOM 策略**：缩减规则表大小、关闭非关键连接、降级到 L1（Solver-only 路径）。

## 文件布局

```
crates/drivers/net/
├── Cargo.toml                    # version → "0.30.0"
└── src/
    ├── lib.rs                    # 添加 pub mod security; pub mod perf; + VERSION="0.30.0"
    ├── [现有文件不修改]           # v0.27.0~v0.29.0 的 18 个源文件
    ├── security/                 # ★ v0.30.0 新增
    │   ├── mod.rs
    │   ├── firewall.rs
    │   ├── rate_limit.rs
    │   └── ddos.rs
    └── perf/                     # ★ v0.30.0 新增
        ├── mod.rs
        └── benchmark.rs

crates/drivers/cellular/          # ★ v0.30.1 + v0.30.2 新增 crate
├── Cargo.toml
└── src/
    ├── lib.rs                    # 模块声明 + re-exports + VERSION="0.30.0"
    ├── error.rs                  # CellularError + FailoverError
    ├── at_command.rs             # v0.30.1: AT 命令
    ├── ppp.rs                    # v0.30.1: PPP 协议
    ├── modem.rs                  # v0.30.1: CellularModem 驱动
    ├── redundancy.rs             # v0.30.2: 双网冗余管理器
    ├── heartbeat.rs              # v0.30.2: 心跳监测
    └── failover.rs               # v0.30.2: 故障切换

docs/drivers/
├── net-security-design.md        # v0.30.0 设计文档
├── cellular-modem-design.md      # v0.30.1 设计文档
└── dual-network-redundancy.md    # v0.30.2 设计文档

configs/
├── net-security.toml             # v0.30.0 配置
├── cellular.toml                 # v0.30.1 配置
└── failover.toml                 # v0.30.2 配置
```

## 依赖

- **v0.30.0**: 无新增外部依赖（复用 smoltcp 类型）
- **v0.30.1**: `eneros-hal`（HalSerial）、`smoltcp`（Device trait）— 通过 workspace 依赖
- **v0.30.2**: 复用 v0.30.1 的 cellular crate

## 偏差声明

1. **HashMap → BTreeMap**：蓝图代码使用 `HashMap`，v0.30.x 改用 `BTreeMap`（no_std 合规，避免 hashbrown 依赖）
2. **PPP 最小实现**：蓝图描述了完整 PPP 协商，v0.30.1 仅实现状态机 + 基础帧结构，完整 PPP 协议栈（LCP/IPCP/PAP/CHAP 完整报文）需硬件验证时完善
3. **PppDevice 接口定义**：定义 smoltcp Device trait 适配器接口，实际数据通道需硬件验证
4. **集成测试延后**：v0.30.1/v0.30.2 的集成测试（真实 modem、拔网线）需硬件环境，v0.30.x 仅交付单元测试
