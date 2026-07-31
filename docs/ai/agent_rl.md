# EnerOS Agent 强化学习 (v0.187.0)

> **子系统**：crates/ai/rl（eneros-rl）  
> **Phase 定位**：P4-D 高级 AI 第二版（ADR-0002 研究课题）

---

## 1. 概述

实现 Agent RL 框架，使调度策略通过 Reward 反馈自进化，超越静态优化。

## 2. RL 循环

```text
状态(电价/SOC/负荷) → 策略推理(no_grad) → 动作(充放电功率)
  → 环境执行 → Reward(收益-成本-惩罚) → Replay Buffer
  → 达 rollout → PPO 更新 → 循环
```

## 3. 核心模块

| 模块 | 职责 |
|------|------|
| policy | 策略推理 + 动作采样（确定性） |
| reward | Reward Shaping（收益-成本-安全-SOC-延迟） |
| ppo | PPO 训练器（act/observe/update） |
| types | AgentPolicy, Transition, PpoConfig, ReplayBuffer |

## 4. Reward 设计

```
reward = w_revenue × (revenue - cost)
       - w_safety × (safety_violation × 10)
       - w_soc × (|soc_deviation| × 0.5)
       - lag × 0.1
```

## 5. GPU 规则

- 策略网络 policy.to(device)
- act 推理包 with torch.no_grad()
- GPU 不可用回退 CPU
- 当前 Rust 实现为 mock，PyTorch FFI 预留

## 6. PPO 配置

| 参数 | 默认值 | 说明 |
|------|--------|------|
| clip | 0.2 | PPO clip 范围 |
| gamma | 0.99 | 折扣因子 |
| lam | 0.95 | GAE lambda |
| lr | 3e-4 | 学习率 |
| rollout_steps | 128 | 更新前收集步数 |
