# v0.79.0 — gPTP 时间同步（IEEE 802.1AS）Spec

## Why

EnerOS Phase 2（P2-B）需要实现跨 Edge Box 的时间同步，作为联邦多机时间一致性的基础。gPTP（IEEE 802.1AS）通过主从时钟 + 硬件时间戳机制，将时钟差控制在 < 1ms，是后续 v0.80.0 TSN 调度、v0.53.0 SOE 事件顺序记录、分布式追踪的前提条件。

本版本交付**纯 Rust 类型与算法骨架**（无真实网络 I/O、无硬件时间戳集成），通过 Mock 注入消息的方式验证 BMCA 选举、偏移计算、低通滤波、时钟调整逻辑的正确性。真实网卡集成与硬件时间戳延后到具备硬件环境后的集成测试。

## What Changes

- **新增 crate**：`crates/protocols/tsn-time/`（4 个源文件：`lib.rs` / `clock.rs` / `port.rs` / `bmca.rs` / `gptp.rs`）
- **新增类型**：`ClockIdentity`（EUI-64）/ `MacAddr` / `PtpTime` / `Port` / `PortRole` / `PortState` / `AnnounceMessage` / `BmcaResult` / `SyncMessage` / `FollowUpMessage` / `GptpConfig` / `GptpClock`
- **新增算法**：`compare_priority()`（BMCA 优先级比较）/ `run_bmca()`（最佳主时钟选举）/ `handle_sync()`（偏移计算 + 低通滤波）/ `handle_follow_up()`（精确时间戳更新）/ `adjust_clock()`（小幅渐进调整）/ `compute_offset()` / `current_time()` / `to_announce()`
- **新增配置**：`configs/gptp.toml`（主时钟优先级、端口角色模板）
- **新增文档**：`docs/protocols/gptp-sync-design.md`（12 章节 + 2 Mermaid 图 + D1~D14 偏差声明表）
- **版本同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 由 `0.78.0` → `0.79.0`
- **workspace members**：根 `Cargo.toml` 新增 `"crates/protocols/tsn-time"`

## Impact

- **Affected specs**：无（纯新增 crate；v0.75.0~v0.78.0 `eneros-agent-bus-dds` 类型签名不变）
- **Affected code**：
  - 新增：`crates/protocols/tsn-time/` 全部源文件
  - 新增：`configs/gptp.toml`
  - 新增：`docs/protocols/gptp-sync-design.md`
  - 修改：根 `Cargo.toml`（version + members）
  - 修改：`Makefile`（version）
  - 修改：`.github/workflows/ci.yml`（version）
  - 修改：`ci/src/gate.rs`（clippy/test 段注释新增 `eneros-tsn-time v0.79.0` 与类型列表）
- **后续解锁**：v0.80.0（TSN 调度）、v0.92.0（VPP 聚合响应 < 30s 依赖时间对齐）、v0.117.0（审计哈希链时间戳一致性）

## ADDED Requirements

### Requirement: gPTP 时钟数据结构

系统 SHALL 提供 `ClockIdentity`（EUI-64，`pub [u8; 8]` newtype）、`MacAddr`（`pub [u8; 6]` newtype）、`PtpTime`（`seconds: u64` + `nanos: u32`）类型，并实现 `PtpTime::to_ns() -> i128`、`PtpTime::add_ns(&mut self, ns: i64)`、`PtpTime::diff_ns(&self, other: &PtpTime) -> i64`。

#### Scenario: PtpTime 纳秒换算
- **WHEN** 构造 `PtpTime { seconds: 2, nanos: 500_000_000 }`
- **THEN** `to_ns()` 返回 `2_500_000_000`

#### Scenario: PtpTime 负向调整
- **WHEN** 对 `PtpTime { seconds: 1, nanos: 0 }` 调用 `add_ns(-500_000_000)`
- **THEN** 结果为 `PtpTime { seconds: 0, nanos: 500_000_000 }`

### Requirement: 端口角色与状态

系统 SHALL 提供 `PortRole`（`Master` / `Slave` / `Passive` / `Disabled`）与 `PortState`（`Initializing` / `Listening` / `Master` / `Slave` / `Passive`）枚举，并实现 `Display`。`Port` 结构体 SHALL 含 `port_id: u16` / `role: PortRole` / `state: PortState` / `mac: MacAddr` / `hw_timestamping: bool` 字段。

