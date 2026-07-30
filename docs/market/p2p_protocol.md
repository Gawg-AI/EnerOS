# P2P 电力交易协议 (v0.175.0)

## 概述

`eneros-p2p` 实现 Agent 间点对点电力交易基础协议，支持报价(Bid)、询价(Ask)、
撮合(Match)、确认(Confirm)、拒绝(Reject) 五类消息的广播与接收。

## 消息类型

| 消息       | 方向       | 说明                         |
| ---------- | ---------- | ---------------------------- |
| `P2pBid`   | 卖方 → 网络 | 报价：出售电量 + 单价        |
| `P2pAsk`   | 买方 → 网络 | 询价：需求电量 + 最高接受价  |
| `P2pMatch` | 撮合 → 双方 | 撮合结果通知                 |
| `P2pConfirm` | 双方 → 撮合 | 成交确认                   |
| `P2pReject`  | 双方 → 撮合 | 成交拒绝 + 原因            |

## 验证规则

- **Bid**: `volume_kwh > 0`, `price >= 0`, `now < valid_until`
- **Ask**: `volume_kwh > 0`, `max_price >= 0`, `now < valid_until`

## 撮合条件

```text
bid.price <= ask.max_price
```

卖方报价不超过买方最高接受价即可成交。

## 传输层

协议通过 `P2pTransport` trait 抽象传输：

```rust
pub trait P2pTransport {
    fn broadcast(&mut self, topic: &str, msg: &P2pMsg) -> Result<(), P2pError>;
    fn recv(&mut self) -> Result<P2pMsg, P2pError>;
}
```

测试环境使用 `MockTransport`（内存队列）实现。

## 错误类型

| 变体            | 含义             |
| --------------- | ---------------- |
| `InvalidBid`    | 报价参数非法     |
| `InvalidAsk`    | 询价参数非法     |
| `Expired`       | 消息已过有效期   |
| `TransportError`| 传输层故障       |
| `NoMessage`     | 收件箱为空       |
| `MatchFailed`   | 撮合失败         |

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖
- f64 表示价格/电量，u64 表示 ID/时间戳
