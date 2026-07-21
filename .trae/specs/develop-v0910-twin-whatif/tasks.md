# Tasks

- [x] Task 1: 实现 `src/whatif.rs`（上）— 数据结构 + apply_action + 测试 T1~T14
  - [x] SubTask 1.1: `Action` 4 变体（D7）：`SetDevicePower { device_id: u64, power: f64 }` / `RemoveDevice { device_id: u64 }` / `SetGridPower { active_power: f32 }` / `SetMarketPrice { price: f32 }`（Debug, Clone, Copy, PartialEq，中文字段 doc）
  - [x] SubTask 1.2: `Scenario { name: &'static str, actions: Vec<Action>, duration_ms: u64 }`（Debug, Clone；D2/D3）
  - [x] SubTask 1.3: `Outcome { metric: &'static str, value: f32, baseline: f32 }`（Debug, Clone, Copy, PartialEq；D2）
  - [x] SubTask 1.4: `RiskLevel { Low, Medium, High, Critical }`（Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord + Default=Low + serde Serialize，序即严重度）
  - [x] SubTask 1.5: `ScenarioResult { scenario: &'static str, outcomes: Vec<Outcome>, risk_level: RiskLevel }`（Debug, Clone + serde Serialize + `summary_json()` 摘要，仿 v0.89.0 TwinSnapshot 模式：serde_json::to_string 失败兜底 `"{}"`）
  - [x] SubTask 1.6: `WhatIfError { ModelUnavailable, Diverged }`（Debug, Clone, Copy, PartialEq, Eq；D10 两变体，无 DDS 透传——本版本不触总线）
  - [x] SubTask 1.7: `apply_action(state: &mut TwinModel, action: &Action)` 自由函数（D8，model.rs 零改动）：SetDevicePower 仅更新存在设备 `power = sanitize_f64`；RemoveDevice 移除；SetGridPower 更新 `grid.active_power = sanitize`；SetMarketPrice `market = Some(MarketMirror { timestamp: 保留或0, current_price: sanitize })`（None→新建，Some→覆盖 price）
  - [x] SubTask 1.8: NaN/Inf 防御（D12）：f32 功率复用 `model_forecast::sanitize`；f64 power 本地等价处理（非有限 → 0.0）
  - [x] SubTask 1.9: 中文模块文档（D2/D3/D7/D8/D10/D12 引用）；use 仅 alloc + core + serde + crate::model + crate::model_forecast；无 std/panic!/unsafe/todo!/unimplemented!/unwrap（主代码）
  - [x] SubTask 1.10: T1 — Action 4 变体构造 + Copy 语义 + PartialEq 相等/不等
  - [x] SubTask 1.11: T2 — Scenario 构造 + Clone（name/actions 长度/duration_ms 回显）
  - [x] SubTask 1.12: T3 — Outcome 3 字段 + Copy + 相等判定
  - [x] SubTask 1.13: T4 — RiskLevel 序：Low < Medium < High < Critical + Default==Low + Copy
  - [x] SubTask 1.14: T5 — ScenarioResult 构造 + Clone + risk_level 回显
  - [x] SubTask 1.15: T6 — `summary_json()` 可被 serde_json::Value 解析，含 scenario / risk_level / outcomes 数组
  - [x] SubTask 1.16: T7 — WhatIfError 两变体 Debug + PartialEq（ModelUnavailable != Diverged）
  - [x] SubTask 1.17: T8 — apply_action SetDevicePower：存在设备 1（power 1.0→2.5）更新正确
  - [x] SubTask 1.18: T9 — apply_action SetDevicePower 不存在设备 → 无副作用不 panic（设备表不变）
  - [x] SubTask 1.19: T10 — apply_action SetDevicePower NaN / +Inf power → 0.0（D12）
  - [x] SubTask 1.20: T11 — apply_action RemoveDevice：设备表不含该 id；重复移除不 panic
  - [x] SubTask 1.21: T12 — apply_action SetGridPower：grid.active_power==8.0，其余 grid 字段不变
  - [x] SubTask 1.22: T13 — apply_action SetGridPower NaN → 0.0
  - [x] SubTask 1.23: T14 — apply_action SetMarketPrice：market None → Some(0.65)；已有 → 覆盖 price

