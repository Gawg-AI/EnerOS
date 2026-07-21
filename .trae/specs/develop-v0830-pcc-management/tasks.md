# Tasks

- [x] Task 1: 创建 `crates/agents/grid_agent/src/pcc.rs` — 数据结构 + PccReader trait + MockPccReader
  - [x] SubTask 1.1: `BreakerStatus` 枚举（4 变体 `Closed` / `Open` / `Tripped` / `Unknown`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Unknown`）
  - [x] SubTask 1.2: `PowerDirection` 枚举（3 变体 `Import` / `Export` / `Idle`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Idle`）
  - [x] SubTask 1.3: `PccStatus` 枚举（3 变体 `GridConnected` / `Islanded` / `Transitioning`），派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Transitioning`）
  - [x] SubTask 1.4: `PccReading` 结构体（3 字段：`breaker_status: BreakerStatus` / `active_power: f32` / `reactive_power: f32`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.5: `PccState` 结构体（7 字段：`pcc_id: u32` / `breaker_status: BreakerStatus` / `power_direction: PowerDirection` / `power_factor: f32` / `active_power: f32` / `reactive_power: f32` / `status: PccStatus`），派生 `Debug, Clone, Copy, PartialEq, Default`
  - [x] SubTask 1.6: `PccReader` trait 定义 `fn read(&mut self, pcc_id: u32, now_ms: u64) -> Result<PccReading, GridError>;`（不要求 `Send + Sync`，D3）
  - [x] SubTask 1.7: `MockPccReader` 结构体（字段 `next_reading: PccReading` / `fail: bool`），派生 `Debug, Clone`
  - [x] SubTask 1.8: `MockPccReader::new(reading: PccReading) -> Self`（`fail = false`）
  - [x] SubTask 1.9: `MockPccReader::new_failing() -> Self`（`fail = true`，`next_reading = PccReading::default()`）
  - [x] SubTask 1.10: `MockPccReader::with_reading(mut self, reading: PccReading) -> Self` builder
  - [x] SubTask 1.11: `impl PccReader for MockPccReader` — `fail == true` 返回 `Err(GridError::SampleFailed)`；否则返回 `Ok(self.next_reading)`
  - [x] SubTask 1.12: `pcc.rs` 使用 `use crate::GridError;`（复用 v0.82.0 错误类型，D4）；无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`（no_std 合规）

- [x] Task 2: 在 `pcc.rs` 实现 `PccManager` + 辅助函数 + 防抖逻辑
  - [x] SubTask 2.1: `compute_power_direction(active_power: f32) -> PowerDirection` 公开函数 — `active_power > 1.0 → Import` / `< -1.0 → Export` / 否则 `Idle`（D11）
  - [x] SubTask 2.2: `compute_power_factor(active_power: f32, reactive_power: f32) -> f32` 公开函数 — `|active_power| < 0.1 → 1.0`；否则 `active_power / (active_power*active_power + reactive_power*reactive_power).sqrt()` 使用 `core::f32::sqrt`（D7）
  - [x] SubTask 2.3: `compute_stable_status(breaker: BreakerStatus) -> PccStatus` 私有函数 — `Closed → GridConnected` / `Open → Islanded` / `Tripped → Islanded` / `Unknown → Transitioning`
  - [x] SubTask 2.4: `PccManager` 结构体（6 字段：`pcc_id: u32` / `reader: Box<dyn PccReader>` / `state: PccState` / `debounce_ms: u64` / `last_breaker_status: BreakerStatus` / `last_change_ms: u64`）
  - [x] SubTask 2.5: `PccManager::new(pcc_id: u32, reader: Box<dyn PccReader>, debounce_ms: u64) -> Self` — 初始化 `state = PccState::default()` / `last_breaker_status = BreakerStatus::Unknown` / `last_change_ms = 0`
  - [x] SubTask 2.6: `PccManager::current(&self) -> &PccState` 返回 `&self.state`
  - [x] SubTask 2.7: `PccManager::is_islanded(&self) -> bool` 返回 `self.state.status == PccStatus::Islanded`
  - [x] SubTask 2.8: `PccManager::update(&mut self, now_ms: u64) -> Result<PccState, GridError>` — 核心逻辑：
    - 调用 `reader.read(self.pcc_id, now_ms)`，失败返回 `Err(GridError::SampleFailed)`
    - 取 `new_breaker = reading.breaker_status`
    - **防抖**（D6）：若 `new_breaker != self.last_breaker_status` → `last_breaker_status = new_breaker` / `last_change_ms = now_ms` / `state.status = Transitioning`；否则若 `now_ms - last_change_ms >= debounce_ms` → `state.status = compute_stable_status(new_breaker)`；否则保持 `Transitioning`
    - 更新 `state.breaker_status = new_breaker` / `state.active_power = reading.active_power` / `state.reactive_power = reading.reactive_power` / `state.power_direction = compute_power_direction(reading.active_power)` / `state.power_factor = compute_power_factor(reading.active_power, reading.reactive_power)`
    - 返回 `Ok(self.state)`
  - [x] SubTask 2.9: `PccManager` 字段 `reader` 使用 `use alloc::boxed::Box;`（no_std 合规）
  - [x] SubTask 2.10: `pcc.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`

