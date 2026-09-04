# EnerOS 63 候选登记总册（candidates-registry）

> 立册：蓝图 63 卷 63.3 WP-63.3.5（2026-09-03）。四列：ID / 锚点（rg 可验，R-63.f）/ 状态（Open｜Done｜Blocked｜Deferred）/ 登记处。
> 唯一状态面：本文件收编蓝图 62 `00-总索引 §4`、`执行卡-62.3 §H`、`tools/run62-vol3-gate.sh` 尾注三处散布登记；在册项只标注状态不删除。
> 维护纪律：每行锚点写完逐行 rg 自检；P2 失锚教训（62 §3.1）不再犯——锚点失验即标"锚点失验-需重查"。

## A. 蓝图 62 §4（原 63 候选）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-W2 | `crates/agents/agent-sdk/src/`（MQTT 插件订阅/收件箱样板——`pub mod` 面与 protocol_ladder 平行结构） | Open | 蓝图 62 00-总索引 §4 |
| REG-W4 | `crates/protocols/protocol-services/src/service_cfg.rs`（GET_ENTRY `"protocol-<proto>"` 键消费面）+ `crates/runtime/config-bundle/src/toml_compile.rs`（`compile_protocol` 编译面）+ 逐协议 `config.rs`（agent-bus-dds/cj188/dlt645/gbt27930/iec101/iec103/iec104-master/iec104-slave）——62 原锚 glob `protocol-*/src/config*.rs` 仅解析 protocol-abstract 一处，2026-09-03 独立复查重锚 | **Done（蓝图 64 卷 64.1：键表单源 eneros-protocol-keys 双面消费，goose vlan_id 活缺陷同卷修复）** | 蓝图 62 00-总索引 §4 |
| REG-W5 | `crates/ai/dual-brain/src/tick_policy.rs`（超时降级状态机；ADR-030）——62 原锚 agent-sdk 失验，立册时重锚 | Open | 蓝图 62 00-总索引 §4 |
| REG-P1 | `crates/protocols/protocol-services/src/protocol_ladder.rs:run_capability_shell` + `ingestion_stage.rs:build_ingested_service` | **Done（63.2.4）** | 蓝图 62 00-总索引 §4 |
| REG-P2 | `crates/runtime/scale/src/election.rs`（VoteTally 双实现收敛；**62 原锚 "VoteTally" 全仓 rg 零命中——本行重锚至 scale/election.rs quorum 面，锚点仍需逐臂重查**） | Open（重锚） | 蓝图 62 00-总索引 §4 |
| REG-P3 | 插件健康记账四件套（`PluginIoService`/health 统计面——protocol-services service.rs） | Open | 蓝图 62 00-总索引 §4 |
| REG-PROF | product profile 驱动 root-task 接线（root 读 profile 通路 + 内存预算前置） | Open | 蓝图 62 00-总索引 §4 |

## B. 蓝图 62.3 §H（执行卡登记）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-SM2 | `crates/agents/federation/src/{offer,pbft,tunnel}.rs` + model-security/did/config-bundle（sm2_verify 直调点） | **Done（63.3.2 收口 6 处）** | 执行卡-62.3 §H.1 |
| REG-SM2-SB | `crates/security/secure-boot/src/manifest.rs`（`sm2_verify` 直调 `Ok(true)` match；代码内豁免注释登记，注释中旧 ID "SB-SM2-EXEMPT" 即本行——2026-09-03 复查去行号锚：原 :106 实为注释位，直调在 :109） | Deferred（ADR-033 分层豁免，代码内登记注释） | 执行卡-62.3 §H.1 |
| REG-GRANT-EDGE | `crates/agents/agent-sdk/src/eipc_grants.rs:EIPC_GRANTS`（svc-iec104→svc-telemetry 边） | Blocked（调查实证：激活即回归——iec104 事件 epoch=本机 now_ms 毒化评估器，evidence/63/vol63-2/investigation-*；前置=epoch 语义对齐；锁测试 wp6323_iec104_telemetry_edge_stays_ungranted 防误接通） | 执行卡-62.3 §H.2 |