- [x] Task 2: 实现 `src/whatif.rs`（中）— SimModel + AnalyticalSimModel + compute_outcomes + assess_risk + 测试 T15~T31
  - [x] SubTask 2.1: `SimModel` trait（D4 无 Send+Sync）：`run(&self, state: TwinModel, duration_ms: u64) -> Result<TwinModel, WhatIfError>` + `fn name(&self) -> &'static str`
  - [x] SubTask 2.2: `AnalyticalSimModel { battery_capacity_kwh: f64 }`（字段 pub）+ `new(battery_capacity_kwh: f64)`：非有限或 ≤ 0 → 100.0（D12）
  - [x] SubTask 2.3: `run` 实现（D9）：`hours = duration_ms as f64 / 3_600_000.0`；逐设备 `soc -= sanitize(power as f32) as f64 * hours / capacity`，clamp [0,1]；grid / market / last_update 透传；`name() == "analytical"`
  - [x] SubTask 2.4: `compute_outcomes(baseline: &TwinModel, final_state: &TwinModel) -> Vec<Outcome>`（D11 三指标，固定顺序）：`grid_active_power` / `total_device_power`（Σ 设备 power，f32 饱和转换）/ `min_soc`（空设备 → 1.0 中性）
  - [x] SubTask 2.5: `assess_risk(outcomes: &[Outcome]) -> RiskLevel`（D11 取最重）：min_soc ≤ 0 → Critical；min_soc < 0.2 → High；grid_active_power 相对波动 > 50% → Medium；else Low；空 outcomes → Low
  - [x] SubTask 2.6: 波动率除零防御：|baseline| < 1e-6 时 |final| < 1e-6 → 波动 0；否则视为大波动（Medium 路径）
  - [x] SubTask 2.7: T15 — new 钳制：NaN / 0.0 / -5.0 / +Inf → 100.0；50.0 保留
  - [x] SubTask 2.8: T16 — SOC 放电推演：soc=0.8 power=10.0 cap=100 duration=1h → soc==0.7（±1e-9）
  - [x] SubTask 2.9: T17 — 充电：power=-5.0 → soc 增加 0.05
  - [x] SubTask 2.10: T18 — clamp 双界：放超 → 0.0；充超 → 1.0
  - [x] SubTask 2.11: T19 — duration_ms=0 → soc 不变
  - [x] SubTask 2.12: T20 — 多设备各自推演 + grid/market/last_update 透传 + `name()=="analytical"`
  - [x] SubTask 2.13: T21 — 设备 power NaN → sanitize 0.0 → soc 不变（D12）
  - [x] SubTask 2.14: T22 — compute_outcomes 恰好 3 条且 metric 集合 == {grid_active_power, total_device_power, min_soc}（固定顺序）
  - [x] SubTask 2.15: T23 — outcomes 数值正确：baseline/final 各自 value 与 baseline 字段回显（grid 10→16；设备 Σ 3.0→3.0；min_soc 0.8→0.7）
  - [x] SubTask 2.16: T24 — 空设备模型 → min_soc outcome == 1.0（value 与 baseline 均 1.0）
  - [x] SubTask 2.17: T25 — assess_risk min_soc==0.0 → Critical
  - [x] SubTask 2.18: T26 — assess_risk min_soc==0.15 → High
  - [x] SubTask 2.19: T27 — 边界：min_soc==0.2 且 grid 平稳 → Low（非 High）；min_soc==0.19 → High
  - [x] SubTask 2.20: T28 — min_soc 安全 + grid 波动 >50%（baseline 10 → final 16）→ Medium
  - [x] SubTask 2.21: T29 — 波动恰 50%（10→15）→ Low（严格 > 才 Medium）；全平稳 → Low
  - [x] SubTask 2.22: T30 — 空 outcomes → Low
  - [x] SubTask 2.23: T31 — 取最重：min_soc==0.0 且 grid 大波动 → Critical（非 Medium）

