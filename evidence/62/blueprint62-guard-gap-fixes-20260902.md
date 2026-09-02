# 蓝图 62 复查·守护缺口修复档案（2026-09-02，R0 收编）

> 补档背景：本档案记录的三处改动在 2026-09-02 复查（蓝图 62 卷 62.1-A/B
> 与 wiring 绝对基线）后落于工作树未提交；grep `62.1-A|62.1-B` 全仓零命中
> ——复查与修复在 蓝图/62 与 docs/ 无书面档案，改动悬空无登记。由蓝图 63
> R0（WP-63.1.0）收编：补档案 + 提交（独立 commit，不占 bump）。
> 内容（代码 + 验证证据）均已实测：蓝图 63 [01] §5 六 crate 测试基线含本
> 三文件（control-dispatch 100 / root-task wiring 118 全绿），本节补复跑留证。

## 定性

三项均为**低危守护缺口**（现有守卫测不到的具体缺陷形态），非活性 bug。

## 修复清单

| # | 级别 | 缺口 | 修复 |
|---|---|---|---|
| 1 | 低危（62.1-A） | settle_command Applied 分支是**手写两相**（intent Committed → journal AppliedConfirmed），表内 durable 序只是并行表示——表级单测锁不住手写分支；若该分支把 journal 推到 AppliedConfirmed 发生在 durable ack 成功之前，外部将见到"Committed 未落盘/写序反转" | `tests/settlement_lock.rs` 新增 `FailOnCommittedStore`（对 `Committed` 更新注入 `Unavailable`）+ `AppliedChannel`（两相成功帧 `[0x00,0x00]`）+ 端到端锁 `applied_write_order_durable_ack_failure_never_confirms_journal`：durable ack 失败 → journal 不得被推至 AppliedConfirmed → 终态 Indeterminate（含同 key 重放幂等锁）；ack 成功负控制 → Committed |
| 2 | 低危（62.1-B） | engine.rs `mod unit_tests` 的 IntentAppendFailed/JournalPersistFailed 用例夹具用 `ack_states()`（已含 Persisted/Delivered 等**结算后**状态）——与实际触发位不符（execute_core：Authorized/Reserved 已推、Persisted 未推 = pre-persist 阶段）；未来该触发改 StatesSuffix 形态时测试夹具会掩盖词形漂移 | 新增 `pre_persist_states()`（Proposed/Validated/Authorized/Reserved）并替换两用例断言与 audit 渲染输入 |
| 3 | 低危（wiring 绝对基线） | SERVICE_WIRING 表数据化后相对断言（同相位/错峰）锁不住绝对值漂移——若 15/10/5 被误改（如 16/14/6），相对测试全绿只剩 QEMU 门能抓 | `wiring.rs` `mod tests` 新增 `driven_schedule_absolute_baseline_locked`：modbus-tcp=(15,10)、mqtt=(15,10)、iec104=(15,5) 绝对基线断言（WP-47.1 `PROTO_EXECUTE_PERIOD`=15 + offset 10 + D-1 错峰 offset 5） |

## 验证证据

- 蓝图 63 [01] §5 基线（2026-09-03 实跑，工作树含本三文件）：
  control-dispatch 100 / agent-sdk 147 / signer-keyring 11 / business-services
  92 / snapshot-service 19 / ingestion-core 44 全绿。
- R0 复跑留证（见执行卡-63.1 F 节）：
  `cargo test -p eneros-control-dispatch`（lib 63 + 集成 9+3+8+9+8）绿；
  `cargo test -p eneros-root-task`（118，含 `driven_schedule_absolute_baseline_locked`）绿；
  `cargo xtask audit all`（62.2 卷 `--no-verify` 前科复核）exit 0，
  8 profile + 1 layout 全 PASS。

## 附：62.2 卷 audit 门前科复核（R0 动作 2）

62.2 卷曾因 audit 门网络断连走 `--no-verify`。本次离线重跑 `cargo xtask
audit all`：exit 0，全部 profile/layout PASS——该门当前判定与 62.2 卷
`--no-verify` 的结论一致，前科无残余问题（留证执行卡-63.1）。
