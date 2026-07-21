//! EnerOS v0.89.0 + v0.90.0 + v0.91.0 Digital Twin Agent — 数据镜像 + 短期预测 + What-if 分析.
//!
//! # 版本目标（v0.89.0 数据镜像）
//!
//! 实现 Digital Twin Agent，旁路订阅 Agent Bus 的 `/power/state/*` 状态主题，
//! 将设备/电网/市场状态实时镜像到孪生模型 [`model::TwinModel`]（过期不更新、
//! 字段缺失保留旧值），并周期性向 `/power/twin/update` 发布摘要快照，为预测、
//! What-if 分析与云端远程观测（v0.112.0 云端孪生主节点）提供实时状态输入。
//!
//! # 版本目标（v0.90.0 短期预测）
//!
//! 基于孪生状态做秒~分钟级短期预测（设备行为模型）：[`Predictor`] 以
//! [`ForecastModel`] trait 抽象主模型，失败时自动切换 [`PersistenceModel`] 兜底，
//! 产出带置信区间的 [`ForecastResult`]（confidence × 区间紧度，`degraded` 标记），
//! 并向 `/power/twin/forecast` 发布全量 JSON，为 v0.91.0 What-if 分析提供预测底座。
//!
//! # 版本目标（v0.91.0 What-if 分析）
//!
//! 基于孪生模型做决策前预演：[`WhatIfAnalyzer`] 将 [`Scenario`]（名称 + [`Action`] 序列 +
//! 推演时长）应用到模型 clone，经 [`SimModel`]（[`AnalyticalSimModel`] 简化解析模型）
//! 推演后与基线对比得出 3 条 [`Outcome`]（grid_active_power / total_device_power / min_soc），
//! 再按取最重规则评估 [`RiskLevel`]；仿真发散转 Critical 空结果、模型不可用拒绝分析，
//! 为 v0.112.0 云端孪生联合仿真打基础。
//!
//! # 核心类型
//!
//! ## v0.89.0 数据镜像
//!
//! - [`TwinMirror`] — 旁路镜像器（订阅 → 合并 → 周期发布）
//! - [`TwinModel`] / [`DeviceTwin`] / [`MarketMirror`] — 孪生数据模型
//! - [`TwinSnapshot`] — 一致性快照（clone 语义）+ 摘要 JSON
//! - [`TwinError`] — 错误类型（DDS 透传）
//!
//! ## v0.90.0 短期预测
//!
//! - [`Predictor`] — 预测器（主模型 + 持续法兜底链 + 步长/点数/置信阈值配置）
//! - [`ForecastModel`] — 预测模型抽象（[`PersistenceModel`] / [`MeanModel`] 基线）
//! - [`ForecastPoint`] / [`ForecastResult`] — 预测区间点 / 预测结果（含 confidence + degraded）
//! - [`ForecastError`] — 预测错误（DDS 透传）
//! - [`compute_confidence`] / [`publish_forecast`] — 置信度计算 / 预测发布辅助
//!
//! ## v0.91.0 What-if 分析
//!
//! - [`WhatIfAnalyzer`] — What-if 分析器（clone → 逐 action 应用 → 仿真 → 结果/风险）
//! - [`Scenario`] / [`Action`] — 场景（名称 + 动作序列 + 时长）/ 假设动作 4 变体
//! - [`SimModel`] / [`AnalyticalSimModel`] — 仿真模型抽象 / 简化解析模型（SOC 线性推演）
//! - [`ScenarioResult`] / [`Outcome`] / [`RiskLevel`] — 分析结果 / 指标 / 风险等级
//! - [`WhatIfError`] — 分析错误（ModelUnavailable 拒绝 / Diverged 转 Critical）
//! - [`apply_action`] / [`compute_outcomes`] / [`assess_risk`] — 动作应用 / 结果计算 / 风险评估
//!
//! # 偏差声明（v0.89.0 D1~D12）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 |
//! |------|---------|-----------|
//! | **D1** | `pub async fn run()` 事件循环 | sync `on_tick(now_ms)`，外部调度驱动（no_std 无 async runtime） |
//! | **D2** | `HashMap<DeviceId, DeviceTwin>` | `BTreeMap<u64, DeviceTwin>`（no_std 标准选择，有序遍历） |
//! | **D3** | `device_id: String` | `device_id: u64`（topic 末段 `/power/state/battery/{id}` 解析） |
//! | **D4** | `DeviceTwin` 5 字段（含 soc/power/timestamp） | 2 字段（soc/power/时间戳已含于 `DeviceState`，去重） |
//! | **D5** | `new(bus: &DdsNode)` + 单 `DdsReader` 字段 | `Box<dyn DdsNode>` 持有节点 + `Vec<(String, ReaderId)>` 多订阅 |
//! | **D6** | 本地设备状态定义 | 复用 `eneros-device-agent::DeviceState`（单一事实源） |
//! | **D7** | 本地电网状态定义 | 复用 `eneros-grid-agent::GridState`（单一事实源） |
//! | **D8** | `interval(1s)` 内部节拍器 | `publish_interval_ms` + 外部 `now_ms` 驱动（no_std 无 Instant） |
//! | **D9** | 全量模型快照发布 | 摘要 JSON 发布（device_count/时间戳/计数器），全量经 `snapshot()` 本地取用 |
//! | **D10** | `market: Option<MarketData>` | `Option<MarketMirror>`（timestamp/current_price 极简 2 字段） |
//! | **D11** | `tests/twin_mirror.rs` 集成测试 | src 内嵌单元测试 T1~T40（沿用 v0.73~v0.88 模式） |
//! | **D12** | crate `twin_agent` + `docs/phase2/digital_twin.md` + 订阅配置文件 | crate `twin-agent`（连字符命名同 device-agent）；本版本仅代码+内嵌测试，文档/配置后续补齐 |
//!
//! # 偏差声明（v0.90.0 D1~D12）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 |
//! |------|---------|-----------|
//! | **D1** | crate 路径 `twin_agent/src/{predictor,model_forecast}.rs` | 既有 `twin-agent/src/` 内新增同名 2 文件（沿用 v0.89.0 D12 连字符命名） |
//! | **D2** | `target: String` | `target: &'static str`（避免堆分配；默认 `"power"`，多目标扩展后续版本） |
//! | **D3** | `horizon: Duration` | `horizon_ms: u64`（全 crate 统一 u64 ms 外部时间注入惯例，同 v0.89.0 D8） |
//! | **D4** | `ForecastModel: Send + Sync` | 不要求 Send+Sync（no_std 单线程惯例，同 agent-bus-dds D2） |
//! | **D5** | `docs/phase2/forecast.md` | `docs/agents/twin-forecast-design.md`（记忆 §2.3.3 文档分类强制） |
//! | **D6** | `tests/forecast_accuracy.rs` 独立集成测试 | src 内嵌单元测试 T1~T40（项目惯例）；历史回测误差 <5% 标注**集成阶段验收**（v0.25.0 TSDB 历史接入后回测），本版本以合成平滑数据验证持续法 MAPE < 5% |
//! | **D7** | 选型 LSTM/GRU ⭐ 采用 | 本版本交付 `ForecastModel` trait 抽象 + `PersistenceModel`/`MeanModel` 基线 + 兜底链；LSTM 权重分发与推理后端后续版本接入（v0.61.0 模型部署线仅覆盖 LLM GGUF，不含 LSTM 通道） |
//! | **D8** | §4.4"退化均值预测" vs §5.1"持续法兜底"表述不一致 | 统一兜底链：主模型 `Err` → `PersistenceModel`；`MeanModel` 为可选主模型（无历史缓冲时均值 ≡ 持续法单样本，v0.89.0 镜像仅存最新态） |
//! | **D9** | `forecast(model, horizon)` 无步长参数 | `Predictor::new(step_ms, max_points, confidence_threshold)` 构造注入；点数 = `horizon_ms / step_ms` 向上取整，钳制 `1..=max_points`（默认 96，§43.6 内存预算防 OOM） |
//! | **D10** | §4.4"置信度低 → 标记" | `ForecastResult` 增加 `degraded: bool`（走兜底链 或 confidence < threshold 时置位）；confidence = base_confidence × 区间紧度，确定性计算可复现 |
//! | **D11** | 发布 `/power/twin/forecast` | `ForecastResult::to_json()`（全量含 points 数组，≤96 点约 4KB）+ `publish_forecast(node, writer, result)` 辅助函数；复用 agent-bus-dds `DdsNode`，不新增 writer 管理逻辑 |
//! | **D12** | —（蓝图未覆盖） | NaN/Inf 防御（v0.88.0 C140 教训）：输入功率非有限 → `sanitize` 按 0.0 处理，`confidence` 置 0 且 `degraded = true` |
//!
//! # 偏差声明（v0.91.0 D1~D12）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 |
//! |------|---------|-----------|
//! | **D1** | crate 路径 `twin_agent/src/whatif.rs` | 既有 `twin-agent/src/whatif.rs`（连字符命名，v0.89.0 D12 惯例） |
//! | **D2** | `name: String` / `scenario: String` / `metric: String` | 全部 `&'static str`（无堆分配，同 v0.90.0 D2；场景模板名静态化，动态命名后续版本） |
//! | **D3** | `duration: Duration` | `duration_ms: u64`（全 crate 统一 u64 ms 外部时间注入惯例） |
//! | **D4** | `Box<dyn SimModel>`（蓝图未标注约束） | trait 不要求 Send+Sync（no_std 单线程惯例，同 v0.90.0 D4） |
//! | **D5** | `docs/phase2/whatif.md` | `docs/agents/twin-whatif-design.md`（记忆 §2.3.3 文档分类强制） |
//! | **D6** | `tests/whatif.rs` 独立集成测试 | src 内嵌单元测试 T1~T40（项目惯例）；分析 <1s 标注**集成阶段验收** |
//! | **D7** | 蓝图 `Vec<Action>` 但 `Action` 类型未定义 | 本地定义 4 变体：`SetDevicePower { device_id, power: f64 }` / `RemoveDevice { device_id }` / `SetGridPower { active_power: f32 }` / `SetMarketPrice { price: f32 }`（覆盖 TwinModel 设备/电网/市场三面） |
//! | **D8** | 蓝图 `sim_state.apply(action)`（TwinModel 无此方法） | `whatif.rs` 内自由函数 `apply_action(state: &mut TwinModel, action: &Action)`（model.rs 零改动，Surgical） |
//! | **D9** | 选型"简化解析模型 ⭐ 实时" | `AnalyticalSimModel { battery_capacity_kwh: f64 }`：仅 SOC 线性推演（`soc -= power × hours / capacity`，clamp [0,1]），grid/market 透传；详细动态仿真/蒙特卡洛后续版本 |
//! | **D10** | §4.4"仿真发散 → 标记高风险"+"模型不可用 → 拒绝分析" | `WhatIfError { ModelUnavailable, Diverged }`；`SimModel::run` 返回 `Err(Diverged)` → analyze 转 `Ok(result)` 且 `risk_level = Critical` + outcomes 空；`Err(ModelUnavailable)` → analyze 拒绝（透传 Err） |
//! | **D11** | `compute_outcomes`/`assess_risk` 蓝图仅骨架未定义 | 本版本定义 3 指标：`grid_active_power` / `total_device_power` / `min_soc`（空设备 min_soc=1.0 中性）；风险规则取最重：min_soc ≤ 0 → Critical；min_soc < 0.2 → High；grid 功率相对波动 > 50% → Medium；else Low |
//! | **D12** | —（蓝图未覆盖） | NaN/Inf 防御（v0.88.0 C140 教训）：action/sim 中非有限功率复用 `model_forecast::sanitize` 按 0.0；`battery_capacity_kwh` 非有限或 ≤ 0 → 默认 100.0 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` / `core::*` 与声明依赖（agent-bus-dds / grid-agent /
//! device-agent / serde / serde_json），可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod mirror;
pub mod model;
pub mod model_forecast;
pub mod predictor;
pub mod whatif;

pub use mirror::{TwinError, TwinMirror};
pub use model::{DeviceTwin, MarketMirror, TwinModel, TwinSnapshot};
pub use model_forecast::{
    compute_confidence, ForecastError, ForecastModel, ForecastPoint, ForecastResult, MeanModel,
    PersistenceModel,
};
pub use predictor::{publish_forecast, Predictor};
pub use whatif::{
    apply_action, assess_risk, compute_outcomes, Action, AnalyticalSimModel, Outcome, RiskLevel,
    Scenario, ScenarioResult, SimModel, WhatIfAnalyzer, WhatIfError,
};
