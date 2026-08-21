# 47 卷 Exit Gate 自证（收尾验收复验）

> 日期：2026-08-22 · 版本：0.313.24 · 代码 commit：71a74a3 · 镜像 sha256：854a20c2…
> 状态：**Accepted**——本会话为收尾验收复验：Exit 各项全部重跑 + 两处证据
> 缺陷实弹补证（审计入链目标侧可观测、串口日志入仓）+ 诚实登记扩充。

## 复验结论总览

| Exit 项 | 上轮声称 | 本轮重跑结果 |
|---|---|---|
| 1 QEMU 实弹写链 | PASS（审计入链仅 host 面） | **PASS（目标侧补齐）**——写链双证据复现 + `AUDIT-CHAIN appended=44 dropped=0` 实弹 |
| 2 负例矩阵 | PASS | **PASS**——host 测试全数实跑（9+5+3）；上轮所引「QEMU 观察负例」无持久化证据，如实改登记 |
| 3 密钥 grep + fail-closed | PASS（grep=0 表述不准） | **PASS（精确分类后）**——w40-prod-key=0；dev-signing.key 宽匹配 4 处全豁免分类；无键构建实弹 FAIL exit=2 |
| 4 全量 CI 绿 | PASS（375 块） | **PASS**——--no-fail-fast 全量 EXIT=0（375 ok 块）+ clippy 0 + fmt 0 + closure gate PASS |
| 5 45/46 抽查 | PASS | **PASS**——4 项抽查命令实跑全过 |

## 项 1：QEMU 实弹（本轮复跑，镜像 854a20c2…，0.313.24）

证据文件：`evidence/47/qemu-write/{qemu-serial.log, modbus.transcript.jsonl, mqtt.transcript.jsonl}`
（串口日志本轮起 force-add 入仓——上轮被 `.gitignore` `*.log` 规则遗漏，
exit-gate 引用的双证据之一实际不在库，已修正。）

串口（qemu-serial.log；对端 = 非 EnerOS python 进程）：

```
[svc-control-dispatch] RATE-GATE quota=100000/h（产品默认 60/h）
[svc-control-dispatch] DURABLE-INTENT-STORE open OK（AppendOutbox over svc-storage）
[svc-modbus-tcp] MASTER-WRITE device=1 point=10 power=50 ACK        ← L18805
[svc-modbus-tcp] MASTER-READBACK point=10 eng=50 target=50 VERIFIED
[svc-control-dispatch] INTENT state=Committed note=None             ← L18878
[svc-control-dispatch] AUDIT-CHAIN appended=44 dropped=0            ← L18879（本轮新增）
[svc-mqtt] INTENT-FWD protocol=mqtt device=1 point=10 power=Float(50.0) seq=1 state=8 total=1
```

对端 transcript（mqtt_broker47 + modbus_slave47，独立进程）：

```
mqtt:   FORWARD "SET 1 10 50.0"（conn 2 SCADA → guest 订阅者）       ← L21
modbus: WRITE fc06 addr=0x200 value=50                              ← L1002
        READ  fc03 addr=0x200 value=50（从站值变化 + 独立读回）      ← L1004
```

审计入链补证（本轮实弹修复）：engine 新增 `audit_append_count`（成功入链
计数；失败仍走 drop 计数），svc-control-dispatch 在意图处理后打印
`AUDIT-CHAIN appended=N dropped=M`——测试先行（`audit_append_count_observable`
先红后绿）。上轮 chain.log 成功路径静默，QEMU 侧审计入链零直接证据；本轮
实弹 44 条控制事件入链零丢弃。

## 项 2：负例矩阵（host 测试实跑，0.313.24 套件内）

| 负例 | 证据（测试名） |
|---|---|
| 未授权写 deny | `intent_out_of_binding_denied_zero_delivery` / `intent_policy_deny_zero_delivery` → NotAuthorized 零投递 |
| 读回不符 Failed | `intent_readback_mismatch_is_failed`（OutcomeFrame → Failed）；audit_chain Failed 入链 + 截断/篡改定位 |
| 超时 Indeterminate | `intent_delivery_timeout_is_indeterminate`（不猜成功不记账） |
| 断电恢复 | durable_intent：`power_loss_tear_recovers_intact_pending` / `pending_intent_recovers_after_restart` / `committed_after_restart_not_pending` |
| 重放拒绝 | `intent_replay_same_sequence_zero_write`（重复 seq → Rejected 零写） |
| rate 预算 | `intent_budget_exceeded_is_err_zero_delivery`（BudgetExceeded 零投递） |

诚实登记：上轮 exit-gate 所引「QEMU 观察负例（BadResponse/BudgetExceeded
早期拒绝）」无持久化证据文件（早期运行串口被后续运行覆盖）——负例矩阵判据
以 host 测试为准；QEMU 本轮为全通正向窗口。

## 项 3：密钥 grep + release 无键 fail-closed

