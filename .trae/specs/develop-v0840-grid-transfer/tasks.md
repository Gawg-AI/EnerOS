# Tasks

- [x] Task 1: 创建 `crates/agents/grid_agent/src/island_detect.rs` — 数据结构 + IslandDetector（双源融合 + 连续确认）
  - [x] SubTask 1.1: `IslandResult` 枚举（3 变体 `Islanded` / `GridOk` / `Uncertain`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `GridOk`）
  - [x] SubTask 1.2: `IslandConfig` 结构体（5 字段：`confirmation_threshold: u32`（默认 3）/ `freq_min: f32`（默认 49.5）/ `freq_max: f32`（默认 50.5）/ `voltage_min: f32`（默认 200.0）/ `voltage_max: f32`（默认 240.0）），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.3: `IslandDetector` 结构体（2 字段：`config: IslandConfig` / `consecutive_count: u32`），派生 `Debug, Clone, PartialEq`（不派生 `Copy`，因含可变状态语义）
  - [x] SubTask 1.4: `IslandDetector::new(config: IslandConfig) -> Self`（`consecutive_count = 0`）
  - [x] SubTask 1.5: `IslandDetector::new_default() -> Self`（`config = IslandConfig::default()`）
  - [x] SubTask 1.6: `IslandDetector::detect(&mut self, pcc: &PccState, grid: &GridState) -> IslandResult` 核心逻辑：
    - 主源（PCC breaker）：`pcc.status == PccStatus::Islanded → raw_islanded = true`
    - 辅源（GridState 频率/电压）：`grid.frequency < freq_min || grid.frequency > freq_max || grid.voltage_a < voltage_min || grid.voltage_a > voltage_max → raw_islanded = true`
    - 连续确认（D6）：`raw_islanded == true → consecutive_count += 1`；否则 `consecutive_count = 0`
    - 返回值：`consecutive_count >= confirmation_threshold → Islanded`；`consecutive_count > 0 → Uncertain`；否则 `GridOk`
  - [x] SubTask 1.7: `IslandDetector::current_count(&self) -> u32` 公开访问器（用于测试与调试）
  - [x] SubTask 1.8: `IslandDetector::reset(&mut self)` 重置 `consecutive_count = 0`
  - [x] SubTask 1.9: `island_detect.rs` 使用 `use crate::GridState;` + `use crate::PccState;` + `use crate::PccStatus;`（复用 v0.82.0/v0.83.0 既有类型，D3）
  - [x] SubTask 1.10: `island_detect.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`（no_std 合规）

