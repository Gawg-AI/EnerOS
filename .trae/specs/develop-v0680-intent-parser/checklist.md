# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.68.0`
- [x] C2 members 列表已添加 `crates/ai/intent-parser`（置于 `crates/ai/safety-validator` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/intent-parser/Cargo.toml` 存在，package name = `eneros-intent-parser`
- [x] C5 dependencies 包含 `eneros-energy-lp-model` / `eneros-safety-validator` / `eneros-solver-model` / `eneros-solver-core`
- [x] C6 dependencies 包含 `serde = { version = "1.0", default-features = false, features = ["alloc", "derive"] }`（D1）
- [x] C7 dependencies 包含 `serde_json = { version = "1.0", default-features = false, features = ["alloc"] }`（D1）
- [x] C8 无 `[features]` 段
- [x] C9 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`（D10）
- [x] C10 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C11 模块声明：error / intent / parser

## error.rs — IntentError
- [x] C12 `IntentError` 枚举：`ParseError(String)` / `InvalidConfig(String)` / `ConstraintConflict(String)` / `CompileError(String)`
- [x] C13 派生 `Debug`（D7：不派生 Clone/PartialEq）
- [x] C14 使用 `alloc::string::String`

## intent.rs — Intent + IntentType + TimeRange + PowerIntent + SocIntent
- [x] C15 `IntentType` 枚举：Charge / Discharge / Hold / Stop / EmergencyStop / AutonomousSchedule / SetSetpoint
- [x] C16 `IntentType` 派生 `Debug + Clone + PartialEq + Serialize + Deserialize`（D8）
- [x] C17 `TimeRange` 结构体：`start_period: usize` / `end_period: usize`
- [x] C18 `PowerIntent` 结构体：`power_kw: f64` / `power_ratio: Option<f64>`
- [x] C19 `SocIntent` 结构体：`target_soc: f64` / `by_period: usize`
- [x] C20 `Intent` 结构体：intent_type / time_range / power / soc_target / priority / reason / confidence
- [x] C21 `priority` 加 `#[serde(default = "default_priority")]`（默认 3，D9）
- [x] C22 `reason` 加 `#[serde(default)]`（默认空字符串，D9）
- [x] C23 `confidence` 加 `#[serde(default)]`（默认 0.0，D9）
- [x] C24 TimeRange/PowerIntent/SocIntent/Intent 派生 `Debug + Clone + Serialize + Deserialize`
- [x] C25 编译通过

## parser.rs — IntentParser
- [x] C26 `IntentParser` 结构体：`default_config: ScheduleConfig` / `system_state: SystemState`
- [x] C27 `IntentParser::new(default_config: ScheduleConfig, state: SystemState) -> Self`
- [x] C28 `parse_json(&self, json: &str) -> Result<Intent, IntentError>` — 使用 `serde_json::from_str`（D1）
- [x] C29 `to_schedule_config` AutonomousSchedule 分支：设 `soc_final` 如有
- [x] C30 `to_schedule_config` Charge 分支：修改 `price[t]` 为负值（D3：安全索引 `get_mut`）
- [x] C31 `to_schedule_config` Discharge 分支：修改 `price[t]` 为高值（D3：安全索引）
- [x] C32 `to_schedule_config` Hold 分支：`pcs_power_kw = 0.0`
- [x] C33 `to_schedule_config` Stop 分支：`pcs_power_kw = 0.0`
- [x] C34 `to_schedule_config` EmergencyStop 分支：`pcs_power_kw = 0.0` + `soc_min = soc_max = system_state.soc_pct`（D2）
- [x] C35 `to_schedule_config` SetSetpoint 分支：`pcs_power_kw = power.power_kw.abs()`
- [x] C36 `to_schedule_config` 末尾调用 `validate_config`
- [x] C37 `to_opt_problem` 调用 `to_schedule_config` + `EnergyScheduleModel::new(config.clone())`（D11）+ `model.compile().map_err(...)`（D4）
- [x] C38 `validate_config` 校验：num_periods==0 / pcs_power_kw<0 / soc 范围 / price 长度
- [x] C39 实现 `Default` for `IntentParser`（使用 ScheduleConfig::default() + SystemState::default()）
- [x] C40 编译通过

