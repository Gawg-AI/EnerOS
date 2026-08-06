# ADR-0032：trust/reputation 信誉体系裁决（ARC-002）

> 状态：Accepted（2026-08-05，S4 ARC-002）

## 1. 决策

**双体系保留 + 定义双向映射**（不二选一删除）：

- `agent::TrustLevel`（4 级离散：Untrusted/Verified/Trusted/System）——**静态授权
  等级**：Agent 描述符内，用于访问控制（能力授予、配额），由宿主在装载时判定；
- `reputation::ReputationScore`（0–100 连续分 + grade A/B/C/D + 事件更新）——
  **动态信誉评分**：行为驱动的持续评估。

两者语义正交（授权快照 vs 行为评估），删除任一都会丢失语义；重复面仅
"信任"词汇——以**映射**消除重复。

## 2. 映射（实现于 reputation crate，types.rs）

| 信誉分 | TrustLevel |
|---|---|
| < 25 | Untrusted |
| 25 ≤ s < 50 | Verified |
| 50 ≤ s < 75 | Trusted |
| ≥ 75 | **Trusted 封顶**（System 为特权级，不由信誉分授予） |

反向：Untrusted→0、Verified→25、Trusted→75、System→100（区间下限）。

## 3. 依据

- reputation crate 当前无消费者（纯评估引擎）；agent::TrustLevel 被 descriptor/
  错误路径引用——映射使评估引擎的产出可直接驱动授权等级；
- System 封顶防信誉分冒用特权（安全语义）。

## 4. 后果

- 后续信誉引擎接入 Agent 授权时用 `trust_level()` 映射；TrustLevel 的
  授权语义不变（ADR-0014 系列）；
- 测试：边界映射（24.9/25/49.9/50/99.9）+ 反向 4 级。