#### Scenario: Port 构造
- **WHEN** 调用 `Port::new(port_id, mac, hw_timestamping)`
- **THEN** `role == PortRole::Disabled`、`state == PortState::Initializing`

### Requirement: GptpConfig 与 GptpClock

系统 SHALL 提供 `GptpConfig`（含 `priority1: u8` / `priority2: u8` / `ports: Vec<Port>` 字段，实现 `Default`），以及 `GptpClock` 结构体（含蓝图 §4.1 全部 11 字段 + `sync_interval: Duration` + `frequency_offset: i64` + `last_jump_ns: Option<i64>`）。

#### Scenario: GptpClock 初始化
- **WHEN** 调用 `GptpClock::new(identity, &config)`
- **THEN** `steps_removed == 0`、`offset == 0`、`grandmaster_identity == identity`、`frequency_offset == 0`、`last_jump_ns == None`、`current_time` 为传入参数（D7：无 `PtpTime::now()`）

### Requirement: BMCA 最佳主时钟选举

系统 SHALL 实现 `AnnounceMessage`（含 `grandmaster_identity` / `priority1` / `clock_class` / `accuracy` / `priority2` / `steps_removed` / `source_port_id` / `source_mac` 字段）与 `BmcaResult`（`ElectedAsMaster` / `FollowMaster(ClockIdentity)` 两变体，实现 `Display`）。

`compare_priority(a: &AnnounceMessage, b: &AnnounceMessage) -> core::cmp::Ordering` SHALL 按 BMCA 优先级顺序比较：`priority1` → `clock_class` → `accuracy` → `priority2` → `grandmaster_identity`（数值小者优先）。

`GptpClock::run_bmca(&mut self, announces: &[AnnounceMessage]) -> BmcaResult` SHALL：
1. 收集所有候选 Announce（含自身 `to_announce()`）
2. 按 `compare_priority` 升序排序
3. 若最优候选的 `grandmaster_identity == self.identity` → 返回 `ElectedAsMaster`，并将所有 `ports` 设为 `PortRole::Master`
4. 否则 → 返回 `FollowMaster(best.grandmaster_identity)`，更新 `self.grandmaster_identity` 与 `self.steps_removed = best.steps_removed + 1`

#### Scenario: 空候选列表
- **WHEN** 调用 `run_bmca(&[])`
- **THEN** 返回 `BmcaResult::ElectedAsMaster`（自身为唯一候选，自动当选）

#### Scenario: 远端优先级更高
- **WHEN** 远端 Announce `priority1 = 100`，自身 `priority1 = 200`
- **THEN** 返回 `BmcaResult::FollowMaster(remote_identity)`，`steps_removed` 增 1

#### Scenario: 优先级平局按 identity 决胜
- **WHEN** 两个 Announce 的 `priority1` / `clock_class` / `accuracy` / `priority2` 全相同
- **THEN** `grandmaster_identity` 字节数组小者优先

### Requirement: Sync 消息处理与偏移计算

系统 SHALL 实现 `SyncMessage`（`origin_timestamp: PtpTime` / `sequence_id: u16` / `steps_removed: u16`）与 `FollowUpMessage`（`sequence_id: u16` / `precise_origin_timestamp: PtpTime`）。

`GptpClock::handle_sync(&mut self, sync: &SyncMessage, rx_ts: PtpTime, delay_ns: i64)` SHALL：
1. 计算 `new_offset = rx_ts.diff_ns(&sync.origin_timestamp) - delay_ns`
2. 低通滤波：`self.offset = (self.offset * 7 + new_offset) / 8`

> **D9 偏差**：蓝图原签名 `handle_sync(&mut self, sync: &SyncMessage, rx_ts: PtpTime)` 内部调用 `origin.delay_to(&rx_ts)` 与 `rx_ts.diff_ns(&origin)`，二者对同一时间戳对计算结果相同，相减恒为 0（蓝图 bug）。本 spec 将 `delay_ns` 提升为参数（来自 P2P 延迟测量或预配置），使偏移计算物理意义正确。

