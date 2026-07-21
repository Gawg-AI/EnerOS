# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.67.0` → `0.68.0`
  - [x] members 添加 `crates/ai/intent-parser`（置于 `crates/ai/safety-validator` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 `eneros-intent-parser` crate 骨架
  - [x] 新建 `crates/ai/intent-parser/Cargo.toml`，package name = `eneros-intent-parser`
  - [x] dependencies：`eneros-energy-lp-model` / `eneros-safety-validator` / `eneros-solver-model` / `eneros-solver-core` / `serde`（derive+alloc） / `serde_json`（alloc）
  - [x] 无 `[features]` 段（D7：纯 Rust，无 FFI）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / intent / parser
  - [x] lib.rs 包含 D1~D12 偏差声明表

- [x] Task 3: 实现 `error.rs` — IntentError
  - [x] `IntentError` 枚举：`ParseError(String)` / `InvalidConfig(String)` / `ConstraintConflict(String)` / `CompileError(String)`
  - [x] 派生 `Debug`（D7：不派生 Clone/PartialEq）
  - [x] 使用 `alloc::string::String`

- [x] Task 4: 实现 `intent.rs` — Intent + IntentType + TimeRange + PowerIntent + SocIntent
  - [x] `IntentType` 枚举：7 种变体，派生 `Debug + Clone + PartialEq + Serialize + Deserialize`（D8）
  - [x] `TimeRange` 结构体：`start_period: usize` / `end_period: usize`
  - [x] `PowerIntent` 结构体：`power_kw: f64` / `power_ratio: Option<f64>`
  - [x] `SocIntent` 结构体：`target_soc: f64` / `by_period: usize`
  - [x] `Intent` 结构体：intent_type / time_range / power / soc_target / priority / reason / confidence
  - [x] `priority` 加 `#[serde(default = "default_priority")]`（默认 3，D9）
  - [x] `reason` 加 `#[serde(default)]`（默认空字符串，D9）
  - [x] `confidence` 加 `#[serde(default)]`（默认 0.0，D9）
  - [x] 派生 `Debug + Clone + Serialize + Deserialize`

- [x] Task 5: 实现 `parser.rs` — IntentParser 主接口
  - [x] `IntentParser` 结构体：`default_config: ScheduleConfig` / `system_state: SystemState`
  - [x] `IntentParser::new(default_config: ScheduleConfig, state: SystemState) -> Self`
  - [x] `parse_json(&self, json: &str) -> Result<Intent, IntentError>` — `serde_json::from_str`（D1）
  - [x] `to_schedule_config` AutonomousSchedule：设 `soc_final` 如有
  - [x] `to_schedule_config` Charge：修改 `price[t]` 为负值（D3：安全索引 `get_mut`）
  - [x] `to_schedule_config` Discharge：修改 `price[t]` 为高值（D3：安全索引）
  - [x] `to_schedule_config` Hold：`pcs_power_kw = 0.0`
  - [x] `to_schedule_config` Stop：`pcs_power_kw = 0.0`
  - [x] `to_schedule_config` EmergencyStop：`pcs_power_kw = 0.0` + `soc_min = soc_max = system_state.soc_pct`（D2）
  - [x] `to_schedule_config` SetSetpoint：`pcs_power_kw = power.power_kw.abs()`
  - [x] `to_opt_problem`：调用 `to_schedule_config` + `EnergyScheduleModel::new(config.clone())`（D11）+ `model.compile().map_err(...)`（D4）
  - [x] `validate_config`：校验 num_periods==0 / pcs_power_kw<0 / soc 范围 / price 长度
  - [x] 实现 `Default` for `IntentParser`（使用 ScheduleConfig::default() + SystemState::default()）
  - [x] **偏差**：`validate_config` 中 `soc_min >= soc_max` 改为 `soc_min > soc_max`，允许 EmergencyStop 的点约束（soc_min==soc_max）通过