## 集成测试（lib.rs）
- [x] C41 T1 IntentType 枚举变体 + PartialEq
- [x] C42 T2 TimeRange 构造
- [x] C43 T3 PowerIntent 构造（含 power_ratio）
- [x] C44 T4 SocIntent 构造
- [x] C45 T5 Intent 构造（全字段）
- [x] C46 T6 Intent serde 反序列化（缺字段时默认值）
- [x] C47 T7 IntentParser::new 构造
- [x] C48 T8 IntentParser::default 等价 new
- [x] C49 T9 parse_json 完整 JSON 成功
- [x] C50 T10 parse_json 缺可选字段成功
- [x] C51 T11 parse_json 无效 JSON 返回 ParseError
- [x] C52 T12 to_schedule_config AutonomousSchedule
- [x] C53 T13 to_schedule_config Charge（price 负值）
- [x] C54 T14 to_schedule_config Discharge（price 高值）
- [x] C55 T15 to_schedule_config Hold（pcs_power=0）
- [x] C56 T16 to_schedule_config Stop（pcs_power=0）
- [x] C57 T17 to_schedule_config EmergencyStop（soc 冻结）
- [x] C58 T18 to_schedule_config SetSetpoint（pcs_power 调整）
- [x] C59 T19 validate_config 合理配置通过
- [x] C60 T20 validate_config 非法配置失败（4 种错误情况）
- [x] C61 T21 to_opt_problem 端到端
- [x] C62 T22 端到端：JSON → Intent → Config → Validate
- [x] C63 `cargo test -p eneros-intent-parser` 22/22 通过

## 设计文档
- [x] C64 `docs/ai/intent-parser-design.md` 存在
- [x] C65 12 章节完整
- [x] C66 2 Mermaid 图（类图 + 意图转换流程图）
- [x] C67 D1~D12 偏差声明表
- [x] C68 文档在 `docs/ai/` 下

## 版本同步
- [x] C69 `Makefile` 版本号 `0.68.0`（header + VERSION 变量 2 处）
- [x] C70 `.github/workflows/ci.yml` 版本号 `0.68.0`
- [x] C71 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-intent-parser`

## 构建校验（§2.4.2 C6~C11）
- [x] C72 `cargo metadata --format-version 1` 成功
- [x] C73 `cargo test -p eneros-intent-parser` 全部通过（22 tests）
- [x] C74 `cargo build -p eneros-intent-parser --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C75 `cargo fmt -p eneros-intent-parser -- --check` 通过
- [x] C76 `cargo clippy -p eneros-intent-parser --all-targets -- -D warnings` 无 warning
- [x] C77 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C78 无 `use std::*`（仅 `alloc::*` / `core::*` / `serde` / `serde_json`）
- [x] C79 无 `panic!` / `todo!` / `unimplemented!`
- [x] C80 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C81 无 `unsafe` 块
- [x] C82 serde / serde_json 使用 `no_std + alloc` 配置

## 目录规范
- [x] C83 crate 在 `crates/ai/intent-parser/`
- [x] C84 跨 crate path 引用均为相对路径（`../energy-lp-model` / `../safety-validator` / `../solver-model` / `../solver-core`）
- [x] C85 文档在 `docs/ai/` 下
- [x] C86 无根目录 crate（除 `ci/`）
- [x] C87 无垃圾文件

## 依赖复用（D5/D6）
- [x] C88 复用 v0.66.0 `ScheduleConfig` / `ScheduleEntry`（不重定义）
- [x] C89 复用 v0.67.0 `SystemState`（D6：通过 `eneros-safety-validator` 依赖）
- [x] C90 复用 v0.65.0 `EnergyScheduleModel`（不重定义）
- [x] C91 复用 v0.64.0 `LpProblem` / `SolverError`（不重定义）
- [x] C92 **不依赖** v0.26.0 配置管理系统（D5）

## 简化设计验证（Karpathy 原则）
- [x] C93 `IntentError` 不派生 Clone/PartialEq（D7：Simplicity First）
- [x] C94 `IntentType` 派生 PartialEq（D8：match 需要 + 测试断言）
- [x] C95 `Intent` serde 默认值容错（D9：priority/reason/confidence `#[serde(default)]`）
- [x] C96 无 `[features]` 段（纯 Rust）

## 安全索引（D3）
- [x] C97 `config.price.get_mut(t)` 使用安全访问而非直接 `config.price[t]`
- [x] C98 `time_range.start_period`/`end_period` 边界检查（min with num_periods-1）

## 错误映射（D4）
- [x] C99 `SolverError` → `IntentError::CompileError` 映射（不使用 `?` 直接传播）
- [x] C100 `serde_json::Error` → `IntentError::ParseError` 映射
