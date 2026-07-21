# Checklist

## Task 1: model_forecast.rs 数据结构与基线模型
- [x] C1: `ForecastPoint` 4 字段（time/value/lower/upper）+ 派生 Debug, Clone, Copy, PartialEq, Default + serde Serialize/Deserialize
- [x] C2: `ForecastResult` 5 字段（target: &'static str / horizon_ms / points / confidence / degraded）+ Debug, Clone + serde Serialize（D2/D3/D10）
- [x] C3: `ForecastError` 单变体 `Dds(DdsError)` + Debug + `From<DdsError>`
- [x] C4: `ForecastModel` trait 3 方法（predict/name/base_confidence），无 Send+Sync 约束（D4）
- [x] C5: `sanitize` 非有限值 → 0.0（D12）
- [x] C6: `current_power` 辅助：grid 优先 / devices 求和回退 / 全空 (0.0, false)
- [x] C7: `PersistenceModel` predict：点数 ceil(horizon/step) 钳制 1..=96；time 序列 last_update+(i+1)*step；value 恒定；±5% 区间带
- [x] C8: `PersistenceModel` 无数据 → 全 0 点；name=="persistence"；base_confidence==0.6
- [x] C9: `MeanModel` 同逻辑 ±3% 带；name=="mean"；base_confidence==0.7（D8）
- [x] C10: `compute_confidence`：空 → 0.0 / NaN 点 → 0.0 / 全零点 → 0.0 / 正常 ∈(0,base] / 钳制 [0,1]
- [x] C11: model_forecast.rs 无 std/panic!/unsafe/todo!/unimplemented!/unwrap（主代码）+ 中文模块文档含偏差引用
- [x] C12: T1~T6（结构/default/serde/Error/sanitize/current_power）存在且断言符合 spec
- [x] C13: T7~T12（Persistence 点数/时间序列/区间/设备求和/全空/钳制/name）存在且通过
- [x] C14: T13~T14（MeanModel 等价性/区间更窄/全空）存在且通过
- [x] C15: T15~T19（compute_confidence 空/NaN/全零/宽窄对照/钳制）存在且通过
- [x] C16: T20（NaN 输入防御）存在且通过

## Task 2: predictor.rs Predictor + 发布
- [x] C17: `Predictor` 4 字段全 pub（model/step_ms/max_points/confidence_threshold）
- [x] C18: `new` 钳制：step_ms 0→1 / max_points 0→1 / threshold 钳制 [0,1]
- [x] C19: `forecast`：主模型 Ok → 其输出（max_points truncate 防御）；Err → PersistenceModel 兜底 + degraded=true
- [x] C20: `forecast`：confidence < threshold → degraded=true；target=="power"；horizon_ms 回显
- [x] C21: `forecast_and_publish`：forecast → publish → 返回 result
- [x] C22: `publish_forecast`：serde_json::to_vec → node.write；失败 → Err(ForecastError::Dds(_))（D11）
- [x] C23: predictor.rs 主代码无 std/async/unwrap/panic!/unsafe + 中文模块文档
- [x] C24: 测试辅助 FailingModel + MockDdsNode 可用
- [x] C25: T21~T26（new 钳制/主成功/兜底/空模型/horizon 边界/max_points 截断）存在且通过
- [x] C26: T27~T32（确定性/阈值置位/零阈值/target 回显/MeanModel 主/兜底完整性）存在且通过
- [x] C27: T33~T36（publish 成功/payload 可解析/publish 失败/端到端一致）存在且通过
- [x] C28: T37~T38（恒定数据 MAPE==0 < 5% / 缓变斜坡 MAPE < 5%）存在且通过（D6 占位验证）
- [x] C29: T39~T40（多设备求和端到端 / 连续两次发布无串扰）存在且通过

## Task 3: lib.rs 集成
- [x] C30: `pub mod model_forecast;` + `pub mod predictor;` 追加（mirror/model 不变）
- [x] C31: 9 项新重导出（compute_confidence/ForecastError/ForecastModel/ForecastPoint/ForecastResult/MeanModel/PersistenceModel/publish_forecast/Predictor）
- [x] C32: crate 文档含 v0.90.0 段 + D1~D12 偏差简表（v0.89.0 文档保留）
- [x] C33: 既有 v0.89.0 40 测试零改动全部通过

