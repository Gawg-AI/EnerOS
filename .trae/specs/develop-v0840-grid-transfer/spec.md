# v0.84.0 Grid Transfer Spec — Grid Agent 并离网切换

## Why

v0.83.0 完成 PCC 并网点管理（开关状态 + 功率方向 + 防抖），但尚未实现**并离网快速切换**能力。当主网故障时，微电网需要在 < 100ms 内完成孤岛检测 + 切换执行，以保障本地重要负荷。本版本扩展 `eneros-grid-agent` crate 增加两个新模块：`island_detect.rs`（孤岛检测，多源融合 + 连续确认）与 `transfer.rs`（切换状态机 + RTOS 快平面命令通道抽象）。为 v0.87.0 Energy Agent 孤岛调度提供基础。

## What Changes

- **ADDED**：`crates/agents/grid_agent/src/island_detect.rs` — 孤岛检测模块
  - `IslandResult` 枚举（3 变体：`Islanded` / `GridOk` / `Uncertain`，默认 `GridOk`）
  - `IslandConfig` 结构体（5 字段：confirmation_threshold / freq_min / freq_max / voltage_min / voltage_max，默认阈值 3 / 49.5 / 50.5 / 200.0 / 240.0）
  - `IslandDetector` 结构体（2 字段：config / consecutive_count）
  - `IslandDetector::detect(&mut self, pcc: &PccState, grid: &GridState) -> IslandResult` — 双源融合检测（PCC breaker 为主 + GridState 频率/电压为辅）+ 连续确认逻辑
- **ADDED**：`crates/agents/grid_agent/src/transfer.rs` — 切换状态机模块
  - `TransferState` 枚举（3 变体：`GridConnected` / `Islanded` / `Transferring`，默认 `GridConnected`）
  - `TransferReason` 枚举（4 变体：`Manual` / `IslandDetected` / `GridRecovered` / `Fault`）
  - `TransferCommand` 枚举（2 变体：`OpenPccAndIsland` / `ClosePccAndSync`）
  - `TransferRecord` 结构体（5 字段：timestamp / from / to / duration_ms / reason，派生 `Copy`）
  - `TransferError` 枚举（4 变体：`InvalidTarget` / `AlreadyInTarget` / `ChannelTimeout` / `ChannelError`）
  - `RtosChannel` trait + `MockRtosChannel`（沿用 v0.82.0 `GridSampler` trait + Mock 模式）
  - `GridTransfer` 结构体（4 字段：detector / state / last_transfer / rtos_channel）
  - `GridTransfer::transfer_to(target, reason, now_ms) -> Result<TransferRecord, TransferError>`
  - `GridTransfer::check_and_transfer(pcc, grid, now_ms) -> Option<TransferRecord>`
