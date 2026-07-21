# Tasks

- [x] Task 1: 实现 `src/model_forecast.rs` — 数据结构 + ForecastModel trait + 基线模型 + 测试 T1~T26
  - [x] SubTask 1.1: `ForecastPoint { time: u64, value: f32, lower: f32, upper: f32 }`（Debug, Clone, Copy, PartialEq, Default + serde Serialize/Deserialize，中文字段 doc）
  - [x] SubTask 1.2: `ForecastResult { target: &'static str, horizon_ms: u64, points: Vec<ForecastPoint>, confidence: f32, degraded: bool }`（Debug, Clone + serde Serialize；D2/D3/D10）
  - [x] SubTask 1.3: `ForecastError { Dds(DdsError) }` 单变体（Debug；`From<DdsError>`；复用 `eneros_agent_bus_dds::DdsError`，同 v0.89.0 TwinError 模式）
  - [x] SubTask 1.4: `ForecastModel` trait（D4 无 Send+Sync）：`predict(&self, input: &TwinModel, horizon_ms: u64, step_ms: u64) -> Result<Vec<ForecastPoint>, ForecastError>` + `fn name(&self) -> &'static str` + `fn base_confidence(&self) -> f32`
  - [x] SubTask 1.5: `sanitize(v: f32) -> f32`（D12：非有限 → 0.0）+ `current_power(model: &TwinModel) -> (f32, bool)` 辅助：grid.timestamp>0 → sanitize(grid.active_power), has_data=true；否则 devices 非空 → Σ sanitize(device.power), true；否则 (0.0, false)
  - [x] SubTask 1.6: `PersistenceModel`（unit struct）：predict → 点数 `ceil(horizon_ms/step_ms)` 钳制 `1..=96`（内部常量，Predictor 侧另有 max_points）；time = `input.last_update + (i+1)*step_ms`；value 恒定 = current_power；lower/upper = value ∓/± |value|*0.05；无数据（has_data=false）→ 返回全 0 值点（value=0.0, lower=0.0, upper=0.0）；`name()=="persistence"`，`base_confidence()==0.6`
  - [x] SubTask 1.7: `MeanModel`（unit struct）：同 PersistenceModel 逻辑（D8 无历史均值≡持续法），但 ±3% 带、`name()=="mean"`、`base_confidence()==0.7`
  - [x] SubTask 1.8: `compute_confidence(base: f32, points: &[ForecastPoint]) -> f32`：空 → 0.0；任一点 value/lower/upper 非有限 → 0.0；全零值点（无数据标记）→ 0.0；否则 `base * (1.0 - mean_rel_width)`，rel_width = `(upper-lower)/2 / (|value|+1e-6)`，结果钳制 [0,1]（D10）
  - [x] SubTask 1.9: 中文模块文档（D2/D3/D4/D8/D10/D12 引用）；use 仅 alloc + core + serde + agent-bus-dds + crate::model；无 std/panic!/unsafe/todo!/unimplemented!/unwrap（主代码）
  - [x] SubTask 1.10: T1 — ForecastPoint default 全零 + Copy 语义 + 字段赋值
  - [x] SubTask 1.11: T2 — ForecastResult 构造 + Clone（target=="power" / points 长度 / degraded）
  - [x] SubTask 1.12: T3 — ForecastResult serde：`to_json()`（或 serde_json::to_string）后可被 `serde_json::Value` 解析，含 target/horizon_ms/confidence/degraded/points
  - [x] SubTask 1.13: T4 — ForecastError：`From<DdsError>` 转换 + Debug 格式含 "Dds"
  - [x] SubTask 1.14: T5 — `sanitize`：NaN → 0.0 / +Inf → 0.0 / -Inf → 0.0 / 正常值不变
  - [x] SubTask 1.15: T6 — `current_power`：grid 有值优先（12.3）/ grid 空 devices 求和（1.5+2.5=4.0）/ 全空 (0.0, false) / NaN 功率被 sanitize
  - [x] SubTask 1.16: T7~T8 — PersistenceModel：60 点 + time 序列 `last_update+(i+1)*step` 逐点断言；value 恒定 + `lower < value < upper`（±5%）
  - [x] SubTask 1.17: T9~T10 — PersistenceModel：grid 空 + 2 设备 → value==4.0；全空 → 全 0 点（lower==upper==value==0.0）
  - [x] SubTask 1.18: T11 — 点数计算：horizon 60000/step 1000 → 60；horizon 500/step 1000（不足一步）→ 1；horizon 0 → 1；horizon 10_000_000/step 1 → 96 钳制
  - [x] SubTask 1.19: T12 — PersistenceModel name=="persistence" / base_confidence∈(0,1)；MeanModel name=="mean" / base_confidence > persistence 的
  - [x] SubTask 1.20: T13~T14 — MeanModel：有值场景等价持续法（value 相同，D8）但区间更窄（upper-lower 更小）；全空 → 全 0
  - [x] SubTask 1.21: T15 — compute_confidence 空 points → 0.0
  - [x] SubTask 1.22: T16 — compute_confidence 含 NaN 点 → 0.0（D12 不传播）
  - [x] SubTask 1.23: T17 — compute_confidence 全零值点 → 0.0（无数据标记）
  - [x] SubTask 1.24: T18 — compute_confidence 正常：∈(0, base]，区间窄 > 区间宽（两组对照）
  - [x] SubTask 1.25: T19 — compute_confidence 输出恒 ∈ [0,1]（构造极端大区间点仍 ≤1.0）
  - [x] SubTask 1.26: T20 — PersistenceModel 输入 NaN 功率 → 点 value 全有限（0.0）不 panic

