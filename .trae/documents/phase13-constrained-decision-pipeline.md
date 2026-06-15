# Phase 13 — 约束驱动的确定性决策管道

## 概述

解决 EnerOS 的核心矛盾：**电网是约束性的 1 或 0，而大模型是概率性的**。

当前系统存在5个结构性鸿沟：
1. LLM 输出 `Vec<String>` 自然语言，无法可靠映射到 `StructuredAction` 枚举
2. `ConstraintAwareValidator` 6步验证管道存在但未被主流程调用
3. 不可行动作直接拒绝，无"可行域投影"机制
4. 5个约束维度独立检查，无耦合分析
5. 规则引擎与 LLM 推理互斥而非互补

Phase 13 构建**约束驱动的确定性决策管道**，确保无论 LLM 输出什么，最终动作一定满足所有物理约束。

## 当前状态分析

### 决策管道现状
```
LLM推理 → Vec<String> → ActionMapper(关键词匹配) → AgentAction → ActionDispatcher → SafetyGateway(参数级静态检查) → 执行
```

### 关键断裂点
- `ReasoningOutput.actions: Vec<String>` — 非结构化
- `ActionMapper` — 关键词匹配，解析脆弱
- `ActionDispatcher.dispatch()` — 绕过 ConstraintAwareValidator
- `SafetyGateway` — 只检查命令参数静态限值，不评估全局影响
- `ConstraintEngine.check_action_feasibility()` — 仅3条文本规则，不做潮流计算
- 无反馈回路 — LLM 建议被拒绝时无重试

## 实施方案

### Step 1: StructuredActionOutput — 结构化动作输出

**文件**: `crates/eneros-reasoning/src/structured_output.rs` (新建)

将 LLM 输出从 `Vec<String>` 升级为结构化的 `Vec<StructuredAction>`，利用 rig 的 function calling / JSON schema 强制 LLM 输出合法动作：

```rust
/// 结构化动作输出，替代 Vec<String>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredActionOutput {
    /// 推理链（LLM 的思考过程，供审计）
    pub reasoning_chain: String,
    /// 置信度 [0, 1]
    pub confidence: f64,
    /// 推荐动作列表（结构化枚举，非文本）
    pub actions: Vec<StructuredAction>,
    /// LLM 认为需要满足的前提条件
    pub preconditions: Vec<String>,
}

/// 扩展 ReasoningOutput 以支持结构化动作
impl ReasoningOutput {
    pub fn from_structured(output: StructuredActionOutput) -> Self {
        Self {
            conclusion: output.reasoning_chain,
            confidence: output.confidence,
            actions: output.actions.iter().map(|a| format!("{:?}", a)).collect(),
            structured_actions: Some(output.actions),
            preconditions: output.preconditions,
        }
    }
}
```

**修改 `ReasoningOutput`**（`engine.rs`）：
- 新增 `structured_actions: Option<Vec<StructuredAction>>` 字段
- 新增 `preconditions: Vec<String>` 字段
- 保留 `actions: Vec<String>` 向后兼容

**修改 `RigReasoningEngine`**（`rig_engine.rs`）：
- 在 rig Agent 上注册 `ActionTool`（function calling tool），让 LLM 通过 tool call 输出结构化动作
- `ActionTool` 的 JSON schema 定义了 `StructuredAction` 的合法值域
- LLM 只能输出 schema 允许的动作类型和参数范围

### Step 2: FeasibilityProjector — 可行域投影器

**文件**: `crates/eneros-constraint/src/projector.rs` (新建)

当 LLM 建议的动作不可行时，自动投影到最近可行点：