- **MODIFIED**：`crates/agents/grid_agent/src/lib.rs` — 追加 2 个 `pub mod` + 重导出（surgical：仅追加，不修改 v0.82.0/v0.83.0 既有代码）
- **MODIFIED**：`crates/agents/grid_agent/Cargo.toml` — `description` 字段追加 "+ v0.84.0 并离网切换"（无新依赖）
- **ADDED**：`configs/grid_transfer.toml` — 切换策略配置模板
- **ADDED**：`docs/agents/grid-transfer-design.md` — 设计文档（12 章 + Mermaid 图 + D1~D14 偏差表）
- **MODIFIED**：根 `Cargo.toml` workspace 版本 `0.83.0` → `0.84.0`
- **MODIFIED**：`Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本同步
- **未新增 crate**：两个新模块追加到既有 `eneros-grid-agent` crate

无 **BREAKING** 变更：v0.82.0/v0.83.0 既有公共 API（`GridAgent` / `GridState` / `DataQuality` / `GridSampler` / `GridPublisher` / `GridError` / `PccManager` / `PccState` / `BreakerStatus` / `PccStatus` 等）全部保留；新增类型与函数仅追加。

## Impact

- **Affected specs**：v0.83.0 PCC 管理（消费 `PccState` 作为孤岛检测主源）；v0.82.0 Grid Agent 状态感知（消费 `GridState` 作为被动检测辅源）；为 v0.87.0 Energy Agent 孤岛调度提供切换能力
- **Affected code**：
  - `crates/agents/grid_agent/src/island_detect.rs`（新建）
  - `crates/agents/grid_agent/src/transfer.rs`（新建）
  - `crates/agents/grid_agent/src/lib.rs`（追加 2 个 `pub mod` + 重导出）
  - `crates/agents/grid_agent/Cargo.toml`（description 字段更新）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
- **依赖不变**：无新第三方依赖；无新 workspace crate 依赖（仅复用本 crate 内 v0.82.0 `GridState` + v0.83.0 `PccState`）；SBOM 不变
- **回归面**：v0.82.0 的 46 tests + v0.83.0 的 40 tests 必须仍全部通过；v0.79.0/v0.80.0/v0.81.0/v0.72.0/v0.73.0 等既有 crate 必须无回归

## ADDED Requirements

### Requirement: Island Detection Data Structures

系统 SHALL 提供孤岛检测相关数据结构，全部派生 `Debug, Clone, Copy, PartialEq`（`Eq`/`Default` 视情况）：

- `IslandResult` 枚举（3 变体：`Islanded` / `GridOk` / `Uncertain`，`#[default]` on `GridOk`）
- `IslandConfig` 结构体（5 字段：`confirmation_threshold: u32`（默认 3）/ `freq_min: f32`（默认 49.5）/ `freq_max: f32`（默认 50.5）/ `voltage_min: f32`（默认 200.0）/ `voltage_max: f32`（默认 240.0）），派生 `Debug, Clone, Copy, PartialEq, Default`
- `IslandDetector` 结构体（2 字段：`config: IslandConfig` / `consecutive_count: u32`），派生 `Debug, Clone, PartialEq`

#### Scenario: Default config
- **WHEN** 调用 `IslandConfig::default()`
- **THEN** `confirmation_threshold == 3` / `freq_min == 49.5` / `freq_max == 50.5` / `voltage_min == 200.0` / `voltage_max == 240.0`

#### Scenario: Default result
- **WHEN** 调用 `IslandResult::default()`
- **THEN** 返回 `IslandResult::GridOk`

### Requirement: IslandDetector

系统 SHALL 提供 `IslandDetector` 实现多源融合孤岛检测 + 连续确认：

- `IslandDetector::new(config: IslandConfig) -> Self`（`consecutive_count = 0`）
- `IslandDetector::new_default() -> Self`（`config = IslandConfig::default()`）
- `IslandDetector::detect(&mut self, pcc: &PccState, grid: &GridState) -> IslandResult` — 核心逻辑：
  1. **主源**（PCC breaker）：若 `pcc.status == PccStatus::Islanded` → `raw_islanded = true`
  2. **辅源**（GridState 频率/电压）：若 `grid.frequency < config.freq_min || grid.frequency > config.freq_max || grid.voltage_a < config.voltage_min || grid.voltage_a > config.voltage_max` → `raw_islanded = true`
  3. **连续确认**（D6）：
     - 若 `raw_islanded == true` → `consecutive_count += 1`
     - 否则 → `consecutive_count = 0`
  4. **返回值**：
     - 若 `consecutive_count >= config.confirmation_threshold` → `IslandResult::Islanded`
     - 否则若 `consecutive_count > 0` → `IslandResult::Uncertain`（待确认）
     - 否则 → `IslandResult::GridOk`

#### Scenario: PCC islanded, first call returns Uncertain
- **WHEN** 首次调用 `detect(pcc_islanded, grid_normal)` 且 `confirmation_threshold = 3`
- **THEN** 返回 `IslandResult::Uncertain`（count=1 < 3）

#### Scenario: PCC islanded, 3 consecutive calls returns Islanded
- **WHEN** 连续 3 次调用 `detect(pcc_islanded, grid_normal)` 且 `confirmation_threshold = 3`
- **THEN** 第 3 次返回 `IslandResult::Islanded`

#### Scenario: Frequency out of range triggers detection
- **WHEN** PCC status 为 `GridConnected`，但 `grid.frequency = 49.0`（< 49.5），连续 3 次调用
- **THEN** 第 3 次返回 `IslandResult::Islanded`（辅源触发）

