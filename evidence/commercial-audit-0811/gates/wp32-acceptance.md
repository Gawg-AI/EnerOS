# 32 版本验收报告（蓝图/32 §8 DoD）

执行日期：2026-08-11。版本：32（Agent/AI 与数据完整性）。提交 0e531fb。

## DoD 逐项

| # | 判据 | 状态 | 证据 |
|---|---|---|---|
| 1 | 11 项全销（AG-03 决策 B 明示） | ✅ | AI-03/02/04/06/08/05/07 + AG-01/02/03/04；封焊面 grep 复核：3 处 ControlCommand 构造全在 cfg(test)（e2e_scenarios/assembly_demo/ingress signed_envelope），生产零旁路 |
| 2 | S9 台账修正 | ✅ | wp32-1-5-evidence.md 声明 W2.3/W5.1/W2.5 修复前 PENDING 计 |
| 3 | 触及 crate 全绿 | ✅ | 343 测试全绿 |
| 4 | 回归 | ✅ | workspace 3846 passed / 0 failed |

## AG-03 决策（默认 B，提交明示）
5 个市场/优化 toml（bid_strategy/spot_market/realtime_market/domain_optimizer/
multi_objective）标 `# status: deferred (v1.x)`——34 验收不得声称该能力；
A（补闭环）待产品确认另开工单。

## 治理自查
- gate 四子命令未回退（32 无门禁改动）。
- mock 门控：无新生产 Mock 面。
- 封焊面：AI/Agent 控制写仍经 ChainCommandSink → EnvelopeDispatcher →
  ingress 复验 → AuthorizedWriter（零旁路复核）。
