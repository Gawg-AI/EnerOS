# Agent 信誉经济激励 (v0.181.0)

## 概述

`eneros-reputation` 实现信誉→权限→收益的完整联动链路，为电力交易 Agent 提供基于信誉的经济激励机制。

## 核心模块

| 模块 | 职责 |
|------|------|
| `types` | 数据类型：ReputationGrade, PermissionLevel, ReputationKind, ReputationScore, ReputationEvent |
| `scorer` | 信誉评分引擎：事件驱动分数更新、等级计算 |
| `incentive` | 经济激励策略：收益分成、权限映射、手续费折扣 |
| `linkage` | 联动表：grade→(permission, share, discount) 一站式查询 |

## 信誉等级

| 等级 | 分数区间 | 权限 | 收益系数 | 手续费折扣 |
|------|----------|------|----------|------------|
| A | >= 80 | Full | 1.0 | 0.2 |
| B | >= 60 | Full | 0.9 | 0.1 |
| C | >= 40 | Restricted | 0.7 | 0.0 |
| D | < 40 | Banned | 0.5 | 0.0 |

## 默认事件 Delta

| 事件类型 | Delta |
|----------|-------|
| OnTimeDelivery | +5 |
| Deviation | -10 |
| Dispute | -15 |
| MarketResponsiveness | +3 |
| Misbehavior | -30 |

## 权限门控 (PermissionGate)

- **Banned**: 拒绝所有交易
- **Restricted**: 交易量上限 100 kWh
- **Full / Probation**: 无限制

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖
- f64 用于金额/分数，u64 用于 ID/时间戳
