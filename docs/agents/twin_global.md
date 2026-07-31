# 全局数字孪生 (twin-global)

> EnerOS v0.192.0 — Phase 4 出口版

## 概述

`eneros-twin-global` 实现多域联合仿真与全局优化，是 Phase 4 的核心出口组件。
通过将储能、光伏、风电、负荷、电网、市场六大领域进行协同建模与联合优化，
实现相较单域优化 **≥ 15%** 的全局性能提升。

## 模块结构

| 模块 | 职责 |
|------|------|
| `types` | 核心类型定义 (DomainKind, DomainTwin, OptimizationResult 等) |
| `multi_domain` | 多域联合仿真引擎，管理 6 域孪生体并执行仿真步进 |
| `optimizer` | 全局优化器，支持 HiGHS/Gurobi/Cbc 求解器 |
| `fusion` | 模型融合，加权平均/贝叶斯/神经代理策略 |
| `benchmark` | 基准对比，验证 Phase 4 出口指标 |

## Phase 4 出口标准

- 全局优化相较单域优化提升 **≥ 15%**
- 通过 `benchmark::meets_exit_criteria()` 量化验证
- 所有测试通过，clippy 无警告

## 使用示例

```rust
use eneros_twin_global::{MultiDomainSimulator, GlobalOptimizer, SolverKind, ObjectiveKind};
use eneros_twin_global::{compare, meets_exit_criteria};

let mut sim = MultiDomainSimulator::new();
sim.simulate_step();

let optimizer = GlobalOptimizer::new(SolverKind::HiGHS, ObjectiveKind::MaxRevenue);
let result = optimizer.optimize(&sim.domains).unwrap();
assert!(result.improvement_pct >= 15.0);
```

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖
- f32/f64 用于数值，u64 用于 ID/时间戳
