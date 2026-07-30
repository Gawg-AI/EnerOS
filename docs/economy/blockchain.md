# 轻量区块链 (eneros-chain)

> 版本: v0.179.0 | 状态: 已实现

## 概述

`eneros-chain` 提供面向边缘侧电力交易场景的轻量区块链实现，
采用 SM3 国密哈希构建不可篡改的交易存证链。

## 核心组件

| 模块 | 职责 |
|------|------|
| `types` | 基础类型：Sm3Digest、TxKind、TxRecord、Block |
| `block` | 区块构建：Merkle 根计算、区块哈希、创世块 |
| `hash_chain` | LightChain 实现 Chain trait（append/mine/verify） |
| `consensus` | 轻量共识 trait + SingleNodeConsensus |

## 数据结构

```text
Block {
    index: u64,          // 区块高度
    prev_hash: Sm3Digest,// 前块 SM3 哈希
    txs: Vec<TxRecord>,  // 交易列表
    merkle_root: Sm3Digest, // Merkle 根
    timestamp: u64,      // 时间戳
    proposer: u64,       // 提议者 ID
}
```

## 哈希策略

- **Merkle 根**: 拼接所有 tx_id(LE) + payload 后 SM3 哈希；空交易则 hash([0u8;32])
- **区块哈希**: hash(index || prev_hash || merkle_root || timestamp)
- **链验证**: 逐块校验 prev_hash == hash(前一区块)

## 共识

当前实现 `SingleNodeConsensus`（单节点直接接受），
后续可扩展 PBFT / Raft 等多节点共识。

## 使用示例

```rust
use eneros_chain::{LightChain, Chain, TxRecord, TxKind};

let mut chain = LightChain::new();
chain.append(TxRecord { tx_id: 1, kind: TxKind::Deal, payload: vec![0x01], at: 100 }).unwrap();
chain.mine_block(1000, 1).unwrap();
assert!(chain.verify().unwrap());
```

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 唯一外部依赖: `eneros-crypto`（SM3 哈希）
- u64 表示 ID 与时间戳
