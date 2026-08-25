# WP-54.0 Entry 锚点重验留证（2026-08-25）

HEAD=12442fc（立卷提交后）。重验命令与原始输出见同目录 `wp54-entry-audit.txt` / `wp54-entry-eipc-outbox.txt` / `wp54-entry-wire-deadisland.txt` / `wp54-entry-followups.txt` / `wp54-entry-deadisland.txt`。

## 锚点判定

| 锚点 | 结果 | 说明 |
|---|---|---|
| audit 装配散点 | ✅ 无漂移 | `AuditLoggerImpl::new` 恰 13 处；生产种子四式位置全部命中（panic.rs:210/:34、ingress.rs:483、svc-control-dispatch.rs:354） |
| engine 双入口 | ✅ 无漂移 | run_intent:251 / run:463 / deliver_and_settle:624 逐字吻合 |
| 护栏测试 | ✅ | 三件在库 |
| AgentRuntime 归属 | ✅ | 消费者五 crate + 自身，与预期一致 |
| EIPC 授予两侧 | ⚠ 轻微漂移 | spawn.rs:1531/:1606 精确；dispatch 消费侧硬编码行号 293→**279**（88/234/274/**279**/322），新增注释命中 :125 |
| outbox 样板 | ⚠ 已确认 | market/cloud-sync 两份逐字同构确认；**第三份确认存在**：svc-observability.rs ~:70-96，已部分走 `MaybeOutbox` 形状（54.5 实施时按现状收编） |
| 线码三处 | ✅ 无漂移 | durable_intent.rs:124（真实路径 crates/protocols/control-dispatch/src/durable_intent.rs）；dispatch_service.rs state_code:69/state_from_code:86；裸字节 actor_kind!=2 于 :127/:193。`ActorKind::from_byte` 在 **crates/protocols**/telemetry-model/src/command.rs:**108** 精确命中；**「未被复用」说法失效**：control-gateway/envelope.rs:283 已复用一处 |
| 死岛依赖图 | ✅ 无漂移 | 循环脚本输出与预期逐项一致（milp←warm；p2p←p2p-match←∅；gguf-loader=model-deploy+xtask+fuzz；tracing=vpp-scenario 非零传递活链；intent-parser=intent-contract+dual-brain；online-learning 自身 manifest 为唯一命中=零外部消费者） |
| 双契约主线 | ✅ 无漂移 | remote_brain.rs:187 parse_and_validate |
| crc32 抄本 | ✅ 无漂移 | 恰 12 份，清单一致 |
| audit 持久化语义(A4) | ✅ 无漂移 | chain.rs:115 recover：空存储等价 new；MemStore/CrashStore 内存态确认 |
| trust 第三刻度 | ✅ 无漂移 | Silver/Gold/Platinum(400/700/900–1000) |

## 新发现（影响 54.1 范围）

**N1（范围扩张）**：svc-ai-worker.rs:228 与 :256 存在两处生产 `DecisionAudit::new([0x47;32])` 魔数种子（:227 注释自称与 dispatch 装配常量同源）。原四式清单只登记了 dispatch 一处。**54.1 genesis 统一必须一并覆盖 ai-worker**。

**N2（DoD 判据澄清）**：54.1 DoD#1 的 grep 模式按字面全仓执行会命中 crypto/rng/tpm 等合法零缓冲使用。判据按意图收窄为：审计链种子装配位（genesis 实参位、DecisionAudit::new 实参位）在生产 bin/lib 非测试代码中零魔数命中；命令以文件白名单形式落证据。

**N3（排除项核实）**：agent/capability/manager.rs:130 `CsRng::from_seed(&[0u8;32])` 位于 `#[cfg(any(test, feature="mock"))]` 门内（WP-53.6 收编产物 new_test_only_zero_seed），非生产面，不属本卷范围。

## 假设清单状态

A1 ✅ owner 开卷指令（改号登记在卷首）；A2 ✅ 附录 A 七项+一项未入实施范围；A3 ✅ root-task Cargo.toml:39；A4 ✅（上表）；A5 ✅ D1 无撤销记录。