```
w40-prod-key：
  grep -rn "w40-prod-key" crates/ xtask/ product/  → 0 命中
  （全仓命中仅蓝图/审查报告/evidence 文档——文档豁免）

dev-signing.key（宽匹配，crates/ xtask/ product/）→ 4 处全分类豁免：
  ① xtask/src/gate.rs:1038（注释）/1069（.arg 探针）——keys 门禁自身的
    回归检测机制（gate 运行时 --exclude 自引用）；
  ② xtask/src/keys.rs:36 ——唯一解析入口常量，值 =
    "product/security/keys-local/dev-signing.key"（gitignore 白名单目录，
    非旧默认路径）；
  ③ product/factory/key-domains.json:12 ——owner 维护的工厂密钥域登记
    （schema v1 note 明示「执行 AI 不得自行把某 key 登记到某域」），
    属 owner 登记文档。
  旧路径字面量 "product/security/dev-signing.key" 在源码零功能性引用。

cargo xtask qa gate keys → KEYS-GATE tracked-keys=0 PASS /
  default-refs=0 PASS / test-keys=present PASS /
  production-injection=fail-closed（单测覆盖）

release 无键构建 fail-closed（本轮实弹）：
  $ env -u ENEROS_IMAGE_SIGNING_KEY cargo xtask image qemu-prod
  xtask: FAIL: 生产装配签名键必须显式传入：--sign-key <path> 或
         ENEROS_SIGN_KEY（或 KMS_DIR 提供）——fail-closed      （exit=2）
```

## 项 4：全量测试/CI 绿 + 版本

```
cargo test --workspace --no-fail-fast → EXIT=0，375 test result: ok 块，0 失败
  （首轮 fail-fast 运行遇 iec104-master loopback 抖动失败 EXIT=101；
    该测试单独重跑即过、并在 --no-fail-fast 全量中通过——并行负载抖动，
    非回归）
cargo clippy --workspace --all-targets → 0 error / 0 warning
cargo fmt --all --check → 0 diff（本轮新测试格式已修）
cargo xtask closure gate --profile qemu-prod → CLOSURE-GATE PASS
  （denied=0 crypto_violations=0 binaries=28 reached_packages=121）
版本 0.313.23 → 0.313.24（bump-version patch）
```

## 项 5：45/46 卷 Exit 抽查（各 2 项实跑）

```
45-a：grep -rn "_points" crates/protocols/*/src/plugin.rs | wc -l → 0
45-e：cargo xtask qa gate config-refs → CONFIG-REFS-CF01 OK（active 13）
46-1：l2-{modbus-tcp,iec104,mqtt,mms,goose,sv}-verified.json ×6 全
      VERIFIED、commit 字段均绑 4f7344c
46-3：grep '10\.0\.2\.|127\.0\.0\.1' protocol-services/src → 5 处全在
      #[cfg(test)] 模块/文档注释（业务路径零硬编码）
```

## 复验新增修复（本轮）

1. **审计入链目标侧可观测**：engine `audit_append_count` + 串口行
   （红→绿测试先行；QEMU 实弹 appended=44 dropped=0）。
2. **串口日志入仓**：`git add -f evidence/47/qemu-write/qemu-serial.log`
   （45 卷先例同；上轮该文件被 ignore 规则遗漏未入库）。
3. 新测试 fmt 格式修正。

## 诚实登记（复验后完整清单）

- QEMU 断电重放（A/B 三态法）为扩展项：断电恢复语义由 durable_intent host
  撕裂测试覆盖；QEMU 侧 DURABLE-INTENT-STORE open OK 本轮串口 L11856 复现。
- modbus-rtu 出站写随 46 卷 BLOCKED（无第二 PL011）；645 类写 48 卷。
- EL0 审计链 MemStore（易失）：AUDIT 行证明入链事实；持久 AuditStore over
  BlockIo 登记后续卷。
- **PRODUCT-READY: false（degraded: svc-control-dispatch）在上轮与本轮串口
  均出现**：svc-control-dispatch 于 `_start` 内先做重初始化（config fetch/
  durable store open/delivery bind，均 EIPC 往返）后才经 control_service_main
  发就绪信号，超过 root READINESS_DEADLINE_MS=3000 → Degraded。服务实际完整
  启动并跑通写链（DURABLE-INTENT-STORE OK / Committed 均在 PRODUCT-READY 行
  之后）。就绪信号时机语义修正登记后续卷；非本卷 Exit 判据。
- QEMU 演示 rate_per_hour=100000 经 ACTIVE config 进签名 bundle（含 release
  profile）：代码缺省 60/h 为 fail-closed 面；产品部署须移除该键或设产品值
  （config 注释已声明）——部署纪律项。
- run47 脚本 rc 处理与 harness 现行为（超时=failure 收窗）不一致：本轮
  RUN_EXIT=2 但 300s 证据窗口完整，不影响证据效力。
