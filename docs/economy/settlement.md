# 自动结算系统 (eneros-settlement)

> 版本: v0.180.0 | 模块: `crates/agents/settlement`

## 概述

自动结算系统为电力交易提供完整的清算、对账与争议处理能力，
适用于分布式能源交易场景下的自动结算流程。

## 核心模块

### 清算 (clearing)

`ClearingEngine` 根据实际交付电量与合同电量执行清算：

- 计算偏差率: `|delivered - contract| / contract`
- 偏差率 ≤ 5%: 正常清算 (Cleared)，无考核费
- 偏差率 > 5%: 标记 ShortDelivered / OverDelivered，收取考核费
- 考核费 = 结算金额 × penalty_rate (默认 5%)

### 对账 (reconcile)

`Reconciler` 将清算结果与预期金额逐笔核对：

- 按 deal_id 匹配清算记录
- 金额容差: ±0.01 元
- 输出 `ReconcileReport`（匹配数/不匹配数/不匹配列表）

### 争议处理 (dispute)

`DisputeManager` 管理争议生命周期：

- `Open` → `Arbitrating` → `Resolved`
- 同一交易不允许存在多个未解决争议
- 已解决的争议不影响后续新争议

## 数据类型

| 类型 | 说明 |
|------|------|
| `ClearingJob` | 清算输入（交易ID/交付量/合同量/电价/时间） |
| `ClearingResult` | 清算输出（金额/考核费/状态） |
| `ReconcileReport` | 对账报告 |
| `DisputeCase` | 争议案例 |

## 错误类型

`SettlementError`: ClearingFailed / InvalidJob / NotFound / InvalidTransition / DuplicateDispute / ReconcileFailed

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖
- f64 表示金额，u64 表示 ID/时间戳
