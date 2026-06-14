# Phase 14 — 接通确定性决策闭环（Close the Ghost Loop）

## 概述

Phase 13 实现了高质量的约束决策管道组件（`ConstrainedDecisionPipeline`、
`FeasibilityProjector`、`FeedbackLoop`、`StructuredActionOutput`），但代码审查
发现**数据流闭环从未接通**——组件都造好了，但没有任何真实数据流经它们。

Phase 14 的目标是**接通这条"幽灵闭环"**，让 Phase 13 的承诺
（"无论 LLM 输出什么，最终动作一定满足所有物理约束"）真正生效。

## Phase 13 的三个断裂点

| # | 位置 | 问题 |
|---|------|------|
| 1 | `llm_prompt.rs` 解析器 | LLM 响应解析**硬编码** `structured_actions: None`；`ReasoningOutput::from_structured()` 是死代码 |
| 2 | `action_mapping.rs:map_reasoning_output` | **只读** `output.actions: Vec<String>`，从不读 `structured_actions` |
| 3 | `orchestrator.rs:process_event/tick_all` | 只调 `dispatcher.dispatch()`（绕过管道），从不调 `dispatch_structured()`（死代码） |

## 两个架构约束（决定接线方式）

1. **依赖方向**：`eneros-gateway` 不依赖 `eneros-reasoning`（仅 dev-dep）；
   `eneros-agent` 依赖两者。→ **FeedbackLoop 必须在 agent 层接线，gateway 保持纯约束**。
2. **sync/async 不匹配**：pipeline 全同步；`FeedbackLoop::reason_with_feedback`
   是 async；`AgentContext.reasoning` 已是 `Arc<dyn ReasoningEngine>`。→ **重试闭环
   在 agent 的 async 编排点（`process_event`/`tick_all`）完成**。

## 架构原则

**Gateway 保持纯约束（同步、无推理依赖）；Agent 层负责"推理→管道→被拒→反馈重推理"
的 async 编排。** 这是最干净的分层，不引入反向依赖。

## 接通后的闭环

```
LLM 推理 ──产出 StructuredAction──► ActionMapper(优先读结构化)
                                          │
                            ┌─────────────┴──────────────┐
                       有 StructuredAction            无(回退文本匹配)
                            │                            │
                  dispatch_structured()           dispatch()(原有路径)
                            │
            ConstrainedDecisionPipeline.decide_enhanced()
                            │
              ┌─────────────┴──────────────┐
          执行成功                     被拒绝/投影失败
          (闭环达成)                        │
                              FeedbackLoop.reason_with_feedback()
                                   (agent 层 async, 最多 2 轮)
                            │
                    重新产出 StructuredAction → 回到管道
```

## 实施步骤（7 步）

### Step 1: StructuredAction JSON 解析器（reasoning 层）
**文件**: `crates/eneros-reasoning/src/llm_prompt.rs`

- 新增 `RawStructuredAction` 中间 enum（`#[serde(tag = "action_type")]`），
  将 LLM 的 JSON `structured_actions` 数组反序列化为 `StructuredAction`。
- `parse_structured_actions()` 逐条解析，**容错跳过**格式错误的条目
  （LLM 输出不可信，单条失败不拖垮整个数组）。
- `parse_llm_response()` 当 JSON 含有效 `structured_actions` 时，调用
  `ReasoningOutput::from_structured()`（**复活死代码**）产出
  `structured_actions: Some(...)`。
- 纯文本回退路径完全保留——LLM 不输出结构化时系统照常工作。

### Step 2: RigReasoningEngine prompt 引导结构化输出（reasoning 层）
**文件**: `crates/eneros-reasoning/src/llm_prompt.rs`

- `build_power_system_prompt()` 的输出格式说明中新增 `structured_actions` 数组
  的 JSON schema 文档（6 种合法 action_type + 参数）。
- 这是"尽力而为"——不强制 function calling，靠 prompt 引导 + 解析层容错。

### Step 3: ActionMapper 优先消费结构化动作（agent 层）
**文件**: `crates/eneros-agent/src/action_mapping.rs`

- `map_reasoning_output()` **先检查 `output.structured_actions`**，若有则产出
  `AgentAction::ExecuteStructured(...)`；无则回退现有文本关键词匹配。
- 这让 orchestrator 能识别结构化动作并路由到管道。

