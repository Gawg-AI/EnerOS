# Phase 2 — Agent 运行时 开发计划

## 概述

Phase 1 内核基座已完成（91 个测试通过），建立了拓扑、潮流、约束、设备的物理世界模型。Phase 2 将在此基础上构建 Agent 运行时层，使 AI Agent 能够运行在电力内核之上，具备生命周期管理、记忆、工具调用和推理能力。

## 当前状态

### Phase 1 已完成
- eneros-core: BranchParams、TopologyChange 补全
- eneros-topology: 环检测、TopologyEngine 测试
- eneros-equipment: 6 种设备 to_admittance + MultiAdmittanceContribution
- eneros-powerflow: 3 个 P0 Bug 修复 + IEEE 14 数据修复 + 变比支持
- eneros-constraint: N-1 开断分析 + 电压稳定性检查
- eneros-network: PowerNetwork 统一管线 + pipeline 全流程分析

### Phase 2 需要新建的 crate
- `eneros-agent` — Agent 生命周期管理（P0）
- `eneros-memory` — Agent 记忆系统（P0）
- `eneros-tool` — Agent 工具引擎（P1）
- `eneros-reasoning` — 推理引擎集成（P1）

### 现有基础设施依赖
- `eneros-eventbus`: Agent 通过事件总线接收电网事件，`EventHandler` trait 可注册 Agent 为消费者
- `eneros-gateway/safety`: Agent 控制指令必须经过 SafetyCheck 校验
- `eneros-network/pipeline`: Agent 调用 PowerNetwork 执行电网仿真
- `eneros-timeseries`: Agent 查询历史时序数据辅助决策
- `eneros-core`: ElementId、Result、EnerOSError 等基础类型

---

## 执行计划

### Step 1: eneros-agent — Agent 生命周期管理

**新建 crate**: `crates/eneros-agent/`

**Cargo.toml 依赖**: eneros-core, eneros-eventbus, eneros-network, eneros-gateway, tokio, serde, tracing, async-trait, uuid, chrono, thiserror

**核心设计**:

```
src/
├── lib.rs          # 模块入口 + 重新导出
├── agent.rs        # Agent trait + AgentContext + AgentRuntime
├── lifecycle.rs    # Agent 生命周期状态机
└── registry.rs     # Agent 注册表
```

**agent.rs — 核心类型**:
- `Agent` trait: Agent 统一接口
  ```rust
  #[async_trait]
  pub trait Agent: Send + Sync {
      fn id(&self) -> &str;
      fn name(&self) -> &str;
      fn agent_type(&self) -> AgentType;
      async fn start(&mut self, ctx: &AgentContext) -> Result<()>;
      async fn stop(&mut self) -> Result<()>;
      async fn handle_event(&mut self, event: &Event) -> Result<Vec<AgentAction>>;
      async fn tick(&mut self, ctx: &AgentContext) -> Result<Vec<AgentAction>>;
  }
  ```
- `AgentType` 枚举: `Dispatcher`(调度), `Operator`(运维), `Planner`(规划), `Trader`(交易), `Custom(String)`
- `AgentAction` 枚举: Agent 的动作输出
  ```rust
  pub enum AgentAction {
      PublishEvent(Event),           // 发布事件
      ExecuteCommand(Command),       // 发送控制指令（经 SafetyGateway）
      QueryNetwork(NetworkQuery),    // 查询电网状态
      QueryTimeSeries(TimeQuery),    // 查询时序数据
      LogMessage(String),            // 记录日志
      NoOp,                          // 空操作
  }
  ```
- `AgentContext`: Agent 运行上下文
  ```rust
  pub struct AgentContext {
      network: PowerNetwork,         // 电网模型
      event_bus: EventBus,           // 事件总线
      memory: AgentMemory,           // 记忆系统
      tool_engine: ToolEngine,       // 工具引擎
  }
  ```

