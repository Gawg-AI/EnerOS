# EnerOS 信誉评分规则 (v0.168.0)

> 本文档定义 Agent Trust Score 的评分规则与配置方法。

---

## 1. 基础规则

初始分数：**500**（Silver 等级）
分数范围：**0 ~ 1000**

## 2. 行为评分表

| 行为 | 基础分 | 成功修正 | 失败修正 |
|------|--------|---------|---------|
| 任务完成 | +5 | +5 | +2（减半） |
| 任务失败 | -3 | -6（加倍） | -3 |
| 越权访问 | -50 | -100（加倍） | -50 |
| 恶意行为 | -200 | -400（加倍） | -200 |
| 崩溃 | -10 | -20（加倍） | -10 |
| 恢复 | +2 | +2 | +1（减半） |
| 协作 | +3 | +3 | +1（减半） |

## 3. 自定义规则

```rust
use eneros_trust::ScoringRules;

let rules = ScoringRules::custom(
    10,   // task_complete
    -5,   // task_failed
    -100, // unauthorized
    -500, // malicious
    -20,  // crash
    5,    // recovery
    8,    // cooperation
);

let sys = TrustSystemImpl::with_rules(rules);
```

## 4. 时间衰减配置

```rust
use eneros_trust::{DecayConfig, TrustAdjuster};

let config = DecayConfig {
    period: 7200,        // 每 7200 时间单位
    decay_per_period: 2, // 衰减 2 分
    enabled: true,
};
let adjuster = TrustAdjuster::new(config);
```

## 5. 等级阈值

| 等级 | 最低分 | 最高分 |
|------|--------|--------|
| Blocked | 0 | 99 |
| Bronze | 100 | 399 |
| Silver | 400 | 699 |
| Gold | 700 | 899 |
| Platinum | 900 | 1000 |

## 6. 封禁与恢复

- 分数 < 100 → 自动封禁（Blocked）
- 封禁后 Agent 所有能力被禁用
- 恢复途径：管理员手动调整分数至 ≥ 100
- 时间衰减也会使低分 Agent 缓慢恢复（向 500 回归）
