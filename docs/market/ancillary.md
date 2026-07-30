# 辅助服务参与 (Ancillary Service Participation)

> v0.172.0 — 调频/调峰 + AGC 接口

## 概述

本模块实现电力辅助服务市场参与的核心逻辑，包括：

- **调频 (Frequency Regulation)**: 基于死区 + 下垂特性的快速频率响应
- **调峰 (Peak Shaving)**: 基于负荷阈值和 SOC 约束的削峰放电
- **AGC 接口**: 接收调度命令并执行确认

## 架构

```
AncillaryParticipant (trait)
├── submit_bid()       → 报价提交
├── handle_agc()       → AGC 命令处理
└── available_capacity_kw()

AgcHandler
├── recv_command()     → 命令入队
├── execute_command()  → 执行 + 确认
└── has_pending()

FreqRegImpl (FrequencyRegulator)
└── on_freq_deviation() → 死区 + 下垂计算

PeakShaveController
├── should_discharge()       → 阈值判断
├── calculate_discharge_kw() → 削峰量
└── check_soc()              → SOC 校验
```

## 调频算法

1. 若 |偏差| < 死区 → 不响应 (0 kW)
2. 否则: `P = -Δf / (droop%/100) × Pmax / 50`
3. 限幅到 ±Pmax

## 配置

参见 `configs/ancillary.toml`。

## Crate

- 路径: `crates/agents/ancillary/`
- 名称: `eneros-ancillary`
- 约束: `no_std`, 零第三方依赖, f64 功率/频率
