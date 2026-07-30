# P2P 撮合引擎 (p2p-match)

> 版本: v0.176.0 | Crate: `eneros-p2p-match` | no_std 兼容

## 概述

P2P 撮合引擎为去中心化电力交易提供价格时间优先撮合算法与成交生命周期管理能力。

## 核心组件

### 撮合策略 (MatchStrategy)

- **PriceTimePriority**: 价格时间优先撮合
  - 卖方 (Bid): 价格升序 → 同价按 seq 时间优先
  - 买方 (Ask): 最高出价降序 → 同价按 seq 时间优先
  - 撮合条件: `bid.price <= ask.max_price`
  - 成交价: `bid.price` (卖方报价)
  - 成交量: `min(bid.volume, ask.volume)`

### 撮合引擎 (MatchEngine)

```rust
let mut engine = MatchEngine::new();
engine.add_bid(bid);
engine.add_ask(ask);
if let Some(deal) = engine.run_match() {
    // 处理成交
}
```

### 成交管理 (DealManager)

状态机转换规则:

```
Matched → Confirmed → Delivering → Settled
   ↓          ↓
Cancelled  Cancelled
```

```rust
let mut mgr = DealManager::new();
mgr.add_deal(deal);
mgr.transition(deal_id, DealState::Confirmed)?;
mgr.record_execution(deal_id, report)?;
```

## 数据类型

| 类型 | 说明 |
|------|------|
| `P2pBid` | 卖方报价 (agent, volume_kwh, price, valid_until, seq) |
| `P2pAsk` | 买方询价 (agent, volume_kwh, max_price, valid_until, seq) |
| `Deal` | 成交记录 (deal_id, seller, buyer, volume, price, window, state) |
| `DealState` | 成交状态枚举 |
| `ExecReport` | 执行报告 (delivered_kwh, deviation) |

## 错误类型

- `NoMatch`: 无匹配订单
- `DealNotFound`: 成交记录不存在
- `InvalidTransition`: 非法状态转换
- `VolumeMismatch`: 交割量不匹配
- `PriceGap`: 价格差距无法撮合

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖
- f64 用于价格/电量, u64 用于 ID/时间戳
