# EnerOS 多 Agent 强化学习 (v0.188.0)

> **子系统**：crates/ai/marl（eneros-marl）  
> **Phase 定位**：P4-D 高级 AI 第三版（ADR-0002 研究课题）

---

## 1. 概述

实现多 Agent 强化学习（MARL），使多个边缘 Agent 在联邦协同中共同进化，优化全局收益。采用 MAPPO（Multi-Agent PPO）算法，集中训练分散执行（CTDE）。

## 2. CTDE 架构

```text
各 Agent 局部状态 → 各策略推理(no_grad, 分散执行) → 联合动作
  → 环境执行 → 全局 Reward → 信用分配 → MAPPO 更新(集中训练)
```

## 3. 核心模块

| 模块 | 职责 |
|------|------|
| types | MultiAgentPolicy, JointTransition, MapConfig |
| coord | 信用分配（EqualCredit / ProportionalCredit）+ GlobalCritic |
| mappo | MAPPO 训练器（joint_act / observe / update） |

## 4. 信用分配

- **EqualCredit**：全局奖励均分（简单、可解释）
- **ProportionalCredit**：按局部奖励比例分配（更精细）

## 5. GPU 规则

- 各 Agent 策略网络 .to(device)
- joint_act 推理 no_grad
- GPU 不可用回退 CPU
- 当前 Rust 实现为 mock，PyTorch FFI 预留

## 6. 容错

- 某 Agent 离线：跳过该智能体，不阻塞联合更新
- 信用分配异常：退化为均分
- 训练发散：回滚所有策略
