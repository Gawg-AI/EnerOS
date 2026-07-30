# 需求响应聚合 (Demand Response Aggregation)

> v0.173.0 · crate: `eneros-dr` · no_std · 零第三方依赖

## 概述

需求响应聚合模块负责接收电网/调度平台下发的 DR 信号，聚合分布式负荷资源，
按比例分配削减/增加目标，并跟踪各参与者的实际响应与偏差。

## 核心概念

| 概念 | 说明 |
|------|------|
| DrSignal | DR 事件信号，包含事件窗口、目标功率、激励金额 |
| DrParticipant | 参与 DR 的负荷代理，具有基线/最小/最大功率 |
| DrAssignment | 调度分配结果，记录每个代理被分配的功率 |
| AggregatedCapacity | 聚合后的可削减/可增加总容量 |

## 信号类型

- **LoadReduce** — 削负荷（降载）
- **LoadIncrease** — 增负荷（填谷）
- **Curtailment** — 限电（强制削减）

## 调度算法

1. 过滤可用参与者 (`available == true`)
2. 根据信号类型计算每个参与者的可用容量：
   - LoadReduce / Curtailment: `base_kw - min_kw`
   - LoadIncrease: `max_kw - base_kw`
3. 按比例分配: `assign = (cap_i / total_cap) * target_kw`
4. 钳位到参与者自身容量上限

## 响应跟踪

- `record_actual`: 记录实际响应功率
- `acknowledge`: 确认分配
- `deviation_pct`: 计算偏差百分比 `|actual - assigned| / assigned * 100`

## 错误处理

| 错误 | 含义 |
|------|------|
| InvalidSignal | 信号参数无效 |
| NoParticipants | 无可用参与者 |
| CapacityInsufficient | 聚合容量不足以满足目标 |
| ParticipantNotFound | 参与者不存在 |
| AlreadyAcknowledged | 重复确认 |
| EventNotActive | 事件未在激活窗口内 |

## 配置

参见 `configs/dr.toml`。