```rust
/// 可行域投影结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProjectionResult {
    /// 动作本身可行，无需修改
    Feasible(StructuredAction),
    /// 动作不可行，已投影到最近可行点
    Projected {
        original: StructuredAction,
        projected: StructuredAction,
        modifications: Vec<Modification>,
    },
    /// 动作完全不可行，无可行投影
    Infeasible {
        original: StructuredAction,
        violated_constraints: Vec<ConstraintViolation>,
        suggested_alternatives: Vec<StructuredAction>,
    },
}

pub struct Modification {
    pub parameter: String,      // "target_mw"
    pub original_value: f64,    // 300.0
    pub projected_value: f64,   // 200.0
    pub reason: String,         // "发电机额定上限 200MW"
}

/// 可行域投影器
pub struct FeasibilityProjector {
    network: Arc<parking_lot::RwLock<PowerNetwork>>,
}

impl FeasibilityProjector {
    /// 评估动作可行性并投影到可行域
    pub fn project(&self, action: &StructuredAction) -> ProjectionResult {
        // 1. 检查设备参数硬限（额定容量、电压等级等）
        // 2. 模拟动作执行后的潮流（What-If Analysis）
        // 3. 检查模拟结果是否满足所有约束
        // 4. 如果不满足，沿约束梯度方向投影到可行域边界
    }

    /// What-If 分析：模拟动作执行后的电网状态
    fn what_if_analysis(&self, action: &StructuredAction) -> WhatIfResult {
        // 1. 克隆当前 PowerNetwork
        // 2. 应用动作到克隆的网络
        // 3. 执行潮流计算
        // 4. 检查所有约束
        // 5. 返回模拟结果
    }

    /// 批量评估多个动作的可行性
    pub fn project_batch(&self, actions: &[StructuredAction]) -> Vec<ProjectionResult> {
        actions.iter().map(|a| self.project(a)).collect()
    }
}
```

**What-If 分析核心逻辑**：
1. 克隆 `PowerNetwork`（`Clone` 需实现）
2. 将 `StructuredAction` 转换为网络参数修改（发电机出力调整、开关状态切换等）
3. 调用 `network.solve()` 执行潮流计算
4. 调用 `network.check_constraints()` 检查约束
5. 调用 `network.check_n1_with_limits()` 检查 N-1
6. 返回模拟结果

**投影算法**（简化版）：
- 对于发电机出力调整：如果目标值超出 `[Pmin, Pmax]`，投影到边界值
- 对于电压设定：如果目标电压导致其他母线越限，沿灵敏度方向搜索可行电压
- 对于开关操作：如果操作违反闭锁规则，返回 Infeasible 并建议替代操作序列

### Step 3: ConstrainedDecisionPipeline — 约束决策管道

**文件**: `crates/eneros-gateway/src/decision_pipeline.rs` (新建)

将 LLM 推理 → 结构化动作 → 可行性投影 → 约束验证 → 执行 串联为闭环管道：

