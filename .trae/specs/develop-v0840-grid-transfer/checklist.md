# Checklist

## Task 1: island_detect.rs — 数据结构 + IslandDetector
- [x] C1: `crates/agents/grid_agent/src/island_detect.rs` 文件创建
- [x] C2: `IslandResult` 枚举 3 变体 `Islanded` / `GridOk` / `Uncertain`
- [x] C3: `IslandResult` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `GridOk`）
- [x] C4: `IslandConfig` 结构体 5 字段（`confirmation_threshold: u32` / `freq_min: f32` / `freq_max: f32` / `voltage_min: f32` / `voltage_max: f32`）
- [x] C5: `IslandConfig` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C6: `IslandConfig::default()` 返回 `confirmation_threshold == 3` / `freq_min == 49.5` / `freq_max == 50.5` / `voltage_min == 200.0` / `voltage_max == 240.0`
- [x] C7: `IslandDetector` 结构体 2 字段（`config: IslandConfig` / `consecutive_count: u32`）
- [x] C8: `IslandDetector` 派生 `Debug, Clone, PartialEq`（不派生 `Copy`）
- [x] C9: `IslandDetector::new(config: IslandConfig) -> Self`（`consecutive_count = 0`）
- [x] C10: `IslandDetector::new_default() -> Self`（`config = IslandConfig::default()`）
- [x] C11: `IslandDetector::detect(&mut self, pcc: &PccState, grid: &GridState) -> IslandResult` 存在
- [x] C12: `detect` 主源（PCC breaker）：`pcc.status == PccStatus::Islanded → raw_islanded = true`
- [x] C13: `detect` 辅源（GridState 频率/电压）：`grid.frequency < freq_min || > freq_max || grid.voltage_a < voltage_min || > voltage_max → raw_islanded = true`
- [x] C14: `detect` 连续确认（D6）：`raw_islanded == true → consecutive_count += 1`；否则 `consecutive_count = 0`
- [x] C15: `detect` 返回值：`count >= threshold → Islanded`；`count > 0 → Uncertain`；否则 `GridOk`
- [x] C16: `IslandDetector::current_count(&self) -> u32` 公开访问器
- [x] C17: `IslandDetector::reset(&mut self)` 重置 `consecutive_count = 0`
- [x] C18: `island_detect.rs` 使用 `use crate::GridState;` + `use crate::PccState;` + `use crate::PccStatus;`（复用 v0.82.0/v0.83.0 既有类型，D3）
- [x] C19: `island_detect.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`（no_std 合规）
- [x] C20: `island_detect.rs` 无 `Instant::now()` / 无 `Duration::from_millis()` / 无 `eneros-time` 依赖（D1）

## Task 2: island_detect.rs — 单元测试 T87~T96
- [x] C21: T87 — `IslandConfig::default()` 全字段默认值（threshold=3 / freq_min=49.5 / freq_max=50.5 / voltage_min=200.0 / voltage_max=240.0）
- [x] C22: T88 — `IslandResult::default() == GridOk`
- [x] C23: T89 — `IslandDetector::new_default()` 初始化 `consecutive_count == 0` / `config == IslandConfig::default()`
- [x] C24: T90 — PCC `Islanded` 首次调用 `detect` 返回 `Uncertain`（count=1 < 3）
- [x] C25: T91 — PCC `Islanded` 连续 3 次调用 `detect`，第 3 次返回 `Islanded`
- [x] C26: T92 — PCC `GridConnected` 但 `grid.frequency = 49.0`（< 49.5），连续 3 次返回 `Islanded`（辅源触发）
- [x] C27: T93 — PCC `GridConnected` 但 `grid.voltage_a = 180.0`（< 200.0），连续 3 次返回 `Islanded`
- [x] C28: T94 — 2 次 `Uncertain` 后第 3 次输入 `pcc_status = GridConnected` + 正常 grid → 返回 `GridOk`（count 重置为 0）
- [x] C29: T95 — `IslandConfig { confirmation_threshold: 1, .. }` + `pcc.status == Islanded` → 首次返回 `Islanded`
- [x] C30: T96 — PCC `GridConnected` + 频率/电压正常 → 返回 `GridOk`（count 保持 0）
- [x] C31: T96b — `IslandDetector::current_count()` 在 `Uncertain` 调用后返回正确值；`reset()` 后归零
- [x] C32: T96c — 频率过高（51.0 > 50.5）触发辅源检测（连续 3 次后 Islanded）
- [x] C33: T96d — 电压过高（260.0 > 240.0）触发辅源检测

