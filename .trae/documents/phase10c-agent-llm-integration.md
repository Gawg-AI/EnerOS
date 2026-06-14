# Phase 10 Part C — Agent LLM 推理集成与全局验证

## Summary
完成 Phase 10 最后一步：将 LlmReasoningEngine 接入 OperationAgent 和 DispatchAgent，使 Agent 具备 AI 推理能力；然后进行全局验证并更新 README。

## Current State Analysis

### 已完成
- **Part A**: IEEE 14-bus 精度验证测试已通过（eneros-powerflow, eneros-network）
- **Part B**: LlmReasoningEngine 已实现（eneros-reasoning），含 OpenAI 兼容 API 调用、降级回退、环境变量配置

### 关键发现
1. **AgentContext.reasoning 字段已存在**：`Arc<dyn ReasoningEngine>` 是一等公民，所有 Agent 都能通过 `ctx.reasoning` 访问
2. **Agent 未使用推理引擎**：OperationAgent 和 DispatchAgent 都没有调用 `ctx.reasoning.reason()`，推理引擎"已注入但未使用"
3. **ActionMapper 已就绪**：`map_reasoning_output(&ReasoningOutput) → Vec<AgentAction>` 转换管道已存在
4. **main.rs 使用 RuleBasedEngine**：`eneros_reasoning::RuleBasedEngine::new()` 作为默认推理引擎

### 集成策略
- **不改变 Agent 构造函数签名**：推理引擎通过 AgentContext 注入，Agent 无需知道是 LLM 还是 RuleBased
- **渐进增强**：先尝试 LLM 推理，失败时回退到硬编码逻辑（而非依赖 LlmReasoningEngine 的 fallback，因为 Agent 内部逻辑更可靠）
- **最小改动原则**：只在关键决策点添加 LLM 推理调用，不重构 Agent 整体架构

## Proposed Changes

### 1. 修改 OperationAgent — 故障诊断增强
**文件**: `crates/eneros-agent/src/agents/operation_agent.rs`

**改动**: 在 `handle_event()` 中，当收到 ConstraintViolation/SystemAlarm 时：
1. 先用硬编码 `diagnose()` 做快速模式匹配
2. 如果诊断结果置信度低（< 0.5）或无匹配，调用 `ctx.reasoning.reason()` 做 LLM 推理
3. 将 ReasoningOutput 通过 `ActionMapper::map_reasoning_output()` 转换为 AgentAction
4. 合并硬编码诊断和 LLM 推理结果

**具体代码变更**:
- `handle_event()` 中添加 LLM 推理分支：构建 `ReasoningInput`（goal="故障诊断", observations=症状列表, constraints=安全约束），调用 `ctx.reasoning.reason()`
- 新增 `diagnose_with_reasoning()` 私有方法封装 LLM 推理逻辑
- 保持 `diagnose()` 硬编码方法不变（作为快速路径和 fallback）

### 2. 修改 DispatchAgent — 调度决策增强
**文件**: `crates/eneros-agent/src/agents/dispatch_agent.rs`

**改动**: 在 `handle_event()` 的 ConstraintViolation 分支和 `handle_emergency()` 中：
1. 执行数学优化（economic_dispatch）获得调度方案
2. 调用 `ctx.reasoning.reason()` 对调度方案做合理性审查
3. 如果 LLM 建议不同策略，记录为 LogMessage 供人工审核（不自动覆盖数学优化结果）
4. 紧急场景下，LLM 可辅助判断优先级

**具体代码变更**:
- `handle_event()` ConstraintViolation 分支末尾添加 LLM 审查调用
- `handle_emergency()` 末尾添加 LLM 辅助判断
- 新增 `review_dispatch_with_reasoning()` 私有方法

### 3. 修改 main.rs — 支持 LLM 推理引擎
**文件**: `crates/eneros-api/src/main.rs`

**改动**: 根据环境变量决定使用 RuleBasedEngine 还是 LlmReasoningEngine：
- 如果设置了 `ENEROS_LLM_PROVIDER`，创建 `LlmReasoningEngine` 并以 `RuleBasedEngine` 作为 fallback
- 否则使用 `RuleBasedEngine`（保持现有行为）

### 4. 新增 Agent LLM 集成测试
**文件**: `crates/eneros-agent/tests/e2e_domain.rs`（如已存在则追加，否则新建）

**测试用例**:
- `test_operation_agent_reasoning_diagnosis` — OperationAgent 使用推理引擎进行故障诊断
- `test_dispatch_agent_reasoning_review` — DispatchAgent 使用推理引擎审查调度方案
- `test_agent_reasoning_fallback` — 推理引擎失败时回退到硬编码逻辑

### 5. 全局验证
- `cargo test --workspace` — 全部通过
- `cargo clippy --workspace` — 零警告
- 更新 README.md 路线图，标记 Phase 10 完成

## Assumptions & Decisions

1. **LLM 推理是增强而非替代**：数学优化（economic_dispatch）和硬编码模式匹配（diagnose）始终执行，LLM 作为补充
2. **LLM 建议不自动执行**：DispatchAgent 中 LLM 的调度建议仅记录为 LogMessage，不覆盖数学优化结果（安全性优先）
3. **OperationAgent 中 LLM 可产生动作**：故障诊断场景下 LLM 的建议可通过 ActionMapper 转换为 AgentAction（因为硬编码模式有限，LLM 可覆盖更多故障类型）
4. **测试使用 RuleBasedEngine**：E2E 测试不需要真实 LLM，用 RuleBasedEngine 验证管道连通性
5. **不引入 mock HTTP server**：避免增加测试复杂度，用 RuleBasedEngine 替代 LLM 进行测试

## Verification Steps

1. `cargo test -p eneros-agent` — Agent LLM 集成测试通过
2. `cargo test --workspace` — 全部通过
3. `cargo clippy --workspace` — 零警告
4. README.md Phase 10 标记完成