- [x] Task 3: 在 `pcc.rs` 添加 `#[cfg(test)] mod tests` 单元测试 T47~T76+（D9）
  - [x] SubTask 3.1: T47 — `PccState::default()` 全字段默认值（pcc_id=0 / Unknown / Idle / 0.0 / Transitioning）
  - [x] SubTask 3.2: T48 — `BreakerStatus::default() == Unknown`
  - [x] SubTask 3.3: T49 — `PowerDirection::default() == Idle`
  - [x] SubTask 3.4: T50 — `PccStatus::default() == Transitioning`
  - [x] SubTask 3.5: T51 — `PccReading::default()` 全默认
  - [x] SubTask 3.6: T52 — `MockPccReader::new(reading)` fail=false
  - [x] SubTask 3.7: T53 — `MockPccReader::new_failing()` fail=true
  - [x] SubTask 3.8: T54 — `MockPccReader::with_reading(reading)` builder
  - [x] SubTask 3.9: T55 — `MockPccReader::read()` 成功路径返回 `Ok(reading)`
  - [x] SubTask 3.10: T56 — `MockPccReader::read()` 失败路径返回 `Err(SampleFailed)`
  - [x] SubTask 3.11: T57 — `compute_power_direction(10.0) == Import`
  - [x] SubTask 3.12: T58 — `compute_power_direction(-10.0) == Export`
  - [x] SubTask 3.13: T59 — `compute_power_direction(0.5) == Idle`
  - [x] SubTask 3.14: T60 — `compute_power_direction(-0.5) == Idle`（|P|≤1.0）
  - [x] SubTask 3.15: T61 — `compute_power_factor(3.0, 4.0) ≈ 0.6`（误差 < 1e-6）
  - [x] SubTask 3.16: T62 — `compute_power_factor(0.0, 0.0) == 1.0`（避免除零）
  - [x] SubTask 3.17: T63 — `compute_power_factor(0.05, 0.0) == 1.0`（|P|<0.1 阈值）
  - [x] SubTask 3.18: T64 — `compute_stable_status(Closed) == GridConnected`（通过 PccManager 行为间接验证，或公开为 pub(crate) 测试）
  - [x] SubTask 3.19: T65 — `compute_stable_status(Open) == Islanded`
  - [x] SubTask 3.20: T66 — `compute_stable_status(Tripped) == Islanded`
  - [x] SubTask 3.21: T67 — `compute_stable_status(Unknown) == Transitioning`
  - [x] SubTask 3.22: T68 — `PccManager::new(...)` 初始化 `state.status == Transitioning` / `is_islanded() == false`
  - [x] SubTask 3.23: T69 — `PccManager::current()` 返回 `&state`
  - [x] SubTask 3.24: T70 — 首次 `update(1000)` 返回 `Closed` → `state.status == Transitioning`（防抖期内）
  - [x] SubTask 3.25: T71 — 第二次 `update(1100)`（debounce_ms=100 已过）返回 `Closed` → `state.status == GridConnected`
  - [x] SubTask 3.26: T72 — `update` 防抖期后 `Open` → `state.status == Islanded` / `is_islanded() == true`
  - [x] SubTask 3.27: T73 — `update` 防抖期后 `Tripped` → `state.status == Islanded`
  - [x] SubTask 3.28: T74 — `update` 防抖期后 `Unknown` → `state.status == Transitioning`
  - [x] SubTask 3.29: T75 — `update` reader 失败 → 返回 `Err(SampleFailed)` / `state` 不变
  - [x] SubTask 3.30: T76 — 稳态 `Closed` 后切换 `Open` → `state.status == Transitioning`（防抖重置）
  - [x] SubTask 3.31: T77 — `update` 功率方向 `Import`（P=10.0 → `state.power_direction == Import`）
  - [x] SubTask 3.32: T78 — `update` 功率方向 `Export`（P=-10.0 → `state.power_direction == Export`）
  - [x] SubTask 3.33: T79 — `update` 功率方向 `Idle`（P=0.5 → `state.power_direction == Idle`）
  - [x] SubTask 3.34: T80 — `update` 功率因数（P=3.0, Q=4.0 → `state.power_factor ≈ 0.6`）
  - [x] SubTask 3.35: T81 — `update` 功率因数（P=0.0, Q=0.0 → `state.power_factor == 1.0`）
  - [x] SubTask 3.36: T82 — `debounce_ms=0` 时首次 `update` 立即稳定（无 Transitioning 中间态）
  - [x] SubTask 3.37: T83 — `update` 两次连续相同 `breaker_status`，第二次（防抖期内）保持 `Transitioning`
  - [x] SubTask 3.38: T84 — `is_islanded()` 在 `status == GridConnected` 时返回 `false`
  - [x] SubTask 3.39: T85 — `update` 后 `state.pcc_id` 始终等于构造时传入的 `pcc_id`（不被 reader 覆盖）
  - [x] SubTask 3.40: T86 — `PccState` 派生 `Copy` 可复制（`let s2 = state;` 后 `s2 == state`）

