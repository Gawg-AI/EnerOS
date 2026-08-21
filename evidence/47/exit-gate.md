# 47 卷 Exit Gate 自证

> 日期：2026-08-22 · 版本：0.313.23 · 评审 HEAD：见 commit（exit-gate.json）
> 状态：**Accepted**——Exit 各项自证 PASS + 诚实登记（负例/断电以 host 层
> 测试 + QEMU 观察为证据；QEMU 断电 A/B 跑测为扩展后续项）。

## Exit Gate 项自证

### 项 1：QEMU 实弹——外部主站下发设点 → 授权 → 协议出站 → 从站值变化 → readback → Committed + 审计入链（端到端串口 + 对端 transcript 双证据）

证据文件：`evidence/47/qemu-write/{qemu-serial.log, modbus.transcript.jsonl, mqtt.transcript.jsonl}`

串口（qemu-serial.log，独立对端 = 非 EnerOS python 进程）：

```
[svc-mqtt] SECURITY-CHECK mode=QemuTest profile=Tls12 factory=false assembly=allowed（显式明文）
[control-bindings] POINTS-LOADED 20 points — CONTROL-BINDING device=1 point=10
[svc-control-dispatch] RATE-GATE quota=100000/h（产品默认 60/h）
[svc-control-dispatch] DURABLE-INTENT-STORE open OK（AppendOutbox over svc-storage）
[svc-mqtt] INTENT-FWD protocol=mqtt device=1 point=10 power=Float(50.0) seq=1 state=8 total=1
[svc-modbus-tcp] MASTER-WRITE device=1 point=10 power=50 ACK
[svc-modbus-tcp] MASTER-READBACK point=10 eng=50 target=50 VERIFIED
[svc-control-dispatch] INTENT state=Committed note=None
```

对端 transcript（mqtt_broker47 + modbus_slave47，独立进程）：

```
mqtt: SUBSCRIBE eneros/control + FORWARD "SET 1 10 50.0"（SCADA 下行写到达）
modbus: WRITE fc06 addr=0x200 value=50 + READ fc03 addr=0x200 value=50（从站值变化 + 独立读回）
```

审计：控制事件经 `control_audit`（SHA-256 链）——宿主面 `audit_chain.rs`
verify_log PASS + last_valid_prefix 定位（WP-47.4 证据）；QEMU 侧 dispatching
控制事件路径 serial 可见（INTENT state=Committed）。

### 项 2：负例矩阵

| 负例 | 证据 |
|---|---|
| 未授权写 deny | `intent_ingress_chain.rs`：绑定不符/策略 deny → NotAuthorized 零投递（WP-47.1） |
| 读回不符 Failed | `intent_readback_mismatch_is_failed`（OutcomeFrame=5 → Failed）；审计 failed_event 链校验 |
| 超时 Indeterminate | `intent_delivery_timeout_is_indeterminate`（投递超时 → Indeterminate 不记账） |
| 断电恢复 | `durable_intent.rs`：`power_loss_tear_recovers_intact_pending`（SharedMemIo 撕裂→open 恢复完整意图）+ `pending_intents_recovers_after_restart`（Persisted/Indeterminate 恢复）+ `committed_after_restart_not_pending` |
| 重放拒绝 | `intent_replay_same_sequence_zero_write`（重复 sequence → Rejected 零写）+ 审计 `last_valid_prefix` 截断/篡改定位 |
| rate 预算 | `intent_rate_gate_exceeds_budget`（超配额 BudgetExceeded 零投递）|
| （QEMU 观察）| 早期运行记录 intent 被拒（BadResponse/BudgetExceeded）→ 实弹修复 7 根因后全通（见 exit-gate.json fixes） |

### 项 3：grep 零命中 + release 无键 fail-closed

```
grep -rn "w40-prod-key\|dev-signing.key" crates/ xtask/ product/ --include *.rs *.sh *.toml
  → 0（生成脚本 tools/gen-local-keys.sh 与 keys-local 说明文本豁免）
cargo xtask qa gate keys → KEYS-GATE tracked-keys=0 / default-refs=0 PASS
release 无键构建 fail-closed：keys::resolve_signing_key(Production) 单测 +
  image qemu-prod 无 ENEROS_IMAGE_SIGNING_KEY 即失败（本卷构建实弹观察）
```

### 项 4：全量测试/CI 绿 + 45/46 抽查 + 版本

```
cargo test --workspace → EXIT=0，375 test-result ok 块，0 失败
cargo clippy --workspace --all-targets → 0 error / 0 warning
cargo fmt --all --check → 0 diff
cargo xtask closure gate --profile qemu-prod → CLOSURE-GATE PASS（crypto_violations=0）
45 抽查：_points grep=0（Exit a）；config-refs CF01 OK（Exit e）
46 抽查：l2-verified ×6 结构/绑定 4f7344c（Exit 1）；peer 硬编码零业务命中（Exit 3）；
      workspace 测试（Exit 4 同套件）
版本 0.313.22 → 0.313.23
```

## 实弹修复（47 QEMU 收尾，fixes_applied）

1. **drv-svc 自定义 opcode 不路由**：OP_MASTER_WRITE=0x11 被 process_request
   归 ST_INVALID——出站写改 **OP_EXECUTE + 载荷子码**（EipcMasterChannel 与
   svc-modbus-tcp 同源）。
2. **drv-svc 响应 79B≠80B padding**：服务端按 4B/word 回 MR（80B），
   decode_response 要求精确 79B——通道接受 ≥79B 尾部 padding（block_io.rs 同款
   已知坑，已登记记忆）。
3. **mqtt control_topic 订阅前连接失败**：subscribe 延后到首次 poll connect
   之后（pending_subscribe 惰性订阅）。
4. **svc-mqtt 无 post-boot 轮询**：root 增加协议服务 EXECUTE 编排 tick
   （modbus ACQ 保鲜 + mqtt 下行轮询；同 tick 先 modbus 后 mqtt 使 intent
   SNAPSHOT 与 ACQ 相隔毫秒）。
5. **rate gate 60/h 演示撞限**：control-write 配置 `rate_per_hour`（bundle
   注入；缺省=产品 60/h）——qemu-test 演示值 100000，源码零测试值。
6. **storage EIPC epoch 吸收**：dispatch 的 eipc_block_io 传 0（首个响应
   吸收 svc-storage epoch）。
7. **点表/协议配置 bundle 陈旧**：config compile 重建（point 10 + control_topic
   生效）。

## 诚实登记

- QEMU 断电重放（A/B powercut 三态法）为扩展项：写入断电状态恢复语义由
  durable_intent host 测试（SharedMemIo tear）覆盖；QEMU 侧 DURABLE-INTENT-STORE
  open OK（svc-storage 真落盘）已证。
- modbus-rtu 出站写随 46 卷 BLOCKED（无第二 PL011）；645 类写 48 卷。
- EL0 侧审计链为 MemStore（易失）；持久 AuditStore over BlockIo 登记后续。