## Task 3: transfer.rs — 数据结构 + RtosChannel + MockRtosChannel + GridTransfer
- [x] C34: `crates/agents/grid_agent/src/transfer.rs` 文件创建
- [x] C35: `TransferState` 枚举 3 变体 `GridConnected` / `Islanded` / `Transferring`
- [x] C36: `TransferState` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `GridConnected`）
- [x] C37: `TransferReason` 枚举 4 变体 `Manual` / `IslandDetected` / `GridRecovered` / `Fault`
- [x] C38: `TransferReason` 派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C39: `TransferCommand` 枚举 2 变体 `OpenPccAndIsland` / `ClosePccAndSync`
- [x] C40: `TransferCommand` 派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C41: `TransferRecord` 结构体 5 字段（`timestamp: u64` / `from: TransferState` / `to: TransferState` / `duration_ms: u32` / `reason: TransferReason`）
- [x] C42: `TransferRecord` 派生 `Debug, Clone, Copy, PartialEq`
- [x] C43: `TransferError` 枚举 4 变体 `InvalidTarget` / `AlreadyInTarget` / `ChannelTimeout` / `ChannelError`
- [x] C44: `TransferError` 派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C45: `RtosChannel` trait 定义 `fn send_emergency(&mut self, cmd: TransferCommand, now_ms: u64) -> Result<u64, TransferError>;`
- [x] C46: `RtosChannel` 不要求 `Send + Sync`（D2）
- [x] C47: `MockRtosChannel` 结构体字段 `elapsed_ms: u64` / `fail: bool`，派生 `Debug, Clone`
- [x] C48: `MockRtosChannel::new(elapsed_ms: u64) -> Self`（`fail = false`）
- [x] C49: `MockRtosChannel::new_failing() -> Self`（`fail = true`，`elapsed_ms = 0`）
- [x] C50: `impl RtosChannel for MockRtosChannel` — `fail == true` 返回 `Err(TransferError::ChannelError)`；否则 `Ok(self.elapsed_ms)`
- [x] C51: `GridTransfer` 结构体 4 字段（`detector: IslandDetector` / `state: TransferState` / `last_transfer: Option<TransferRecord>` / `rtos_channel: Box<dyn RtosChannel>`）
- [x] C52: `GridTransfer::new(detector, rtos_channel)` 初始化 `state = TransferState::GridConnected` / `last_transfer = None`
- [x] C53: `GridTransfer::current_state(&self) -> TransferState` 返回 `self.state`
- [x] C54: `GridTransfer::last_transfer(&self) -> Option<TransferRecord>` 返回 `self.last_transfer`（Copy 语义）
- [x] C55: `GridTransfer::transfer_to(&mut self, target, reason, now_ms) -> Result<TransferRecord, TransferError>` 存在
- [x] C56: `transfer_to` — `self.state == target` → `Err(AlreadyInTarget)`
- [x] C57: `transfer_to` — `target == Transferring` → `Err(InvalidTarget)`
- [x] C58: `transfer_to` — 记录 `from = self.state` + 设置 `self.state = Transferring`
- [x] C59: `transfer_to` — 映射 `Islanded → OpenPccAndIsland` / `GridConnected → ClosePccAndSync`
- [x] C60: `transfer_to` 成功路径：构建 `TransferRecord { timestamp: now_ms, from, to: target, duration_ms: elapsed_ms as u32, reason }` / `state = target` / `last_transfer = Some(record)` / 返回 `Ok(record)`
- [x] C61: `transfer_to` 失败路径（D5 回滚）：`self.state = from` / 返回 `Err(e)`
- [x] C62: `GridTransfer::check_and_transfer(&mut self, pcc, grid, now_ms) -> Option<TransferRecord>` 存在
- [x] C63: `check_and_transfer` — `state == Transferring` → 返回 `None`（避免重入）
- [x] C64: `check_and_transfer` — 调用 `self.detector.detect(pcc, grid)`
- [x] C65: `check_and_transfer` — `(Islanded, GridConnected)` → `transfer_to(Islanded, IslandDetected, now_ms).ok()`
- [x] C66: `check_and_transfer` — `(GridOk, Islanded)` → `transfer_to(GridConnected, GridRecovered, now_ms).ok()`
- [x] C67: `check_and_transfer` — 其他组合 → `None`
- [x] C68: `transfer.rs` 使用 `use alloc::boxed::Box;` + `use crate::island_detect::IslandDetector;` + `use crate::island_detect::IslandResult;` + `use crate::PccState;` + `use crate::GridState;`
- [x] C69: `transfer.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`
- [x] C70: `transfer.rs` 无 `Instant::now()` / 无 `Duration::from_millis()` / 无 `eneros-time` 依赖（D1）

