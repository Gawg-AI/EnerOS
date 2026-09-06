# kms-dev · 开发档 KMS 目录模板（SPEC-SERIES-2026-08-27/A WP-A.1）

qemu-test/qemu-prod 开发/QEMU 档的显式注入源。**键值不得出现于任何 .rs
源文件**（CI grep 判据 C1）；量产档密钥必须由 owner 流程经真实 KMS 提供，
本目录不得进入量产介质装配。

| 文件 | 用途 | 消费方 |
|------|------|--------|
| `control-chain.key` | 控制链信封 HMAC-SM3 密钥（32B ASCII） | `xtask config compile` 注入链第三级：`ENEROS_KMS_DIR=<repo>/configs/kms-dev` |
| `degrade-lease.key` | D3 降级 lease HMAC 键（32B ASCII；蓝图 68.5 WP-68.5.3） | `xtask config compile` 注入链第三级（ai-peer 编译消费） |

注入链优先级：`--control-chain-key-hex` > `ENEROS_CONTROL_CHAIN_KEY` >
`$ENEROS_KMS_DIR/control-chain.key`；三者皆无 ⇒ bundle 编译 Err（fail-closed）。
降级 lease 键同款：`--degrade-lease-key-hex` > `ENEROS_DEGRADE_LEASE_KEY` >
`$ENEROS_KMS_DIR/degrade-lease.key`（68.5 裁决 7：TOML 零键，compile_security
同款纪律）。

登记测试键值沿用了装配时代 `ASSEMBLY_TEST_KEY` 的原值，防编排基线漂移
（OAQ-1 默认裁决）；**换键 = 新值落本文件 + 受影响卷重新出证据**。