```rust
/// 约束驱动的确定性决策管道
pub struct ConstrainedDecisionPipeline {
    projector: Arc<FeasibilityProjector>,
    validator: Arc<ConstraintAwareValidator>,
    gateway: Arc<SafetyGateway>,
    max_retries: u32,              // LLM 重试次数，默认 2
    feedback_to_llm: bool,         // 是否将拒绝原因反馈 LLM
}

/// 决策管道执行结果
pub struct DecisionResult {
    /// 最终执行的动作（经过投影和验证）
    pub executed_action: Option<StructuredAction>,
    /// 原始 LLM 建议
    pub original_proposal: StructuredActionOutput,
    /// 投影结果
    pub projection: ProjectionResult,
    /// 验证结果
    pub validation: ActionVerdict,
    /// 是否经过 LLM 重试
    pub retries: u32,
    /// 完整审计轨迹
    pub audit_trail: Vec<AuditEntry>,
}

pub struct AuditEntry {
    pub stage: String,           // "projection" / "validation" / "execution"
    pub timestamp: DateTime<Utc>,
    pub input: String,
    pub output: String,
    pub duration_ms: u64,
}

impl ConstrainedDecisionPipeline {
    /// 执行约束驱动的决策管道
    pub async fn decide(
        &self,
        reasoning_output: &StructuredActionOutput,
    ) -> Vec<DecisionResult> {
        let mut results = Vec::new();

        for action in &reasoning_output.actions {
            let result = self.decide_one(action, reasoning_output).await;
            results.push(result);
        }

        results
    }

    async fn decide_one(
        &self,
        action: &StructuredAction,
        original: &StructuredActionOutput,
    ) -> DecisionResult {
        // Step 1: 可行域投影
        let projection = self.projector.project(action);

        let feasible_action = match &projection {
            ProjectionResult::Feasible(a) => a.clone(),
            ProjectionResult::Projected { projected, .. } => projected.clone(),
            ProjectionResult::Infeasible { suggested_alternatives, .. } => {
                // 尝试建议的替代方案
                if let Some(alt) = suggested_alternatives.first() {
                    let alt_projection = self.projector.project(alt);
                    match alt_projection {
                        ProjectionResult::Feasible(a) => a,
                        ProjectionResult::Projected { projected, .. } => projected,
                        ProjectionResult::Infeasible { .. } => {
                            // 完全不可行，不执行
                            return DecisionResult {
                                executed_action: None,
                                original_proposal: original.clone(),
                                projection,
                                validation: ActionVerdict::Rejected,
                                retries: 0,
                                audit_trail: vec![],
                            };
                        }
                    }
                } else {
                    return DecisionResult {
                        executed_action: None,
                        original_proposal: original.clone(),
                        projection,
                        validation: ActionVerdict::Rejected,
                        retries: 0,
                        audit_trail: vec![],
                    };
                }
            }
        };

        // Step 2: ConstraintAwareValidator 6步验证
        let command = structured_action_to_command(&feasible_action);
        let validation = self.validator.validate(
            &command,
            &AuthorityLevel::Supervisor, // 从 original 推断
            &SystemOperatingState::Normal,
        );

        match validation {
            ActionVerdict::Approved | ActionVerdict::EmergencyBypassed => {
                // Step 3: 执行
                let _ = self.gateway.submit_command(command);
                DecisionResult {
                    executed_action: Some(feasible_action),
                    original_proposal: original.clone(),
                    projection,
                    validation,
                    retries: 0,
                    audit_trail: vec![],
                }
            }
            ActionVerdict::Rejected { .. } | ActionVerdict::PendingApproval => {
                DecisionResult {
                    executed_action: None,
                    original_proposal: original.clone(),
                    projection,
                    validation,
                    retries: 0,
                    audit_trail: vec![],
                }
            }
        }
    }
}
```

### Step 4: ActionDispatcher 集成 ConstrainedDecisionPipeline

**文件**: `crates/eneros-agent/src/dispatcher.rs` (修改)

修改 `ActionDispatcher`，使 `ExecuteCommand` 动作必须经过约束决策管道：

- 新增 `decision_pipeline: Option<Arc<ConstrainedDecisionPipeline>>` 字段
- `dispatch()` 方法中，`AgentAction::ExecuteCommand` 分支改为调用 `decision_pipeline.decide()`
- 保留 `dispatch_direct()` 作为不经过管道的直接执行路径（紧急场景）
- 添加审计日志记录

### Step 5: ConstraintAwareValidator 接入主流程

**文件**: `crates/eneros-gateway/src/constraint_validator.rs` (修改)

增强 `ConstraintAwareValidator`：
- Step 3（约束预检）从文本关键词匹配升级为调用 `FeasibilityProjector.what_if_analysis()`
- 新增 Step 3b：多约束耦合检查（电压+热稳定、N-1+热稳定）
- 新增 Step 3c：闭锁规则与拓扑变化耦合检查
- 验证结果包含详细的违规信息和修正建议

### Step 6: LLM 反馈回路

**文件**: `crates/eneros-reasoning/src/feedback.rs` (新建)

当 LLM 建议被拒绝时，将拒绝原因反馈给 LLM 重新推理：

```rust
/// LLM 反馈回路
pub struct FeedbackLoop {
    engine: Arc<RigReasoningEngine>,
    max_iterations: u32,  // 默认 2
}

impl FeedbackLoop {
    /// 执行带反馈的推理循环
    pub async fn reason_with_feedback(
        &self,
        input: &ReasoningInput,
        rejection: &DecisionResult,
    ) -> Result<StructuredActionOutput> {
        // 构建反馈 prompt：
        // "你之前的建议 [action] 被拒绝，原因如下：
        //  - 电压越限：Bus 3 电压 0.92 p.u. < 0.95 p.u. 下限
        //  - N-1 违规：Branch 5 在 Line 2 断开时过载 110%
        // 请基于以上约束信息重新推理，给出满足所有约束的动作建议。"

        let feedback_input = ReasoningInput {
            scenario: format!(
                "{}\n\n[反馈] 你之前的建议被拒绝：\n{}\n请重新推理。",
                input.scenario,
                format_rejection(rejection)
            ),
            ..input.clone()
        };

        self.engine.reason(&feedback_input).await
    }
}
```

