# v0.80.0 — TSN 网络配置（IEEE 802.1Qbv）Spec

## Why

EnerOS Phase 2（P2-B）在 v0.79.0 gPTP 时间同步之上建立**时间感知整形（Time-Aware Shaper, TAS）调度层**，为 Agent 控制命令、GOOSE 跳闸、SV 采样等关键流量预留确定性时隙，确保端到端时延可预测、抖动可控。本版本是 v0.81.0 端到端时延验证的基础。

本版本交付**纯 Rust 类型与算法骨架**（无真实 netlink/taprio 下发、无硬件网卡集成），通过 `NicApplier` trait + `MockNicApplier` 注入的方式验证门控列表闭合性、流量分类、下一窗口计算逻辑。真实网卡下发延后到具备 TSN 硬件环境的集成测试。

## What Changes

- **扩展 crate**：`crates/protocols/tsn-time/`（v0.79.0 已建，本版本新增 3 个源文件）
  - 新增：`src/tas.rs` — TAS 核心类型与调度算法
  - 新增：`src/stream.rs` — Stream 过滤数据类型（最小骨架）
  - 新增：`src/config_loader.rs` — 配置构造器
  - 修改：`src/lib.rs` — 新增 `pub mod` 声明 + `pub use` 导出 + T26~T52 测试 + D1~D19 偏差表
  - 修改：`Cargo.toml` — 版本 `0.79.0` → `0.80.0`
- **新增类型**：`TrafficClass`（8 变体）/ `Packet` / `GateState` / `GateControlList` / `TasPort` / `TasConfig` / `TasError` / `TasScheduler` / `NicApplier` trait / `MockNicApplier` / `StreamId` / `StreamFilter`
- **新增算法**：`TasScheduler::new` / `validate_schedule` / `classify_packet` / `next_gate_window` / `apply_to_nic` / `GateControlList::increment_cycle` / `build_tas_config`
- **新增配置**：`configs/tas.toml`（门控列表模板）
- **新增文档**：`docs/protocols/tsn-qbv-design.md`（12 章节 + 2 Mermaid 图 + D1~D19 偏差声明表）
- **版本同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 由 `0.79.0` → `0.80.0`
- **workspace members**：`"crates/protocols/tsn-time"` 已在 v0.79.0 注册，本版本不变

## Impact

- **Affected specs**：v0.79.0 `eneros-tsn-time` crate（仅扩展，不修改 v0.79.0 类型签名）；v0.75.0~v0.78.0 `eneros-agent-bus-dds` 类型签名不变
- **Affected code**：
  - 新增：`crates/protocols/tsn-time/src/tas.rs`
  - 新增：`crates/protocols/tsn-time/src/stream.rs`
  - 新增：`crates/protocols/tsn-time/src/config_loader.rs`
  - 新增：`configs/tas.toml`
  - 新增：`docs/protocols/tsn-qbv-design.md`
  - 修改：`crates/protocols/tsn-time/src/lib.rs`（新增模块声明 + 导出 + 测试 + 偏差表）
  - 修改：`crates/protocols/tsn-time/Cargo.toml`（版本号）
  - 修改：根 `Cargo.toml`（version）
  - 修改：`Makefile`（version）
  - 修改：`.github/workflows/ci.yml`（version）
  - 修改：`ci/src/gate.rs`（clippy/test 段注释更新 `eneros-tsn-time` 类型列表至 v0.80.0）
- **后续解锁**：v0.81.0（TSN 网络驱动与确定性时延验证 — 全链路时延测试）

## ADDED Requirements

### Requirement: TrafficClass 流量分类枚举

系统 SHALL 提供 `TrafficClass` 枚举（`#[repr(u8)]`），含 8 个变体对应 802.1Q PCP 等级：

| 变体 | code | 含义 |
|------|------|------|
| `Be(0)` | 0 | Best Effort |
| `BK(1)` | 1 | Background |
| `EE(2)` | 2 | Energy Efficiency（Agent 状态） |
| `CA(3)` | 3 | Critically Auth（Agent 命令） |
| `VO(4)` | 4 | Voice（GOOSE） |
| `VI(5)` | 5 | Video（SV） |
| `NC(6)` | 6 | Network Control（gPTP） |
| `ST(7)` | 7 | Strategic（保留） |

实现 `code(&self) -> u8` 与 `from_code(u8) -> Option<Self>` 方法，并派生 `Debug, Clone, Copy, PartialEq, Eq, Hash`。

#### Scenario: TrafficClass 编码与反编码
- **WHEN** 调用 `TrafficClass::NC(6).code()`
- **THEN** 返回 `6`
- **WHEN** 调用 `TrafficClass::from_code(3)`
- **THEN** 返回 `Some(TrafficClass::CA(3))`
- **WHEN** 调用 `TrafficClass::from_code(8)`
- **THEN** 返回 `None`

### Requirement: Packet 数据包描述符

