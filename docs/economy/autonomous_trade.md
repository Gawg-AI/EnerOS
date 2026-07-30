# Agent 间自主交易 (v0.178.0)

## 概述

`eneros-autonomous` 实现 Agent 间自主交易全闭环，覆盖从报价撮合到最终结算的完整流程。

## 交易状态机

```
Matched → Frozen → Confirmed → Delivering → Delivered → Settled
   ↓         ↓         ↓           ↓            ↓
 Failed    Failed    Failed      Failed       Failed
```

- **Matched**: 买卖双方报价撮合成功
- **Frozen**: 买方资金/额度已冻结
- **Confirmed**: 双方确认交易条款
- **Delivering**: 电力交付进行中
- **Delivered**: 交付完成，等待结算
- **Settled**: 结算完成（终态）
- **Failed**: 交易失败（终态）

## 模块结构

| 模块 | 职责 |
|------|------|
| `types` | TradePhase / Settlement / TradeContext 类型定义 |
| `orchestrator` | TradeOrchestrator 状态机编排 |
| `settlement` | SettlementTrigger trait + AutoSettlement 实现 |
| `closed_loop` | ClosedLoop trait + 自动推进至终态 |

## 使用示例

```rust
use eneros_autonomous::{TradeOrchestrator, AutoSettlement, ClosedLoopImpl, ClosedLoop};

let mut orch = TradeOrchestrator::new();
let deal_id = orch.create_trade(seller_id, buyer_id, 500.0, 0.65);

let mut closed = ClosedLoopImpl::new(orch, AutoSettlement::new(timestamp));
let final_phase = closed.run_to_completion(deal_id).unwrap();
// final_phase == TradePhase::Settled
```

## 结算规则

- `seller_credit_kwh` = 已交付电量
- `buyer_debit_kwh` = 已交付电量
- `amount_yuan` = 已交付电量 × 单价

## 约束

- `#![no_std]` 环境，仅依赖 `alloc`
- 零第三方依赖
- 金额使用 `f64`，ID/时间戳使用 `u64`