- [x] Task 2: 在 `island_detect.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T87~T96（D9）
  - [x] SubTask 2.1: T87 — `IslandConfig::default()` 全字段默认值（threshold=3 / freq_min=49.5 / freq_max=50.5 / voltage_min=200.0 / voltage_max=240.0）
  - [x] SubTask 2.2: T88 — `IslandResult::default() == GridOk`
  - [x] SubTask 2.3: T89 — `IslandDetector::new_default()` 初始化 `consecutive_count == 0` / `config == IslandConfig::default()`
  - [x] SubTask 2.4: T90 — PCC `Islanded` 首次调用 `detect` 返回 `Uncertain`（count=1 < 3）
  - [x] SubTask 2.5: T91 — PCC `Islanded` 连续 3 次调用 `detect`，第 3 次返回 `Islanded`
  - [x] SubTask 2.6: T92 — PCC `GridConnected` 但 `grid.frequency = 49.0`（< 49.5），连续 3 次返回 `Islanded`（辅源触发）
  - [x] SubTask 2.7: T93 — PCC `GridConnected` 但 `grid.voltage_a = 180.0`（< 200.0），连续 3 次返回 `Islanded`
  - [x] SubTask 2.8: T94 — 2 次 `Uncertain` 后第 3 次输入 `pcc_status = GridConnected` + 正常 grid → 返回 `GridOk`（count 重置为 0）
  - [x] SubTask 2.9: T95 — `IslandConfig { confirmation_threshold: 1, .. }` + `pcc.status == Islanded` → 首次返回 `Islanded`
  - [x] SubTask 2.10: T96 — PCC `GridConnected` + 频率/电压正常 → 返回 `GridOk`（count 保持 0）
  - [x] SubTask 2.11: T96b — `IslandDetector::current_count()` 在 `Uncertain` 调用后返回正确值；`reset()` 后归零
  - [x] SubTask 2.12: T96c — 频率过高（51.0 > 50.5）触发辅源检测（连续 3 次后 Islanded）
  - [x] SubTask 2.13: T96d — 电压过高（260.0 > 240.0）触发辅源检测

- [x] Task 3: 创建 `crates/agents/grid_agent/src/transfer.rs` — 数据结构 + RtosChannel trait + MockRtosChannel + GridTransfer
  - [x] SubTask 3.1: `TransferState` 枚举（3 变体 `GridConnected` / `Islanded` / `Transferring`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `GridConnected`）
  - [x] SubTask 3.2: `TransferReason` 枚举（4 变体 `Manual` / `IslandDetected` / `GridRecovered` / `Fault`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 3.3: `TransferCommand` 枚举（2 变体 `OpenPccAndIsland` / `ClosePccAndSync`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 3.4: `TransferRecord` 结构体（5 字段：`timestamp: u64` / `from: TransferState` / `to: TransferState` / `duration_ms: u32` / `reason: TransferReason`），派生 `Debug, Clone, Copy, PartialEq`
  - [x] SubTask 3.5: `TransferError` 枚举（4 变体 `InvalidTarget` / `AlreadyInTarget` / `ChannelTimeout` / `ChannelError`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 3.6: `RtosChannel` trait 定义 `fn send_emergency(&mut self, cmd: TransferCommand, now_ms: u64) -> Result<u64, TransferError>;`（不要求 `Send + Sync`，D2）
  - [x] SubTask 3.7: `MockRtosChannel` 结构体（字段 `elapsed_ms: u64` / `fail: bool`），派生 `Debug, Clone`
  - [x] SubTask 3.8: `MockRtosChannel::new(elapsed_ms: u64) -> Self`（`fail = false`）
  - [x] SubTask 3.9: `MockRtosChannel::new_failing() -> Self`（`fail = true`，`elapsed_ms = 0`）
  - [x] SubTask 3.10: `impl RtosChannel for MockRtosChannel` — `fail == true` 返回 `Err(TransferError::ChannelError)`；否则返回 `Ok(self.elapsed_ms)`
  - [x] SubTask 3.11: `GridTransfer` 结构体（4 字段：`detector: IslandDetector` / `state: TransferState` / `last_transfer: Option<TransferRecord>` / `rtos_channel: Box<dyn RtosChannel>`）
  - [x] SubTask 3.12: `GridTransfer::new(detector: IslandDetector, rtos_channel: Box<dyn RtosChannel>) -> Self` — 初始化 `state = TransferState::GridConnected` / `last_transfer = None`
  - [x] SubTask 3.13: `GridTransfer::current_state(&self) -> TransferState` 返回 `self.state`
  - [x] SubTask 3.14: `GridTransfer::last_transfer(&self) -> Option<TransferRecord>` 返回 `self.last_transfer`（Copy 语义，无需引用）
  - [x] SubTask 3.15: `GridTransfer::transfer_to(&mut self, target: TransferState, reason: TransferReason, now_ms: u64) -> Result<TransferRecord, TransferError>` 核心逻辑：
    - `self.state == target` → `Err(AlreadyInTarget)`
    - `target == Transferring` → `Err(InvalidTarget)`
    - 记录 `from = self.state`
    - 设置 `self.state = Transferring`
    - 映射 target → command：`Islanded → OpenPccAndIsland` / `GridConnected → ClosePccAndSync`
    - 调用 `rtos_channel.send_emergency(cmd, now_ms)`：
      - 成功 → 取 `elapsed_ms`，构建 `TransferRecord { timestamp: now_ms, from, to: target, duration_ms: elapsed_ms as u32, reason }`，设置 `self.state = target` / `self.last_transfer = Some(record)`，返回 `Ok(record)`
      - 失败 → **回滚** `self.state = from`（D5），返回 `Err(e)`
  - [x] SubTask 3.16: `GridTransfer::check_and_transfer(&mut self, pcc: &PccState, grid: &GridState, now_ms: u64) -> Option<TransferRecord>` 自动切换：
    - `self.state == Transferring` → 返回 `None`（避免重入）
    - `let result = self.detector.detect(pcc, grid);`
    - 匹配 `(result, self.state)`：`(Islanded, GridConnected)` → `transfer_to(Islanded, IslandDetected, now_ms).ok()`；`(GridOk, Islanded)` → `transfer_to(GridConnected, GridRecovered, now_ms).ok()`；其他 → `None`
  - [x] SubTask 3.17: `transfer.rs` 使用 `use alloc::boxed::Box;` + `use crate::island_detect::IslandDetector;` + `use crate::island_detect::IslandResult;` + `use crate::PccState;` + `use crate::GridState;`
  - [x] SubTask 3.18: `transfer.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`（no_std 合规）

- [x] Task 4: 在 `transfer.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T97~T126（D9）
  - [x] SubTask 4.1: T97 — `TransferState::default() == GridConnected`
  - [x] SubTask 4.2: T98 — `TransferReason` 4 变体 `Debug` 输出非空
  - [x] SubTask 4.3: T99 — `TransferCommand` 2 变体 `Debug` 输出非空
  - [x] SubTask 4.4: T100 — `TransferRecord` 5 字段构造与访问
  - [x] SubTask 4.5: T101 — `TransferError` 4 变体 `PartialEq` 相等性
  - [x] SubTask 4.6: T102 — `MockRtosChannel::new(50)` `fail == false` / `elapsed_ms == 50`
  - [x] SubTask 4.7: T103 — `MockRtosChannel::new_failing()` `fail == true` / `elapsed_ms == 0`
  - [x] SubTask 4.8: T104 — `MockRtosChannel::new(50).send_emergency(OpenPccAndIsland, 1000)` 返回 `Ok(50)`
  - [x] SubTask 4.9: T105 — `MockRtosChannel::new_failing().send_emergency(ClosePccAndSync, 1000)` 返回 `Err(ChannelError)`
  - [x] SubTask 4.10: T106 — `GridTransfer::new(...)` 初始化 `current_state() == GridConnected` / `last_transfer() == None`
  - [x] SubTask 4.11: T107 — `transfer_to(Islanded, IslandDetected, 1000)` 成功返回 `Ok(record)`
  - [x] SubTask 4.12: T108 — 成功切换后 `current_state() == Islanded`
  - [x] SubTask 4.13: T109 — 成功切换后 `last_transfer() == Some(record)` / `record.to == Islanded` / `record.from == GridConnected`
  - [x] SubTask 4.14: T110 — `record.duration_ms == 50`（来自 MockRtosChannel::new(50)）
  - [x] SubTask 4.15: T111 — `record.timestamp == 1000`
  - [x] SubTask 4.16: T112 — `record.reason == IslandDetected`
  - [x] SubTask 4.17: T113 — `transfer_to(GridConnected, ...)` 当 `state == GridConnected` → `Err(AlreadyInTarget)`，state 不变
  - [x] SubTask 4.18: T114 — `transfer_to(Transferring, ...)` → `Err(InvalidTarget)`
  - [x] SubTask 4.19: T115 — `MockRtosChannel::new_failing()` 时 `transfer_to(Islanded, ...)` → `Err(ChannelError)` 且 `current_state() == GridConnected`（D5 回滚）
  - [x] SubTask 4.20: T116 — `transfer_to(Islanded, ...)` 成功后再 `transfer_to(GridConnected, GridRecovered, 2000)` 成功 → `current_state() == GridConnected`
  - [x] SubTask 4.21: T117 — `check_and_transfer` 在 `state == GridConnected` + `detect` 返回 `GridOk` → `None`，state 不变
  - [x] SubTask 4.22: T118 — `check_and_transfer` 在 `state == GridConnected` + 连续 3 次 PCC `Islanded` → 第 3 次返回 `Some(record)` / `record.to == Islanded` / `current_state() == Islanded`
  - [x] SubTask 4.23: T119 — `check_and_transfer` 在 `state == Islanded` + 连续 3 次 PCC `GridConnected` + grid 正常 → 第 3 次返回 `Some(record)` / `record.to == GridConnected` / `current_state() == GridConnected`（GridRecovered）
  - [x] SubTask 4.24: T120 — `check_and_transfer` 在 `state == Transferring` 时返回 `None`（避免重入，理论场景）
  - [x] SubTask 4.25: T121 — `check_and_transfer` 第 1 次 PCC `Islanded`（Uncertain，count=1）→ `None`，state 仍 `GridConnected`
  - [x] SubTask 4.26: T122 — `check_and_transfer` 第 2 次 PCC `Islanded`（Uncertain，count=2）→ `None`
  - [x] SubTask 4.27: T123 — `TransferRecord` 派生 `Copy`，可复制（`let r2 = record; assert_eq!(r2, record);`）
  - [x] SubTask 4.28: T124 — `Option<TransferRecord>` 也实现 `Copy`（`let opt = last_transfer(); let opt2 = opt; assert_eq!(opt, opt2);`）
  - [x] SubTask 4.29: T125 — 多次连续切换：`GridConnected → Islanded → GridConnected` 后 `last_transfer().unwrap().to == GridConnected`
  - [x] SubTask 4.30: T126 — `IslandDetector` 内嵌于 `GridTransfer` 时，`check_and_transfer` 调用后 detector 的 `current_count()` 反映最近一次 detect 的 count

