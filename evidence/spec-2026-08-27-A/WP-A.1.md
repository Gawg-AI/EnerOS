# WP-A.1 控制链信封密钥出仓（SPEC-SERIES-2026-08-27/A）

代码基线：b5d939b → 本卷工作树（版本 0.314.15）；执行日期 2026-08-27。

## 判据结果

| 判据 | 结果 | 留证 |
|------|------|------|
| C1 非测试 .rs 零命中 `w40-test-key` | ✅ | `grep -rn "w40-test-key" --include="*.rs" .` → 零命中；唯一残留 = 本规格文档正文（审计留痕） |
| C2 注入链正/负例 host 单测 | ✅ | `eneros-config-bundle --features compile`：`security_compile_decode_roundtrip` / `security_missing_injection_fails_closed`(exit=2 文案含指引) / `security_zero_key_rejected` / `security_toml_plaintext_attempt_rejected`；xtask 实测：`ENEROS_KMS_DIR=configs/kms-dev` 编译 `CONFIG-COMPILE: security → 35B`，无键 → `xtask: FAIL: 控制链信封密钥未提供：--control-chain-key-hex / ENEROS_CONTROL_CHAIN_KEY / ENEROS_KMS_DIR/control-chain.key——release 无键构建 fail-closed（SPEC WP-A.1）` exit=2 |
| C3 QEMU 正例打穿如旧 | ✅ | gate-final 镜像（image sha256=7a378ac8…，本卷终态代码）：串口 `MASTER-WRITE device=1 point=10 power=5 ACK` × `CONTROL-CHAIN: dispatch=Committed`（计数 MASTER-WRITE=1/Committed=1；过程目录 gate-run 曾录得 171/171 同链路大样本） / `PRODUCT-READY=1` / `CHAIN-KEY-MISSING=0`；`virtual-pcs registered`、`RATE-GATE quota=100000/h`、`CONTROL-BINDING device=1 point=10` 在串口。证据：`target/evidence-wA/{gate-run,gate-final}/qemu-serial.log` |
| C4 缺键/错键 bundle 拒载 | ✅（缺节变体） | neg-final 镜像（同 sha 源码态、security 条目已摘除重编）：`CHAIN-KEY-MISSING=2`（svc-device-agent + svc-control-dispatch 各一）`Committed=0` `PANIC=0`。证据：`target/evidence-wA/neg-nokey/qemu-serial.log`。注：外部对端无信封验证面（mqtt peer JSON 无信封，登记在案），错键对称内建场景在本环境不可构造，按缺节变体如实执行 |
| C5 线格式 byte 级不变 | ✅ | 换的是键来源（bundle security 节注入），未动 control-gateway/envelope.rs 协议语义（白名单禁止区零触碰）；host `dispatch_ingress_chain` 信封帧用例全绿 |

## 装配要点

- config-bundle：`schema::security`（CONFIG_ID "security"，FIELD_CONTROL_CHAIN_KEY=1，32B）；`control_chain_key.rs` 解码器（缺字段/长度≠32/全零 → Err fail-closed）；`toml_compile::compile_security`（TOML 零键纪律 + 惰性解析——仅 manifest 含 security 节才走注入链，避免破坏无该节的其他 profile 构建）。
- xtask：`--control-chain-key-hex` > `ENEROS_CONTROL_CHAIN_KEY` > `$ENEROS_KMS_DIR/control-chain.key`（文件宽容 64-hex 或恰好 32B 原始 ASCII）；三者全缺 → Err。
- 消费：`cfg_bindings::fetch_control_chain_key`（ConfigGet("security") 同 points wire）；两 bin 持 `Option<ingress/engine>`，键不可得 → `CHAIN-KEY-MISSING` + err_receipt("chain-key-missing")。
- svc-config：新增 `SecurityConsumer`（registry 双向完备性——否则 unconsumed-config 整包拒载，首跑实证抓到）。
- 键值真源：`configs/kms-dev/control-chain.key`（32B ASCII，沿用装配时代原值，OAQ-1 默认裁决）；`configs/security.toml` 零键；CI ci.yml:546 以 `ENEROS_KMS_DIR=configs/kms-dev` 注入。
