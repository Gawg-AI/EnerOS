# Phase 10 — 精度验证与 LLM 推理集成

## Summary
两大目标：1) 验证潮流计算结果与 IEEE 标准答案一致，确保计算基座可信；2) 集成 LLM 推理引擎，让 Agent 具备 AI 推理能力。

## Current State Analysis

### 潮流计算精度
- `eneros-powerflow/src/solver.rs` 的 `test_ieee14_convergence()` 仅验证电压在 0.9~1.2 pu 范围内
- 未与 IEEE 14-bus 标准答案（Bus 1: 1.060∠0°, Bus 2: 1.045∠-4.98° 等）对比
- IEEE 14 数据在 `eneros-powerflow/src/ieee.rs`，包含 `Ieee14BusData` 结构体和标准答案（`v_pu`, `angle_deg`）
- `PowerNetwork::from_ieee14()` 已有完整的 IEEE 14 数据加载

### LLM 推理
- `eneros-reasoning` 仅有 `RuleBasedEngine`（关键词匹配 + 数值阈值规则）
- `ReasoningEngine` trait 已定义：`async fn reason(input: ReasoningInput) -> Result<ReasoningOutput>`
- `ReasoningInput` 包含 goal, observations, constraints, memory_entries, available_tools, power_observation
- `ReasoningOutput` 包含 conclusion, confidence, actions, reasoning_chain
- Cargo.toml 依赖：eneros-core, eneros-tool, eneros-memory, serde, async-trait, tokio

## Proposed Changes

### Part A: IEEE 标准答案精度验证

#### 文件: `crates/eneros-powerflow/src/solver.rs`
- 修改 `test_ieee14_convergence()` — 逐母线对比电压幅值（容差 1e-3 pu）和角度（容差 0.05°）
- 新增 `test_ieee14_voltage_accuracy()` — 严格对比所有 14 个母线的 V 和 θ
- 新增 `test_ieee14_branch_flow_accuracy()` — 对比支路功率与标准答案
- 新增 `test_ieee14_total_losses()` — 验证总损耗接近标准值（~13.8 MW）

#### 文件: `crates/eneros-powerflow/src/ieee.rs`
- 确认 `Ieee14Bus.angle_deg` 字段包含标准答案角度值
- 如缺少标准答案数据，补充 IEEE 14-bus 标准潮流结果

#### 文件: `crates/eneros-network/src/network.rs`
- 新增 `test_from_ieee14_accuracy()` — 通过 PowerNetwork 验证端到端精度

### Part B: LLM 推理引擎集成

#### 文件: `crates/eneros-reasoning/Cargo.toml`
- 新增 `reqwest = { workspace = true, features = ["json"] }` — HTTP 客户端调用 LLM API
- 新增 `serde_json` 已有

#### 新建文件: `crates/eneros-reasoning/src/llm_engine.rs`
- `LlmConfig` 结构体：api_url (String), model (String), api_key (Option<String>), max_tokens (u32, default 1024), temperature (f64, default 0.7)
- `LlmProvider` 枚举：OpenAI, Ollama, Custom(String)
- `LlmReasoningEngine` 结构体：config (LlmConfig), client (reqwest::Client)
- 实现 `ReasoningEngine` trait：
  - `name()` → "llm-reasoning"
  - `reason()` → 构建 prompt（包含 goal, observations, constraints, power_observation 摘要），调用 LLM API，解析响应为 ReasoningOutput
- Prompt 模板：将 PowerObservation 转换为结构化文本描述，包含电压/频率/潮流状态
- 响应解析：从 LLM 输出提取 conclusion, confidence, actions, reasoning_chain
- 降级策略：LLM 调用失败时回退到 RuleBasedEngine

#### 新建文件: `crates/eneros-reasoning/src/llm_prompt.rs`
- `build_power_system_prompt(input: &ReasoningInput) -> String` — 构建电力系统专用 prompt
- `parse_llm_response(response: &str) -> Result<ReasoningOutput>` — 解析 LLM 响应
- 包含电力系统知识上下文（安全约束、运行规程要点）

#### 文件: `crates/eneros-reasoning/src/lib.rs`
- 新增 `pub mod llm_engine;`
- 新增 `pub mod llm_prompt;`
- 导出 `LlmReasoningEngine`, `LlmConfig`, `LlmProvider`

#### 文件: `crates/eneros-agent/src/agents/` — Agent 使用 LLM 推理
- 修改 `OperationAgent` — 故障诊断时可选使用 LlmReasoningEngine
- 修改 `DispatchAgent` — 紧急调度决策时可选使用 LLM 推理
- 通过 `AgentContext` 注入 ReasoningEngine（可选，None 时用规则引擎）

### Part C: 集成验证

#### 文件: `crates/eneros-agent/tests/e2e_domain.rs`
- 新增测试：LLM 推理引擎创建和配置
- 新增测试：Agent 使用 LLM 推理进行故障诊断（mock LLM 响应）

## Assumptions & Decisions

1. **LLM API 调用方式**：使用 OpenAI 兼容 API（/v1/chat/completions），支持 OpenAI、Ollama、vLLM 等后端
2. **API Key 安全**：通过环境变量 `ENEROS_LLM_API_KEY` 传入，不硬编码
3. **降级策略**：LLM 不可用时自动回退到 RuleBasedEngine，不阻塞 Agent 运行
4. **精度容差**：电压幅值 1e-3 pu，角度 0.05°，这是电力系统潮流计算的常规精度要求
5. **不引入重量级依赖**：不使用 candle/tract 等本地推理框架，仅通过 HTTP API 调用 LLM

## Verification Steps

1. `cargo test -p eneros-powerflow` — IEEE 14 精度测试通过
2. `cargo test -p eneros-reasoning` — LLM 引擎测试通过（含 mock 测试）
3. `cargo test -p eneros-agent` — Agent LLM 集成测试通过
4. `cargo test --workspace` — 全部通过
5. `cargo clippy --workspace` — 零警告