## Task 4: configs/twin_forecast.toml
- [x] C34: 文件位于 `configs/twin_forecast.toml`
- [x] C35: `[forecast]` 段：step_ms=1000 / max_points=96 / confidence_threshold=0.5 / target="power"
- [x] C36: publish_topic="/power/twin/forecast" + model_path 占位（D7）+ fallback="persistence"（D8）
- [x] C37: 中文注释含误差 <5% / 推理 <100ms（均标注集成阶段）/ 只读安全 / NaN 防御 / 点数上限防 OOM

## Task 5: docs/agents/twin-forecast-design.md
- [x] C38: 文件位于 `docs/agents/twin-forecast-design.md`（非 docs/phase2，D5）
- [x] C39: 12 章节完整
- [x] C40: Mermaid 图 1（核心算法：TwinModel → 主模型 → 兜底 → 置信度 → 发布）
- [x] C41: Mermaid 图 2（forecast 决策流程：Err 兜底 / 阈值判定 / 钳制）
- [x] C42: D1~D12 偏差声明表完整
- [x] C43: 前置依赖引用 v0.89.0 + v0.75.0 + v0.82.0 + v0.73.0
- [x] C44: 性能目标（误差 <5% / 推理 <100ms，标注集成阶段验收）
- [x] C45: 选型对比表（持续法 / ARIMA / LSTM-GRU ⭐ / 均值法 D8）
- [x] C46: GPU 规则说明（LSTM 接入时优先 GPU + 禁用梯度；本版本纯标量 CPU）
- [x] C47: 下游引用 v0.91.0 / v0.112.0；风险含突变失效 / 无历史缓冲 / 内存上限

## Task 6: 版本同步
- [x] C48: 根 `Cargo.toml` version = "0.90.0"
- [x] C49: 根 `Cargo.toml` members 不变（twin-agent 已存在，无新增 member）
- [x] C50: `Makefile` `# Version: v0.90.0` + `VERSION := 0.90.0`
- [x] C51: `.github/workflows/ci.yml` `# Version: v0.90.0`
- [x] C52: `ci/src/gate.rs` clippy 段注释追加 v0.90.0 类型列表
- [x] C53: `ci/src/gate.rs` test 段注释追加 v0.90.0 类型列表

## Task 7: 构建校验（§2.4.2）
- [x] C54: `cargo metadata` 成功
- [x] C55: `cargo test -p eneros-twin-agent` 80 tests 全过（40 既有 + 40 新增）
- [x] C56: aarch64-unknown-none 交叉编译通过
- [x] C57: `cargo fmt --check` 通过
- [x] C58: `cargo clippy --all-targets -- -D warnings` 无 warning
- [x] C59: `cargo deny check licenses bans sources` 通过
- [x] C60: 回归 grid-agent / device-agent / energy-market-agent / agent-bus-dds 全过

## 总体校验
- [x] C61: 无新 crate（复用 crates/agents/twin-agent/，§2.3.1 目录结构不变）
- [x] C62: 无 `docs/` 根目录平面化文档（docs/agents/ 下）
- [x] C63: 配置文件在 `configs/` 下（非 config/）
- [x] C64: `.gitignore` 无需更新（无新文件类型）
- [x] C65: `git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C66: ADR 决策未违反（未引入研究特性、复用既有 DdsNode/TwinModel、无重复造轮子）
- [x] C67: no_std 合规：子模块不重复 crate 属性 + 无 std/async
- [x] C68: 内存预算合规：points ≤ max_points（默认 96）钳制防 OOM（§43.6）
- [x] C69: SBOM 无新第三方依赖（serde/serde_json/agent-bus-dds 既有）
- [x] C70: Surgical Changes：mirror.rs / model.rs 零改动；lib.rs 仅追加；既有 crate 零改动（仅根目录 4 文件版本同步）
- [x] C71: 命名不冲突（model_forecast/predictor 前缀全新）
- [x] C72: 时间注入合规：点时间由 model.last_update + step 推导，无 Instant::now()
- [x] C73: 无效输入无 panic（NaN/空模型/零 horizon/零 step 全部走安全路径）
- [x] C74: 确定性：无随机源，同输入两次 forecast 结果一致
- [x] C75: 预测只读安全：forecast 不修改 TwinModel（&self + &TwinModel 不可变引用，蓝图 §7.3）
