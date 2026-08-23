# 蓝图 50 卷 Entry 登记（锚点重验 + 前置检查）

> 日期：2026-08-22 · 基线版本：0.313.35 · 基线 commit：e6037dc（49 卷
> 收尾复验登记提交）

## 锚点重验（44-00 §4.5.1，逐条实跑）

| 锚点 | 结果 |
|---|---|
| exporter 断 | ✅ `bin/svc-observability.rs:103-113` `UnavailableExporter`——`export_batch` 恒 `Err("agentbus-exporter-unavailable")`；durable outbox 已可挂接（`:43-81`，storage 授予时 AppendOutbox over BlockIo） |
| mgmt 未接 | ✅ `bin/svc-mgmt.rs:82-132` 真实动作经注入 executor：StatusQuery/ConfigGet 已接（EIPC svc-config），UpdateCheck/Apply/Rollback → Failed "ota-service-not-bound"，其余 → Failed "action-not-wired"（不空成功） |
| dashboard 未装配 | ✅ observability/dashboard、mgmt-cli/ops-gate 为 workspace host 侧成员；qemu-prod closure gate binaries=32 不含其任何 bin |
| 产线工具 | ✅ 蓝图 16 有设计（历史产品级 TUI/GUI/并行面标注「不得判已开发」）；代码事实 = xtask flasher 文件工位写面已有（list/lab-create/authorize/write/birth-record/key-domains），无 factory provision 编排 |
| 手册 | ✅ 全仓无用户/制造/升级回滚/故障处置四手册（仅 serial-debug/ci-cd 开发文档）；43 DoD 手册项空缺 |

## 上卷回归（§4.5.2，≥2 项照抄实跑，2026-08-22 实测）

1. **intent-contract::peer 14 测试**：`cargo test -p eneros-intent-contract peer`
   → `test result: ok. 14 passed; 0 failed` ✅
2. **remote_brain 10 测试**：`cargo test -p eneros-control-services` → crate
   48 passed 全绿（含 remote_brain::tests ×10：happy_path / timeout 降级 /
   consecutive→unhealthy / probe recover / over_budget_rejected 等）✅
3. **closure gate**：`cargo xtask closure gate --profile qemu-prod` →
   `binaries=32 reached_packages=122 denied=0 digest=bdd2332e… PASS`（与 49
   Exit 登记逐字一致）✅
4. **keys gate**：`cargo xtask qa gate keys` → tracked-keys=0 /
   default-refs=0 / test-keys=present / production-injection=fail-closed ✅

## 锚点修订记录

- 无偏差（五锚点与卷内登记一致，未触发 STOP）。

## 执行模式确认（卷首覆盖条款）

本卷无 owner 裁决停顿点：exporter 通道 v1.0 单选 MQTT（卷内已定）；手册
演练自行在 QEMU/测试环境执行并登记。按 SOP §4.5.3 连续执行 WP-50.1→50.4
→ Exit Gate。

## 文件清单（§4.5.3，开工前登记）

- **50.1**：`crates/agents/agent-sdk/src/telemetry_relay.rs`(新)+lib.rs(mod)、
  `business-services/src/observability.rs`（批量帧编解码+稳定幂等键+补传
  测试）、`business-services/src/bin/svc-observability.rs`（真 MQTT 导出器+
  outbox 16 槽）、`protocols/mqtt/src/plugin.rs`（telemetry/mgmt 主题+断连
  检测）、`protocol-services/src/net_transport.rs`（TcpStreamHandle::reconnect）、
  `protocol-services/src/bin/svc-mqtt.rs`（遥测分片+mgmt 转发）、
  `protocol-abstract/src/transport.rs`（reconnect seam）、root-task spawn.rs
  （授予 +1）+service.rs（OBS-REPORT 驱动）、configs/protocol-mqtt.toml +
  config-bundle protocol.rs（键表 +3）、evidence/50/WP-50.1.md、bump。
- **50.2**：`business-services/src/mgmt_rpc.rs`（每动作 RBAC+白名单+测试）、
  `bin/svc-mgmt.rs`（真实 executor 五动作）、spawn.rs（svc-mqtt→svc-mgmt）、
  evidence/50/WP-50.2.md、bump。
- **50.3**：`xtask/src/factory.rs`(新)+main.rs 注册、evidence/50/WP-50.3.md、
  bump。
- **50.4**：`docs/manuals/{user,manufacturing,upgrade-rollback,fault-
  troubleshooting}-manual.md`(新×4)+演练记录、evidence/50/WP-50.4.md、bump。
- **Exit**：`evidence/50/exit-gate.{json,md}`、状态板、45–47 抽查各 2 项。
