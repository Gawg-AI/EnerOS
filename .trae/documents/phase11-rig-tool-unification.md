# Phase 11 — rig Tool 实化 + 统一推理引擎

## Summary
1. 将 rig_tools.rs 中的 4 个占位 Tool 连接到真实电力系统分析能力（PowerNetwork）
2. 在 RigReasoningEngine 中注册 Tool 到 rig Agent，实现真正的 tool-calling
3. 废弃 LlmReasoningEngine，统一到 RigReasoningEngine

## Current State Analysis

### 已完成
- Phase 1-10 全部完成
- rig 框架已集成（RigReasoningEngine + 4 个 Tool 结构体）
- 但 Tool 的 `call()` 方法是占位实现，返回硬编码 JSON
- RigReasoningEngine 构建 Agent 时没有注册任何 Tool
- LlmReasoningEngine 和 RigReasoningEngine 功能重叠

### 关键断裂点
1. **Tool 未注册到 Agent** — `tool_set` 标记 `dead_code`，Agent 构建时无 `.tool()` 调用
2. **Tool call() 是占位** — 返回硬编码 JSON，未调用真实分析引擎
3. **reasoning crate 缺少分析依赖** — Cargo.toml 无 `eneros-network`/`eneros-constraint`
4. **RigReasoningEngine 无网络引用** — 创建时不传入 PowerNetwork
5. **LlmReasoningEngine 与 RigReasoningEngine 重叠** — 维护两套 LLM 调用逻辑

### 电力系统分析能力位置
- `PowerNetwork::solve()` → PowerFlowResult（潮流计算）
- `PowerNetwork::check_constraints(&PowerFlowResult)` → Vec<Violation>
- `PowerNetwork::check_n1_with_limits(v_min, v_max, thermal)` → Vec<N1Result>
- `PowerNetwork::check_stability(&PowerFlowResult)` → StabilityResult

所有分析能力都在 `PowerNetwork` 上，而 `PowerNetwork` 已在 `AgentContext` 中作为 `Arc<RwLock<PowerNetwork>>` 存在。

## Proposed Changes

### 1. 添加依赖到 eneros-reasoning/Cargo.toml
**文件**: `crates/eneros-reasoning/Cargo.toml`

添加 `eneros-network` 和 `eneros-constraint` 依赖（feature-gated）：
```toml
[features]
rig = ["rig-core", "schemars", "eneros-network", "eneros-constraint"]

[dependencies]
# rig integration (optional — feature-gated)
eneros-network = { path = "../eneros-network", optional = true }
eneros-constraint = { path = "../eneros-constraint", optional = true }
```

### 2. 改造 rig_tools.rs — Tool 实化
**文件**: `crates/eneros-reasoning/src/rig_tools.rs`

**改动**:
- 每个 Tool 结构体添加 `network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>` 字段
- `PowerSystemToolSet` 改为持有 `network` 引用，提供 `new(network)` 和 `all(network)` 构造方法
- `PowerFlowTool::call()` — 调用 `network.read().solve()`，返回 JSON 格式的潮流结果
- `ConstraintCheckTool::call()` — 调用 `network.read().solve()` + `check_constraints()`，返回违规列表
- `N1AnalysisTool::call()` — 调用 `network.read().check_n1_with_limits()`，返回 N-1 结果
- `VoltageStabilityTool::call()` — 调用 `network.read().solve()` + `check_stability()`，返回稳定裕度
- 每个工具的 `call()` 增加错误处理：求解失败时返回错误信息 JSON

### 3. 改造 rig_engine.rs — 注册 Tool 到 Agent
**文件**: `crates/eneros-reasoning/src/rig_engine.rs`

**改动**:
- `RigReasoningEngine::new()` 接受额外的 `network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>` 参数
- 移除 `#[allow(dead_code)]`，`tool_set` 使用 `PowerSystemToolSet::all(network)` 构建
- `reason_via_rig()` 中根据 `tool_set` 配置，在 Agent builder 上调用 `.tool()` 注册工具
- 处理 rig Agent 的多轮工具调用结果，将工具调用记录添加到 `ReasoningOutput.reasoning_chain`
- 将 `build_agent()` 返回类型改为动态（使用 `Box<dyn AgentTrait>` 或在 `reason_via_rig()` 中内联构建）

### 4. 废弃 LlmReasoningEngine
**文件**: `crates/eneros-reasoning/src/llm_engine.rs`

**改动**:
- 在 `LlmReasoningEngine` 和 `LlmConfig` 上添加 `#[deprecated(since = "0.2.0", note = "Use RigReasoningEngine instead")]` 标注
- 保留代码不删除（向后兼容），但 clippy 会警告使用方迁移
- `lib.rs` 中的导出保留但标注 deprecated

### 5. 简化 main.rs 引擎选择
**文件**: `crates/eneros-api/src/main.rs`

**改动**:
- 简化为两层选择：`ENEROS_RIG_PROVIDER` → RigReasoningEngine，否则 → RuleBasedEngine
- 移除 `ENEROS_LLM_PROVIDER` 分支（LlmReasoningEngine 已 deprecated）
- RigReasoningEngine 创建时传入 `network_rw` 引用
- 添加提示信息说明迁移方式

### 6. 更新 lib.rs 导出
**文件**: `crates/eneros-reasoning/src/lib.rs`

**改动**:
- `LlmReasoningEngine` 和 `LlmConfig` 导出添加 `#[allow(deprecated)]`
- rig feature 导出保持不变

### 7. 添加集成测试
**文件**: `crates/eneros-reasoning/tests/rig_integration.rs`（新建）

**测试用例**:
- `test_rig_power_flow_tool` — PowerFlowTool 调用真实 PowerNetwork::solve()
- `test_rig_constraint_check_tool` — ConstraintCheckTool 调用真实约束检查
- `test_rig_n1_analysis_tool` — N1AnalysisTool 调用真实 N-1 分析
- `test_rig_voltage_stability_tool` — VoltageStabilityTool 调用真实稳定分析
- `test_rig_engine_with_tools` — RigReasoningEngine 构建带 Tool 的 Agent

## Assumptions & Decisions

1. **Tool 通过 Arc<RwLock<PowerNetwork>> 访问分析能力** — 不直接依赖 ConstraintEngine，因为 PowerNetwork 已封装了所有分析方法
2. **parking_lot::RwLock 而非 tokio::sync::RwLock** — PowerNetwork 的 solve() 是 CPU 密集型同步操作，不需要 async lock
3. **LlmReasoningEngine 标记 deprecated 但不删除** — 向后兼容，给用户迁移时间
4. **rig feature 仍为可选** — 默认编译不需要 rig，启用 `--features rig` 才有 AI 能力
5. **Tool call() 中捕获所有错误** — 求解不收敛、网络为空等异常情况返回错误 JSON 而非 panic
6. **不改变 ReasoningEngine trait** — 保持 `reason(&self, ReasoningInput) -> Result<ReasoningOutput>` 不变

## Verification Steps

1. `cargo test -p eneros-reasoning --features rig` — rig 集成测试通过
2. `cargo test --workspace` — 全部通过
3. `cargo clippy --workspace` — 零错误（deprecated 警告可接受）
4. `cargo clippy -p eneros-reasoning --features rig` — 零错误