## C. 蓝图 63 本轮新增

### C1. 四机制存废裁决表（63.3.3——机制/现状证据/裁决/激活前置/登记 ID）

| 机制 | 现状证据（rg 可验） | 裁决 | 激活前置 | 登记 ID |
|---|---|---|---|---|
| SnapshotModule | `SnapshotModule::build` 唯一 crate 外调用者 = `crates/agents/control-services/src/snapshot_diff.rs`（整体 `#[cfg(test)]`） | 冻结登记（数据类型面 TrustedSnapshot/NormalizedObservation/CompletenessReport 保持 pub 真实消费；机制面零生产） | svc-snapshot 独立服务接线 + 存储句柄移交（BLOCKED） | SM-deferred |
| StoreFactEventSink | `crates/agents/business-services/src/store_fact_sink.rs`（crate 外零引用） | 冻结登记 | 目标侧存储句柄移交（BLOCKED） | SFS-deferred |
| StoreSink 桥 | `crates/runtime/event-store/src/ingest_bridge.rs`（仅 tests/ingest_bridge.rs 消费；生产采集 bin 全走 InMemorySink） | 冻结登记 | 目标侧存储句柄移交（BLOCKED） | SS-deferred |
| ModelHarness | `crates/ai/model-harness/`（全仓 Cargo.toml 零依赖者） | 冻结登记 | DecisionBackend 替换独立评估（00 §4） | MH-deferred |

### C2. 37 孤儿 crate 标注清单（63.3.5；判定口径=cargo metadata 反向依赖 2026-09-03 实测；"零反向依赖 lib"55 个，下表按三类收敛——**删除归后续卷逐个裁决，本蓝图不动**）

- **在用（非违规）**：`crates/agents/certifier`（ADR-0051 feature gate 守护）、`crates/agents/federation`（svc-federation bin + pentest 半连接）、`eneros-business-services`/`eneros-did`/`eneros-model-harness`/`eneros-ai-runner`/`eneros-oom-probe`/`eneros-installer-uefi`/`eneros-svc-pie`/`eneros-mvp-scenario`（bin/测试/探针装配体，profile 或验证面直接消费）。
- **Deferred（留置，无现役接入计划）**：aiops、alarm、ancillary、app-cert、autonomous、bsp-cert、calibration、cellular、chain、cj188、cloud-coordinator、compliance、data-governance、dialog、fault-recorder、firmware-agent、fleet-ops、hmi、iec62351、marl、mgmt-cli、modbus-tcp-server、model-ota、online-learning、ops-gate、ops-lifecycle、p2p-match、power、protocol-sdk、realtime-market、reputation、rl、safety-catalog、scale、simulator、soe-engine、solver-pareto、solver-warm、spot-market、standards、support-bundle、trust、twin-agent、twin-global、vpp-scenario、agent-hot-update（锚点：`rg -l "name = \"eneros-<crate>\"" crates/*/Cargo.toml` 逐个可验；复验口径 = `cargo metadata` 反向依赖为零）。
- **待裁决**：`eneros-root-task`/`eneros-xtask`/`eneros-ci`（根装配体非孤儿，排除口径登记于此防复课）。

### C3. 本轮新增登记

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-1664B | `crates/agents/agent-sdk/src/obs_batch_wire.rs`（1664B 字面量 3 处：doc+注释×2——上限算式已 const 单源，字面仅注释面） | Speculative（注释面残留，非双源） | 蓝图 63 63.3.5 |
| REG-HEX16 | `crates/protocols/protocol-services/src/bin/svc-modbus-tcp.rs:443`（hex16_into 定义点；62 体检"四抄"现锚仅 1 文件——需跨 bin 复查抄本） | **Done（蓝图 64 卷 64.3 WP-64.3.4：eneros-hex 叶 crate 单源 + 域守卫，hex16_into 定义点已删——实锚扩至 13+ 抄本全归一）** | 蓝图 63 63.3.5 |
| REG-TELEMETRY-SPLIT | `crates/protocols/telemetry-model/src/lib.rs`（拆分候选——单 lib 面过大） | Open | 蓝图 63 63.3.5 |
| REG-ROOT-RECEIPT | root `control_chain_tick` 裸索引 `tick_resp[2]`（ReceiptView 未实施——63.1 复查订正移交；候选落点 = 回执 wire 解码收进 dispatch-state） | Open（2026-09-04 64.3 开工核实：`crates/runtime/root-task/src/service.rs:1495` `Some(tick_resp[2])` 仍在——64.2 root 四线未触及本面，维持 Open） | 执行卡-63.1 §I |