## Task 4: transfer.rs — 单元测试 T97~T126
- [x] C71: T97 — `TransferState::default() == GridConnected`
- [x] C72: T98 — `TransferReason` 4 变体 `Debug` 输出非空
- [x] C73: T99 — `TransferCommand` 2 变体 `Debug` 输出非空
- [x] C74: T100 — `TransferRecord` 5 字段构造与访问
- [x] C75: T101 — `TransferError` 4 变体 `PartialEq` 相等性
- [x] C76: T102 — `MockRtosChannel::new(50)` `fail == false` / `elapsed_ms == 50`
- [x] C77: T103 — `MockRtosChannel::new_failing()` `fail == true` / `elapsed_ms == 0`
- [x] C78: T104 — `MockRtosChannel::new(50).send_emergency(OpenPccAndIsland, 1000)` 返回 `Ok(50)`
- [x] C79: T105 — `MockRtosChannel::new_failing().send_emergency(ClosePccAndSync, 1000)` 返回 `Err(ChannelError)`
- [x] C80: T106 — `GridTransfer::new(...)` 初始化 `current_state() == GridConnected` / `last_transfer() == None`
- [x] C81: T107 — `transfer_to(Islanded, IslandDetected, 1000)` 成功返回 `Ok(record)`
- [x] C82: T108 — 成功切换后 `current_state() == Islanded`
- [x] C83: T109 — 成功切换后 `last_transfer() == Some(record)` / `record.to == Islanded` / `record.from == GridConnected`
- [x] C84: T110 — `record.duration_ms == 50`（来自 MockRtosChannel::new(50)）
- [x] C85: T111 — `record.timestamp == 1000`
- [x] C86: T112 — `record.reason == IslandDetected`
- [x] C87: T113 — `transfer_to(GridConnected, ...)` 当 `state == GridConnected` → `Err(AlreadyInTarget)`，state 不变
- [x] C88: T114 — `transfer_to(Transferring, ...)` → `Err(InvalidTarget)`
- [x] C89: T115 — `MockRtosChannel::new_failing()` 时 `transfer_to(Islanded, ...)` → `Err(ChannelError)` 且 `current_state() == GridConnected`（D5 回滚）
- [x] C90: T116 — `transfer_to(Islanded, ...)` 成功后再 `transfer_to(GridConnected, GridRecovered, 2000)` 成功 → `current_state() == GridConnected`
- [x] C91: T117 — `check_and_transfer` 在 `state == GridConnected` + `detect` 返回 `GridOk` → `None`，state 不变
- [x] C92: T118 — `check_and_transfer` 在 `state == GridConnected` + 连续 3 次 PCC `Islanded` → 第 3 次返回 `Some(record)` / `record.to == Islanded` / `current_state() == Islanded`
- [x] C93: T119 — `check_and_transfer` 在 `state == Islanded` + 连续 3 次 PCC `GridConnected` + grid 正常 → 第 3 次返回 `Some(record)` / `record.to == GridConnected` / `current_state() == GridConnected`（GridRecovered）
- [x] C94: T120 — `check_and_transfer` 在 `state == Transferring` 时返回 `None`（避免重入）
- [x] C95: T121 — `check_and_transfer` 第 1 次 PCC `Islanded`（Uncertain，count=1）→ `None`，state 仍 `GridConnected`
- [x] C96: T122 — `check_and_transfer` 第 2 次 PCC `Islanded`（Uncertain，count=2）→ `None`
- [x] C97: T123 — `TransferRecord` 派生 `Copy`，可复制（`let r2 = record; assert_eq!(r2, record);`）
- [x] C98: T124 — `Option<TransferRecord>` 也实现 `Copy`（`let opt = last_transfer(); let opt2 = opt; assert_eq!(opt, opt2);`）
- [x] C99: T125 — 多次连续切换：`GridConnected → Islanded → GridConnected` 后 `last_transfer().unwrap().to == GridConnected`
- [x] C100: T126 — `IslandDetector` 内嵌于 `GridTransfer` 时，`check_and_transfer` 调用后 detector 的 `current_count()` 反映最近一次 detect 的 count