#### Scenario: Count resets on GridOk reading
- **WHEN** 2 次 `Uncertain` 后第 3 次输入 `pcc_status = GridConnected, frequency = 50.0`
- **THEN** 返回 `IslandResult::GridOk`（count 重置为 0）

#### Scenario: Custom threshold = 1
- **WHEN** `IslandConfig { confirmation_threshold: 1, .. }` + `pcc.status == Islanded`
- **THEN** 首次调用返回 `IslandResult::Islanded`

### Requirement: Transfer State Machine Data Structures

系统 SHALL 提供切换状态机相关数据结构，全部派生 `Debug, Clone, Copy, PartialEq, Eq`（`Default` 视情况）：

- `TransferState` 枚举（3 变体：`GridConnected` / `Islanded` / `Transferring`，`#[default]` on `GridConnected`）
- `TransferReason` 枚举（4 变体：`Manual` / `IslandDetected` / `GridRecovered` / `Fault`）
- `TransferCommand` 枚举（2 变体：`OpenPccAndIsland` / `ClosePccAndSync`）
- `TransferRecord` 结构体（5 字段：`timestamp: u64` / `from: TransferState` / `to: TransferState` / `duration_ms: u32` / `reason: TransferReason`），派生 `Debug, Clone, Copy, PartialEq`
- `TransferError` 枚举（4 变体：`InvalidTarget` / `AlreadyInTarget` / `ChannelTimeout` / `ChannelError`），派生 `Debug, Clone, Copy, PartialEq, Eq`

#### Scenario: Default state
- **WHEN** 调用 `TransferState::default()`
- **THEN** 返回 `TransferState::GridConnected`

### Requirement: RtosChannel Trait + MockRtosChannel

系统 SHALL 提供 `RtosChannel` trait 抽象 RTOS 快平面紧急命令通道，不要求 `Send + Sync`（no_std 单线程）：

```rust
pub trait RtosChannel {
    /// 下发紧急切换命令，返回 elapsed_ms（命令执行耗时）.
    /// 失败时返回 TransferError（ChannelTimeout / ChannelError）.
    fn send_emergency(&mut self, cmd: TransferCommand, now_ms: u64) -> Result<u64, TransferError>;
}
```

系统 SHALL 提供 `MockRtosChannel` 用于测试：
- 字段 `elapsed_ms: u64` / `fail: bool`，派生 `Debug, Clone`
- `MockRtosChannel::new(elapsed_ms: u64) -> Self`（`fail = false`）
- `MockRtosChannel::new_failing() -> Self`（`fail = true`，`elapsed_ms = 0`）
- `impl RtosChannel for MockRtosChannel` — `fail == true` 返回 `Err(TransferError::ChannelError)`；否则返回 `Ok(self.elapsed_ms)`

#### Scenario: Channel success
- **WHEN** `MockRtosChannel::new(50).send_emergency(OpenPccAndIsland, 1000)`
- **THEN** 返回 `Ok(50)`

#### Scenario: Channel failure
- **WHEN** `MockRtosChannel::new_failing().send_emergency(OpenPccAndIsland, 1000)`
- **THEN** 返回 `Err(TransferError::ChannelError)`

### Requirement: GridTransfer Manager

系统 SHALL 提供 `GridTransfer` 管理并离网切换状态机：

