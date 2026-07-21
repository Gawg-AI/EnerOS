# v0.90.0 Digital Twin Agent — 短期预测 Spec

## Why

v0.89.0 已将全网状态实时镜像进 `TwinModel`，但孪生只有"现在"没有"未来"。蓝图 phase2 v0.90.0 要求基于孪生状态做秒~分钟级短期预测（设备行为模型），提前预判设备行为、支持前馈控制，并为 v0.91.0 What-if 分析提供预测底座。

## What Changes

- 在既有 `crates/agents/twin-agent/` 内**新增 2 个源文件**（Surgical：不改动 mirror.rs / model.rs 任何行）：
  - `src/model_forecast.rs` — `ForecastPoint` / `ForecastResult` / `ForecastError` / `ForecastModel` trait + `PersistenceModel`（持续法兜底）+ `MeanModel`（均值法）+ `compute_confidence` + `sanitize`（NaN 防御）
  - `src/predictor.rs` — `Predictor`（主模型 + 兜底链 + 周期步长配置）+ `publish_forecast`（发布 `/power/twin/forecast`）
- `src/lib.rs` 仅追加 `pub mod` + 重导出 + crate 文档升级 v0.90.0（含 D1~D12 偏差简表）
- 新增 `configs/twin_forecast.toml`（步长/点数上限/置信阈值/模型路径占位）
- 新增 `docs/agents/twin-forecast-design.md`（12 章节 + 2 Mermaid + D1~D12 偏差表）
- 根目录 4 文件版本同步 0.89.0 → 0.90.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（T1~T40），含 MockDdsNode 端到端发布测试
- **无 BREAKING**：既有 40 个 v0.89.0 测试与全部下游 crate 零影响

## Impact

- Affected specs：develop-v0890-digital-twin-mirror（上游，复用 TwinModel）
- Affected code：`crates/agents/twin-agent/`（新增 2 文件 + lib.rs 追加）、`configs/`、`docs/agents/`、根 4 文件
- 下游解锁：v0.91.0 What-if 分析、v0.112.0 云端孪生联合仿真

## 偏差声明（D1~D12，Karpathy Think Before Coding：显式取舍）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `twin_agent/src/{predictor,model_forecast}.rs` | 既有 `twin-agent/src/` 内新增同名 2 文件（沿用 v0.89.0 D12 连字符命名） |
| **D2** | `target: String` | `target: &'static str`（避免堆分配；默认 `"power"`，多目标扩展后续版本） |
| **D3** | `horizon: Duration` | `horizon_ms: u64`（全 crate 统一 u64 ms 外部时间注入惯例，同 v0.89.0 D8） |
| **D4** | `ForecastModel: Send + Sync` | 不要求 Send+Sync（no_std 单线程惯例，同 agent-bus-dds D2） |
| **D5** | `docs/phase2/forecast.md` | `docs/agents/twin-forecast-design.md`（记忆 §2.3.3 文档分类强制） |
| **D6** | `tests/forecast_accuracy.rs` 独立集成测试 | src 内嵌单元测试 T1~T40（项目惯例）；历史回测误差 <5% 标注**集成阶段验收**（v0.25.0 TSDB 历史接入后回测），本版本以合成平滑数据验证持续法 MAPE < 5% |
| **D7** | 选型 LSTM/GRU ⭐ 采用 | 本版本交付 `ForecastModel` trait 抽象 + `PersistenceModel`/`MeanModel` 基线 + 兜底链；LSTM 权重分发与推理后端后续版本接入（v0.61.0 模型部署线仅覆盖 LLM GGUF，不含 LSTM 通道） |
| **D8** | §4.4"退化均值预测" vs §5.1"持续法兜底"表述不一致 | 统一兜底链：主模型 `Err` → `PersistenceModel`；`MeanModel` 为可选主模型（无历史缓冲时均值 ≡ 持续法单样本，v0.89.0 镜像仅存最新态） |
| **D9** | `forecast(model, horizon)` 无步长参数 | `Predictor::new(step_ms, max_points, confidence_threshold)` 构造注入；点数 = `horizon_ms / step_ms` 向上取整，钳制 `1..=max_points`（默认 96，§43.6 内存预算防 OOM） |
| **D10** | §4.4"置信度低 → 标记" | `ForecastResult` 增加 `degraded: bool`（走兜底链 或 confidence < threshold 时置位）；confidence = base_confidence × 区间紧度，确定性计算可复现 |
| **D11** | 发布 `/power/twin/forecast` | `ForecastResult::to_json()`（全量含 points 数组，≤96 点约 4KB）+ `publish_forecast(node, writer, result)` 辅助函数；复用 agent-bus-dds `DdsNode`，不新增 writer 管理逻辑 |
| **D12** | —（蓝图未覆盖） | NaN/Inf 防御（v0.88.0 C140 教训）：输入功率非有限 → `sanitize` 按 0.0 处理，`confidence` 置 0 且 `degraded = true` |