**lifecycle.rs — 生命周期状态机**:
- `AgentState` 枚举: `Created` → `Initializing` → `Running` → `Paused` → `Stopping` → `Stopped` → `Failed(String)`
- `AgentLifecycle`: 管理状态转换，确保合法转换
- 转换规则: Created→Initializing→Running, Running↔Paused, Running→Stopping→Stopped, 任意→Failed

**registry.rs — Agent 注册表**:
- `AgentRegistry`: 管理所有 Agent 实例
  - `register(agent: Box<dyn Agent>)` — 注册 Agent
  - `unregister(id: &str)` — 注销 Agent
  - `get(id: &str)` — 获取 Agent
  - `list()` — 列出所有 Agent
  - `list_by_type(AgentType)` — 按类型过滤

**测试**:
- `test_agent_lifecycle_transitions` — 状态转换合法性
- `test_agent_lifecycle_invalid_transition` — 非法转换被拒绝
- `test_agent_registry_register` — 注册/注销
- `test_agent_registry_list_by_type` — 按类型查询
- `test_mock_agent_tick` — MockAgent 的 tick 行为

---

### Step 2: eneros-memory — Agent 记忆系统

**新建 crate**: `crates/eneros-memory/`

**Cargo.toml 依赖**: eneros-core, serde, serde_json, chrono, tokio, tracing, thiserror, parking_lot

**核心设计**:

```
src/
├── lib.rs          # 模块入口 + 重新导出
├── memory.rs       # Memory trait + InMemoryMemory
├── types.rs        # MemoryEntry, MemoryType, RecallQuery
└── store.rs        # 记忆存储引擎（短期/长期分离）
```

**memory.rs — 核心接口**:
- `AgentMemory` trait: 记忆系统统一接口
  ```rust
  #[async_trait]
  pub trait AgentMemory: Send + Sync {
      async fn store(&self, agent_id: &str, entry: MemoryEntry) -> Result<()>;
      async fn recall(&self, agent_id: &str, query: &RecallQuery) -> Result<Vec<MemoryEntry>>;
      async fn forget(&self, agent_id: &str, entry_id: &str) -> Result<()>;
      async fn clear(&self, agent_id: &str) -> Result<()>;
      async fn count(&self, agent_id: &str) -> usize;
  }
  ```
- `InMemoryMemory`: 基于内存的实现，使用 `HashMap<String, Vec<MemoryEntry>>` + `RwLock`

**types.rs — 记忆类型**:
- `MemoryType` 枚举:
  - `Episodic` — 事件记忆（如"14:00 母线3电压越限"）
  - `Semantic` — 知识记忆（如"母线3是 PV 母线"）
  - `Procedural` — 过程记忆（如"N-1 越限时先调压后切负荷"）
- `MemoryEntry`:
  ```rust
  pub struct MemoryEntry {
      pub id: String,
      pub memory_type: MemoryType,
      pub content: String,              // JSON 序列化的内容
      pub importance: f64,              // 0.0~1.0 重要性权重
      pub timestamp: DateTime<Utc>,
      pub tags: Vec<String>,            // 标签（如 "voltage", "bus3"）
      pub access_count: u32,            // 访问次数
  }
  ```
- `RecallQuery`:
  ```rust
  pub struct RecallQuery {
      pub memory_type: Option<MemoryType>,
      pub tags: Vec<String>,            // 按标签过滤
      pub keyword: Option<String>,      // 关键词搜索
      pub min_importance: Option<f64>,  // 最低重要性
      pub limit: usize,                 // 返回数量上限
      pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
  }
  ```

**store.rs — 存储引擎**:
- `MemoryStore`: 短期记忆 + 长期记忆分离
  - 短期记忆: 最近 N 条，自动淘汰（FIFO）
  - 长期记忆: importance >= 阈值 的记忆持久保留
  - `consolidate()`: 将高 importance 短期记忆提升为长期记忆

**测试**:
- `test_memory_store_and_recall` — 存取基本功能
- `test_memory_recall_by_type` — 按类型过滤
- `test_memory_recall_by_tags` — 按标签过滤
- `test_memory_recall_by_importance` — 按重要性过滤
- `test_memory_forget` — 删除记忆
- `test_memory_consolidation` — 短期→长期提升
- `test_memory_clear` — 清空记忆