- [x] Task 5: 修改 `crates/agents/grid_agent/src/lib.rs` — 追加 2 个 `pub mod` + 重导出（surgical）
  - [x] SubTask 5.1: 在 `pub mod pcc;` 之前（按字母序，`i` < `p`）追加 `pub mod island_detect;`
  - [x] SubTask 5.2: 在 `pub mod pcc;` / `pub mod publisher;` / `pub mod sampler;` / `pub mod state;` 之后（按字母序）追加 `pub mod transfer;`
  - [x] SubTask 5.3: 追加 `pub use island_detect::{IslandConfig, IslandDetector, IslandResult};` 重导出
  - [x] SubTask 5.4: 追加 `pub use transfer::{GridTransfer, MockRtosChannel, RtosChannel, TransferCommand, TransferError, TransferReason, TransferRecord, TransferState};` 重导出
  - [x] SubTask 5.5: 顶部模块文档注释追加 v0.84.0 段落（核心类型列表 + v0.84.0 D1~D14 偏差表新增段落）
  - [x] SubTask 5.6: 不修改任何 v0.82.0/v0.83.0 既有代码行（`GridError` 定义 / `impl From` / 既有 `pub mod` / 既有 `pub use` / 既有 86 个测试全部保留）
  - [x] SubTask 5.7: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