## Task 5: lib.rs surgical 修改
- [x] C101: `pub mod island_detect;` 在 `pub mod pcc;` 之前（按字母序 `i` < `p`）
- [x] C102: `pub mod transfer;` 在 `pub mod state;` 之后（按字母序 `s` < `t`）
- [x] C103: `pub use island_detect::{IslandConfig, IslandDetector, IslandResult};` 重导出
- [x] C104: `pub use transfer::{GridTransfer, MockRtosChannel, RtosChannel, TransferCommand, TransferError, TransferReason, TransferRecord, TransferState};` 重导出
- [x] C105: 顶部模块文档注释追加 v0.84.0 类型说明 + D1~D14 偏差表（新增"v0.84.0 并离网切换偏差"段落）
- [x] C106: v0.82.0 既有 `pub mod publisher;` / `pub mod sampler;` / `pub mod state;` 保留不变
- [x] C107: v0.83.0 既有 `pub mod pcc;` 保留不变
- [x] C108: v0.82.0 既有 `pub use publisher::{...};` / `pub use sampler::{...};` / `pub use state::{...};` 保留不变
- [x] C109: v0.83.0 既有 `pub use pcc::{...};` 保留不变
- [x] C110: v0.82.0 既有 `GridError` 枚举定义与 `impl From<GridError> for AgentRuntimeError` 保留不变
- [x] C111: v0.82.0 既有 46 个测试 + v0.83.0 既有 40 个测试（共 86 个）保留不变
- [x] C112: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 6: Cargo.toml description 更新
- [x] C113: `description` 字段更新为含 "v0.84.0 并离网切换" 字样
- [x] C114: `[dependencies]` 段不变（仍为 `eneros-agent` + `eneros-energy-market-agent`）
- [x] C115: workspace members 列表不变

## Task 7: configs/grid_transfer.toml
- [x] C116: 文件位于 `configs/grid_transfer.toml`
- [x] C117: TOML 模板含 `[island_detection]` 段 + `confirmation_threshold` / `freq_min` / `freq_max` / `voltage_min` / `voltage_max` 字段
- [x] C118: 含 `[transfer]` 段 + `max_duration_ms` / `default_reason` 字段
- [x] C119: 含 `[rtos_channel]` 段或文档注释说明（`timeout_ms` / `retry_count`）
- [x] C120: 含中文注释说明各字段用途（与 v0.82.0 grid_points.toml / v0.83.0 pcc.toml 风格一致）

## Task 8: docs/agents/grid-transfer-design.md
- [x] C121: 文件位于 `docs/agents/grid-transfer-design.md`（非 `docs/phase2/`，符合 D8 + 工作区规则 §2.3.3）
- [x] C122: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
- [x] C123: 至少 1 个 Mermaid 图（GridTransfer.transfer_to 状态机：GridConnected → Transferring → Islanded / 失败回滚）
- [x] C124: 至少 1 个 Mermaid 图（IslandDetector.detect 双源融合决策流程）
- [x] C125: D1~D14 偏差声明表完整
- [x] C126: 引用 v0.83.0 PCC 管理 + v0.82.0 Grid State 作为前置依赖
- [x] C127: 包含性能目标说明（切换 < 100ms，标注为"硬件集成阶段验收，本版本仅算法骨架"）
- [x] C128: 引用 v0.87.0 Energy Agent 孤岛调度作为下游消费者
- [x] C129: 包含状态机映射表（TransferState × IslandResult → 行为）

