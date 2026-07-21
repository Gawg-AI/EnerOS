# v0.91.0 Digital Twin Agent — What-if 分析 Spec

## Why

v0.90.0 让孪生能"看未来"（预测），但决策前还需要"如果……会怎样"的预演能力。蓝图 phase2 v0.91.0（P2-C 收尾）要求基于孪生模型做 What-if 分析：应用假设动作 → 仿真推演 → 对比基线得出 Outcome → 评估风险等级，避免决策失误，并为 v0.112.0 云端孪生联合仿真打基础。

## What Changes

- 在既有 `crates/agents/twin-agent/` 内**新增 1 个源文件**（Surgical：mirror.rs / model.rs / model_forecast.rs / predictor.rs 零改动）：
  - `src/whatif.rs` — `Action` 4 变体 / `Scenario` / `Outcome` / `RiskLevel` / `ScenarioResult` / `WhatIfError` / `SimModel` trait + `AnalyticalSimModel`（简化解析模型 ⭐）+ `WhatIfAnalyzer` + `apply_action` / `compute_outcomes` / `assess_risk` 自由函数
- `src/lib.rs` 仅追加 `pub mod whatif;` + 重导出 + crate 文档升级 v0.91.0（含 D1~D12 偏差简表）
- 新增 `configs/twin_whatif.toml`（`[whatif]` 容量/风险阈值 + `[[scenario]]` 场景模板 3 例）
- 新增 `docs/agents/twin-whatif-design.md`（12 章节 + 2 Mermaid + D1~D12 偏差表）
- 根目录 4 文件版本同步 0.90.0 → 0.91.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（T1~T40），含发散/不可用故障注入测试
- **无 BREAKING**：既有 80 个 v0.89.0+v0.90.0 测试与全部下游 crate 零影响

## Impact

- Affected specs：develop-v0890-digital-twin-mirror（TwinModel 来源）、develop-v0900-twin-forecast（sanitize 复用）
- Affected code：`crates/agents/twin-agent/`（新增 1 文件 + lib.rs 追加）、`configs/`、`docs/agents/`、根 4 文件
- 下游解锁：v0.112.0 云端孪生联合仿真（P2-C 收尾，本版完成后进入 P2-D Coordinator）

## 偏差声明（D1~D12，Karpathy Think Before Coding：显式取舍）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `twin_agent/src/whatif.rs` | 既有 `twin-agent/src/whatif.rs`（连字符命名，v0.89.0 D12 惯例） |
| **D2** | `name: String` / `scenario: String` / `metric: String` | 全部 `&'static str`（无堆分配，同 v0.90.0 D2；场景模板名静态化，动态命名后续版本） |
| **D3** | `duration: Duration` | `duration_ms: u64`（全 crate 统一 u64 ms 外部时间注入惯例） |
| **D4** | `Box<dyn SimModel>`（蓝图未标注约束） | trait 不要求 Send+Sync（no_std 单线程惯例，同 v0.90.0 D4） |
| **D5** | `docs/phase2/whatif.md` | `docs/agents/twin-whatif-design.md`（记忆 §2.3.3 文档分类强制） |
| **D6** | `tests/whatif.rs` 独立集成测试 | src 内嵌单元测试 T1~T40（项目惯例）；分析 <1s 标注**集成阶段验收** |
| **D7** | 蓝图 `Vec<Action>` 但 `Action` 类型未定义 | 本地定义 4 变体：`SetDevicePower { device_id, power: f64 }` / `RemoveDevice { device_id }` / `SetGridPower { active_power: f32 }` / `SetMarketPrice { price: f32 }`（覆盖 TwinModel 设备/电网/市场三面） |
| **D8** | 蓝图 `sim_state.apply(action)`（TwinModel 无此方法） | `whatif.rs` 内自由函数 `apply_action(state: &mut TwinModel, action: &Action)`（model.rs 零改动，Surgical） |
| **D9** | 选型"简化解析模型 ⭐ 实时" | `AnalyticalSimModel { battery_capacity_kwh: f64 }`：仅 SOC 线性推演（`soc -= power × hours / capacity`，clamp [0,1]），grid/market 透传；详细动态仿真/蒙特卡洛后续版本 |
| **D10** | §4.4"仿真发散 → 标记高风险"+"模型不可用 → 拒绝分析" | `WhatIfError { ModelUnavailable, Diverged }`；`SimModel::run` 返回 `Err(Diverged)` → analyze 转 `Ok(result)` 且 `risk_level = Critical` + outcomes 空；`Err(ModelUnavailable)` → analyze 拒绝（透传 Err） |
| **D11** | `compute_outcomes`/`assess_risk` 蓝图仅骨架未定义 | 本版本定义 3 指标：`grid_active_power` / `total_device_power` / `min_soc`（空设备 min_soc=1.0 中性）；风险规则取最重：min_soc ≤ 0 → Critical；min_soc < 0.2 → High；grid 功率相对波动 > 50% → Medium；else Low |
| **D12** | —（蓝图未覆盖） | NaN/Inf 防御（v0.88.0 C140 教训）：action/sim 中非有限功率复用 `model_forecast::sanitize` 按 0.0；`battery_capacity_kwh` 非有限或 ≤ 0 → 默认 100.0 |