- [x] Task 4: 修改 `crates/agents/grid_agent/src/lib.rs` — 追加 `pub mod pcc;` + 重导出（surgical）
  - [x] SubTask 4.1: 在 `pub mod publisher;` 之前（按字母序）追加 `pub mod pcc;`
  - [x] SubTask 4.2: 追加 `pub use pcc::{compute_power_direction, compute_power_factor, MockPccReader, PccManager, PccReader, PccReading, PccState, PccStatus, BreakerStatus, PowerDirection};`
  - [x] SubTask 4.3: 顶部模块文档注释追加 v0.83.0 PCC 类型说明（核心类型列表 + D1~D14 偏差表追加到既有 D1~D14 表后，或新增"v0.83.0 PCC 偏差"段落）
  - [x] SubTask 4.4: 不修改任何 v0.82.0 既有代码行（`GridError` 定义 / `impl From` / 既有 `pub mod` / 既有 `pub use` / 既有 46 个测试全部保留）
  - [x] SubTask 4.5: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

- [x] Task 5: 修改 `crates/agents/grid_agent/Cargo.toml` — 更新 description（surgical）
  - [x] SubTask 5.1: `description` 字段更新为 `"EnerOS v0.82.0 Grid Agent — 电网状态感知 + v0.83.0 PCC 并网点管理 (采样/异常检测/DDS 发布/PCC 状态抽象, no_std)"`
  - [x] SubTask 5.2: `[dependencies]` 段不变（无新依赖）
  - [x] SubTask 5.3: workspace members 列表不变（pcc.rs 是既有 crate 的新模块）

- [x] Task 6: 创建配置文件 `configs/pcc.toml`（D8）
  - [x] SubTask 6.1: TOML 模板含 `[pcc]` 段 + `pcc_id` / `debounce_ms` / `breaker_point_id` / `active_power_point_id` / `reactive_power_point_id` 字段（中文注释）
  - [x] SubTask 6.2: 含 `[thresholds]` 段 + `import_power_w` / `export_power_w` / `pf_epsilon` / `pf_warn_below` 字段
  - [x] SubTask 6.3: 含 `[stable_status]` 段说明（或文档注释）breaker 状态到 PccStatus 映射（Closed→GridConnected / Open→Islanded / Tripped→Islanded / Unknown→Transitioning）
  - [x] SubTask 6.4: 含中文注释说明各字段用途（与 v0.82.0 grid_points.toml / v0.79.0 gptp.toml 风格一致）

