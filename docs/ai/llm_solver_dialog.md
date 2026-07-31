# LLM↔Solver 迭代对话式协商

> 版本: v0.189.0 | Crate: `eneros-dialog` (`crates/ai/dialog`)

## 概述

本模块实现 LLM 与数学求解器 (Solver) 之间的多轮迭代对话协商框架。
LLM 负责生成优化意图与方案提案，Solver 负责可行性验证与目标评估，
双方通过多轮对话逐步收敛至可行最优解。

## 协商流程

```
Problem 定义
  → LLM 生成提案 (intent + rationale + confidence)
  → Solver 评估 (feasible / objective / violations)
  → 反馈回 LLM → 调整提案
  → 重复直至收敛或达到 max_rounds
```

## 核心类型

| 类型 | 职责 |
|------|------|
| `DialogState` | 对话状态 (Running / Converged / Failed / Timeout) |
| `LlmProposal` | LLM 提案 (intent, rationale, confidence) |
| `SolverFeedback` | Solver 反馈 (feasible, objective, violations) |
| `Round` | 单轮对话记录 |
| `Problem` | 问题定义 (description, constraints, max_rounds) |
| `Solution` | 最终解 (plan, objective, rounds_used) |

## 收敛策略

- **FeasibilityChecker**: 最后一轮 Solver 报告 feasible → 收敛
- **ObjectiveChecker**: 连续两轮目标函数改善 < epsilon → 收敛

## 使用示例

```rust
use eneros_dialog::{MockNegotiator, DialogNegotiator, Problem};

let mut negotiator = MockNegotiator::new(3);
let problem = Problem {
    description: "economic dispatch".into(),
    constraints: vec!["power balance".into()],
    max_rounds: 10,
};
let solution = negotiator.run(&problem).unwrap();
assert_eq!(solution.rounds_used, 3);
```

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖，零 unsafe
- f32/f64 用于分数，u64 用于 ID/时间戳