- [x] Task 6: 修改 `crates/agents/grid_agent/Cargo.toml` — 更新 description（surgical）
  - [x] SubTask 6.1: `description` 字段更新为 `"EnerOS v0.82.0 Grid Agent — 电网状态感知 + v0.83.0 PCC 并网点管理 + v0.84.0 并离网切换 (采样/异常检测/PCC/孤岛检测/切换状态机, no_std)"`
  - [x] SubTask 6.2: `[dependencies]` 段不变（无新依赖）
  - [x] SubTask 6.3: workspace members 列表不变（两新模块是既有 crate 的新文件）

- [x] Task 7: 创建配置文件 `configs/grid_transfer.toml`（D8）
  - [x] SubTask 7.1: TOML 模板含 `[island_detection]` 段 + `confirmation_threshold` / `freq_min` / `freq_max` / `voltage_min` / `voltage_max` 字段（中文注释）
  - [x] SubTask 7.2: 含 `[transfer]` 段 + `max_duration_ms` / `default_reason` 字段（切换超时阈值与默认原因）
  - [x] SubTask 7.3: 含 `[rtos_channel]` 段说明（紧急通道配置：超时 `timeout_ms` / 重试 `retry_count`）
  - [x] SubTask 7.4: 含中文注释说明各字段用途（与 v0.82.0 grid_points.toml / v0.83.0 pcc.toml 风格一致）