- [x] Task 2: 实现 `src/predictor.rs` — Predictor + publish + 测试 T21~T40
  - [x] SubTask 2.1: `Predictor { model: Box<dyn ForecastModel>, step_ms: u64, max_points: usize, confidence_threshold: f32 }`（全 pub，中文字段 doc）
  - [x] SubTask 2.2: `new(model, step_ms, max_points, confidence_threshold)`：step_ms==0 → 1；max_points==0 → 1；threshold 钳制 [0,1]
  - [x] SubTask 2.3: `forecast(&self, twin, horizon_ms) -> Result<ForecastResult, ForecastError>`：主模型 predict → Ok 用其输出（按 max_points truncate 防御）；Err → PersistenceModel 兜底 + degraded=true；confidence = compute_confidence(所用模型 base, points)；confidence < threshold → degraded=true；target="power"（D2/D9/D10）
  - [x] SubTask 2.4: `forecast_and_publish(&self, twin, horizon_ms, node, writer) -> Result<ForecastResult, ForecastError>`：forecast → publish_forecast → 返回 result
  - [x] SubTask 2.5: `publish_forecast(node: &mut dyn DdsNode, writer: WriterId, result: &ForecastResult) -> Result<(), ForecastError>`：serde_json::to_vec(result) → node.write → 失败 `Err(ForecastError::Dds(_))`（D11）
  - [x] SubTask 2.6: 中文模块文档（D8/D9/D10/D11 引用）；无 std/async/unwrap/panic!/unsafe
  - [x] SubTask 2.7: 测试辅助 — `FailingModel`（predict 恒 Err，base_confidence 0.9，name "failing"）+ MockDdsNode + 外部 reader 校验样本
  - [x] SubTask 2.8: T21 — new 钳制：step_ms 0→1 / max_points 0→1 / threshold 1.5→1.0 / -0.5→0.0
  - [x] SubTask 2.9: T22 — forecast 主模型成功：用 PersistenceModel 为主，grid=12.3 → 60 点 value==12.3，degraded==false（confidence 0.57 ≥ 默认阈值 0.5）
  - [x] SubTask 2.10: T23 — forecast 主模型失败（FailingModel）→ 兜底持续法 points + degraded==true
  - [x] SubTask 2.11: T24 — forecast 空 TwinModel → 全 0 点 + confidence==0.0 + degraded==true
  - [x] SubTask 2.12: T25 — horizon_ms 0 → 1 点；horizon 500/step 1000 → 1 点
  - [x] SubTask 2.13: T26 — 点数 ≤ max_points：max_points=10, horizon 60000/step 1000 → 10 点（truncate 生效）
  - [x] SubTask 2.14: T27 — 确定性：同输入两次 forecast → 逐点一致 + confidence 一致
  - [x] SubTask 2.15: T28 — confidence < threshold 置 degraded：threshold=0.99，持续法（conf≈0.57）→ degraded==true
  - [x] SubTask 2.16: T29 — threshold=0.0 → 持续法正常数据 degraded==false
  - [x] SubTask 2.17: T30 — target=="power" + horizon_ms 回显正确
  - [x] SubTask 2.18: T31 — 主模型为 MeanModel：value 正确 + confidence > 持续法同场景（base 0.7 vs 0.6）
  - [x] SubTask 2.19: T32 — FailingModel 失败时 target/horizon 仍正确（兜底结果完整）
  - [x] SubTask 2.20: T33 — publish_forecast：MockDdsNode 写入成功，外部 reader take 到 1 条样本
  - [x] SubTask 2.21: T34 — publish payload 可解析：`target=="power"` + `points` 为数组 + `confidence` 为数值 + `degraded` 为 bool
  - [x] SubTask 2.22: T35 — publish 失败：node shutdown → `Err(ForecastError::Dds(_))`
  - [x] SubTask 2.23: T36 — forecast_and_publish 端到端：返回 result 与 reader 收到样本内容一致（points 数一致）
  - [x] SubTask 2.24: T37 — 合成平滑数据精度验证（D6）：value=50.0 恒定 → 持续法预测 MAPE == 0.0 < 5%（误差目标占位验证，集成回测在 TSDB 接入后）
  - [x] SubTask 2.25: T38 — 缓变斜坡数据（每步 +0.1%）持续法 MAPE < 5%
  - [x] SubTask 2.26: T39 — 多设备场景：3 设备 power 和 = 6.0 → forecast value==6.0（grid 无数据回退路径端到端）
  - [x] SubTask 2.27: T40 — 连续两次 forecast_and_publish → reader 收到 2 条样本（无状态串扰）