- [x] Task 6: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）— 22 项测试
  - [x] T1 IntentType 枚举变体 + PartialEq
  - [x] T2 TimeRange 构造
  - [x] T3 PowerIntent 构造（含 power_ratio）
  - [x] T4 SocIntent 构造
  - [x] T5 Intent 构造（全字段）
  - [x] T6 Intent serde 反序列化（缺字段时 priority=3、reason=""、confidence=0.0）
  - [x] T7 IntentParser::new 构造
  - [x] T8 IntentParser::default 等价 new
  - [x] T9 parse_json 完整 JSON 成功
  - [x] T10 parse_json 缺可选字段成功（serde 默认值）
  - [x] T11 parse_json 无效 JSON 返回 ParseError
  - [x] T12 to_schedule_config AutonomousSchedule（默认 + soc_final）
  - [x] T13 to_schedule_config Charge（price 修改为负值）
  - [x] T14 to_schedule_config Discharge（price 修改为高值）
  - [x] T15 to_schedule_config Hold（pcs_power_kw=0）
  - [x] T16 to_schedule_config Stop（pcs_power_kw=0）
  - [x] T17 to_schedule_config EmergencyStop（pcs_power_kw=0 + soc 冻结）
  - [x] T18 to_schedule_config SetSetpoint（pcs_power_kw 调整）
  - [x] T19 validate_config 合理配置通过
  - [x] T20 validate_config 非法配置失败（num_periods=0 / pcs_power<0 / soc_min>soc_max / price 长度不匹配）
  - [x] T21 to_opt_problem 端到端（AutonomousSchedule → config → compile → LpProblem）
  - [x] T22 端到端：JSON → parse_json → to_schedule_config → validate_config
  - [x] 验证：`cargo test -p eneros-intent-parser` 全部通过（22/22）

- [x] Task 7: 设计文档 `docs/ai/intent-parser-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / Intent 数据结构 / IntentType 枚举 / TimeRange/PowerIntent/SocIntent / IntentParser 主接口 / 意图转换策略 / 配置校验 / no_std 合规 / 错误处理 / 偏差声明 / 测试与验收
  - [x] 2 Mermaid 图：IntentParser 类图 + 意图转换流程图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/ai/` 下
  - [x] 文档行数：1806 行

- [x] Task 8: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.67.0` → `0.68.0`（header + VERSION 变量 2 处）
  - [x] `.github/workflows/ci.yml` 版本号 `0.67.0` → `0.68.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-intent-parser` 说明（2 处）

- [x] Task 9: 构建校验（§2.4.2 C6~C11）
  - [x] **C6** `cargo metadata --format-version 1` 成功
  - [x] **C7** `cargo test -p eneros-intent-parser` 全部通过（22 tests, 0 failed）
  - [x] **C8** `cargo build -p eneros-intent-parser --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过（4.11s）
  - [x] **C9** `cargo fmt -p eneros-intent-parser -- --check` 格式通过
  - [x] **C10** `cargo clippy -p eneros-intent-parser --all-targets -- -D warnings` lint 通过
  - [x] **C11** `cargo deny check licenses bans sources` 安全扫描通过（bans ok, licenses ok, sources ok）

- [x] Task 10: 更新 tasks.md + checklist.md 所有项 → [x]
  - [x] tasks.md 10 任务全部 [x]
  - [x] checklist.md 所有检查点全部 [x]

# Task Dependencies

- Task 2（crate 骨架）→ Task 1（metadata 验证需骨架）
- Task 3（error）独立
- Task 4（intent）独立（不直接用 IntentError）
- Task 5（parser）依赖 Task 3 + Task 4 + v0.66.0/v0.67.0/v0.65.0/v0.64.0
- Task 6（集成测试）→ Task 3~5
- Task 7（设计文档）可与 Task 5~6 并行（已并行执行）
- Task 8（版本同步）→ Task 7
- Task 9（构建校验）→ Task 8
- Task 10（更新文档）→ Task 9

# Parallelizable Work

- Task 3（error）+ Task 4（intent）可并行
- Task 5（parser）依赖 Task 3 + Task 4
- Task 6（集成测试）依赖 Task 5
- Task 7（设计文档）可与 Task 5~6 并行（已并行执行）