## ADDED Requirements

### Requirement: 场景与结果数据结构

系统 SHALL 提供：`Action`（D7 四变体，Debug/Clone/Copy/PartialEq）、`Scenario { name: &'static str, actions: Vec<Action>, duration_ms: u64 }`、`Outcome { metric: &'static str, value: f32, baseline: f32 }`、`RiskLevel { Low, Medium, High, Critical }`（Ord：Low < Medium < High < Critical，Default=Low）、`ScenarioResult { scenario, outcomes, risk_level }`（Debug/Clone + serde Serialize + `summary_json()` 仿 v0.89.0 摘要模式），全部 no_std + alloc 兼容。

#### Scenario: 动作应用语义

- **WHEN** 对含设备 1（power=1.0）的 TwinModel 依次应用 `SetDevicePower{1, 2.5}` 与 `SetGridPower{8.0}`
- **THEN** clone 后 sim 态设备 1 power==2.5、grid.active_power==8.0，原 model 不变
- **WHEN** 应用 `RemoveDevice{1}` / `SetMarketPrice{0.65}`
- **THEN** 设备表不含 1 / market == Some(MarketMirror{ current_price: 0.65, .. })

### Requirement: 简化解析仿真模型

系统 SHALL 提供 `SimModel` trait（`run(&self, state: TwinModel, duration_ms: u64) -> Result<TwinModel, WhatIfError>` + `name()`）与 `AnalyticalSimModel::new(battery_capacity_kwh: f64)`（非有限或 ≤0 → 100.0，D12）：对每台设备 `soc -= sanitize(power) × (duration_ms/3_600_000) / capacity`，clamp [0,1]；grid/market/last_update 透传；`name() == "analytical"`。

#### Scenario: SOC 放电推演

- **WHEN** 设备 soc=0.8、power=10.0 kW、capacity=100.0 kWh、duration_ms=3_600_000（1h）
- **THEN** 仿真后 soc == 0.7（0.8 − 10×1/100）；负 power（充电）soc 增加；越界 clamp 到 [0,1]

### Requirement: Outcome 计算与风险评估

系统 SHALL 提供 `compute_outcomes(baseline: &TwinModel, final_state: &TwinModel) -> Vec<Outcome>`（3 指标，D11）与 `assess_risk(outcomes: &[Outcome]) -> RiskLevel`（取最重规则；空 outcomes → Low）。

#### Scenario: 风险分级

- **WHEN** final min_soc == 0.0 → **THEN** Critical
- **WHEN** final min_soc == 0.15 → **THEN** High
- **WHEN** min_soc 安全但 grid_active_power 相对基线波动 > 50% → **THEN** Medium
- **WHEN** 全部平稳 → **THEN** Low

### Requirement: WhatIfAnalyzer 分析流程

系统 SHALL 提供 `WhatIfAnalyzer { sim_model: Box<dyn SimModel> }` 与 `analyze(&self, scenario: &Scenario, model: &TwinModel) -> Result<ScenarioResult, WhatIfError>`：clone 模型 → 逐 action apply → sim_model.run → 发散转 Critical 结果（D10）→ compute_outcomes → assess_risk → 返回（scenario 名回显）。

#### Scenario: 重放电场景判高风险

- **WHEN** 设备 soc=0.3、power=50.0 kW、capacity=100 kWh、场景 duration=1h
- **THEN** analyze 返回 Ok，min_soc outcome == 0.0（clamp），risk_level == Critical

#### Scenario: 仿真发散故障注入

- **WHEN** SimModel 返回 `Err(Diverged)`
- **THEN** analyze 返回 `Ok`，outcomes 为空，risk_level == Critical（蓝图 §4.4 / §6.5）

#### Scenario: 模型不可用拒绝分析

- **WHEN** SimModel 返回 `Err(ModelUnavailable)`
- **THEN** analyze 返回 `Err(WhatIfError::ModelUnavailable)`（蓝图 §4.4 拒绝分析）

#### Scenario: 确定性与只读

- **WHEN** 同一 scenario + model 两次 analyze
- **THEN** 结果逐字段一致（无随机源）；输入 model 不被修改（分析在 clone 上进行）

## MODIFIED Requirements

### Requirement: twin-agent crate 文档与导出

`src/lib.rs` crate 文档升级为 v0.89.0 + v0.90.0 + v0.91.0 三版本说明（镜像 + 预测 + What-if），追加 `pub mod whatif;` 与重导出（`Action, Scenario, Outcome, RiskLevel, ScenarioResult, WhatIfError, SimModel, AnalyticalSimModel, WhatIfAnalyzer, apply_action, compute_outcomes, assess_risk`）。**既有 pub 项与 4 个既有模块零改动。**

## REMOVED Requirements

无。