- [x] Task 3: 实现 `src/whatif.rs`（下）— WhatIfAnalyzer + 测试 T32~T40
  - [x] SubTask 3.1: `WhatIfAnalyzer { sim_model: Box<dyn SimModel> }`（字段 pub，中文字段 doc；D4）
  - [x] SubTask 3.2: `analyze(&self, scenario: &Scenario, model: &TwinModel) -> Result<ScenarioResult, WhatIfError>`：clone → 逐 action apply_action → `sim_model.run(sim, duration_ms)` → 发散分支（D10：`Err(Diverged)` → `Ok(ScenarioResult { outcomes: vec![], risk_level: Critical })`）→ `Err(ModelUnavailable)` 透传 → compute_outcomes(model, &final) → assess_risk → scenario 名回显
  - [x] SubTask 3.3: 测试辅助 — `DivergingSimModel`（run 恒 Err(Diverged)，name "diverging"）+ `UnavailableSimModel`（恒 Err(ModelUnavailable)，name "unavailable"）
  - [x] SubTask 3.4: T32 — 重放电端到端（soc=0.3 power=50 cap=100 1h）→ Ok + min_soc outcome==0.0 + risk==Critical
  - [x] SubTask 3.5: T33 — 平稳场景（小功率 30min）→ risk==Low + outcomes 3 条 + scenario 名回显
  - [x] SubTask 3.6: T34 — DivergingSimModel → Ok + outcomes 为空 + risk==Critical（D10 / 蓝图 §4.4 §6.5）
  - [x] SubTask 3.7: T35 — UnavailableSimModel → `Err(WhatIfError::ModelUnavailable)`（蓝图 §4.4 拒绝分析）
  - [x] SubTask 3.8: T36 — 确定性：同 scenario+model 两次 analyze → scenario/outcomes/risk 逐字段一致（f32 位级一致）
  - [x] SubTask 3.9: T37 — 只读：analyze 后输入 model 与事前 clone 逐字段相等（分析在 clone 上进行）
  - [x] SubTask 3.10: T38 — 组合动作：SetDevicePower{1, 20} + RemoveDevice{2} → final total_device_power 只含设备 1，min_soc 按设备 1 推演
  - [x] SubTask 3.11: T39 — 空 actions + duration_ms=0 → outcomes value==baseline 全等 + risk==Low
  - [x] SubTask 3.12: T40 — SetGridPower{16.0} 场景（baseline grid 10.0）→ grid_active_power outcome value==16.0 baseline==10.0 + risk==Medium

- [x] Task 4: lib.rs 集成（Surgical 追加）
  - [x] SubTask 4.1: `pub mod whatif;`（保持 mirror/model/model_forecast/predictor 顺序在前）
  - [x] SubTask 4.2: 12 项重导出追加：`pub use whatif::{apply_action, assess_risk, compute_outcomes, Action, AnalyticalSimModel, Outcome, RiskLevel, Scenario, ScenarioResult, SimModel, WhatIfAnalyzer, WhatIfError};`
  - [x] SubTask 4.3: crate 文档升级：标题加 v0.91.0 What-if 分析段 + D1~D12 偏差简表（v0.89.0/v0.90.0 文档保留，新旧并存标注版本）
  - [x] SubTask 4.4: 既有 80 个 v0.89.0+v0.90.0 测试全部保持通过（零改动验证）

- [x] Task 5: 创建 `configs/twin_whatif.toml`
  - [x] SubTask 5.1: `[whatif]` 段：`battery_capacity_kwh = 100.0`（AnalyticalSimModel 容量，D9/D12）
  - [x] SubTask 5.2: 风险阈值：`min_soc_high_threshold = 0.2` / `grid_deviation_medium_threshold = 0.5`（D11，与 assess_risk 规则一致）
  - [x] SubTask 5.3: `[[scenario]]` 场景模板 3 例：重放电（SetDevicePower 50kW × 1h）/ 设备退出（RemoveDevice）/ 电网功率设定（SetGridPower 16.0）
  - [x] SubTask 5.4: 中文注释：分析 <1s（蓝图 §7.2，集成阶段验收）/ 高风险拦截（§7.3）/ 解析模型仅 SOC 线性推演局限（D9）/ NaN 防御（D12）/ 发散→Critical 语义（D10）

