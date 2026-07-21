# Checklist

## Task 1: pcc.rs — 数据结构 + PccReader trait + MockPccReader
- [x] C1: `crates/agents/grid_agent/src/pcc.rs` 文件创建
- [x] C2: `BreakerStatus` 枚举 4 变体 `Closed` / `Open` / `Tripped` / `Unknown`
- [x] C3: `BreakerStatus` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Unknown`）
- [x] C4: `PowerDirection` 枚举 3 变体 `Import` / `Export` / `Idle`
- [x] C5: `PowerDirection` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Idle`）
- [x] C6: `PccStatus` 枚举 3 变体 `GridConnected` / `Islanded` / `Transitioning`
- [x] C7: `PccStatus` 派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（`#[default]` on `Transitioning`）
- [x] C8: `PccReading` 结构体 3 字段（`breaker_status` / `active_power` / `reactive_power`）
- [x] C9: `PccReading` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C10: `PccState` 结构体 7 字段（`pcc_id: u32` / `breaker_status` / `power_direction` / `power_factor: f32` / `active_power: f32` / `reactive_power: f32` / `status`）
- [x] C11: `PccState` 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C12: `PccReader` trait 定义 `fn read(&mut self, pcc_id: u32, now_ms: u64) -> Result<PccReading, GridError>;`
- [x] C13: `PccReader` 不要求 `Send + Sync`（D3）
- [x] C14: `MockPccReader` 结构体字段 `next_reading: PccReading` / `fail: bool`，派生 `Debug, Clone`
- [x] C15: `MockPccReader::new(reading: PccReading) -> Self`（`fail = false`）
- [x] C16: `MockPccReader::new_failing() -> Self`（`fail = true`，`next_reading = PccReading::default()`）
- [x] C17: `MockPccReader::with_reading(mut self, reading: PccReading) -> Self` builder
- [x] C18: `impl PccReader for MockPccReader` — `fail == true` 返回 `Err(GridError::SampleFailed)`；否则 `Ok(self.next_reading)`
- [x] C19: `pcc.rs` 使用 `use crate::GridError;`（复用 v0.82.0 错误类型，D4）
- [x] C20: `pcc.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`

