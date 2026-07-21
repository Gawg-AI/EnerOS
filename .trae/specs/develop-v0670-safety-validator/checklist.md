# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.67.0`
- [x] C2 members 列表已添加 `crates/ai/safety-validator`（置于 `crates/ai/energy-lp-model` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/safety-validator/Cargo.toml` 存在，package name = `eneros-safety-validator`
- [x] C5 dependencies 包含 `eneros-energy-lp-model = { path = "../energy-lp-model" }`（复用 ScheduleResult/ScheduleEntry，D8）
- [x] C5.1 dev-dependencies 包含 `eneros-solver-core = { path = "../solver-core" }`（测试构造 ScheduleResult 需 SolveStatus）
- [x] C6 **不声明** `[features]`（D7：纯 Rust，无 FFI）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + `#![allow(dead_code)]`（D12）
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：rule / validator / electrical / protection / state / result

## state.rs — SystemState（D2）
- [x] C10 `SystemState` 结构体：voltage_v: f64 / current_a: f64 / frequency_hz: f64 / soc_pct: f64 / timestamp_ms: u64
- [x] C11 派生 `Debug` + `Clone` + 手动实现 `Default`（默认 voltage=380.0, current=0.0, freq=50.0, soc=0.5, ts=0）
- [x] C12 单元测试：默认值验证（T1）

## result.rs — ValidationResult + Violation + Severity
- [x] C13 `Severity` 枚举：Info / Warning / Critical / Fatal（派生 `Debug` + `Clone` + `Copy` + `PartialEq`，D8）
- [x] C14 `Violation` 结构体：rule: String / period: usize / field: String / original_value: f64 / safe_value: f64 / severity: Severity
- [x] C15 `ValidationResult` 结构体：passed: bool / clamped: bool / clamped_schedule: Option<ScheduleResult> / violations: Vec<Violation>
- [x] C16 Violation + ValidationResult 派生 `Debug` + `Clone`（D9：不派生 PartialEq）
- [x] C17 单元测试：Severity 枚举 + PartialEq（T2/T3/T4）

## rule.rs — SafetyRule trait（D1）
- [x] C18 `SafetyRule` trait 定义（**无 Send + Sync bound**，D1）
- [x] C19 必需方法：`name() -> &str` / `validate(&self, schedule: &ScheduleResult, state: &SystemState) -> ValidationResult`
- [x] C20 默认方法：`priority() -> u32`（默认 100）/ `is_hard() -> bool`（默认 true）
- [x] C21 trait 编译通过

## electrical.rs — ElectricalSafetyRule
- [x] C22 `ElectricalSafetyRule` 结构体：max_power_kw / max_current_a / voltage_range: (f64, f64) / freq_range: (f64, f64)（D12：保留全部字段）
- [x] C23 `ElectricalSafetyRule::new(max_power_kw, max_current_a, voltage_range, freq_range) -> Self`
- [x] C24 实现 `SafetyRule`：name="electrical_safety" / priority=10 / is_hard=true
- [x] C25 validate：校验 charge_power_kw ≤ max_power_kw（超限截断，Critical）
- [x] C26 validate：校验 discharge_power_kw ≤ max_power_kw（超限截断，Critical）
- [x] C27 validate：校验 soc_pct ≤ 0.95（截断，Critical）
- [x] C28 validate：校验 soc_pct ≥ 0.05（截断，Fatal）
- [x] C29 D11：精确复制蓝图截断逻辑 — **不**重算 net_power_kw / revenue_yuan（蓝图原文亦未重算）
- [x] C30 `passed = violations.iter().all(|v| v.severity != Severity::Fatal)`
- [x] C31 单元测试：全部通过 / 功率超限 / SOC 上限 / SOC 下限致命（T5~T10）

## protection.rs — ProtectionCoordinationRule
- [x] C32 `ProtectionCoordinationRule` 结构体：overcurrent_threshold / overvoltage_threshold / undervoltage_threshold / freq_protection: (f64, f64) / max_ramp_rate
- [x] C33 `ProtectionCoordinationRule::new(...) -> Self`
- [x] C34 实现 `SafetyRule`：name="protection_coordination" / priority=20 / is_hard=true
- [x] C35 validate：遍历相邻时段，计算 `delta_per_min = |curr_net - prev_net| / 0.25`
- [x] C36 validate：超限时按蓝图截断逻辑（safe_delta = max_ramp_rate * 0.25 * curr.signum()，`discharge += diff.max(0.0); charge += (-diff).max(0.0)`）
- [x] C37 D11：精确复制蓝图截断逻辑 — **不**重算 net_power_kw
- [x] C38 `passed = violations.iter().all(|v| v.severity != Severity::Fatal)`
- [x] C39 单元测试：正常 / 爬坡超限截断（T11~T13）

## validator.rs — SafetyValidator
- [x] C40 `SafetyValidator` 结构体：rules: Vec<Box<dyn SafetyRule>>（D4：alloc::boxed::Box + alloc::vec::Vec）
- [x] C41 `SafetyValidator::new() -> Self` — 注册 ElectricalSafetyRule（max_power=100, max_current=200, voltage=(340,420), freq=(49.5,50.5)）+ ProtectionCoordinationRule（overcurrent=220, overvoltage=440, undervoltage=320, freq_prot=(49,51), max_ramp=200）
- [x] C42 `add_rule(&mut self, rule: Box<dyn SafetyRule>)` — 添加并按 `priority()` 排序（D5：sort_by_key）
- [x] C43 `validate(&self, schedule: &ScheduleResult, state: &SystemState) -> ValidationResult` — 链式执行
- [x] C44 链式执行：前序规则截断后继续执行后续规则（前序 result.clamped_schedule 喂给下一规则）
- [x] C45 致命违规（Fatal）立即终止后续规则
- [x] C46 截断后返回 `clamped_schedule: Some(...)`，无截断返回 `None`
- [x] C46.1 修复蓝图 borrow-after-move：先缓存 `has_fatal` 标志再 `extend`
- [x] C46.2 实现 `Default` trait（委托 `new()`）
- [x] C47 单元测试：new 默认规则数 = 2 / 全部通过 / 链式截断 / 致命终止 / add_rule（T14~T19）