- 字段（4 个）：`detector: IslandDetector` / `state: TransferState` / `last_transfer: Option<TransferRecord>` / `rtos_channel: Box<dyn RtosChannel>`
- `GridTransfer::new(detector: IslandDetector, rtos_channel: Box<dyn RtosChannel>) -> Self` — 初始化 `state = TransferState::GridConnected` / `last_transfer = None`
- `GridTransfer::current_state(&self) -> TransferState` 返回 `self.state`
- `GridTransfer::last_transfer(&self) -> Option<TransferRecord>` 返回 `self.last_transfer`
- `GridTransfer::transfer_to(&mut self, target: TransferState, reason: TransferReason, now_ms: u64) -> Result<TransferRecord, TransferError>` — 核心切换逻辑：
  1. 若 `self.state == target` → 返回 `Err(TransferError::AlreadyInTarget)`
  2. 若 `target == TransferState::Transferring` → 返回 `Err(TransferError::InvalidTarget)`（不能显式切换到 Transferring）
  3. 记录 `from = self.state`
  4. 设置 `self.state = TransferState::Transferring`
  5. 映射 target → command：`Islanded → OpenPccAndIsland` / `GridConnected → ClosePccAndSync`
  6. 调用 `self.rtos_channel.send_emergency(cmd, now_ms)`：
     - 成功 → 取 `elapsed_ms`，构建 `TransferRecord { timestamp: now_ms, from, to: target, duration_ms: elapsed_ms as u32, reason }`，设置 `self.state = target` / `self.last_transfer = Some(record)`，返回 `Ok(record)`
     - 失败 → **回滚** `self.state = from`（D5，§4.4 "保持原状态"），返回 `Err(e)`
- `GridTransfer::check_and_transfer(&mut self, pcc: &PccState, grid: &GridState, now_ms: u64) -> Option<TransferRecord>` — 自动切换：
  1. 若 `self.state == TransferState::Transferring` → 返回 `None`（切换中，避免重入）
  2. `let result = self.detector.detect(pcc, grid);`
  3. 匹配 `(result, self.state)`：
     - `(Islanded, GridConnected)` → `self.transfer_to(Islanded, IslandDetected, now_ms).ok()`
     - `(GridOk, Islanded)` → `self.transfer_to(GridConnected, GridRecovered, now_ms).ok()`
     - 其他 → `None`

#### Scenario: Initial state
- **WHEN** `GridTransfer::new(detector, Box::new(MockRtosChannel::new(50)))`
- **THEN** `current_state() == GridConnected` / `last_transfer() == None`

#### Scenario: transfer_to Islanded success
- **WHEN** `transfer_to(Islanded, IslandDetected, 1000)` with `MockRtosChannel::new(50)`
- **THEN** 返回 `Ok(record)`，`record.duration_ms == 50` / `record.from == GridConnected` / `record.to == Islanded`；`current_state() == Islanded`；`last_transfer() == Some(record)`

#### Scenario: transfer_to same state returns AlreadyInTarget
- **WHEN** `state == GridConnected` 时调用 `transfer_to(GridConnected, ...)`
- **THEN** 返回 `Err(TransferError::AlreadyInTarget)`，state 不变

#### Scenario: transfer_to Transferring returns InvalidTarget
- **WHEN** 调用 `transfer_to(Transferring, ...)`
- **THEN** 返回 `Err(TransferError::InvalidTarget)`

#### Scenario: Channel failure reverts state
- **WHEN** `state == GridConnected`，`MockRtosChannel::new_failing()`，调用 `transfer_to(Islanded, ...)`
- **THEN** 返回 `Err(TransferError::ChannelError)`；`current_state() == GridConnected`（回滚到原状态，D5）

#### Scenario: check_and_transfer auto islanding
- **WHEN** `state == GridConnected`，连续 3 次 `detect` 返回 `Islanded`，第 3 次调用 `check_and_transfer(pcc_islanded, grid_normal, 3000)`
- **THEN** 返回 `Some(record)`，`record.to == Islanded`；`current_state() == Islanded`

#### Scenario: check_and_transfer no action
- **WHEN** `state == GridConnected`，`detect` 返回 `GridOk`
- **THEN** 返回 `None`，state 不变

#### Scenario: check_and_transfer avoids re-entry during Transferring
- **WHEN** `state == Transferring`（理论场景，实际 sync 不会出现）
- **THEN** 返回 `None`（避免重入）

### Requirement: no_std Compliance

所有新增代码 MUST 满足 no_std 合规：
- `island_detect.rs` / `transfer.rs` 不添加 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs crate 级属性）
- `island_detect.rs` 使用 `use crate::GridState;` + `use crate::PccState;` + `use crate::PccStatus;`（复用 v0.82.0/v0.83.0 既有类型）
- `transfer.rs` 使用 `use alloc::boxed::Box;` + `use crate::island_detect::IslandDetector;`（crate 内跨模块引用）
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `Instant::now()` / `Duration::from_millis()`
- 不依赖 `eneros-time` / `eneros-tsn-time`（D1）