- [x] Task 7: 创建设计文档 `docs/agents/pcc-management-design.md`（D8）
  - [x] SubTask 7.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 7.2: 至少 1 个 Mermaid 图（PccManager.update 流程图：read → 防抖判定 → 稳定状态计算 → 功率计算）
  - [x] SubTask 7.3: 至少 1 个 Mermaid 图（breaker 状态机：Closed/Open/Tripped/Unknown → GridConnected/Islanded/Transitioning 映射）
  - [x] SubTask 7.4: D1~D14 偏差声明表完整
  - [x] SubTask 7.5: 引用 v0.82.0 Grid Agent 状态感知 + v0.51.0 协议抽象（作为可选未来集成方向）作为前置依赖
  - [x] SubTask 7.6: 包含性能目标说明（状态更新 < 50ms，但标注为"硬件集成阶段验收，本版本仅算法骨架"）
  - [x] SubTask 7.7: 引用 v0.84.0 并离网切换作为下游消费者

- [x] Task 8: 版本同步根目录文件
  - [x] SubTask 8.1: 根 `Cargo.toml` `[workspace.package] version = "0.82.0"` → `"0.83.0"`
  - [x] SubTask 8.2: 根 `Cargo.toml` `[workspace.members]` 列表**不变**（pcc.rs 不是新 crate）
  - [x] SubTask 8.3: `Makefile` 版本号 `0.82.0` → `0.83.0`（header 注释 + VERSION 变量）
  - [x] SubTask 8.4: `.github/workflows/ci.yml` 版本号 `0.82.0` → `0.83.0`
  - [x] SubTask 8.5: `ci/src/gate.rs` clippy 段注释追加 `+ v0.83.0 PCC 管理：PccState / PccReading / BreakerStatus / PowerDirection / PccStatus / PccReader / MockPccReader / PccManager / compute_power_direction / compute_power_factor`
  - [x] SubTask 8.6: `ci/src/gate.rs` test 段注释同步追加类型列表

- [x] Task 9: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 9.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 9.2: `cargo test -p eneros-grid-agent` 全部通过（v0.82.0 T1~T45 + `_ensure_imports_used` + v0.83.0 T47~T86 = 80+ tests + 1 doctest，0 failures）
  - [x] SubTask 9.3: `cargo build -p eneros-grid-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 9.4: `cargo fmt -p eneros-grid-agent -- --check` 通过
  - [x] SubTask 9.5: `cargo clippy -p eneros-grid-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 9.6: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
  - [x] SubTask 9.7: 回归 — `cargo test -p eneros-tsn-time` 仍通过 84 tests + 1 doctest（无回归）
  - [x] SubTask 9.8: 回归 — `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests + 1 doctest（无回归）
  - [x] SubTask 9.9: 回归 — `cargo test -p eneros-device-agent` 仍通过（AgentRuntime trait 未变）

# Task Dependencies

- Task 1（pcc.rs 数据结构 + PccReader + MockPccReader）必须先完成 — Task 2/3 依赖其类型
- Task 2（PccManager + 辅助函数 + 防抖）依赖 Task 1（`PccReading` / `PccReader` / `BreakerStatus` / `GridError`）
- Task 3（测试 T47~T86）依赖 Task 1 + Task 2 完成
- Task 4（lib.rs 修改）依赖 Task 1 + Task 2 完成（需 `pub mod pcc;` 模块存在才能编译）
- Task 5（Cargo.toml description）可与 Task 1~4 并行
- Task 6（configs/pcc.toml）可与 Task 1~5 并行
- Task 7（docs/agents/pcc-management-design.md）可与 Task 1~6 并行
- Task 8（版本同步根目录文件）依赖 Task 1~7 完成
- Task 9（构建校验）依赖所有前置任务完成

## 并行化建议

- **Sub-Agent A**：Task 1 + Task 2 + Task 3（pcc.rs 完整实现 + 测试，单文件单 agent 串行）
- **Sub-Agent B**：Task 4（lib.rs surgical 修改）— 必须在 Task 1~3 完成后执行
- **Sub-Agent C**：Task 6（configs/pcc.toml）+ Task 7（docs）— 可与 A 并行
- **Sub-Agent D**：Task 5（Cargo.toml description）+ Task 8（版本同步）— Task 5 可与 A 并行；Task 8 须在 A+B 完成后
- **最终串行**：Task 9 由主 agent 在 A/B/C/D 全部完成后执行