## Task 9: 版本同步根目录文件
- [x] C130: 根 `Cargo.toml` 顶层 `[workspace.package] version = "0.84.0"`
- [x] C131: 根 `Cargo.toml` `[workspace.members]` 列表**不变**（两新模块是既有 crate 的新文件）
- [x] C132: `Makefile` 中 `# Version: v0.84.0` 与 `VERSION := 0.84.0`
- [x] C133: `.github/workflows/ci.yml` 中 `# Version: v0.84.0`
- [x] C134: `ci/src/gate.rs` clippy 段注释含 `+ v0.84.0 并离网切换：IslandResult / IslandConfig / IslandDetector / TransferState / TransferReason / TransferCommand / TransferRecord / TransferError / RtosChannel / MockRtosChannel / GridTransfer`
- [x] C135: `ci/src/gate.rs` test 段注释同步追加类型列表

## Task 10: 构建校验（§2.4.2）
- [x] C136: `cargo metadata --format-version 1` 成功
- [x] C137: `cargo test -p eneros-grid-agent` 全部通过（v0.82.0 T1~T45 + `_ensure_imports_used` + v0.83.0 T47~T86 + v0.84.0 T87~T126 = 126+ tests + 1 doctest，0 failures）
- [x] C138: `cargo build -p eneros-grid-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 退出码 0
- [x] C139: `cargo fmt -p eneros-grid-agent -- --check` 退出码 0
- [x] C140: `cargo clippy -p eneros-grid-agent --all-targets -- -D warnings` 无 warning，退出码 0
- [x] C141: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
- [x] C142: 回归 — `cargo test -p eneros-tsn-time` 仍通过 84 tests + 1 doctest（无回归）
- [x] C143: 回归 — `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests + 1 doctest（无回归）
- [x] C144: 回归 — `cargo test -p eneros-device-agent` 仍通过（AgentRuntime trait 未变）

## 总体校验
- [x] C145: 无根目录新 crate（`crates/agents/grid_agent/` 既有 crate 追加 2 个新模块文件，符合 §2.3.1）
- [x] C146: 无 `docs/` 根目录平面化文档（新文档在 `docs/agents/` 下）
- [x] C147: 无 `config/` 目录（新配置在 `configs/grid_transfer.toml`）
- [x] C148: `.gitignore` 未需更新（无新文件类型）
- [x] C149: `git status` 无 `target/` / `*.elf` / `*.bin` / `*.dtb` / IDE 缓存被追踪
- [x] C150: 提交信息遵循 Conventional Commits（如 `feat(agents/grid_agent): v0.84.0 实现并离网切换状态机`）
- [x] C151: ADR 决策未被违反（未引入研究特性、未自研已有开源替代组件、未超出 v1.0.0 范围）
- [x] C152: no_std 合规性：`island_detect.rs` / `transfer.rs` 继承 crate 级 `#![cfg_attr(not(test), no_std)]`
- [x] C153: 内存预算：切换模块 ≤ 1MB（蓝图 §8.3 声明，本版本为算法骨架，实际占用远小于此）
- [x] C154: SBOM 未变化（无新第三方依赖，仅复用 workspace 内既有 crate `eneros-agent` / `eneros-energy-market-agent`）
- [x] C155: 文档同步：v0.82.0/v0.83.0 历史偏差声明保留，v0.84.0 新增 D1~D14 段落
- [x] C156: Surgical Changes 原则：v0.82.0/v0.83.0 既有源文件 `state.rs` / `sampler.rs` / `publisher.rs` / `pcc.rs` 完全未改动
- [x] C157: `lib.rs` 仅追加 2 个 `pub mod` + 2 行 `pub use` + 顶部文档注释（不修改任何 v0.82.0/v0.83.0 既有代码行）