## D. 自检记录（R-63.f）

- 立册时逐行 rg 自检：A/B/C1 全部锚点命中；C2 采用 metadata 反向依赖判定（rg 只验 crate 存在性）。
- 失锚处理：REG-P2（VoteTally）原锚失验已重锚并标注"锚点仍需逐臂重查"。
- 2026-09-03 独立复查重锚 ×2：REG-W4（原 glob 仅解析 1 文件，与其声称面失配——重锚至键串消费/编译双源 + 8 协议 crate config.rs）；REG-SM2-SB（行号锚 :106 为注释位、直调在 :109——去行号改符号锚，并闭环代码注释旧 ID SB-SM2-EXEMPT 与本行 ID 的对照关系）。

## E. 蓝图 64 本轮登记（R-64.f，2026-09-04）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-ELECTION | `crates/runtime/scale/src/election.rs`（quorum 公式/计票语义/4 错误变体双实现 + `now_ms=0` 假时钟基线——第八轮 REG-P2 重锚评估实锤） | Deferred（scale crate 在 62 登记册 C2 Deferred 名单——无现役接入计划，随激活卷执行） | 蓝图 64 00-总索引 §4 |
| REG-ARBITER-TRIM | `crates/protocols/control-dispatch/src/arbiter.rs`（裁剪请求结构 / 唯一生产 adapter 恒供常量——蓝图 61 建制有意预留："一个 adapter=假想 seam"活教材） | Speculative（等首个真实变化轴出现再裁决） | 蓝图 64 00-总索引 §4 |
| REG-RTU-CFG-FACE | `crates/protocols/protocol-services/src/bin/svc-modbus-rtu.rs` config 面闭包（:77-88/:142-149） | **Done（64.2.3：核实适用 → 顺手收编 `ChunkedEndpoint::bind`）** | 蓝图 64 00-总索引 §4 |
| REG-CJ188-COMPILE | `crates/protocols/cj188/src/config.rs`（键表已进叶 crate；编译支持面扩容=行为新增） | Open（`known_keys("protocol-cj188")` 维持 `None`——独立裁决） | 蓝图 64 00-总索引 §4 |
| REG-POINTS-LOADED | modbus ingest bin POINTS-LOADED 计数变更记录：每轮 2→1 行（64.2.3 二次 fetch 删除；行词形不变；run64-vol2/vol3 gate 判据对 63 基线按此校准） | **Done（登记性变更，64.2 已落库）** | 蓝图 64 [03] WP-64.2.3 |
| REG-CI-KEY-DUAL | `.github/workflows/ci.yml:549`（dev root key 字面与 gate-env.sh 双源——workflow 无法 source shell 函数，环境不同） | Deferred（release-key 三元组双源维持既有登记） | 蓝图 64 [04] WP-64.3.6 |

### 蓝图 64 执行期事实差异登记（64.3）

