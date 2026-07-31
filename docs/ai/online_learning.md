# EnerOS 模型在线学习 (v0.186.0)

> **子系统**：crates/ai/online（eneros-online-learning）  
> **Phase 定位**：P4-D 高级 AI 首版（ADR-0002 研究课题）

---

## 1. 概述

实现边缘端模型在线增量微调与联邦学习，让模型随运行数据持续优化，而非依赖离线重训。

## 2. 架构

```text
边缘采样数据 → Batch → 增量微调 → 检查点 → 联邦聚合 → 全局模型
                              ↓
                        发散检测 → 回滚
```

## 3. 核心模块

| 模块 | 职责 |
|------|------|
| finetune | IncrementalTrainer trait + mock 梯度下降 |
| federated | FedAvg 加权聚合（按样本数） |
| checkpoint | 快照/回滚/发散检测 |
| types | OnlineModel, Batch, TrainMetrics |

## 4. GPU 规则

本版本适用蓝图 §43.3 GPU 优先规则：
- 生产环境：model.to("cuda")，data.to("cuda")
- 测试推理：with torch.no_grad()
- GPU 不可用：回退 CPU

当前 Rust 实现为算法逻辑层（mock），PyTorch FFI 为后续预留。

## 5. FedAvg 算法

```
global[i] = Σ(client[j][i] × n_samples[j] / total_samples)
```

- 按各客户端训练样本数加权
- Shape 不匹配的贡献被拒绝
- 聚合后清空贡献池

## 6. 发散保护

- 检测 loss NaN/Inf → 立即报错
- 连续 N 次 loss 上升 → 判定发散
- 自动回滚到历史最优检查点