- [x] Task 3: lib.rs 集成（Surgical 追加）
  - [x] SubTask 3.1: `pub mod model_forecast;` + `pub mod predictor;`（保持 mirror/model 顺序在前）
  - [x] SubTask 3.2: 重导出追加：`pub use model_forecast::{compute_confidence, ForecastError, ForecastModel, ForecastPoint, ForecastResult, MeanModel, PersistenceModel};` + `pub use predictor::{publish_forecast, Predictor};`
  - [x] SubTask 3.3: crate 文档升级：标题加 v0.90.0 短期预测段 + D1~D12 偏差简表（v0.89.0 文档保留，新旧并存标注版本）
  - [x] SubTask 3.4: 既有 40 个 v0.89.0 测试全部保持通过（零改动验证）

- [x] Task 4: 创建 `configs/twin_forecast.toml`
  - [x] SubTask 4.1: `[forecast]` 段：step_ms = 1000 / max_points = 96 / confidence_threshold = 0.5 / target = "power"
  - [x] SubTask 4.2: `publish_topic = "/power/twin/forecast"`（D11）+ `model_path = ""`（LSTM 占位，D7：本版本持续法/均值基线）
  - [x] SubTask 4.3: `fallback = "persistence"`（D8 兜底链）+ 可选 `model = "mean"` 注释示例
  - [x] SubTask 4.4: 中文注释：误差目标 <5%（蓝图 §7.2，集成阶段回测验收）/ 推理 <100ms（集成阶段）/ 只读安全（§7.3）/ NaN 防御（D12）/ 点数上限防 OOM（§43.6）