系统 SHALL 提供 `Packet` 结构体（最小数据集，无真实抓包），含 `ethertype: u16` / `dscp: u8` / `pcp: u8` 字段，并实现 `is_ptp() -> bool`（ethertype == 0x88F7）、`is_goose() -> bool`（ethertype == 0x88B8）、`is_sv() -> bool`（ethertype == 0x88BA）方法。

#### Scenario: PTP 数据包识别
- **WHEN** 构造 `Packet { ethertype: 0x88F7, dscp: 0, pcp: 0 }`
- **THEN** `is_ptp()` 返回 `true`，`is_goose()` / `is_sv()` 返回 `false`

### Requirement: GateState 与 GateControlList

系统 SHALL 提供：

- `GateState { duration: Duration, gates: u8 }`（`gates` 第 i 位 = 1 表示 TCi 开放）
- `GateControlList { entries: Vec<GateState>, cycle_count: u32 }`
- `GateControlList::new(entries: Vec<GateState>) -> Self`（`cycle_count = 0`）
- `GateControlList::increment_cycle(&mut self)`（`cycle_count` 自增 1）

#### Scenario: GCL 周期计数
- **WHEN** 构造 `GateControlList::new(entries)` 后调用 `increment_cycle()` 3 次
- **THEN** `cycle_count == 3`

### Requirement: TasConfig 与 TasPort

系统 SHALL 提供 `TasConfig` 结构体：

```rust
pub struct TasConfig {
    pub cycle_us: u64,
    pub base_time_s: u64,
    pub schedule: Vec<TasScheduleEntry>,
    pub port_count: u8,
}

pub struct TasScheduleEntry {
    pub duration_us: u64,
    pub gate_mask: u8,
}
```

`TasConfig::default()` 返回 `cycle_us = 1_000_000`（1ms 周期）、`base_time_s = 0`、`schedule = Vec::new()`、`port_count = 1`。

`TasPort { port_id: u8, applied: bool }`，`TasPort::new(port_id)` 初始化 `applied = false`。

#### Scenario: TasConfig 默认值
- **WHEN** 调用 `TasConfig::default()`
- **THEN** `cycle_us == 1_000_000`、`port_count == 1`、`schedule.is_empty()`

### Requirement: TasError 错误枚举

系统 SHALL 提供 `TasError` 枚举：

```rust
pub enum TasError {
    ScheduleGap { expected: Duration, actual: Duration },
    TooShort(Duration),
    NicApplyFailed,
    InvalidConfig,
}
```

派生 `Debug, Clone, Copy, PartialEq, Eq`。

### Requirement: TasScheduler 调度器

系统 SHALL 提供 `TasScheduler` 结构体：

```rust
pub struct TasScheduler {
    pub ports: Vec<TasPort>,
    pub base_time: PtpTime,
    pub cycle_time: Duration,
    pub config: GateControlList,
}
```

实现以下方法：

- `TasScheduler::new(config: &TasConfig) -> Self`（D7：使用 `PtpTime::new(config.base_time_s, 0)`，不修改 v0.79.0 的 `clock.rs`）
- `validate_schedule(&self) -> Result<(), TasError>`：所有 entry duration 之和 == cycle_time（否则 `ScheduleGap`）；任一 entry duration < 5µs（否则 `TooShort`）
- `classify_packet(&self, pkt: &Packet) -> TrafficClass`：PTP → NC、GOOSE → VO、SV → VI；否则按 DSCP 分段（0-7 → BE、8-15 → BK、24-31 → EE、40-47 → CA、其他 → BE）
- `next_gate_window(&self, tc: TrafficClass) -> Duration`：遍历 GCL 找首个 `gates >> tc.code() & 1 == 1` 的 entry，返回从周期起点到该 entry 的累计 duration；若 TC 永未开放，返回 `cycle_time`（全周期等待）
- `apply_to_nic(&mut self, applier: &mut dyn NicApplier, iface: &str) -> Result<(), TasError>`：先调用 `validate_schedule()`，再调用 `applier.apply(iface, &self.config)`；任一端口下发失败 → `NicApplyFailed`

#### Scenario: 调度闭合性校验通过
- **WHEN** 构造 GCL entries 总 duration == cycle_time 且每条 >= 5µs
- **THEN** `validate_schedule()` 返回 `Ok(())`

#### Scenario: 调度不闭合
- **WHEN** entries 总 duration = 800µs 但 cycle_time = 1000µs
- **THEN** `validate_schedule()` 返回 `Err(TasError::ScheduleGap { expected: 1000µs, actual: 800µs })`

#### Scenario: 门控时间过短
- **WHEN** 任一 entry duration = 3µs（< 5µs）
- **THEN** `validate_schedule()` 返回 `Err(TasError::TooShort(3µs))`

#### Scenario: PTP 数据包分类
- **WHEN** 调用 `classify_packet(&Packet { ethertype: 0x88F7, dscp: 0, pcp: 0 })`
- **THEN** 返回 `TrafficClass::NC(6)`

#### Scenario: Agent 命令 DSCP 分类
- **WHEN** 调用 `classify_packet(&Packet { ethertype: 0x0800, dscp: 46, pcp: 0 })`
- **THEN** 返回 `TrafficClass::CA(3)`（DSCP 40-47 段）