---

### Step 3: eneros-tool — Agent 工具引擎

**新建 crate**: `crates/eneros-tool/`

**Cargo.toml 依赖**: eneros-core, eneros-network, eneros-powerflow, eneros-constraint, eneros-eventbus, serde, serde_json, async-trait, thiserror, tracing

**核心设计**:

```
src/
├── lib.rs          # 模块入口 + 重新导出
├── tool.rs         # Tool trait + ToolEngine
├── builtin.rs      # 内置工具实现
└── registry.rs     # 工具注册表
```

**tool.rs — 核心接口**:
- `Tool` trait: 工具统一接口
  ```rust
  #[async_trait]
  pub trait Tool: Send + Sync {
      fn name(&self) -> &str;
      fn description(&self) -> &str;
      fn parameters_schema(&self) -> serde_json::Value;  // JSON Schema
      async fn execute(&self, params: serde_json::Value) -> Result<ToolOutput>;
  }
  ```
- `ToolOutput`:
  ```rust
  pub struct ToolOutput {
      pub success: bool,
      pub data: serde_json::Value,
      pub message: String,
  }
  ```
- `ToolEngine`: 工具执行引擎
  - `register(tool: Box<dyn Tool>)` — 注册工具
  - `execute(name: &str, params: Value) -> Result<ToolOutput>` — 执行工具
  - `list_tools() -> Vec<ToolInfo>` — 列出可用工具

**builtin.rs — 内置工具**:
1. `PowerFlowTool` — 执行潮流计算
   - 参数: `{ network_config: ..., p_spec: ..., q_spec: ... }`
   - 输出: PowerFlowResult 的 JSON
2. `N1AnalysisTool` — N-1 安全校验
   - 参数: `{ network_config: ... }`
   - 输出: N-1 结果摘要
3. `ConstraintCheckTool` — 约束校验
   - 参数: `{ bus_voltages: ..., branch_loadings: ..., frequency: ... }`
   - 输出: 违规列表
4. `TopologyQueryTool` — 拓扑查询
   - 参数: `{ query_type: "path"|"neighbors"|"island", ... }`
   - 输出: 拓扑查询结果

**registry.rs — 工具注册表**:
- `ToolRegistry`: 管理工具注册
  - 基于 `HashMap<String, Box<dyn Tool>>`

**测试**:
- `test_tool_engine_register` — 注册/执行
- `test_tool_engine_list` — 列出工具
- `test_tool_engine_unknown_tool` — 未注册工具报错
- `test_powerflow_tool` — 潮流计算工具
- `test_n1_analysis_tool` — N-1 分析工具
- `test_constraint_check_tool` — 约束校验工具

---

### Step 4: eneros-reasoning — 推理引擎集成

**新建 crate**: `crates/eneros-reasoning/`

**Cargo.toml 依赖**: eneros-core, eneros-tool, eneros-memory, serde, serde_json, async-trait, thiserror, tracing, tokio

**核心设计**:

```
src/
├── lib.rs          # 模块入口 + 重新导出
├── engine.rs       # ReasoningEngine trait + 实现
├── strategy.rs     # 推理策略
└── context.rs      # 推理上下文构建
```

**engine.rs — 核心接口**:
- `ReasoningEngine` trait: 推理引擎统一接口
  ```rust
  #[async_trait]
  pub trait ReasoningEngine: Send + Sync {
      fn name(&self) -> &str;
      async fn reason(&self, input: ReasoningInput) -> Result<ReasoningOutput>;
  }
  ```
- `ReasoningInput`:
  ```rust
  pub struct ReasoningInput {
      pub goal: String,                    // 推理目标
      pub observations: Vec<String>,       // 观测事实
      pub constraints: Vec<String>,        // 约束条件
      pub memory_entries: Vec<MemoryEntry>,// 相关记忆
      pub available_tools: Vec<ToolInfo>,  // 可用工具
  }
  ```