## MODIFIED Requirements

### Requirement: eneros-grid-agent crate 公共 API

v0.82.0/v0.83.0 既有公共 API 全部保留不变。本版本追加以下公共 API（仅追加，不修改既有签名）：

- 模块：`pub mod island_detect;` + `pub mod transfer;`
- 重导出（按字母序插入既有 `pub use` 列表中）：
  - `pub use island_detect::{IslandConfig, IslandDetector, IslandResult};`
  - `pub use transfer::{GridTransfer, MockRtosChannel, RtosChannel, TransferCommand, TransferError, TransferReason, TransferRecord, TransferState};`
- crate `description` 字段更新为 `"EnerOS v0.82.0 Grid Agent — 电网状态感知 + v0.83.0 PCC 并网点管理 + v0.84.0 并离网切换 (采样/异常检测/PCC/孤岛检测/切换状态机, no_std)"`

### Requirement: 版本同步

- 根 `Cargo.toml` `[workspace.package] version = "0.84.0"`
- `Makefile` VERSION 变量 + header 注释 → `0.84.0`
- `.github/workflows/ci.yml` header 注释 → `0.84.0`
- `ci/src/gate.rs` clippy 段 + test 段注释追加：`+ v0.84.0 并离网切换：IslandResult / IslandConfig / IslandDetector / TransferState / TransferReason / TransferCommand / TransferRecord / TransferError / RtosChannel / MockRtosChannel / GridTransfer`
- `crates/agents/grid_agent/Cargo.toml` workspace members 列表**不变**（两新模块是既有 crate 的新文件）

## REMOVED Requirements

无。本版本仅追加，不删除任何既有功能。