## 集成测试（lib.rs）— 23 项
- [x] C48 T1 SystemState::default 默认值
- [x] C49 T2 Severity 枚举变体 + PartialEq
- [x] C50 T3 Violation 构造 + 字段访问
- [x] C51 T4 ValidationResult 构造
- [x] C52 T5 ElectricalSafetyRule::new 构造
- [x] C53 T6 ElectricalSafetyRule validate 全部通过
- [x] C54 T7 ElectricalSafetyRule validate 充电功率超限截断
- [x] C55 T8 ElectricalSafetyRule validate 放电功率超限截断
- [x] C56 T9 ElectricalSafetyRule validate SOC 上限截断
- [x] C57 T10 ElectricalSafetyRule validate SOC 下限致命
- [x] C58 T11 ProtectionCoordinationRule::new 构造
- [x] C59 T12 ProtectionCoordinationRule validate 爬坡率正常
- [x] C60 T13 ProtectionCoordinationRule validate 爬坡率超限截断
- [x] C61 T14 SafetyValidator::new 默认注册 2 条规则
- [x] C62 T15 SafetyValidator validate 全部通过
- [x] C63 T16 SafetyValidator validate 链式截断
- [x] C64 T17 SafetyValidator validate 致命违规立即终止
- [x] C65 T18 SafetyValidator add_rule 自定义规则插队（priority=5 排首位）
- [x] C66 T19 SafetyValidator validate 截断后 clamped_schedule 存在
- [x] C67 T20 D11 验证：截断后 net_power_kw **不**重算（蓝图精确复制）
- [x] C68 T21 SafetyRule trait 默认方法（priority=100, is_hard=true）
- [x] C69 T22 端到端：ScheduleResult → validate → ValidationResult
- [x] C69.1 T23 附加：SafetyValidator::default 等价于 new()
- [x] C70 `cargo test -p eneros-safety-validator` 23/23 通过

## 设计文档
- [x] C71 `docs/ai/safety-validator-design.md` 存在（2285 行）
- [x] C72 12 章节完整
- [x] C73 3 Mermaid 图（类图 + 时序图 + 双重屏障架构图）
- [x] C74 D1~D12 偏差声明表（§11 完整 12 行 + 一致性/可追溯性/对照三小节）
- [x] C75 文档在 `docs/ai/` 下（符合目录规范）

## 版本同步
- [x] C76 `Makefile` 版本号 `0.67.0`（header + VERSION 变量 2 处）
- [x] C77 `.github/workflows/ci.yml` 版本号 `0.67.0`
- [x] C78 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-safety-validator`（2 处）

## 构建校验（§2.4.2 C6~C11）
- [x] C79 `cargo metadata --format-version 1` 成功
- [x] C80 `cargo test -p eneros-safety-validator` 全部通过（23 tests, 0 failed）
- [x] C81 `cargo build -p eneros-safety-validator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过（1.21s）
- [x] C82 `cargo fmt -p eneros-safety-validator -- --check` 通过
- [x] C83 `cargo clippy -p eneros-safety-validator --all-targets -- -D warnings` 无 warning
- [x] C84 `cargo deny check licenses bans sources` 通过（bans ok, licenses ok, sources ok）

## no_std 合规
- [x] C85 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C86 无 `panic!` / `todo!` / `unimplemented!`
- [x] C87 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
- [x] C88 无 `unsafe` 块（纯 safe Rust）
- [x] C89 无 `Send + Sync` bounds（D1）

## 目录规范
- [x] C90 crate 在 `crates/ai/safety-validator/`（D6 隐含）
- [x] C91 跨 crate path 引用 `../energy-lp-model` + `../solver-core`（相对路径）
- [x] C92 文档在 `docs/ai/` 下
- [x] C93 无根目录 crate（除 `ci/`）
- [x] C94 无垃圾文件（`target/` / `*.elf` / `*.bin` 被忽略）

## 依赖复用（D8）
- [x] C95 复用 v0.66.0 `ScheduleResult` / `ScheduleEntry`（不重定义）
- [x] C96 复用 v0.64.0 `SolveStatus`（通过 dev-dependency `eneros-solver-core`，仅测试用）
- [x] C97 **不依赖** v0.56.0 ConstraintChecker / v0.57.0 DegradeEngine / v0.52.0 telemetry-model（D6）

## 简化设计验证（Karpathy 原则）
- [x] C98 无 `Send + Sync` bounds（D1：与 v0.59.0/v0.63.0/v0.64.0 一致）
- [x] C99 无 `PartialEq` 派生于 ValidationResult/Violation（D9：当前测试不需要）
- [x] C100 无 `[features]` 段（D7：纯 Rust）

## SystemState 本地定义（D2）
- [x] C101 本地定义 `SystemState`（不依赖 HMI crate）
- [x] C102 SystemState 字段满足安全校验需求（voltage/current/frequency/soc/timestamp）

## 截断策略（D11）
- [x] C103 截断到安全边界而非拒绝（蓝图 §5）
- [x] C104 D11：精确复制蓝图截断逻辑 — **不**重算 net_power_kw（蓝图原文未重算，T20 测试验证）
- [x] C105 致命违规（Fatal）立即终止并返回 passed=false