- [x] Task 6: 创建 `docs/agents/twin-whatif-design.md`
  - [x] SubTask 6.1: 12 章节（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 6.2: Mermaid 图 1：核心算法（TwinModel → clone → 逐 action apply → SimModel.run → compute_outcomes → assess_risk → ScenarioResult）
  - [x] SubTask 6.3: Mermaid 图 2：analyze 决策流程（run Err? → Diverged→Critical 空 outcomes / ModelUnavailable→拒绝；风险分级取最重）
  - [x] SubTask 6.4: D1~D12 偏差声明表（从 spec.md 复制）
  - [x] SubTask 6.5: 前置依赖引用 v0.89.0 镜像（TwinModel）+ v0.90.0 预测（sanitize 复用）；下游 v0.112.0 云端孪生联合仿真
  - [x] SubTask 6.6: 选型对比表（简化解析模型 ⭐ 实时 / 详细动态仿真 离线 / 蒙特卡洛 风险评估，蓝图 §5.1）
  - [x] SubTask 6.7: 性能目标（分析 <1s，标注"集成阶段验收，本版本交付算法骨架+单元验证"）+ GPU 规则说明（蓝图 §6.6：详细仿真接入时优先 GPU 且禁用梯度；本版本纯标量 CPU 无张量）
  - [x] SubTask 6.8: 高风险拦截安全语义（蓝图 §7.3：risk ≥ High 的场景下游决策层应拒绝执行）+ 只读分析（输入模型不变）
  - [x] SubTask 6.9: 风险：保真度不足误导决策（§8.1）/ 仿真与真实偏差需持续校准（§8.5）/ 内存（单次 clone O(设备数)，§43.6）

- [x] Task 7: 版本同步根目录文件
  - [x] SubTask 7.1: 根 `Cargo.toml` `[workspace.package] version = "0.90.0"` → `"0.91.0"`
  - [x] SubTask 7.2: `Makefile` `# Version: v0.91.0` + `VERSION := 0.91.0`
  - [x] SubTask 7.3: `.github/workflows/ci.yml` `# Version: v0.91.0`
  - [x] SubTask 7.4: `ci/src/gate.rs` clippy 段 + test 段注释追加 `+ v0.91.0 What-if 分析：WhatIfAnalyzer / SimModel / AnalyticalSimModel / Scenario / ScenarioResult / Outcome / RiskLevel / Action / WhatIfError`

- [x] Task 8: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 8.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 8.2: `cargo test -p eneros-twin-agent` 120 tests 全过（80 既有 + 40 新增，0 failures）
  - [x] SubTask 8.3: `cargo build -p eneros-twin-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 8.4: `cargo fmt -p eneros-twin-agent -- --check` 通过
  - [x] SubTask 8.5: `cargo clippy -p eneros-twin-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 8.6: `cargo deny check licenses bans sources` 通过（无新第三方依赖）
  - [x] SubTask 8.7: 回归 — `cargo test -p eneros-grid-agent` / `cargo test -p eneros-device-agent` / `cargo test -p eneros-energy-market-agent` / `cargo test -p eneros-agent-bus-dds` 全过

# Task Dependencies

- [Task 2] depends on [Task 1]
- [Task 3] depends on [Task 2]
- [Task 4] depends on [Task 3]
- [Task 5, Task 6] 独立（可与 1~4 并行）
- [Task 7] 独立（仅根目录 4 文件，不碰 crate 源码）
- [Task 8] depends on [Task 4, Task 5, Task 6, Task 7]

# 并行执行计划

- **Sub-Agent A**：Task 1 + Task 2 + Task 3 + Task 4（同 crate 同文件，串行单 agent 保证一致性）
- **Sub-Agent B**：Task 5 + Task 6（configs + docs，与 A 并行）
- **Sub-Agent C**：Task 7（版本同步，与 A/B 并行；仅根目录 4 文件）
- **主 agent**：Task 8（全部完成后统一构建校验 + 回归）