1. `recovery/src/flash.rs:114` 同型 `sm2_verify().unwrap_or(false)` 布尔陷阱——[01] "ota-ab 为 security 面最后一处"失实，64.3.5 一并收口（行为零变化，fail-closed 同型）。
2. gate 前导 `5a%.0s` 实际 14 个 tools 脚本（[01] 记 12——64.1/64.2 卷新增 run64-vol1/vol2-gate.sh 各 1）。
3. DispatchConfig 实为 22 字段（卷册写 17——蓝图 61 增字段漂移）；PlanInput 13 字段（卷册写 14）；`Assignment.asset_id` u64（卷册 WP-64.3.1.4 草图 u16）。
4. DuplicateStep 锚点真身 `control-dispatch/src/compiler.rs:283`（卷册写 objective-engine compiler.rs:281-288 笔误）；mgmt-api 实路径 `crates/observability/mgmt-api/`（[01] 写 agents/ 笔误）。
5. energy 窗两体非逐字同（`now` vs `started`、`plan.ttl_ms()` vs `self.plan_ttl_ms`、`.or(Some)` vs `Some`）——带参私有函数形态（卷册草图 fn(intent, now, ttl) 预判正确）；返回值收敛为 (window_secs, energy_kwh) 二元组（window_h 无独立消费方，登记偏差）。
6. CompiledObjective 测试缝以 `test-util` feature 门落地（`#[cfg(test)]` 不跨 crate 边界——卷册草图执行时调整）。
7. 负例两脚本（run-a-neg-final/run-a-negative-nokey）image 失败退出码 4→3（gate_image_raw 统一 exit 3；4e8a8a0 提交信息声称"漂移登记"当时未落盘，本条补记）——负例原约定 2=compile 失败 / 3=判据违规（UNEXPECTED: security present）/ 4=image 失败，现 image 失败与判据违规同为 3（stdout 词形仍可区分；全仓零自动化退出码消费方）。
8. eneros-hex 守卫面边界：FORBIDDEN 仅锁八类命名 fn 签名——内联 `format!("{:02x}")` 形态生产抄本不在守卫面（svc-federation dump-pubkey :981、model-harness projector `hex16` :263、crypto bigint `U256::to_hex`/`hex_digit`、data-governance `sm3_hex`、ops-lifecycle `integrity_hex`、crash-dump `to_hex`、ai/runner ×2、config-bundle `Display for Digest`）；lib.rs "零抄本"声明已收窄为"零禁写签名"。候选后续卷归一（每处需新增依赖边，涉 target 闭包须随卷评估）。REG-HEX-BEIDOU-CONST 豁免理由订正：const 限定非必需（唯一调用者 verify_checksum 为运行时 fn），真因=按帧内偏移取单 nibble 判据，非整串解码抄本。
9. federation hex 错误路径重构实为内部 Result 化：`decode_hex_arg` 全文件唯一消费者是 `hex_decode_or_exit`（eprintln + exit(2)）——stderr 词形/退出码/可观察行为与原内联实现零变化，[64.3] 提交信息"退进程→Err 登记变更"表述失实订正；svc-federation.rs 注释原指向执行卡（不在仓内），改指本登记册。
10. **model-security `add_trusted_key` 撤销终态回归已修**（64 全卷三轮复查，2026-09-04）：keyring 化引入行为不等价——`revoke_key` 后再 `add_trusted_key` 经 `register`（重置撤销位；keyring 自测 `reregister_clears_revocation_and_overwrites` 锁定的登记语义）使 verify 由 keyring 化前 `Err(KeyRevoked)` 变 `Ok(())`（fail-open；旧 revoked 列表只增不减+吊销门最先=终态）。修复=adapter 以 keyring `is_revoked` 读面在 register 后恢复撤销位（keyring 零改动，其登记语义不动）；SIG12 回归锁+变异实证（去恢复三行即红）。全仓无 revoke→re-add 调用方（在野不可触发，API 级回归）；执行卡 §10 "三门优先级逐 case 等价"的漏网 case 补记。
11. **eneros-hex rg 守卫 const/泛型签名假绿已修**（同轮）：FORBIDDEN 带左括号字面匹配漏 `fn hex_decode<const N: usize>(`——恰为 recovery 被删抄本原形，"任一替换点改回本地抄本→守卫红"对该点不成立；改两态匹配（名后 `(` / `<`，带 `fn ` 前缀防 `hex_decoder` 误伤）+ 谓词单测锁。守卫面边界补充：xtask 6 处生产 hex fn（flasher.rs/evidence_chain.rs/qa.rs ×2/catalog.rs/config_compile.rs）在 crates/ walk 外；tsn-time clock.rs ClockIdentity/MacAddr Display（冒号 MAC）为 E.8 未列内联形态——并入 E.8 候选后续卷。
12. CI feature 面缺口：R-64.a 红测（goose vlan_id/mqtt qos 编译面）与插件侧引用锁 3 道（goose/sv/mms，`net` feature 门=64.1 前既有）分别藏于 config-bundle `compile` / 各插件 `net` feature——`cargo test --workspace`（ci.yml:347 默认 feature）不触达，行为级回归证明仅卷出口 gate 手工实跑在案。建议 CI 增 feature-face 步骤（ci.yml 属 REG-CI-KEY-DUAL 同域登记区）。
13. mqtt `qos`/`will_topic` 反向漂移：插件键表声明（13 键）含两键但插件面无 `get("qos")`/`get("will_topic")` 消费点（plugin.rs:67 自认"已声明可选键"）——pre-64.1 既有，叶 crate 忠实镜像；消费接线候选后续卷。
14. pentest attack_11 注释门序失实（预存，63.3 时代非 64 回归）：`model_guard.check` 白名单门（strict 模式）先于信任钥门短路，"白名单与信任密钥双重不命中"表述不准——拒绝行为正确仅注释失实；trust-key 门负向触达在 tests/model_guard.rs INT17。
15. gate-env.sh 前导注释订正（"标准脚本一行调用"→实际仅 run62-vol1；其余分相/no-key 直调两函数）——64.3.6 复查遗留。gate 脚本三轮复查其余结论：14 脚本中段/判据面/退出码/钥语义（含 no-key fail-closed）与 f1b70ea 基线等价，run61-phase1/2 腰斩恢复属实（中段逐字节一致）；收口候选=双重清场 ×4 脚本（幂等无害）/run63-vol3 前导未迁移/头注宜警示"加装 set -e 将使函数内 exit 2/3 失败路径失效"。