## Task 2: pcc.rs — PccManager + 辅助函数 + 防抖
- [x] C21: `compute_power_direction(active_power: f32) -> PowerDirection` 公开函数
- [x] C22: `compute_power_direction` — `> 1.0 → Import` / `< -1.0 → Export` / 否则 `Idle`（D11）
- [x] C23: `compute_power_factor(active_power: f32, reactive_power: f32) -> f32` 公开函数
- [x] C24: `compute_power_factor` — `|active_power| < 0.1 → 1.0`；否则 `active_power / (p*p+q*q).sqrt()` 使用 `core::f32::sqrt`（D7）
- [x] C25: `compute_stable_status(breaker: BreakerStatus) -> PccStatus` 私有函数
- [x] C26: `compute_stable_status` 映射：`Closed → GridConnected` / `Open → Islanded` / `Tripped → Islanded` / `Unknown → Transitioning`
- [x] C27: `PccManager` 结构体 6 字段（`pcc_id: u32` / `reader: Box<dyn PccReader>` / `state: PccState` / `debounce_ms: u64` / `last_breaker_status: BreakerStatus` / `last_change_ms: u64`）
- [x] C28: `PccManager::new(pcc_id, reader, debounce_ms)` 初始化 `state = PccState::default()` / `last_breaker_status = Unknown` / `last_change_ms = 0`
- [x] C29: `PccManager::current(&self) -> &PccState` 返回 `&self.state`
- [x] C30: `PccManager::is_islanded(&self) -> bool` 返回 `self.state.status == PccStatus::Islanded`
- [x] C31: `PccManager::update(&mut self, now_ms: u64) -> Result<PccState, GridError>` 存在
- [x] C32: `update` 调用 `reader.read(self.pcc_id, now_ms)`，失败返回 `Err(GridError::SampleFailed)`
- [x] C33: `update` 防抖逻辑 D6：`new_breaker != last_breaker_status` → 更新 `last_breaker_status` / `last_change_ms = now_ms` / `state.status = Transitioning`
- [x] C34: `update` 防抖逻辑 D6：相同 breaker 且 `now_ms - last_change_ms >= debounce_ms` → `state.status = compute_stable_status(new_breaker)`
- [x] C35: `update` 防抖逻辑 D6：相同 breaker 但防抖期内 → 保持 `Transitioning`
- [x] C36: `update` 更新 `state.breaker_status` / `active_power` / `reactive_power` / `power_direction` / `power_factor`
- [x] C37: `update` 返回 `Ok(self.state)`（Copy 语义，无 clone 堆分配）
- [x] C38: `PccManager` 字段 `reader` 使用 `use alloc::boxed::Box;`
- [x] C39: `pcc.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 3: pcc.rs — 单元测试 T47~T86
- [x] C40: T47 — `PccState::default()` 全字段默认值
- [x] C41: T48 — `BreakerStatus::default() == Unknown`
- [x] C42: T49 — `PowerDirection::default() == Idle`
- [x] C43: T50 — `PccStatus::default() == Transitioning`
- [x] C44: T51 — `PccReading::default()` 全默认
- [x] C45: T52 — `MockPccReader::new(reading)` fail=false
- [x] C46: T53 — `MockPccReader::new_failing()` fail=true
- [x] C47: T54 — `MockPccReader::with_reading(reading)` builder
- [x] C48: T55 — `MockPccReader::read()` 成功路径返回 `Ok(reading)`
- [x] C49: T56 — `MockPccReader::read()` 失败路径返回 `Err(SampleFailed)`
- [x] C50: T57 — `compute_power_direction(10.0) == Import`
- [x] C51: T58 — `compute_power_direction(-10.0) == Export`
- [x] C52: T59 — `compute_power_direction(0.5) == Idle`
- [x] C53: T60 — `compute_power_direction(-0.5) == Idle`（|P|≤1.0）
- [x] C54: T61 — `compute_power_factor(3.0, 4.0) ≈ 0.6`（误差 < 1e-6）
- [x] C55: T62 — `compute_power_factor(0.0, 0.0) == 1.0`
- [x] C56: T63 — `compute_power_factor(0.05, 0.0) == 1.0`（|P|<0.1 阈值）
- [x] C57: T64 — `compute_stable_status(Closed) == GridConnected`（间接通过 PccManager 行为验证）
- [x] C58: T65 — `compute_stable_status(Open) == Islanded`
- [x] C59: T66 — `compute_stable_status(Tripped) == Islanded`
- [x] C60: T67 — `compute_stable_status(Unknown) == Transitioning`
- [x] C61: T68 — `PccManager::new(...)` 初始化 `status == Transitioning` / `is_islanded() == false`
- [x] C62: T69 — `PccManager::current()` 返回 `&state`
- [x] C63: T70 — 首次 `update(1000)` 返回 `Closed` → `status == Transitioning`（防抖期内）
- [x] C64: T71 — 第二次 `update(1100)`（debounce_ms=100 已过）→ `status == GridConnected`
- [x] C65: T72 — `update` 防抖期后 `Open` → `status == Islanded` / `is_islanded() == true`
- [x] C66: T73 — `update` 防抖期后 `Tripped` → `status == Islanded`
- [x] C67: T74 — `update` 防抖期后 `Unknown` → `status == Transitioning`
- [x] C68: T75 — `update` reader 失败 → `Err(SampleFailed)` / `state` 不变
- [x] C69: T76 — 稳态 `Closed` 后切换 `Open` → `status == Transitioning`（防抖重置）
- [x] C70: T77 — `update` 功率方向 `Import`（P=10.0）
- [x] C71: T78 — `update` 功率方向 `Export`（P=-10.0）
- [x] C72: T79 — `update` 功率方向 `Idle`（P=0.5）
- [x] C73: T80 — `update` 功率因数（P=3.0, Q=4.0 → `power_factor ≈ 0.6`）
- [x] C74: T81 — `update` 功率因数（P=0.0, Q=0.0 → `power_factor == 1.0`）
- [x] C75: T82 — `debounce_ms=0` 时首次 `update` 立即稳定
- [x] C76: T83 — 两次连续相同 breaker，第二次（防抖期内）保持 `Transitioning`
- [x] C77: T84 — `is_islanded()` 在 `status == GridConnected` 时返回 `false`
- [x] C78: T85 — `update` 后 `state.pcc_id` 始终等于构造时传入的 `pcc_id`
- [x] C79: T86 — `PccState` 派生 `Copy` 可复制

## Task 4: lib.rs surgical 修改
- [x] C80: `pub mod pcc;` 在 `pub mod publisher;` 之前（按字母序）
- [x] C81: `pub use pcc::{compute_power_direction, compute_power_factor, MockPccReader, PccManager, PccReader, PccReading, PccState, PccStatus, BreakerStatus, PowerDirection};` 重导出
- [x] C82: 顶部模块文档注释追加 v0.83.0 PCC 类型说明 + D1~D14 偏差表
- [x] C83: v0.82.0 既有 `pub mod publisher;` / `pub mod sampler;` / `pub mod state;` 保留不变
- [x] C84: v0.82.0 既有 `pub use publisher::{...};` / `pub use sampler::{...};` / `pub use state::{...};` 保留不变
- [x] C85: v0.82.0 既有 `GridError` 枚举定义与 `impl From<GridError> for AgentRuntimeError` 保留不变
- [x] C86: v0.82.0 既有 46 个测试（T1~T45 + `_ensure_imports_used`）保留不变
- [x] C87: `lib.rs` 无 `use std::*` / 无 `async` / 无 `panic!` / 无 `unsafe`

## Task 5: Cargo.toml description 更新
- [x] C88: `description` 字段更新为含 "v0.83.0 PCC 并网点管理" 字样
- [x] C89: `[dependencies]` 段不变（仍为 `eneros-agent` + `eneros-energy-market-agent`）
- [x] C90: workspace members 列表不变

## Task 6: configs/pcc.toml
- [x] C91: 文件位于 `configs/pcc.toml`
- [x] C92: TOML 模板含 `[pcc]` 段 + `pcc_id` / `debounce_ms` / `breaker_point_id` / `active_power_point_id` / `reactive_power_point_id` 字段
- [x] C93: 含 `[thresholds]` 段 + `import_power_w` / `export_power_w` / `pf_epsilon` / `pf_warn_below` 字段
- [x] C94: 含 `[stable_status]` 段或文档注释说明 breaker→PccStatus 映射
- [x] C95: 含中文注释说明各字段用途（与 v0.82.0 grid_points.toml 风格一致）

## Task 7: docs/agents/pcc-management-design.md
- [x] C96: 文件位于 `docs/agents/pcc-management-design.md`（非 `docs/phase2/`，符合 D8 + 工作区规则 §2.3.3）
- [x] C97: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
- [x] C98: 至少 1 个 Mermaid 图（PccManager.update 流程图）
- [x] C99: 至少 1 个 Mermaid 图（breaker 状态机或异常检测决策流程图）
- [x] C100: D1~D14 偏差声明表完整
- [x] C101: 引用 v0.82.0 Grid Agent 状态感知 + v0.51.0 协议抽象（可选未来集成）作为前置依赖
- [x] C102: 包含性能目标说明（更新延迟 < 50ms，标注为"硬件集成阶段验收，本版本仅算法骨架"）
- [x] C103: 引用 v0.84.0 并离网切换作为下游消费者

## Task 8: 版本同步根目录文件
- [x] C104: 根 `Cargo.toml` 顶层 `[workspace.package] version = "0.83.0"`
- [x] C105: 根 `Cargo.toml` `[workspace.members]` 列表**不变**（pcc.rs 是既有 crate 的新模块）
- [x] C106: `Makefile` 中 `# Version: v0.83.0` 与 `VERSION := 0.83.0`
- [x] C107: `.github/workflows/ci.yml` 中 `# Version: v0.83.0`
- [x] C108: `ci/src/gate.rs` clippy 段注释含 `+ v0.83.0 PCC 管理：PccState / PccReading / BreakerStatus / PowerDirection / PccStatus / PccReader / MockPccReader / PccManager / compute_power_direction / compute_power_factor`
- [x] C109: `ci/src/gate.rs` test 段注释同步追加类型列表