## ADDED Requirements

### Requirement: 预测数据结构与模型抽象

系统 SHALL 提供 `ForecastPoint { time: u64, value: f32, lower: f32, upper: f32 }`（预测区间点）、`ForecastResult { target, horizon_ms, points, confidence, degraded }`（预测结果）、`ForecastError { Dds(DdsError) }`（DDS 透传单变体，同 v0.89.0 D8 模式），以及 `ForecastModel` trait（`predict(&self, input: &TwinModel, horizon_ms: u64, step_ms: u64) -> Result<Vec<ForecastPoint>, ForecastError>` + `name()` + `base_confidence()`），全部 no_std + alloc 兼容。

#### Scenario: 持续法基线预测

- **WHEN** `TwinModel.grid.active_power = 12.3` 且 `grid.timestamp > 0`，以 `horizon_ms=60000, step_ms=1000` 调用 `PersistenceModel::predict`
- **THEN** 返回 60 个 `ForecastPoint`，`time = model.last_update + (i+1)*step_ms`，`value == 12.3` 恒定，`lower < value < upper`（±5% 带），模型 `name() == "persistence"`

#### Scenario: 空模型安全退化

- **WHEN** `TwinModel` 全空（grid.timestamp == 0 且无设备）
- **THEN** 预测不 panic，所有 `value == 0.0`，结果标记低置信（confidence == 0.0 且 degraded）

#### Scenario: 设备功率求和回退

- **WHEN** `grid.timestamp == 0` 但存在 2 个设备（power = 1.5 / 2.5）
- **THEN** 预测 `value == 4.0`（设备功率求和回退）

### Requirement: Predictor 主模型 + 兜底链

系统 SHALL 提供 `Predictor::new(model: Box<dyn ForecastModel>, step_ms: u64, max_points: usize, confidence_threshold: f32)`（step_ms/max_points 为 0 时钳制为 1）与 `forecast(&self, twin: &TwinModel, horizon_ms: u64) -> Result<ForecastResult, ForecastError>`：主模型 `predict` 成功则用其输出，失败则自动切换 `PersistenceModel` 兜底并置 `degraded = true`；confidence < threshold 时亦置 `degraded = true`。

#### Scenario: 主模型失败自动兜底

- **WHEN** 主模型 `predict` 返回 `Err`
- **THEN** `forecast` 返回 `Ok`，points 来自持续法，`degraded == true`，结果仍完整可用

#### Scenario: 点数钳制防 OOM

- **WHEN** `horizon_ms = 10_000_000, step_ms = 1, max_points = 96`
- **THEN** 返回点数 == 96（不超限分配内存）

#### Scenario: 确定性输出

- **WHEN** 同一 `TwinModel` 两次调用 `forecast`（相同 horizon）
- **THEN** 两次结果逐点一致（无随机源、无 Instant::now()）

### Requirement: 置信度计算与 NaN 防御

系统 SHALL 提供 `compute_confidence(base: f32, points: &[ForecastPoint]) -> f32`：输出钳制 `[0.0, 1.0]`，区间越窄置信越高；空 points → 0.0；任何非有限输入（NaN/Inf）不传播、不 panic（D12）。

#### Scenario: NaN 输入不污染结果

- **WHEN** `grid.active_power = f32::NAN` 调用预测
- **THEN** 所有 point value 为有限值（0.0），confidence == 0.0，degraded == true

### Requirement: 预测结果发布

系统 SHALL 提供 `ForecastResult::to_json()`（serde 序列化全量结果含 points 数组）与 `publish_forecast(node: &mut dyn DdsNode, writer: WriterId, result: &ForecastResult) -> Result<(), ForecastError>`，向 `/power/twin/forecast` 写入 JSON 样本；write 失败返回 `Err(ForecastError::Dds(_))`。

#### Scenario: MockDdsNode 端到端发布

- **WHEN** 经 MockDdsNode 创建 writer 并 `publish_forecast`
- **THEN** 对应 reader 收到 1 条样本，payload 可被 `serde_json::from_str` 解析，含 `target == "power"`、`confidence`、`points` 数组

#### Scenario: 发布失败透传错误

- **WHEN** node 已 shutdown
- **THEN** `publish_forecast` 返回 `Err(ForecastError::Dds(_))`，不 panic

## MODIFIED Requirements

### Requirement: twin-agent crate 文档与导出

`src/lib.rs` crate 文档升级为 v0.89.0 + v0.90.0 双版本说明（镜像 + 预测），追加 `pub mod model_forecast; pub mod predictor;` 与重导出（`ForecastPoint, ForecastResult, ForecastError, ForecastModel, PersistenceModel, MeanModel, Predictor, compute_confidence, publish_forecast`）。**既有 pub 项与 mirror/model 模块零改动。**

## REMOVED Requirements

无。
