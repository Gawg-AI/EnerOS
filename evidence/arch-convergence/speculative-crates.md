# 未接入市场 crate — speculative 标记（ARCH-M04 / ADR-0029）

> 裁定：以下市场域 crate 当前**零消费者**（无任何 workspace crate 依赖），
> 未接入真实 agent 的报价/结算流水线。按 ARCH-M04（用户决策：保留 workspace +
> 明确标记）标注 `[SPECULATIVE]`，不进入任何 release profile；
> 接入 energy-market-agent 报价流水线前不得计入完成度（07 §5）。

## 标记清单（Cargo.toml description 前缀 `[SPECULATIVE]`）

| crate | 语义 | 接入前提 |
|---|---|---|
| eneros-spot-market | 日前/日内报价 + 出清 + 执行跟踪 | 接入 energy-market-agent 报价流水线 |
| eneros-realtime-market | 15min 级实时报价闭环 | 同上（价格单位已 YuanPerMwh 化，M02） |
| eneros-p2p | P2P 电力交易协议 | 有真实 agent 消费者 |
| eneros-p2p-match | P2P 撮合引擎 | 同上 |
| eneros-ancillary | 辅助服务参与（调频/调峰/AGC） | 接入 energy-market-agent 或独立 agent |
| eneros-settlement | 自动结算（清算/对账/争议） | 与 clearing 对账测试通过后接入 |
| eneros-dr | 需求响应聚合 | 接入 DR 业务 agent |
| eneros-account | 能源账户（余额/收支/历史） | 接入计费流水线 |

## 规则

- `[SPECULATIVE]` crate 不进入 release profile 依赖闭包；
- 未接入前不得以"孤立编译通过"计入完成度（07 §0 规则 1/2）；
- 接入时先移除标记并补充真实消费者证据（依赖闭包 + 系统测试）。
