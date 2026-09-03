# EnerOS 63 候选登记总册（candidates-registry）

> 立册：蓝图 63 卷 63.3 WP-63.3.5（2026-09-03）。四列：ID / 锚点（rg 可验，R-63.f）/ 状态（Open｜Done｜Blocked｜Deferred）/ 登记处。
> 唯一状态面：本文件收编蓝图 62 `00-总索引 §4`、`执行卡-62.3 §H`、`tools/run62-vol3-gate.sh` 尾注三处散布登记；在册项只标注状态不删除。
> 维护纪律：每行锚点写完逐行 rg 自检；P2 失锚教训（62 §3.1）不再犯——锚点失验即标"锚点失验-需重查"。

## A. 蓝图 62 §4（原 63 候选）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-W2 | `crates/agents/agent-sdk/src/`（MQTT 插件订阅/收件箱样板——`pub mod` 面与 protocol_ladder 平行结构） | Open | 蓝图 62 00-总索引 §4 |
| REG-W4 | `crates/protocols/protocol-services/src/service_cfg.rs`（GET_ENTRY `"protocol-<proto>"` 键消费面）+ `crates/runtime/config-bundle/src/toml_compile.rs`（`compile_protocol` 编译面）+ 逐协议 `config.rs`（agent-bus-dds/cj188/dlt645/gbt27930/iec101/iec103/iec104-master/iec104-slave）——62 原锚 glob `protocol-*/src/config*.rs` 仅解析 protocol-abstract 一处，2026-09-03 独立复查重锚 | Open | 蓝图 62 00-总索引 §4 |
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
| REG-HEX16 | `crates/protocols/protocol-services/src/bin/svc-modbus-tcp.rs:443`（hex16_into 定义点；62 体检"四抄"现锚仅 1 文件——需跨 bin 复查抄本） | Speculative | 蓝图 63 63.3.5 |
| REG-TELEMETRY-SPLIT | `crates/protocols/telemetry-model/src/lib.rs`（拆分候选——单 lib 面过大） | Open | 蓝图 63 63.3.5 |
| REG-ROOT-RECEIPT | root `control_chain_tick` 裸索引 `tick_resp[2]`（ReceiptView 未实施——63.1 复查订正移交；候选落点 = 回执 wire 解码收进 dispatch-state） | Open | 执行卡-63.1 §I |

## D. 自检记录（R-63.f）

- 立册时逐行 rg 自检：A/B/C1 全部锚点命中；C2 采用 metadata 反向依赖判定（rg 只验 crate 存在性）。
- 失锚处理：REG-P2（VoteTally）原锚失验已重锚并标注"锚点仍需逐臂重查"。
- 2026-09-03 独立复查重锚 ×2：REG-W4（原 glob 仅解析 1 文件，与其声称面失配——重锚至键串消费/编译双源 + 8 协议 crate config.rs）；REG-SM2-SB（行号锚 :106 为注释位、直调在 :109——去行号改符号锚，并闭环代码注释旧 ID SB-SM2-EXEMPT 与本行 ID 的对照关系）。
