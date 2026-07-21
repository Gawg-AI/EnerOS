# Checklist

## Task 1: whatif.rs 数据结构与 apply_action
- [x] C1: `Action` 4 变体（SetDevicePower{device_id:u64,power:f64} / RemoveDevice{device_id} / SetGridPower{active_power:f32} / SetMarketPrice{price:f32}）+ 派生 Debug, Clone, Copy, PartialEq（D7）
- [x] C2: `Scenario` 3 字段（name: &'static str / actions: Vec\<Action\> / duration_ms: u64）+ Debug, Clone（D2/D3）
- [x] C3: `Outcome` 3 字段（metric: &'static str / value: f32 / baseline: f32）+ Debug, Clone, Copy, PartialEq
- [x] C4: `RiskLevel` 4 变体 + PartialOrd/Ord（Low<Medium<High<Critical）+ Default==Low + Copy + serde Serialize
- [x] C5: `ScenarioResult` 3 字段 + Debug, Clone + serde Serialize + `summary_json()` 含 scenario/risk_level/outcomes
- [x] C6: `WhatIfError` 2 变体（ModelUnavailable / Diverged）+ Debug + PartialEq（D10）
- [x] C7: `apply_action` 4 分支语义正确（存在设备才更新 / 移除 / grid 更新 / market or_insert 覆盖）
- [x] C8: apply_action 非有限功率 sanitize 为 0.0（D12，f32 复用 model_forecast::sanitize）
- [x] C9: whatif.rs 无 std/panic!/unsafe/todo!/unimplemented!/unwrap（主代码）+ 中文模块文档含偏差引用
- [x] C10: T1~T4（Action/Scenario/Outcome/RiskLevel 派生与语义）存在且通过
- [x] C11: T5~T7（ScenarioResult Clone/summary_json 可解析/WhatIfError 相等性）存在且通过
- [x] C12: T8~T11（SetDevicePower 更新/不存在无副作用/NaN 防御/RemoveDevice）存在且通过
- [x] C13: T12~T14（SetGridPower/NaN/SetMarketPrice None→Some 与覆盖）存在且通过

## Task 2: SimModel + AnalyticalSimModel + outcomes/risk
- [x] C14: `SimModel` trait 2 方法（run/name），run 消费 TwinModel 返回 Result<TwinModel, WhatIfError>，无 Send+Sync（D4）
- [x] C15: `AnalyticalSimModel::new` 容量钳制：非有限或 ≤0 → 100.0（D12）
- [x] C16: `run`：soc -= sanitize(power)×hours/capacity + clamp [0,1]；grid/market/last_update 透传；name=="analytical"（D9）
- [x] C17: `compute_outcomes` 3 指标固定顺序（grid_active_power / total_device_power / min_soc），空设备 min_soc=1.0 中性（D11）
- [x] C18: `assess_risk` 取最重：min_soc≤0→Critical / <0.2→High / grid 相对波动>50%→Medium / else Low；空 outcomes→Low（D11）
- [x] C19: 波动率除零防御（|baseline|<1e-6 安全路径）
- [x] C20: T15（容量钳制 5 组输入）存在且通过
- [x] C21: T16~T19（放电 0.7/充电/clamp 双界/零时长）存在且通过
- [x] C22: T20~T21（多设备+透传+name/NaN 功率设备 soc 不变）存在且通过
- [x] C23: T22~T24（outcomes 3 指标/数值回显/空设备中性 1.0）存在且通过
- [x] C24: T25~T27（Critical/High/0.2 与 0.19 边界）存在且通过
- [x] C25: T28~T31（Medium 波动/恰 50% Low/空 outcomes/取最重 Critical）存在且通过

## Task 3: WhatIfAnalyzer
- [x] C26: `WhatIfAnalyzer { sim_model: Box<dyn SimModel> }` 字段 pub
- [x] C27: `analyze` 流程：clone → 逐 action apply → run → outcomes → risk → scenario 名回显
- [x] C28: 发散 `Err(Diverged)` → Ok + outcomes 空 + risk==Critical（D10 / 蓝图 §4.4 §6.5）
- [x] C29: `Err(ModelUnavailable)` → 透传 Err 拒绝分析（蓝图 §4.4）
- [x] C30: analyze 只读（输入 model 不变）+ 确定性（无随机源，同输入结果位级一致）
- [x] C31: 测试辅助 DivergingSimModel / UnavailableSimModel 可用
- [x] C32: T32~T33（重放电 Critical min_soc==0.0 / 平稳 Low + 名回显 + 3 outcomes）存在且通过
- [x] C33: T34~T35（发散/不可用故障注入）存在且通过
- [x] C34: T36~T37（确定性/只读）存在且通过
- [x] C35: T38~T39（组合动作/空 actions 恒等 Low）存在且通过
- [x] C36: T40（SetGridPower 场景 outcome 16.0/10.0 + Medium）存在且通过