## F. 蓝图 65 本轮登记（R-65.f，2026-09-05）

### F.1 不做项对账（蓝图 65 [00] §4 逐条，本卷禁止顺手做的裁决维持）

| 项 | 状态 | 说明 |
|---|---|---|
| objective_digest 承重化（零门/证书字段/certifier bind 第 7 配对项） | **Deferred**（变异与读码三重实证推迟，[01] §2.F） | certifier 生产零调用方；`CertifiedPlanFields` 无该字段（certificate.rs:94-107）；adapter 路径 spec-less + journal 冻结。承重化正确时机 = certifier 生产接线卷；本卷 combo 链测已证 plan.objective_digest ≡ 真编译 spec_digest（[02] WP-65.2.5） |
| master_write_v2 入 lib（六分支 host 测）+ Orchestration step 化 | Deferred → **蓝图 66 候选** | 测试性轴，与不变式归位不同卷（[00] §4） |
| bind_shared Result 化、TEL_INGEST 应答双源、ai_request_topic 收拢、intent-request wire 三抄、ObsReject 消费/死返回清理 | Deferred → **蓝图 66 候选** | 本卷不动 protocol-services/root-task wire 面（DBG-EXEC 与 EA_OK 谓词两粒卫生除外，已收口） |
| CommandBatchBuilder compile_meta 组位置化保留 | Done（保留裁决） | 四参全异型，互换不可编译；仅删第二免检构造门（WP-65.2.3） |
| KNOWN_KEYS 22 道点锁退役 | Done（保留裁决） | 类锁生效后点锁退化为廉价接线检查（WP-65.4，零成本保留） |
| REG-TELEMETRY-SPLIT | **建议关闭**（深挖实证前提过期） | telemetry-model lib.rs 463 行 = 12 子模块薄 facade（command/deadband/digital/domain/event/point_spec/quality/teleadjust/telecontrol/telemetry/telesignaling/unit + re-export）——拆分无对象 |
| REG-ROOT-RECEIPT（tick_resp[2] 回执解码） | Open（维持） | EA_OK 谓词一行已收口（WP-65.5.3）；回执解码本体归其执行卷 |
| REG-CJ188-COMPILE | Open（维持） | required 进叶后 cj188 行为逐字节不变（REQUIRED_CJ188 空集显式锁 + 叶/委托双面测试）；编译面仍 None |
| 孤儿 crate 清剿 / GA 红门任何项 | Deferred（维持 63 §4 裁决） | GA 归蓝图 61 [13] |
| election/quorum、CommandArbiter 裁剪结构、W2/W5/P3/PROF、direct-drive 入表 | Deferred（维持 64 §4 原裁决） | — |