- [x] Task 8: 创建设计文档 `docs/agents/grid-transfer-design.md`（D8）
  - [x] SubTask 8.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 8.2: 至少 1 个 Mermaid 图（GridTransfer.transfer_to 状态机：GridConnected → Transferring → Islanded / 失败回滚）
  - [x] SubTask 8.3: 至少 1 个 Mermaid 图（IslandDetector.detect 双源融合决策流程）
  - [x] SubTask 8.4: D1~D14 偏差声明表完整
  - [x] SubTask 8.5: 引用 v0.83.0 PCC 管理 + v0.82.0 Grid State 作为前置依赖
  - [x] SubTask 8.6: 包含性能目标说明（切换 < 100ms，标注为"硬件集成阶段验收，本版本仅算法骨架"）
  - [x] SubTask 8.7: 引用 v0.87.0 Energy Agent 孤岛调度作为下游消费者
  - [x] SubTask 8.8: 包含状态机映射表（TransferState × IslandResult → 行为）

- [x] Task 9: 版本同步根目录文件
  - [x] SubTask 9.1: 根 `Cargo.toml` `[workspace.package] version = "0.83.0"` → `"0.84.0"`
  - [x] SubTask 9.2: 根 `Cargo.toml` `[workspace.members]` 列表**不变**（两新模块是既有 crate 的新文件）
  - [x] SubTask 9.3: `Makefile` 版本号 `0.83.0` → `0.84.0`（header 注释 + VERSION 变量）
  - [x] SubTask 9.4: `.github/workflows/ci.yml` 版本号 `0.83.0` → `0.84.0`
  - [x] SubTask 9.5: `ci/src/gate.rs` clippy 段注释追加 `+ v0.84.0 并离网切换：IslandResult / IslandConfig / IslandDetector / TransferState / TransferReason / TransferCommand / TransferRecord / TransferError / RtosChannel / MockRtosChannel / GridTransfer`
  - [x] SubTask 9.6: `ci/src/gate.rs` test 段注释同步追加类型列表

- [x] Task 10: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 10.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 10.2: `cargo test -p eneros-grid-agent` 全部通过（v0.82.0 T1~T45 + `_ensure_imports_used` + v0.83.0 T47~T86 + v0.84.0 T87~T126 = 126+ tests + 1 doctest，0 failures）
  - [x] SubTask 10.3: `cargo build -p eneros-grid-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 10.4: `cargo fmt -p eneros-grid-agent -- --check` 通过
  - [x] SubTask 10.5: `cargo clippy -p eneros-grid-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 10.6: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
  - [x] SubTask 10.7: 回归 — `cargo test -p eneros-tsn-time` 仍通过 84 tests + 1 doctest（无回归）
  - [x] SubTask 10.8: 回归 — `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests + 1 doctest（无回归）
  - [x] SubTask 10.9: 回归 — `cargo test -p eneros-device-agent` 仍通过（AgentRuntime trait 未变）

# Task Dependencies

- Task 1（island_detect.rs 数据结构 + IslandDetector）必须先完成 — Task 2/3 依赖其类型
- Task 2（island_detect.rs 测试 T87~T96）依赖 Task 1 完成
- Task 3（transfer.rs 数据结构 + RtosChannel + GridTransfer）依赖 Task 1（`IslandDetector` / `IslandResult`）
- Task 4（transfer.rs 测试 T97~T126）依赖 Task 1 + Task 3 完成
- Task 5（lib.rs 修改）依赖 Task 1 + Task 3 完成（需 `pub mod island_detect;` + `pub mod transfer;` 模块存在才能编译）
- Task 6（Cargo.toml description）可与 Task 1~5 并行
- Task 7（configs/grid_transfer.toml）可与 Task 1~6 并行
- Task 8（docs/agents/grid-transfer-design.md）可与 Task 1~7 并行
- Task 9（版本同步根目录文件）依赖 Task 1~8 完成
- Task 10（构建校验）依赖所有前置任务完成

## 并行化建议

- **Sub-Agent A**：Task 1 + Task 2（island_detect.rs 完整实现 + 测试，单文件单 agent 串行）
- **Sub-Agent B**：Task 3 + Task 4（transfer.rs 完整实现 + 测试，单文件单 agent 串行）— 依赖 Task 1 完成，可与 Task 2 并行
- **Sub-Agent C**：Task 7（configs/grid_transfer.toml）+ Task 8（docs）— 可与 A/B 并行
- **Sub-Agent D**：Task 5（lib.rs surgical 修改）+ Task 6（Cargo.toml description）+ Task 9（版本同步）— 须在 A+B 完成后
- **最终串行**：Task 10 由主 agent 在 A/B/C/D 全部完成后执行