## Task 9: 构建校验（§2.4.2）
- [x] C110: `cargo metadata --format-version 1` 成功
- [x] C111: `cargo test -p eneros-grid-agent` 全部通过（v0.82.0 T1~T45 + `_ensure_imports_used` + v0.83.0 T47~T86 = 80+ tests + 1 doctest，0 failures）
- [x] C112: `cargo build -p eneros-grid-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 退出码 0
- [x] C113: `cargo fmt -p eneros-grid-agent -- --check` 退出码 0
- [x] C114: `cargo clippy -p eneros-grid-agent --all-targets -- -D warnings` 无 warning，退出码 0
- [x] C115: `cargo deny check advisories licenses bans sources` 通过（无新依赖引入）
- [x] C116: 回归 — `cargo test -p eneros-tsn-time` 仍通过 84 tests + 1 doctest（无回归）
- [x] C117: 回归 — `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests + 1 doctest（无回归）
- [x] C118: 回归 — `cargo test -p eneros-device-agent` 仍通过（AgentRuntime trait 未变）

## 总体校验
- [x] C119: 无根目录新 crate（`crates/agents/grid_agent/` 既有 crate 追加模块，符合 §2.3.1）
- [x] C120: 无 `docs/` 根目录平面化文档（新文档在 `docs/agents/` 下）
- [x] C121: 无 `config/` 目录（新配置在 `configs/pcc.toml`）
- [x] C122: `.gitignore` 未需更新（无新文件类型）
- [x] C123: `git status` 无 `target/` / `*.elf` / `*.bin` / `*.dtb` / IDE 缓存被追踪
- [x] C124: 提交信息遵循 Conventional Commits（如 `feat(agents/grid_agent): v0.83.0 实现 PCC 并网点管理`）
- [x] C125: ADR 决策未被违反（未引入研究特性、未自研已有开源替代组件、未超出 v1.0.0 范围）
- [x] C126: no_std 合规性：`pcc.rs` 继承 crate 级 `#![cfg_attr(not(test), no_std)]`
- [x] C127: 内存预算：PCC 模块 ≤ 1MB（蓝图 §8.3 声明，本版本为算法骨架，实际占用远小于此）
- [x] C128: SBOM 未变化（无新第三方依赖，仅复用 workspace 内既有 crate `eneros-agent` / `eneros-energy-market-agent`）
- [x] C129: 文档同步：v0.82.0 历史偏差声明保留，v0.83.0 新增 D1~D14 段落
- [x] C130: Surgical Changes 原则：v0.82.0 既有源文件 `state.rs` / `sampler.rs` / `publisher.rs` 完全未改动
- [x] C131: `lib.rs` 仅追加 `pub mod pcc;` + 重导出 + 顶部文档注释（不修改任何 v0.82.0 既有代码行）