- [x] Task 5: 创建 `docs/agents/twin-forecast-design.md`
  - [x] SubTask 5.1: 12 章节（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 5.2: Mermaid 图 1：核心算法（TwinModel → current_power 提取 → 主模型 predict ? → 失败兜底 PersistenceModel → compute_confidence → degraded 判定 → ForecastResult → 发布 /power/twin/forecast）
  - [x] SubTask 5.3: Mermaid 图 2：forecast 决策流程（主模型 Err? → 兜底 / confidence < threshold? → degraded / 点数钳制）
  - [x] SubTask 5.4: D1~D12 偏差声明表（从 spec.md 复制）
  - [x] SubTask 5.5: 前置依赖引用 v0.89.0 镜像（TwinModel）+ v0.75.0 DDS（DdsNode）+ v0.82.0 GridState + v0.73.0 DeviceState
  - [x] SubTask 5.6: 性能目标（误差 <5% + 推理 <100ms，均标注"集成阶段验收，本版本交付算法骨架+合成数据单元验证"）
  - [x] SubTask 5.7: 选型对比表（持续法 兜底 / ARIMA 备选 / LSTM-GRU ⭐ 后续接入，蓝图 §5.1）+ 均值法说明（D8）
  - [x] SubTask 5.8: GPU 规则说明（蓝图 §6.6：LSTM 推理接入时优先 GPU/禁用梯度；本版本纯标量 CPU 计算无张量）+ 下游引用 v0.91.0 What-if / v0.112.0 云端孪生
  - [x] SubTask 5.9: 风险：突变场景预测失效（蓝图 §8.5）/ 无历史缓冲限制（D8，后续接 TSDB v0.25.0）/ 内存（points ≤96，§43.6）

- [x] Task 6: 版本同步根目录文件
  - [x] SubTask 6.1: 根 `Cargo.toml` `[workspace.package] version = "0.89.0"` → `"0.90.0"`
  - [x] SubTask 6.2: `Makefile` `# Version: v0.90.0` + `VERSION := 0.90.0`
  - [x] SubTask 6.3: `.github/workflows/ci.yml` `# Version: v0.90.0`
  - [x] SubTask 6.4: `ci/src/gate.rs` clippy 段 + test 段注释追加 `+ v0.90.0 孪生预测：Predictor / ForecastModel / ForecastResult / ForecastPoint / PersistenceModel / MeanModel / ForecastError`

- [x] Task 7: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 7.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 7.2: `cargo test -p eneros-twin-agent` 80 tests 全过（40 既有 + 40 新增，0 failures）
  - [x] SubTask 7.3: `cargo build -p eneros-twin-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 7.4: `cargo fmt -p eneros-twin-agent -- --check` 通过
  - [x] SubTask 7.5: `cargo clippy -p eneros-twin-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 7.6: `cargo deny check licenses bans sources` 通过（无新第三方依赖）
  - [x] SubTask 7.7: 回归 — `cargo test -p eneros-grid-agent` / `cargo test -p eneros-device-agent` / `cargo test -p eneros-energy-market-agent` / `cargo test -p eneros-agent-bus-dds` 全过

# Task Dependencies

- [Task 2] depends on [Task 1]
- [Task 3] depends on [Task 2]
- [Task 4, Task 5] 独立（可与 1~3 并行）
- [Task 6] 独立（仅根目录 4 文件，不碰 crate 源码）
- [Task 7] depends on [Task 3, Task 4, Task 5, Task 6]

# 并行执行计划

- **Sub-Agent A**：Task 1 + Task 2 + Task 3（同 crate 源文件，串行单 agent 保证一致性）
- **Sub-Agent B**：Task 4 + Task 5（configs + docs，与 A 并行）
- **Sub-Agent C**：Task 6（版本同步，与 A/B 并行；仅根目录 4 文件）
- **主 agent**：Task 7（全部完成后统一构建校验 + 回归）