## 偏差声明（D1~D14，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `use eneros_time::Instant;` + `Instant::now()` + `start.elapsed().as_millis()` | `now_ms: u64` 参数 + `RtosChannel::send_emergency` 返回 `elapsed_ms` | no_std 无 `Instant`；沿用 v0.82.0 D2/D3 + v0.83.0 D1 模式；避免引入 `eneros-time` 依赖（surgical — Cargo.toml deps 不变） |
| **D2** | `RtosCommandChannel` 具体类型 + `wait_ack(Duration::from_millis(80))` | `RtosChannel` trait + `MockRtosChannel`（`send_emergency` 一次性返回 `elapsed_ms`） | 沿用 v0.82.0 D5 + v0.83.0 D3 trait 抽象模式；避免 `Duration` 与阻塞等待语义；`send_emergency` 合并发送+等待，简化 API |
| **D3** | `IslandDetector::detect(&self, state: &GridState)` 单源 | `IslandDetector::detect(&mut self, pcc: &PccState, grid: &GridState) -> IslandResult` 双源融合 | 蓝图 §5.2 "多检测方法融合 + 开关位置确认"；PCC breaker 状态为主（v0.83.0 PccStatus），GridState 频率/电压为辅（v0.82.0）；`&mut self` 因 D6 需更新 consecutive_count |
| **D4** | `self.last_grid_state()` 未定义方法 | `check_and_transfer(&mut self, pcc, grid, now_ms)` 显式参数 | 蓝图代码不完整；surgical — 不增加 GridTransfer 持有状态字段；调用方提供 PCC + Grid 状态 |
| **D5** | "切换超时 → 告警，保持原状态或强制跳闸"（§4.4）未明确实现 | 通道失败时 `state` 回滚到 `from`（原始状态） | 选择"保持原状态"语义（非"强制跳闸"）；避免 `Transferring` 卡死；安全可控；蓝图 §4.4 二选一，本版本取保守路径 |
| **D6** | "连续 3 次确认"（§4.4）未明确实现 | `IslandDetector.consecutive_count` + `IslandConfig.confirmation_threshold`（默认 3） | 满足 §4.4 复检机制；阈值可配置；count > 0 且 < threshold 返回 `Uncertain`，达到 threshold 返回 `Islanded` |
| **D7** | `TransferError` 蓝图未定义变体 | 4 变体：`InvalidTarget` / `AlreadyInTarget` / `ChannelTimeout` / `ChannelError` | 蓝图 line 2489 暗示 `InvalidTarget`；`AlreadyInTarget` 处理同状态切换；通道失败区分超时（`ChannelTimeout`）/错误（`ChannelError`）；当前 `MockRtosChannel` 统一返回 `ChannelError`，`ChannelTimeout` 预留给真实硬件实现 |
| **D8** | `error!("切换超时: {}ms", duration)` 日志 | 移除日志；超时通过返回值 `TransferRecord.duration_ms` 由调用方判断 | no_std 无 `log` crate；沿用 v0.82.0 D1 模式；调用方可根据 `duration_ms > 100` 自行告警 |
| **D9** | `docs/phase2/grid_transfer.md` + `tests/transfer_latency.rs` | `docs/agents/grid-transfer-design.md` + 内嵌单元测试 T87~T126 | 工作区规则 §2.3.3 禁止 `docs/phase2/` 平面化；沿用 v0.82.0/v0.83.0 测试模式（lib.rs 内嵌 test 模块） |
| **D10** | 蓝图 2 文件 `transfer.rs` + `island_detect.rs` | 保持 2 文件（关注点分离：检测 vs 切换执行） | 沿用蓝图结构；模块边界清晰；每文件约 200~300 行可读 |
| **D11** | `TransferState`（GridConnected/Islanded/Transferring）与 v0.83.0 `PccStatus`（GridConnected/Islanded/Transitioning）名称重叠 | 保持两套枚举（语义不同：`PccStatus` 为观测态，`TransferState` 为控制态） | `PccStatus` 描述 PCC 客观状态（breaker 位置）；`TransferState` 描述 GridTransfer 控制意图（切换命令）；`Transferring` ≠ `Transitioning`；避免合并引入语义混淆 |
| **D12** | `Option<TransferRecord>` `last_transfer` 字段 | 保持 `Option<TransferRecord>`（初始 `None`，首次成功切换后 `Some`） | 沿用蓝图字段；`TransferRecord` 派生 `Copy`，无需 `Box`；`Option<TransferRecord>` 也是 `Copy`（因 `TransferRecord: Copy`） |
| **D13** | 性能目标 < 100ms（§7.5） | 标注为"硬件集成阶段验收，本版本仅算法骨架"；测试验证 `duration_ms` 正确记录 | 沿用 v0.82.0/v0.83.0 性能目标处理模式；Mock 通道无法验证真实硬件延迟；T119 验证 `duration_ms == 50`（来自 MockRtosChannel 配置） |
| **D14** | 蓝图未定义 `IslandConfig` | 新增 `IslandConfig { confirmation_threshold, freq_min, freq_max, voltage_min, voltage_max }` 配置结构体 | D6 派生：阈值可配置；频率/电压边界可配置；支持多场景调优（如不同电网标准 60Hz vs 50Hz） |

## no_std 合规声明

本版本所有新增代码：
- 继承 crate 级 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc;`（已在 v0.82.0 lib.rs 设置）
- `island_detect.rs` 仅使用 `crate::GridState` / `crate::PccState` / `crate::PccStatus` + `core::*`
- `transfer.rs` 仅使用 `alloc::boxed::Box` + `crate::island_detect::IslandDetector` + `core::*`
- 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `Instant::now()` / `Duration::from_millis()`
- 可交叉编译到 `aarch64-unknown-none`（`-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）

## Surgical Changes 声明

- v0.82.0/v0.83.0 既有源文件 `state.rs` / `sampler.rs` / `publisher.rs` / `pcc.rs` **完全未改动**
- `lib.rs` 仅追加 2 个 `pub mod` + 2 行 `pub use` + 顶部文档注释追加 v0.84.0 段落（不修改任何既有代码行）
- `Cargo.toml` 仅更新 `description` 字段（依赖列表不变）
- v0.82.0 既有 46 个测试 + v0.83.0 既有 40 个测试（共 86 个）必须仍全部通过
