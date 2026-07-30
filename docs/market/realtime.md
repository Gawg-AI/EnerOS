# 实时市场 (Realtime Market)

> v0.174.0 — 15min 级实时报价闭环

## 概述

实时市场模块实现电力现货市场中 15 分钟级别的实时报价闭环。
支持会话管理、报价调度、报价验证、出清处理等核心流程。

## 核心概念

| 概念 | 说明 |
|------|------|
| Session | 交易会话，一次报价-出清的生命周期 |
| Quote | 报价单，包含电量、价格、方向 |
| Clearing | 出清结果，由市场运营方返回 |
| QuoteSchedule | 报价调度，控制 15min 间隔 |

## 模块结构

```
crates/agents/realtime-market/
├── Cargo.toml
└── src/
    ├── lib.rs          # 入口 + RealtimeError
    ├── types.rs        # 数据类型
    ├── session.rs      # 会话管理
    ├── quote.rs        # 报价调度与验证
    └── quote_loop.rs   # 报价闭环引擎
```

## 报价流程

1. `open_session(now)` — 开启交易会话
2. `should_quote(now)` — 判断是否到达报价窗口
3. `submit_quote(quote, now)` — 提交报价
4. `process_clearing(clearing)` — 处理出清结果
5. `close_session(id)` — 关闭会话

## 约束

- `#![no_std]` 兼容，仅依赖 `alloc`
- 零第三方依赖
- 价格/电量使用 `f64`
- 报价间隔默认 900 秒 (15min)

## 配置

参见 `configs/realtime_market.toml`。