### F.2 REG-SM2 站点台账（蓝图 65 WP-65.3，守卫加宽后全仓 sm2_verify 站点逐站处置）

| 站点 | 处置 | 理由 |
|---|---|---|
| security/crypto、crypto-gmssl、signer-keyring、secure-boot | EXEMPT_CRATES（原有） | 原始域 + ADR-033 登记豁免（REG-SM2-SB） |
| business-services market.rs:280 | 迁移 | 双通道已统一词形；match 三臂 → verify_signature，"signature" 逐字节不变 |
| config-bundle tests/bundle.rs:499 | 迁移 | 测试诊断面；"entry verify FAILED" 词形不变 |
| model-ota signature.rs:36-41 | 迁移 | 三臂塌缩一行；SignatureInvalid 不变（孤儿 crate，零 target 风险） |
| chain types.rs:58 | 迁移 | is_ok ≡ unwrap_or(false)，布尔返回面逐字节等价（孤儿） |
| app-cert types.rs:67 | 迁移 | 同上（孤儿） |
| mgmt-api auth.rs:114-119 | 迁移 | 双通道已统一 SignatureInvalid（闭包身份：经 business-services 路径在闭包；signer-keyring 已在闭包，闭包新增 crate = 0） |
| agent capability/token.rs:412 | 迁移 | 三臂塌缩；TokenSignatureInvalid 不变（R-65.c：闭包新增 crate = 0，signer-keyring 仅依赖 crypto——agent 已依赖；bin 增重 = 出口门内存审计留证） |
| **business-services config_service.rs:625** | **EXEMPT_FILES** | Ok(false)/Err 双通道已**分词形**（"bad-entry-signature" vs "sm2-verify-failed"）——类型化原语统一 BadSignature 即改词形（[02] 禁止项：拒绝词形逐字节不变）。待词形归一卷收口 。**追认注记（code-review D-12）**：[02] 改动 2 原列「直接迁」与其禁止项矛盾，实况按硬规则（禁止项）豁免，待 owner 追认 |
| **agent process.rs:240** | **EXEMPT_FILES** | Err 通道错误传输面 "SM2 verify failed: {e:?}"（内层 e 文案）+ Ok(false) 经布尔返回面二次分流——类型化原语统一即改消费面词形。待错误传输面重构卷收口 |

### F.3 蓝图 66 候选清单（R-65.f 登记，蓝图 65 不做项移交）

1. master_write_v2 入 lib（六分支 host 测）。
2. Orchestration step 化（与 1 同轴天然一对）。
3. bind_shared Result 化。
4. TEL_INGEST 应答双源收拢。
5. ai_request_topic 收拢。
6. intent-request wire 三抄。
7. REG-SM2 EXEMPT_FILES 两站收口（config_service 词形归一卷 / process 错误传输面重构卷——各自独立裁决）。
8. E.8/E.11 遗留：内联 hex 抄本归一候选（每处需新增依赖边，随卷评估）。
9. mqtt `qos`/`will_topic` 插件面消费接线（E.13）。

### F.4 本卷完成项状态更新

- E.10（model-security 撤销终态 adapter 舞步）→ **Done（蓝图 65 WP-65.1）**：不变式归位 keyring tombstone 单一持有；adapter 舞步删除；SIG 面逐 case 等价（执行卡对照表）；SIG12 保留为消费面对照锁。
- REG-SM2 守卫半径（蓝图 64 遗留「布尔陷阱域外清零」失实）→ **Done（蓝图 65 WP-65.3）**：全仓 walk + EXEMPT_FILES 机制 + 本节台账。