#### Scenario: 下一窗口计算
- **WHEN** GCL 为 `[(50µs, 0b01000000), (200µs, 0b00001000)]`（TC6 第一窗口、TC3 第二窗口）
- **THEN** `next_gate_window(NC)` 返回 `0µs`（首条即匹配）、`next_gate_window(CA)` 返回 `50µs`

### Requirement: NicApplier Trait 与 MockNicApplier

系统 SHALL 提供 NIC 下发抽象：

```rust
pub trait NicApplier {
    fn apply(&mut self, iface: &str, config: &GateControlList) -> Result<(), TasError>;
}

pub struct MockNicApplier {
    pub applied: Vec<(String, u32)>,  // (iface, entry_count)
    pub fail: bool,
}
```

`MockNicApplier::new()` 初始化 `applied = Vec::new(), fail = false`。`apply()` 在 `fail = false` 时追加 `(iface.to_string(), config.entries.len() as u32)` 到 `applied` 并返回 `Ok(())`；`fail = true` 时返回 `Err(TasError::NicApplyFailed)`。

#### Scenario: Mock 下发成功
- **WHEN** 调用 `apply_to_nic(&mut mock, "eth0")` 且 `mock.fail = false`
- **THEN** 返回 `Ok(())`，`mock.applied.len() == 1`，`mock.applied[0].0 == "eth0"`

#### Scenario: 调度非法时不下发
- **WHEN** 调度不闭合时调用 `apply_to_nic(&mut mock, "eth0")`
- **THEN** 返回 `Err(ScheduleGap{..})`，`mock.applied.is_empty()`

### Requirement: StreamId 与 StreamFilter（最小骨架）

系统 SHALL 提供 `StreamId(pub u32)` newtype（派生 `Debug, Clone, Copy, PartialEq, Eq, Hash`，实现 `Display`）与 `StreamFilter { stream_id: StreamId, gate_id: u8, priority: u8 }` 结构体（派生 `Debug, Clone, PartialEq, Eq`）。**不实现**真实 802.1Qci per-stream 过滤逻辑（延后到后续版本）。

#### Scenario: StreamId 构造
- **WHEN** 调用 `StreamId::new(42)`
- **THEN** `format!("{}", stream_id)` 输出 `"42"`

### Requirement: build_tas_config 配置构造器

`config_loader.rs` SHALL 提供 `build_tas_config(cycle_us: u64, base_time_s: u64, entries: Vec<TasScheduleEntry>, port_count: u8) -> TasConfig` 构造器，直接组装 `TasConfig` 字段。**不实现** TOML 解析（依赖 eneros-config v0.26.0 上层加载，本版本仅提供纯 Rust 构造器）。

#### Scenario: 构造器组装
- **WHEN** 调用 `build_tas_config(1000, 100, vec![], 2)`
- **THEN** 返回的 `TasConfig` 各字段与传入参数一致

## MODIFIED Requirements

### Requirement: eneros-tsn-time crate（v0.79.0 → v0.80.0 扩展）

v0.79.0 创建的 `crates/protocols/tsn-time/` crate 在 v0.80.0 扩展：

- crate 版本 `0.79.0` → `0.80.0`
- crate description 更新：从 "gPTP 时间同步" → "gPTP 时间同步 + TSN 802.1Qbv 调度"
- 新增 3 个 `pub mod`：`tas` / `stream` / `config_loader`
- 新增 `pub use` 导出：`tas::{TrafficClass, Packet, GateState, GateControlList, TasPort, TasConfig, TasScheduleEntry, TasError, TasScheduler, NicApplier, MockNicApplier}` / `stream::{StreamId, StreamFilter}` / `config_loader::build_tas_config`
- 测试数量：T1~T25（v0.79.0）→ T1~T52（新增 T26~T52 共 27 个测试）
- v0.79.0 的 `clock.rs` / `port.rs` / `bmca.rs` / `gptp.rs` 文件**不变**（Surgical Changes 原则）

## REMOVED Requirements

无（纯扩展，不删除任何 v0.79.0 接口）。

## no_std 合规

本 crate 沿用 v0.79.0 的 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`，仅使用 `core::*` 与 `alloc::*`：

- 使用 `core::time::Duration`（no_std 可用，`Sum` trait 在 core 中定义）
- 使用 `alloc::vec::Vec`、`alloc::string::{String, ToString}`
- **不使用** `std::os::unix::netlink`、`std::fs`、`std::thread`（D6：通过 `NicApplier` trait 抽象，无真实系统调用）
- **不使用** `nix` / `socketcan` / `pcap` 等 Linux 特定 crate
- **不使用** `toml` / `serde` / `serde_json`（D15：配置构造器为纯 Rust 函数）
- 不调用 `panic!` / `todo!` / `unimplemented!`，不含 `unsafe` 块
- 无 `Send + Sync` bound（沿用 v0.79.0 单线程先例）
