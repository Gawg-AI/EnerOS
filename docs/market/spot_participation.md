# 现货市场参与 (Spot Market Participation)

> v0.171.0 — 日前/日内报价 + 出清 + 执行跟踪

## 概述

本模块实现电力现货市场参与的核心业务逻辑，支持日前市场 (Day-Ahead) 和
日内市场 (Intra-Day) 两种交易模式。

## 架构

```
┌─────────────┐     ┌──────────────────┐     ┌────────────────┐
│  BidBuilder │────▶│ ClearingProcessor│────▶│  ExecTracker   │
└─────────────┘     └──────────────────┘     └────────────────┘
       │                     │                        │
       ▼                     ▼                        ▼
  validate_bid()      apply_clearing()       update_actual()
```

## 核心流程

1. **报价构建** — `BidBuilder` 构建多段报价，`validate_bid` 校验合法性
2. **报价提交** — `SpotMarketParticipant::submit_bid` 提交至市场
3. **出清处理** — `ClearingProcessor::apply_clearing` 接收出清结果
4. **执行跟踪** — `ExecTracker` 跟踪状态机 + 偏差检测

## 状态机

```
Submitted ──▶ Cleared ──▶ Dispatching ──▶ Settled
    │             │              │
    ▼             ▼              ▼
  Failed       Failed      Deviated ──▶ Settled
                               │
                               ▼
                            Failed
```

## 偏差管理

- 偏差阈值：5%（可在 `configs/spot_market.toml` 中配置）
- 当 `|actual - planned| / planned > 5%` 时自动标记为 Deviated
- Deviated 状态可继续结算或标记失败

## 类型说明

| 类型 | 说明 |
|------|------|
| `SpotMarketType` | DayAhead / IntraDay |
| `TradeDir` | Buy / Sell |
| `Bid` | 多段报价单 |
| `ClearingResult` | 出清结果 |
| `ExecStatus` | 执行状态枚举 |
| `ExecTracker` | 状态机 + 偏差跟踪 |

## 约束

- `#![no_std]` 兼容，仅依赖 `alloc`
- 零第三方依赖
- 价格/电量使用 `f64`
- 时间使用 UTC 秒 (`u64`)
