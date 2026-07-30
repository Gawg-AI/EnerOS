# Agent 能源账户 (v0.177.0)

## 概述

`eneros-account` 提供 Agent 级别的能源账户管理能力，
支持余额维护、收支记账、资金冻结/解冻以及交易历史查询。

## 核心概念

| 概念 | 说明 |
|------|------|
| EnergyAccount | 能源账户，维护 kWh 与元双余额 |
| LedgerEntry | 账本条目，记录一笔 Credit 或 Debit |
| FrozenAmount | 冻结记录，锁定部分可用余额 |
| FlowDir | 资金方向：Credit(收入) / Debit(支出) |

## 模块结构

```
crates/agents/account/
├── Cargo.toml
└── src/
    ├── lib.rs       # EconError, AccountManager trait, re-exports
    ├── types.rs     # FlowDir, EnergyAccount, LedgerEntry, FrozenAmount
    ├── ledger.rs    # AccountStore 实现 AccountManager
    ├── entry.rs     # create_entry / entry_summary 辅助函数
    └── history.rs   # query_history / recent_entries 查询
```

## AccountManager 接口

- `open(owner)` — 开户，初始余额为零
- `balance(account_id)` — 查询 (kwh, yuan) 余额
- `post(entry)` — 记账，Credit 增加余额，Debit 减少余额（余额不足拒绝）
- `freeze(account_id, amount)` — 冻结金额，减少可用余额
- `unfreeze(freeze_id)` — 解冻，恢复可用余额

## 错误类型 (EconError)

- `AccountNotFound` — 账户不存在
- `InsufficientBalance` — 余额不足
- `Frozen` — 账户已冻结，禁止操作
- `InvalidEntry` — 非法条目（负数金额等）
- `AlreadyReleased` — 重复解冻
- `DuplicateAccount` — 同一 owner 重复开户

## 约束

- `#![cfg_attr(not(test), no_std)]`，零第三方依赖
- f64 表示金额/电量，u64 表示 ID/时间戳
- 冻结逻辑：可用余额 = balance_yuan - 未释放冻结总额