### Step 4: AgentAction::ExecuteStructured 变体 + Orchestrator 路由（agent 层）
**文件**: `crates/eneros-agent/src/agent.rs`、`orchestrator.rs`、`dispatcher.rs`

- `AgentAction` 新增 `ExecuteStructured(StructuredAction)` 变体。
- orchestrator 新增 `route_action()` 统一分发点：
  - `ExecuteStructured(sa)` → `dispatch_via_pipeline()`（**复活死代码**）；
  - 其他 → 维持 `dispatch()`。
- `dispatch()` 对 `ExecuteStructured` 的兜底处理：转换为 Command 直接执行
  （向后兼容，测试路径）。
- `dispatch_with_validation()` 的权限检查扩展覆盖 `ExecuteStructured`。

### Step 5: FeedbackLoop 改 Arc<dyn> + orchestrator 接线（reasoning + agent 层）
**文件**: `crates/eneros-reasoning/src/feedback.rs`、`orchestrator.rs`

- `FeedbackLoop.engine` 从 `Box<dyn ReasoningEngine>` 改为 `Arc<dyn ReasoningEngine>`，
  匹配 `AgentContext.reasoning` 的现有形态（避免克隆不可克隆的 `RigReasoningEngine`）。
- 新增 `new_shared()` / `with_default_iterations_shared()` 构造器；保留旧 `Box`
  构造器（向后兼容现有测试）。
- orchestrator 新增 `feedback_loop: Option<Arc<FeedbackLoop>>` 字段和
  `with_pipeline_and_feedback()` 构造器。
- `dispatch_via_pipeline()` 在动作被拒绝时调用 `retry_with_feedback()`：
  用拒绝原因重新构建 ReasoningInput → FeedbackLoop 重推理 → 拿到新 StructuredAction
  → 重新走管道。最多 2 轮（FeedbackLoop 内部限制）。

### Step 6: API 注入 FeedbackLoop（api 层）
**文件**: `crates/eneros-api/src/main.rs`

- main.rs 用已有的 `reasoning`（`Arc<dyn ReasoningEngine>`）构造 `FeedbackLoop`，
  传入 `AgentOrchestrator::with_pipeline_and_feedback()`。
- 完成生产环境的端到端接线。

### Step 7: 端到端测试 + flaky 测试修复
**文件**: `crates/eneros-agent/tests/decision_loop_integration.rs`、`bridge_client.rs`

- 5 个端到端集成测试（证明闭环真接通）：
  1. `test_structured_action_routed_through_pipeline` — 结构化动作经管道执行 ✓
  2. `test_legacy_action_still_dispatched` — 旧式动作向后兼容 ✓
  3. `test_feedback_loop_fires_after_rejection` — 拒绝后触发反馈重推理 ✓
  4. `test_rejection_without_feedback_loop_is_graceful` — 无反馈时优雅降级 ✓
  5. `test_observer_cannot_execute_structured_action` — 权限门控仍生效 ✓
- 修复 `eneros-bridge` flaky 测试：`test_bridge_script_path_exists` 不再构造
  完整 `BridgeClient`（其 `reqwest::blocking::Client` 初始化在并行测试中竞争），
  改为直接调用 `find_bridge_script()` 验证路径。

## 设计决策

1. **gateway 不引入 reasoning 依赖**：保持分层纯净。FeedbackLoop 的 async 重试
   天然属于 agent 的 async 编排层。
2. **新增 `AgentAction::ExecuteStructured` 变体**而非塞进 `ExecuteCommand`：
   保留结构化语义，让 orchestrator 能精确路由到管道。
3. **LLM 结构化输出是"尽力而为"**：不强制 function calling，靠 prompt 引导 +
   解析层容错。失败回退文本匹配，系统永远可用。
4. **FeedbackLoop 改 `Arc<dyn>`**：匹配 `AgentContext.reasoning` 的现有形态，
   避免克隆不可克隆的 `RigReasoningEngine`。
5. **向后兼容**：`structured_actions: None` 时完全走原路径；`dispatch_structured`
   无管道时回退直接执行。所有现有测试保持通过。

## 验证步骤

1. `cargo build --workspace` — 零警告
2. `cargo test -p eneros-reasoning` — 含 5 个新结构化解析测试
3. `cargo test -p eneros-agent` — 含 5 个端到端闭环测试
4. `cargo test -p eneros-gateway` — 现有管道测试不受影响
5. `cargo test --workspace` — 全绿（含修复的 bridge 测试）
6. `cargo clippy --workspace` — 零警告
