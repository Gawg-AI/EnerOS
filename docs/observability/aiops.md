# AI 运维 (AIOps)

> crate: `eneros-aiops` | 版本: v0.191.0 | no_std

## 概述

EnerOS AI 运维模块为电力原生 Agent OS 提供智能运维能力，包含三大核心功能：

1. **异常检测** — 基于相对偏差的实时指标异常识别
2. **预测性维护** — 基于健康分数趋势的资产寿命预测
3. **自愈引擎** — 基于规则匹配的自动化故障修复

## 架构

```
┌─────────────────────────────────────────────┐
│              AiOps Pipeline                  │
├─────────────┬──────────────┬────────────────┤
│ AnomalyDetector │ PredictiveMaintenance │ SelfHealEngine │
│  (anomaly.rs)   │    (predict.rs)       │ (self_heal.rs) │
└─────────────┴──────────────┴────────────────┘
```

## 模块说明

### anomaly — 异常检测

- `AnomalyDetector::detect(metric, value, baseline, ts) -> AnomalyResult`
- 得分公式: `score = |value - baseline| / baseline`
- 判定: `score > sensitivity` 则为异常
- 零基线保护: baseline=0 时非零值视为无穷大偏差

### predict — 预测性维护

- `PredictiveMaintenance::predict(asset_id, health_scores) -> PredictionResult`
- 线性外推: 根据健康分数斜率推算 RUL (剩余使用寿命)
- 风险分级: RUL > 500 → Low, > 200 → Medium, > 50 → High, ≤ 50 → Critical

### self_heal — 自愈引擎

- `SelfHealEngine` trait: `diagnose` + `execute`
- `RuleBasedHealer`: 基于异常分数的规则映射
  - score > 0.8 → RestartComponent + NotifyHuman (需审批)
  - score > 0.5 → ThrottleLoad + ClearCache
  - 其他 → NotifyHuman

## 错误类型

| 变体 | 含义 |
|------|------|
| `DetectionFailed` | 异常检测失败 |
| `PredictionFailed` | 预测失败 |
| `HealFailed` | 自愈执行失败 |
| `ApprovalRequired` | 需要人工审批 |

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖
- f32/f64 用于分数，u64 用于 ID/时间戳