- `ReasoningOutput`:
  ```rust
  pub struct ReasoningOutput {
      pub conclusion: String,              // 推理结论
      pub confidence: f64,                 // 置信度 0.0~1.0
      pub actions: Vec<AgentAction>,       // 建议动作
      pub reasoning_chain: Vec<String>,    // 推理链（可解释性）
  }
  ```
- `RuleBasedEngine`: 基于规则的推理引擎（Phase 2 先实现这个）
  - 内置电力领域规则集（如"电压越限→调压→切负荷"）
  - 规则格式: `if condition then action with priority`

**strategy.rs — 推理策略**:
- `ReasoningStrategy` 枚举:
  - `Reactive` — 事件驱动，立即响应
  - `Deliberative` — 深度推理，多步规划
  - `Hybrid` — 混合策略

**context.rs — 推理上下文构建**:
- `ReasoningContextBuilder`: 从电网状态 + 事件 + 记忆构建推理输入
  - `from_event(event: &Event)` — 从事件构建
  - `with_network_state(result: &PowerFlowResult)` — 注入电网状态
  - `with_memory(entries: Vec<MemoryEntry>)` — 注入记忆
  - `build()` — 生成 ReasoningInput

**测试**:
- `test_rule_based_engine_voltage_violation` — 电压越限推理
- `test_rule_based_engine_overload` — 过载推理
- `test_reasoning_context_builder` — 上下文构建
- `test_reasoning_output_confidence` — 置信度计算

---

### Step 5: 集成验证 + DEVGUIDE 更新

1. **更新 workspace Cargo.toml**: 添加 4 个新 crate 到 members
2. **更新 DEVGUIDE.md**:
   - 更新架构认知地图（添加 Layer 6: Agent 运行时层）
   - 更新开发工作树完成度
   - 更新 Phase 路线图（Phase 1 已完成，Phase 2 进行中）
3. **集成测试**: 在 eneros-agent 中创建端到端集成测试
   - Agent 注册 → 接收事件 → 调用工具 → 推理决策 → 输出动作
4. **全 workspace 验证**: `cargo test --workspace` + `cargo clippy --workspace`

---

## 依赖关系图

```
eneros-core (L0)
    ├── eneros-topology (L1)
    ├── eneros-powerflow (L1)
    ├── eneros-equipment (L1)
    ├── eneros-eventbus (L2)
    ├── eneros-constraint (L2)
    ├── eneros-timeseries (L2)
    ├── eneros-gateway (L4)
    ├── eneros-network (L5, 聚合层)
    ├── eneros-memory (L6, 新建) ← 仅依赖 core
    ├── eneros-tool (L6, 新建) ← 依赖 core + network + constraint + eventbus
    ├── eneros-reasoning (L6, 新建) ← 依赖 core + tool + memory
    └── eneros-agent (L6, 新建) ← 依赖 core + eventbus + network + gateway + memory + tool + reasoning
```

## 假设与决策

1. **eneros-agent 是最高层聚合**: Agent 依赖所有其他 crate，是最终集成入口
2. **eneros-memory 不依赖网络层**: 记忆系统是通用的，不绑定电网模型
3. **eneros-tool 封装 PowerNetwork**: 工具引擎将 PowerNetwork 操作封装为 Tool 接口
4. **eneros-reasoning 先实现规则引擎**: Phase 2 不集成 LLM，先用规则引擎验证架构；LLM 集成留给后续 Phase
5. **AgentAction 通过 SafetyGateway**: 所有控制指令必须经过安全校验
6. **记忆系统短期/长期分离**: 模拟人类记忆，短期记忆自动淘汰，高重要性记忆提升为长期

## 验证步骤

1. `cargo test -p eneros-agent` — Agent 生命周期 + 注册表测试
2. `cargo test -p eneros-memory` — 记忆存取 + 过滤 + 整合测试
3. `cargo test -p eneros-tool` — 工具注册 + 内置工具执行测试
4. `cargo test -p eneros-reasoning` — 规则推理 + 上下文构建测试
5. `cargo test --workspace` — 全 workspace 测试通过
6. `cargo clippy --workspace` — 无错误