## Task 4: lib.rs 集成
- [x] C37: `pub mod whatif;` 追加（mirror/model/model_forecast/predictor 不变）
- [x] C38: 12 项新重导出（apply_action/assess_risk/compute_outcomes/Action/AnalyticalSimModel/Outcome/RiskLevel/Scenario/ScenarioResult/SimModel/WhatIfAnalyzer/WhatIfError）
- [x] C39: crate 文档含 v0.91.0 段 + D1~D12 偏差简表（v0.89.0/v0.90.0 文档保留）
- [x] C40: 既有 v0.89.0+v0.90.0 共 80 测试零改动全部通过

## Task 5: configs/twin_whatif.toml
- [x] C41: 文件位于 `configs/twin_whatif.toml`
- [x] C42: `[whatif]` 段：battery_capacity_kwh=100.0 + min_soc_high_threshold=0.2 + grid_deviation_medium_threshold=0.5（与 assess_risk 规则一致）
- [x] C43: `[[scenario]]` 场景模板 3 例（重放电/设备退出/电网功率设定），含 name/duration_ms/actions
- [x] C44: 中文注释含分析 <1s（集成阶段）/ 高风险拦截 / 解析模型局限（D9）/ NaN 防御（D12）/ 发散→Critical（D10）

## Task 6: docs/agents/twin-whatif-design.md
- [x] C45: 文件位于 `docs/agents/twin-whatif-design.md`（非 docs/phase2，D5）
- [x] C46: 12 章节完整
- [x] C47: Mermaid 图 1（核心算法：clone → apply → sim → outcomes → risk → result）
- [x] C48: Mermaid 图 2（analyze 决策流：Diverged→Critical 空 outcomes / ModelUnavailable→Err / 风险取最重）
- [x] C49: D1~D12 偏差声明表完整
- [x] C50: 前置依赖引用 v0.89.0 + v0.90.0；下游引用 v0.112.0 云端联合仿真
- [x] C51: 选型对比表（简化解析 ⭐ / 详细动态 / 蒙特卡洛，蓝图 §5.1）
- [x] C52: 性能目标（分析 <1s，标注集成阶段验收）+ GPU 规则说明（§6.6 仿真 GPU 加速禁用梯度；本版纯标量 CPU）
- [x] C53: 高风险拦截安全语义（§7.3）+ 风险（保真度 §8.1 / 持续校准 §8.5 / clone 内存 §43.6）

## Task 7: 版本同步
- [x] C54: 根 `Cargo.toml` version = "0.91.0"
- [x] C55: 根 `Cargo.toml` members 不变（twin-agent 已存在，无新增 member）
- [x] C56: `Makefile` `# Version: v0.91.0` + `VERSION := 0.91.0`
- [x] C57: `.github/workflows/ci.yml` `# Version: v0.91.0`
- [x] C58: `ci/src/gate.rs` clippy 段 + test 段注释追加 v0.91.0 类型列表

## Task 8: 构建校验（§2.4.2）
- [x] C59: `cargo metadata` 成功
- [x] C60: `cargo test -p eneros-twin-agent` 120 tests 全过（80 既有 + 40 新增）
- [x] C61: aarch64-unknown-none 交叉编译通过
- [x] C62: `cargo fmt --check` 通过
- [x] C63: `cargo clippy --all-targets -- -D warnings` 无 warning
- [x] C64: `cargo deny check licenses bans sources` 通过
- [x] C65: 回归 grid-agent / device-agent / energy-market-agent / agent-bus-dds 全过

## 总体校验
- [x] C66: 无新 crate（复用 crates/agents/twin-agent/，§2.3.1 目录结构不变）
- [x] C67: 无 `docs/` 根目录平面化文档（docs/agents/ 下）
- [x] C68: 配置文件在 `configs/` 下（非 config/）
- [x] C69: `.gitignore` 无需更新（无新文件类型）
- [x] C70: `git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C71: ADR 决策未违反（未引入研究特性、复用既有 TwinModel/sanitize、无重复造轮子）
- [x] C72: no_std 合规：子模块不重复 crate 属性 + 无 std/async
- [x] C73: SBOM 无新第三方依赖（serde/serde_json/agent-bus-dds 既有）
- [x] C74: Surgical Changes：mirror.rs / model.rs / model_forecast.rs / predictor.rs 零改动；lib.rs 仅追加；既有 crate 零改动（仅根目录 4 文件版本同步）
- [x] C75: 无效输入无 panic（NaN/空模型/零时长/不存在设备/空 actions 全走安全路径）+ 确定性无随机源