#### Scenario: 偏移计算
- **WHEN** `sync.origin_timestamp = PtpTime{0, 0}`、`rx_ts = PtpTime{1, 0}`（即 1s 后到达）、`delay_ns = 100_000_000`（100ms 路径延迟）
- **THEN** `new_offset = 1_000_000_000 - 100_000_000 = 900_000_000` ns

#### Scenario: 低通滤波平滑
- **WHEN** `self.offset = 0`，连续两次 `handle_sync` 传入相同 `new_offset = 800`
- **THEN** 第一次后 `offset = (0*7 + 800) / 8 = 100`；第二次后 `offset = (100*7 + 800) / 8 = 187`

### Requirement: FollowUp 精确时间戳

`GptpClock::handle_follow_up(&mut self, fu: &FollowUpMessage)` SHALL 根据 `sequence_id` 匹配最近一次 `handle_sync` 的 Sync 消息，并将 `fu.precise_origin_timestamp` 用于重新计算偏移（覆盖 `sync.origin_timestamp` 的粗略值）。

#### Scenario: FollowUp 更新
- **WHEN** 先 `handle_sync(sync, rx_ts, delay_ns)`（`sync.sequence_id = 42`），再 `handle_follow_up(fu)`（`fu.sequence_id = 42`）
- **THEN** `self.offset` 基于 `fu.precise_origin_timestamp` 重新计算（低通滤波后更新）

### Requirement: 时钟调整

`GptpClock::adjust_clock(&mut self, offset: i64)` SHALL：
- 若 `offset.abs() < 1_000_000`（< 1ms）：仅存储 `self.frequency_offset = offset / 100`，**不**修改 `current_time`（小幅频率微调，避免跳变影响 SOE）
- 否则：调用 `self.current_time.add_ns(offset)` 跳跃调整，并记录 `self.last_jump_ns = Some(offset)`（D6：无 `warn!()` 宏，改用字段记录供上层观测）

`GptpClock::compute_offset(&self) -> i64` SHALL 返回 `self.offset`（当前滤波后偏移）。
`GptpClock::current_time(&self) -> PtpTime` SHALL 返回 `self.current_time + self.offset`（应用偏移后的同步时间）。
`GptpClock::to_announce(&self) -> AnnounceMessage` SHALL 构造反映本时钟状态的 Announce 消息。

#### Scenario: 小幅微调
- **WHEN** 调用 `adjust_clock(500_000)`（500μs）
- **THEN** `frequency_offset == 5_000`、`current_time` 不变、`last_jump_ns == None`

#### Scenario: 大幅跳跃
- **WHEN** 调用 `adjust_clock(5_000_000)`（5ms）
- **THEN** `current_time` 增加 5ms、`last_jump_ns == Some(5_000_000)`

## MODIFIED Requirements

### Requirement: workspace members

根 `Cargo.toml` 的 `[workspace] members` 列表 SHALL 在 `crates/protocols/` 下新增 `"crates/protocols/tsn-time"`，保持字母序（位于 `soe-engine` 与 `agent-bus-dds` 之间，按 `tsn-time` 字母序插入合适位置）。

### Requirement: 版本号同步

