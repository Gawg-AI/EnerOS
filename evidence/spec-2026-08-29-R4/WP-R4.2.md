# WP-R4.2 留证 · C5 PersistentAuditStore 死副本删除

日期：2026-08-29 · 基线 0.314.20 @ b5d939b（WIP 树）

## 改动面

| 文件 | 改动 |
|---|---|
| runtime/durable-log/src/lib.rs | 删除 PersistentAuditStore（408-516：struct + open_existing + 112B encode/decode + AuditStore impl）与测试 `persistent_audit_store_roundtrip`；crate 文档头登记"审计持久化语义唯一出处 = security/audit"；audit_scope 注释订正 |
| runtime/durable-log/Cargo.toml | 删除 `eneros-audit` 依赖（grep 证实全 crate 仅该 store 及其测试使用） |

## 判据核对（spec R4 §1 WP-R4.2）

- **C1**：`grep -rn PersistentAuditStore crates/ xtask/src` → **零命中**。
- **C2**：`cargo test -p eneros-durable-log` → **9 passed / 0 failed**
  （存活测试 BlockDurableLog format/append/recover/powercut_20_cycles/
  append_rejects_record_exceeding_buffer 等断言未动全绿）。
- **C3**：全量 workspace 测试见 summary.md；root-task durable_log_probe
  消费面（BlockDurableLog）不受影响（纯删除零消费者）。

## 体检声称比对（报告 C5 卡）

| 体检证据 | 修复后状态 |
|---|---|
| lib.rs:408-517 死孪生零消费者 | ✓ 全部删除（主代 grep 事后复核零命中） |
| 112B 摘要模型被权威亲判死刑（P1-03） | ✓ 删除后不再存在"未来误接 112B 后端"的隐患面 |
| eneros-audit 依赖死挂 | ✓ 依赖已移除 |
| durable-log 收缩为纯 BlockDurableLog | ✓ 达成（crate 文档头如实登记语义归属） |

## 行为变更声明

无运行时行为变更（零消费者纯删除）；crate 公共 API 面缩小（删除项全仓
无调用点）。