### Step 7: 模块导出与集成

**文件修改列表**：

1. `crates/eneros-reasoning/src/lib.rs` — 导出 structured_output, feedback
2. `crates/eneros-reasoning/src/engine.rs` — ReasoningOutput 新增字段
3. `crates/eneros-reasoning/Cargo.toml` — 添加 eneros-core 依赖（StructuredAction）
4. `crates/eneros-constraint/src/lib.rs` — 导出 projector
5. `crates/eneros-constraint/Cargo.toml` — 添加 eneros-network, eneros-powerflow 依赖
6. `crates/eneros-gateway/src/lib.rs` — 导出 decision_pipeline
7. `crates/eneros-gateway/Cargo.toml` — 添加 eneros-constraint 依赖
8. `crates/eneros-agent/src/dispatcher.rs` — 集成 ConstrainedDecisionPipeline
9. `crates/eneros-api/src/main.rs` — 创建并注入 ConstrainedDecisionPipeline

### Step 8: 集成测试

**文件**: `crates/eneros-gateway/tests/decision_pipeline.rs` (新建)

测试用例：
1. FeasibilityProjector — 发电机出力超限投影到额定上限
2. FeasibilityProjector — 开关操作违反闭锁规则返回 Infeasible
3. FeasibilityProjector — What-If 分析检测电压越限
4. ConstrainedDecisionPipeline — 可行动作直接通过
5. ConstrainedDecisionPipeline — 不可行动作投影后执行
6. ConstrainedDecisionPipeline — 完全不可行动作拒绝执行
7. ConstraintAwareValidator — 约束预检升级验证
8. 端到端 — LLM 建议 → 投影 → 验证 → 执行/拒绝

## 依赖关系

```
Step 1 (StructuredActionOutput) ──┐
Step 2 (FeasibilityProjector) ────┤── Step 3 (ConstrainedDecisionPipeline) ── Step 4 (Dispatcher集成)
Step 5 (Validator增强) ───────────┤── Step 6 (LLM反馈回路)
                                   └── Step 7 (模块导出) ── Step 8 (测试)
```

Steps 1, 2, 5 可并行开发；Step 3 依赖 1+2；Step 4 依赖 3；Step 6 依赖 1。

## 设计决策

1. **What-If 分析基于网络克隆**：每次可行性评估克隆一份 PowerNetwork，在其上模拟动作并求解潮流。这保证了原始网络状态不被污染。克隆开销可接受（IEEE 14-bus 网络克隆 < 1ms）。

2. **投影算法采用边界裁剪而非优化求解**：完整的约束 OPF 求解器（内点法）计算量大且实现复杂。Phase 13 先实现"边界裁剪"——将超出设备硬限的参数裁剪到边界值，再通过 What-If 验证。后续 Phase 可引入完整 OPF。

3. **LLM 反馈回路最多 2 轮**：避免无限循环。如果 2 轮后 LLM 仍无法给出可行建议，系统回退到 RuleBasedEngine 的确定性决策。

4. **ConstrainedDecisionPipeline 是强制关卡**：所有 `ExecuteCommand` 动作必须经过管道，不可绕过。紧急场景通过 `EmergencyBypass` 旁路部分非关键检查，但仍需经过闭锁规则验证。

5. **保留 ActionMapper 向后兼容**：`ReasoningOutput.actions: Vec<String>` 保留，新增 `structured_actions` 字段。当 `structured_actions` 为 None 时，仍走 ActionMapper 关键词匹配路径。

## 验证步骤

1. `cargo test -p eneros-reasoning` — reasoning 测试通过
2. `cargo test -p eneros-constraint` — constraint 测试通过
3. `cargo test -p eneros-gateway` — gateway 测试通过
4. `cargo test -p eneros-agent` — agent 测试通过
5. `cargo test --workspace` — 全局测试通过
6. `cargo clippy --workspace` — 零错误