根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` 的版本号 SHALL 由 `0.78.0` 更新为 `0.79.0`。`ci/src/gate.rs` 的 clippy 段与 test 段注释 SHALL 更新为 `eneros-tsn-time v0.79.0` 并列出新增类型（`ClockIdentity` / `MacAddr` / `PtpTime` / `Port` / `PortRole` / `PortState` / `AnnounceMessage` / `BmcaResult` / `SyncMessage` / `FollowUpMessage` / `GptpConfig` / `GptpClock` / `compare_priority` / `run_bmca` / `handle_sync` / `handle_follow_up` / `adjust_clock` / `compute_offset` / `current_time` / `to_announce`）。

## REMOVED Requirements

### Requirement: 真实网络 I/O 与硬件时间戳集成
**Reason**：CI 环境无法验证真实网卡硬件时间戳（需 Intel i210/i225 等特定硬件 + `SO_TIMESTAMPING` socket 选项）。本版本聚焦类型与算法骨架，消息通过参数注入（D5），硬件时间戳能力以 `Port.hw_timestamping: bool` 字段标记（D8：仅标志位，无实际集成）。
**Migration**：真实网卡集成延后到具备硬件环境后的集成测试阶段（蓝图 §6.2 双机同步 < 1ms 验收条件不在 CI 范围内，D12）。

### Requirement: `log` crate 依赖与 `warn!()` 宏
**Reason**：蓝图 `adjust_clock()` 使用 `warn!("Clock jumped {} ns", offset)`，需引入 `log` crate。本 crate 为 no_std 协议层模块，遵循 v0.75.0~v0.78.0 既有约定（无 `log` 依赖），改用 `last_jump_ns: Option<i64>` 字段记录跳跃事件，供上层 Agent 观测。
**Migration**：若后续版本需要日志输出，由上层 Agent Runtime 负责读取 `last_jump_ns` 并通过 `eneros-agent-bus-dds` 总线发布告警消息。

### Requirement: `PtpTime::now()` 系统时钟访问
**Reason**：no_std 环境无系统时钟全局访问。蓝图 `PtpTime::now()` 在 `GptpClock::new()` 中调用以初始化 `current_time`，违反 no_std 合规。
**Migration**：`GptpClock::new()` 接受 `initial_time: PtpTime` 参数，由上层（v0.12.0 RTC 服务 `eneros-time::get_time()`）注入初始时间。`current_time` 字段后续通过 `adjust_clock()` 更新。

### Requirement: 性能基准测试与 24h 漂移测试
**Reason**：CI 无法稳定验证 < 1ms 同步精度（需双机硬件环境）与 24h 长时间漂移 < 10ms（CI 时间预算不允许）。
**Migration**：仅保留算法正确性单元测试（D4 内嵌 `src/lib.rs`）。性能基准与 24h 漂移延后到硬件集成测试阶段。

### Requirement: 双机集成测试
**Reason**：CI 无真实双机网络环境，无法验证双机同步 < 1ms 与主时钟切换 < 3s 收敛。
**Migration**：通过 Mock 注入 `AnnounceMessage` 数组模拟多机场景（D5），验证 BMCA 选举逻辑与偏移计算正确性。真实双机集成延后到硬件环境测试。

---

## 偏差声明（D1~D14）

| 偏差 | 说明 |
|------|------|
| **D1** | 新建 crate 位于 `crates/protocols/tsn-time/`（项目规则 §2.3.1，非蓝图 `crates/tsn_time/`） |
| **D2** | 文档位于 `docs/protocols/gptp-sync-design.md`（项目规则 §2.3.3，非蓝图 `docs/phase2/gptp_sync.md`） |
| **D3** | 配置位于 `configs/gptp.toml`（项目规则 §2.3，非蓝图 `config/gptp.toml`） |
| **D4** | 测试内嵌 `src/lib.rs` T1~T25（沿用 v0.75.0~v0.78.0 模式，非蓝图 `tests/gptp_convergence.rs` / `tests/clock_drift.rs`） |
| **D5** | 无真实网络 I/O — `AnnounceMessage` / `SyncMessage` / `FollowUpMessage` 通过参数注入（沿用 v0.75.0 `MockDdsNode` 模式） |
| **D6** | 无 `log` crate 依赖 — `adjust_clock()` 用 `last_jump_ns: Option<i64>` 字段替代 `warn!()` 宏 |
| **D7** | 无 `PtpTime::now()` — `GptpClock::new()` 接受 `initial_time: PtpTime` 参数注入（no_std 无系统时钟） |
| **D8** | `Port.hw_timestamping: bool` 仅标志位，无实际 `SO_TIMESTAMPING` socket 集成 |
| **D9** | `handle_sync()` 接受 `delay_ns: i64` 参数（修复蓝图 bug：原 `delay_to()` 与 `diff_ns()` 对同一时间戳对相减恒为 0） |
| **D10** | 不实现性能基准测试（CI 无法验证 < 1ms 收敛） |
| **D11** | 不实现 24h 漂移测试（CI 时间预算不允许） |
| **D12** | 不实现双机集成测试（CI 无真实网络环境） |
| **D13** | `ClockIdentity(pub [u8; 8])` newtype（EUI-64，固定 8 字节数组） |
| **D14** | `MacAddr(pub [u8; 6])` newtype（固定 6 字节数组） |

## no_std 合规

本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
仅使用 `alloc::*` 与 `core::*`，无外部依赖（无 `log` / `uuid` / `serde` / `smoltcp` 等）。
不调用 `panic!` / `todo!` / `unimplemented!`，不含 `unsafe` 块。
