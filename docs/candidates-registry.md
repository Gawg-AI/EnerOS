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
| REG-GRANT-EDGE | `crates/agents/agent-sdk/src/eipc_grants.rs:EIPC_GRANTS`（svc-iec104→svc-telemetry 边） | **Done（蓝图 68 卷 68.2 WP-68.2.3 接通）**：原"激活即回归"根因经 68 卷 F13 复核失准（"适配器 now_ms 冒充 epoch"不成立；真根因 = 每服务 open() floor 各取 boot now_ms 必不相等 + 消费侧评估器单全局 epoch"最后钉扎者赢"，evidence/63/vol63-2/investigation-*:5454,13765,13819）；68.2 消费侧 per-source 归位（SourceEpochTable + 钉扎门 restore 同步删除，t1/t2/t4 行为锁）修根因后前提消灭，边追加于 iec104 config 边后（Z6 序冻结 s1 守），锁测试翻转 wp682_iec104_telemetry_edge_granted_and_pinned | 执行卡-62.3 §H.2 → 蓝图 68 卷 68.2 |

## C. 蓝图 63 本轮新增

### C1. 四机制存废裁决表（63.3.3——机制/现状证据/裁决/激活前置/登记 ID）

| 机制 | 现状证据（rg 可验） | 裁决 | 激活前置 | 登记 ID |
|---|---|---|---|---|
| SnapshotModule | 生产入口 = `crates/agents/control-services/src/event_face.rs`（svc-telemetry 快照面：catalog 惰性装载 + 8 批窗口构建 + blob 落 DATA_SNAP_* LFS 卷；certifier 生产消费仍未接线——可达性已验证 wp6844 host 冒烟） | **Done（蓝图 68 卷 68.4 WP-68.4.2 解冻）**：解冻条件"68.3 块面+68.4 接线完成"成立（Z8） | certifier 生产消费归 68.5 后接电卷 | SM-deferred |
| StoreFactEventSink | 生产装配 = `crates/agents/business-services/src/bin/svc-market.rs`（`fact_face::open_fact_face` → DATA_FACT_* 区段 Q0 写透 RPO=0；上游事实 transport 未部署 ⇒ sink 在位零消费，跨 bin 接线登记后续） | **Done（蓝图 68 卷 68.4 WP-68.4.1 解冻）**（Z8） | 事实 transport 到位即写（svc-market） | SFS-deferred |
| StoreSink 桥 | 生产装配 = `crates/agents/control-services/src/event_face.rs`（svc-telemetry OBS 事件面：接受集逐事件 Q0 追加 + SnapshotModule 投影；`store_mut` 访问器补 68.4 保留策略调用方职责通道；drain/交付面生产消费者仍未接线） | **Done（蓝图 68 卷 68.4 WP-68.4.1 解冻）**（Z8） | 交付面消费者归后续卷（位点纪律+OffsetGone 语义见 trait 文档） | SS-deferred |
| ModelHarness | `crates/ai/model-harness/`（全仓 Cargo.toml 零依赖者） | 冻结登记 | DecisionBackend 替换独立评估（00 §4） | MH-deferred |

### C2. 37 孤儿 crate 标注清单（63.3.5；判定口径=cargo metadata 反向依赖 2026-09-03 实测；"零反向依赖 lib"55 个，下表按三类收敛——**删除归后续卷逐个裁决，本蓝图不动**）

- **在用（非违规）**：`crates/agents/certifier`（ADR-0051 feature gate 守护）、`crates/agents/federation`（svc-federation bin + pentest 半连接）、`eneros-business-services`/`eneros-did`/`eneros-model-harness`/`eneros-ai-runner`/`eneros-oom-probe`/`eneros-installer-uefi`/`eneros-svc-pie`/`eneros-mvp-scenario`（bin/测试/探针装配体，profile 或验证面直接消费）。
- **Deferred（留置，无现役接入计划）**：aiops、alarm、ancillary、app-cert、autonomous、bsp-cert、calibration、cellular、chain、cj188、cloud-coordinator、compliance、data-governance、dialog、fault-recorder、firmware-agent、fleet-ops、hmi、iec62351、marl、mgmt-cli、modbus-tcp-server、model-ota、online-learning、ops-gate、ops-lifecycle、p2p-match、power、protocol-sdk、realtime-market、reputation、rl、safety-catalog、scale、simulator、soe-engine、solver-pareto、solver-warm、spot-market、standards、support-bundle、trust、twin-agent、twin-global、vpp-scenario、agent-hot-update（锚点：`rg -l "name = \"eneros-<crate>\"" crates/*/Cargo.toml` 逐个可验；复验口径 = `cargo metadata` 反向依赖为零）。
- **待裁决**：`eneros-root-task`/`eneros-xtask`/`eneros-ci`（根装配体非孤儿，排除口径登记于此防复课）。
- **退役（蓝图 67 WP-67.1.4，2026-09-05）**：`eneros-ci` crate 已删——version/mock/sign-key 门 owner 归 xtask，sign-keys 门进 CI。

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

## G. 蓝图 66 登记（卷 66.1 执行中追加）

### G.1 ASSEMBLY_TEST_KEY 家族站点补注（[02] WP-66.1.1 判据 5）

- config-bundle 签名钥站点（`xtask/src/config_compile.rs` `resolve_signing_key`）已由
  **「静默兜底测试钥」转「显式」**（2026-09-05，蓝图 66 WP-66.1.1，体检 C4）：
  缺省路径 Err fail-closed；测试链经 `--accept-test-signing-key` 显式选择现行测试锚
  （钥字节与 `product/config-bundle-pubkey.hex` 产物锚逐字节不变，冻结词形 [00] §3.4）。
  与 evidence/47/WP-47.2.md 在册的 `ASSEMBLY_TEST_KEY`（dispatch 侧，待 47.3 注入链）
  同族不同站；本站闭合仅关「静默」，钥本身归 47.3 家族裁决不变。

### G.2 蓝图 66 F.3 候选收编结果（卷 66.1 执行完毕更新）

- F.3 第 1 项（master_write_v2 入 lib）→ **Done（蓝图 66 WP-66.3.3）**：核体迁
  `protocol-services/src/master_write.rs`，词形 golden 锁 + 8 host 单测（readback
  三态 scripted 传输）；bin 薄包装净 −74 行。
- F.3 第 3 项（bind_shared Result 化）→ **Done（蓝图 66 WP-66.5.3，形态修订）**：
  实际落点 = `bind_shared_const`（静态标签 ≤64B 结构性必成，违反前置 panic；
  旧 `bind_shared -> Option` 删除不留双入口）——Result 化形态经复查为更劣
  （Err 臂仍死防御），执行卡 D 节无此争议、以 const+expect 收口。
- F.3 第 4 项（TEL_INGEST 应答双源收拢）→ **Done（蓝图 66 WP-66.5.4）**：
  `agent-sdk::tel_wire` 单源（照 ea_wire 先例），root/control-services/
  dispatch_assembly 三侧换装，accepted ≤255 结构性事实 debug_assert 锁。
- F.3 第 5 项（ai_request_topic 收拢）→ **Done（蓝图 66 WP-66.3.2）**：插件内化
  `enqueue_ai_publish`（两分类 fail-closed），mqtt_io 存储②删除，QueueFull 变体
  承载错误词形。
- F.3 第 6 项（intent-request wire 三抄）→ **Done（蓝图 66 WP-66.3.1）**：
  `INTENT_REQUEST_OPCODE` 常量 + `encode/decode_intent_request` 单源 + 引用锁。
- F.3 第 2 项（Orchestration step 化）→ Open（维持，非蓝图 66 范围）。

### G.3 蓝图 66 新登记独立项

| 项 | 锚点 | 状态 | 说明 |
|---|---|---|---|
| crash-dump 设备端写入 seam | `crates/observability/crash-dump/`（编码/解析 host 侧就绪） | Blocked | 前置 = panic 写入通道（新机制开发，超 66.1 卷容）；四处运维面已诚实化（66.6.2） |
| planner 集成测试模型面迁移 | `crates/ai/planner/tests/integration.rs`（ModelBuilder/BuiltModel 直构造两测） | Open | 迁移后 ModelBuilder/BuiltModel 可收缩 pub(crate)（蓝图 66 执行卡 D-8） |

## E-67. 蓝图 67 执行登记（2026-09-05，v0.320.0）

**收编完成**（第十一轮体检候选，[蓝图/67-*/01] §6 登记表对照）：

| 项 | 内容 | 状态 |
|---|---|---|
| A3 门禁栈单源化 | sign-key 门迁 xtask（`qa gate sign-keys`，语义逐字节迁移+3 单测随迁）+ CI 接电（ci.yml 步骤）；version/mock 门 owner 归 xtask 单源（ci 侧双实现删除） | 收编完成 |
| B1 chunked epoch 八站点收编 | 八站点 + 执行期订正面（config_gateway impl/protocol_ladder/svc-device-agent/dispatch_assembly catalog 与 build_engine/fetch_control_write/cfg_bindings chain_key/svc-modbus-rtu×2）全收编 ChunkedEndpoint；仓走守卫（agent-sdk，**零白名单全精度**，[01] §2.1b）执法 | 收编完成 |
| B2 TEL 请求 opcode 单源 | telemetry.rs 三常量 → tel_wire re-export 别名 + include_str! 自守卫；svc-energy-agent 跨 crate 双名混用消除 | 收编完成 |
| B5 求解器参数类型化 | Solver trait 增 set_time_limit_s/set_mip_rel_gap（默认 Err 诚实失败）；HiGHS 直调 double 选项；planner/day_ahead 注入点类型化；0/1 Bool 陷阱实弹回归锁进仓（highs.rs bp67 测试） | 收编完成 |
| A5 plan 死入口收敛 | plan_spawn/plan_spawn_dyn_aware/plan_spawn_with_enforcement/plan_start_order + CONTROL_SERVICE_STACK_PAGES 删除；降级循环单实现（对拍测试先行背书）；spawn.rs -249 行 | 收编完成 |
| 卫生群 | certifier 口径对齐 ADR-051/certificate.rs；planner gate pub(crate)+两 fn 去 pub+solver-model 死依赖删；main.rs:571 腐朽 allow 注记修正 | 收编完成 |
| D1 gate 脚本收敛债 | run67-vol1-gate.sh 照惯例自 65 模板新抄（**本卷未收敛，现状登记**）；收口承诺归下一卷 | Open（债显式登记） |

**新观察项**（G8 run#1 异常，未定位根因）：qemu-prod 首跑出现 svc-telemetry
停顿级联（对 chunked 请求全停应答 → 上游 ACQ/INGEST 停 → 心跳无消息进展判卡死
→ kill+restart ×54，4.5s≈chunked 超时量级）；同镜像同脚本 run#2 全 PASS
（17 判据 + 计数对照全对）。排除确定性代码回归，疑似首启抖动类新形态；
建议第十二轮体检单独勘探（详见蓝图 67 执行卡 G8/G 段）。

## E-68. 蓝图 68 执行登记（2026-09-05，立卷核实新登记）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-CHUNK-EPOCH-DOC | `crates/agents/agent-sdk/src/chunked.rs:16,91,198-199`（doc 称"epoch 拒绝迟到帧/新 header 须同 epoch 续传"——`on_data` 从不比较 epoch 且 Data 帧 wire 无 epoch 字段，文档-实现不符） | **Done（蓝图 68.1 WP-68.1.3 方案 A：doc 归真 5 处 + ChunkedReceiver generation/late_dropped 机制 + service_slots 槽位收口，七测变异验证在库）** | 蓝图 68 01-事实基线 F9 |
| REG-MW-U16 | `crates/protocols/protocol-services/src/master_write.rs:39`（`mapping.device_id != device as u16` u32→u16 截断比较：device=0x10001 折叠成 1 可与点表 device 1 匹配，digest 门用 u32 全值——两门口径不一） | **Done（蓝图 68.2：u16 截断修复，F18 device-range 门，见 13abdbd）** | 蓝图 68 01-事实基线 F18 |
| REG-TEL-GRANT-NOTE | `crates/agents/control-services/src/bin/svc-telemetry.rs`（磁盘轨装配块） | **Done（蓝图 68.3 WP-68.3.4：按 `open_disk_history` 现签名重写装配——eipc_resolve 授予边 → BlockIo（PAYLOAD_CAP 461 扩容分片）→ 持久 mount；失败显式降级 TSDB-DISK-UNAVAILABLE；host 命令预算测 c5 + QEMU TSDB-DISK-OPEN/COMMIT 实弹 evidence/68/vol68-3/）** | 蓝图 68 01-事实基线 F6 |
| REG-G8-X54 | BP67 执行卡 G8 观察项"kill+restart ×54"计数未入档 evidence（最接近留档：evidence/40 单发、spec-2026-08-27-D 与 41 各 1 次）——后续引用须注明未入档 | **Done（蓝图 68.1 WP-68.1.4：修复后镜像三窗全 CLEAN 三次未复现 → 防御性关闭/观测在位；CHUNKED-SLOW 等四观测词形在库。另：gate 实弹另捕获栈贴线静默冻结一家族成员并修复，见 68.1 执行卡 §F）** | 蓝图 68 01-事实基线 F10 |
| REG-BOOT-SLOW-FINAL | `crates/agents/control-services/src/bin/svc-energy-agent.rs`（能量链 boot 窗 FINAL 处理深达 6525ms > 客户端 5s chunked 超时 → root 超时重试 → 未拉完分页被覆盖；68.1 观测词形实弹指认：`[energy-agent] CHUNKED-SLOW call=FINAL dur_ms=6525` 与 `CHUNKED-PAGE-EVICTED slot=0` 同窗相邻出现，telemetry/ai-worker/config/dispatch 亦有 101-1634ms 慢 Final） | Open（预存时序行为非 68.1 回归——旧二进制不可见；候选面：能量链 Final 分段化/客户端超时预算与启动窗负载匹配/重试幂等审计；归后续卷） | 蓝图 68.1 执行卡 §D.6 + evidence/68/vol68-1/normal.qemu-serial.log |
| REG-EDEF01 | `docs/delivery/spec-2026-08-28/INDEX.md`（C 卷 owner 裁决条目）+ `crates/agents/control-services/src/bin/svc-telemetry.rs`（磁盘轨装配） | **Done（蓝图 68 卷 68.3 复通，v0.323.0，2026-09-05）**：PAYLOAD_CAP 64→461（内核 seL4_MsgMaxLength=120 words × 4B/word=480B 顶格）+ eneros-fs 持久 mount（leak-then-mount，`Filesystem<'static>`）；host 实测单 checkpoint 批 ~1.3k-1.5k 命令（52B 时代 ~1.2 万）；burst 子码否决（单帧顶格无多块空间，流式形态命令数同阶） | 蓝图 68 卷 68.3 执行卡（本卷即 C 卷 owner 裁决落地） |
| REG-MQTT-BROKER-HANG | `crates/protocols/protocol-services/src/net_transport.rs:TcpStreamHandle::reconnect`（**根因 68.5 实弹定位**：50 卷"MQTT connect 一律重建 TCP"防御 + 裸 `sockets.remove` 不发段 ⇒ slirp 侧旧连接悬挂 ⇒ 重建新 SYN 与悬挂连接同 4 元组被按重复丢弃 ⇒ SynSent 卡死 ⇒ MQTT 握手 BrokerUnreachable ⇒ svc-mqtt Degraded。无 broker 窗全绿之谜 = host 拒连 RST 快速清理每次重试皆全新） | **Done（蓝图 68.5 附加裁决 6，2026-09-06）**：reconnect/tcp_connect 超时路径先 `TcpSocket::abort()`（RST 出环同步清理 slirp）再移除重建；`mqtt::real::NetTcpTransport` 状态机复位纪律同族防御性修复（SynSent 残留 abort + 有界握手/接收窗）；诊断观测面 `[net] tcp-connect OK/TIMEOUT` + `MQTT-HS`/`MQTT-HS-ERR`（debug_line 通道）入库。实弹复验：探针窗 PRODUCT-READY 无 degraded + PEER-MSG verdict=Accepted + REMOTE-BRAIN used=true resp=[backend=planner]（evidence/68/vol68-5/）。阻塞面解除：TELH/mgmt 端到端实弹待后续卷补窗 | 蓝图 68 卷 68.3 执行卡 §D → 蓝图 68 卷 68.5 执行卡 |
| REG-TSDB-CHECKPOINT-ASYNC | `crates/agents/control-services/src/telemetry_history.rs`（TsdbHistorySink::write_batch 的 checkpoint 段）+ `crates/agents/control-services/src/telemetry.rs`（commit_history_batch 在 OBS/TEL_INGEST 处理内同步执行） | Open（68.3 实弹订正：TCG virtio RMW ~5-15ms/命令，逐批 checkpoint（~1.3k 命令/次=7.7-32s）饿死决策链（run3：agent=OK 0、17 轮/600s）；68.3 落地合并窗 CHECKPOINT_EVERY_BATCHES=128（掉电丢尾 ≤128 批诚实登记，轻批 ACK 路径 0 命令 host 实测）；剩余候选 = checkpoint 异步化（空闲 tick 驱动）/扇区对齐批量写（452B 非对齐致服务端逐扇区 RMW）/index.bin 增量日志——归 68.4+ 裁决） | 蓝图 68 卷 68.3 执行卡 §D |
| REG-DISPATCH-HEAP-HIGHROUNDS | `crates/protocols/control-dispatch/src/`（svc-control-dispatch 堆工作集；vol68-3 run4/bisect 实弹：`PANIC: memory allocation of 2656 bytes failed` 于 384KiB heap-large 池，高轮数窗（465 轮/300s）稳定复现） | Open（**预存缺陷**：bisect 窗无 OBS-flush/无合并窗仍 OOM——独立于 68.3 运行时改动；vol2 基线 170 轮/600s 未触墙。68.3 gate 对该签名做有登记豁免（run68-vol3-gate.sh 判据 1），其余 PANIC 照常判负。后续卷：dispatch 堆剖面 + 每轮累计点定位（journal/saga 已证有界，嫌疑在其余 per-round 面）+ 池扩容/收敛裁决） | 蓝图 68 卷 68.3 执行卡 §D.5 |
| REG-F27-5 | `crates/agents/control-services/src/remote_brain.rs:352-387`（build_plan 硬编码 `SolveStatus::Suboptimal` + `model_digest: None`——伪造求解元数据） | **Done（蓝图 68.5 WP-68.5.2）**：按应答 reason 词形诚实标注三态（heuristic/remote-llm/planner）——`backend_tag` 单源 + `status=` 仅字面 optimal 升格 + `model=<64hex>` 解码回 32B；标签前置保 64 字符截断可读（t3/t3b 锁，变异咬合）；对端词形源 = planner-peer `BACKEND_TAG` | 蓝图 68 01-事实基线 F27（第 5 项） |
| REG-REUSEDISK-TEL-COLDSTART | `crates/agents/control-services/src/bin/svc-telemetry.rs`（复用盘 boot 恢复序列：TSDB mount+事件面 open 扫描+快照 recover）× `svc-modbus-tcp` TEL_INGEST 推送时序——复用盘窗 svc-telemetry 恢复期长（实测两次 TSDB-DISK-OPEN=服务双 boot），modbus 采集首批推送撞恢复窗 → ingested=2/点 1-7 全缺 → SNAPSHOT rejected 138 次（boot 25-28s 段集中）→ trusted 快照不可得 → agent=OK 稀少（13/763 轮） | Open（**预存冷启动形态在 68.5 新场景暴露**：68.4 复用盘窗无 broker/agent 流量判据不覆盖；fresh 盘同流量窗全绿——差异变量=恢复期时长。处置：68.5 窗 B 用 fresh（对齐执行卡）；DEMO-68 复用盘段 BLOCKED 于本项，修复后执行；修复候选=恢复期推送缓冲/采集端重试/watchdog 恢复期豁免——归后续卷） | 蓝图 68 卷 68.5 执行卡（附加裁决 9） |
| REG-TELEMETRY-HEAP-CREEP | `crates/drivers/drv-svc/src/allocator.rs`（heap-large 池）+ `crates/agents/control-services/src/`（svc-telemetry 每批处理面）——"复用数据盘 + broker 流量"窗实测堆水位慢爬升 ~0.35 KiB/s（420-480s 触顶 → EVENT-STORE-OPEN 的 4 KiB 分配失败 PANIC；evidence/68/vol68-5/{reuse1,degrade} 两窗复现；fresh 盘同流量窗 480s PANIC 净值 0——恢复态常驻 + 每批净增叠加） | Open（68.5 处置：池 384→512 KiB 买窗 + 判据=复用盘+broker ≥420s 无 PANIC（已过）；CachedBlocks(16K)/EventStore(~8K)/SnapshotModule(~48K) 静态上界有界排除；泄漏面嫌疑 = checkpoint/事件面/OBS 外发 per-batch 净增，归后续卷 host alloc 计数探针定位） | 蓝图 68 卷 68.5 执行卡（附加裁决 9） |
| REG-F27-STRUCTURED | F27 第 1/2/3/4/6/7/9/10/11 项（schema 闭集拒结构化计划/上行 6 字段不足建 DecisionRequest/1024B+64B 分片/AcceptedPlan 单值化/adapter 再压扁 objective_digest=[0;32]/DecisionGate 按条数计率/审计截断 48/64 维持/目标侧无法复验真证书/单在途槽） | Deferred（**后续卷候选——结构化计划通道**：动 F26 合同须 schema v2 + ADR；mTLS/attestation 通道与 certifier 生产接电同期裁决；单在途槽对端面已由 planner-peer 单线程逐请求 + seq 状态持久承载） | 蓝图 68 卷 68.5 执行卡 WP-68.5.5 |
| REG-F27-8-ARBITER | F27 第 8 项（Arbiter 恒 Auto） | Deferred（ADR-0052 saga 冻结面，禁新投资——维持现状登记） | 蓝图 68 卷 68.5 执行卡 WP-68.5.5 |

## E-68-R. 蓝图 68 全卷联合复查登记（2026-09-06，68.1-68.5 复查会话）

> 五卷代码级联合复查（每卷一路独立复查代理 + 跨卷交互自查；基线 8680/0 绿）。
> 修复六项（含 68.5 出口后一个 P1 可用性死锁）+ 预存登记两项；全部修复带红测与变异咬合记录。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-LEASE-EXPIRY-DEADLOCK | `crates/agents/control-services/src/remote_brain.rs:arm_lease_wiring` + `ai_worker.rs:HeuristicBackend.degradation`（68.5 P1：authorize 对过期 lease 返回 Err 且状态不动，重武装被"已武装"守卫短路、revoke 仅在 peer 恢复时调用——peer 失联 >TTL 后 System1 兜底恒 Unavailable 决策链停供且学习冻结永久化；68.5 前启发式兜底可无限期工作，属可用性回归） | **Done（68 全卷复查，2026-09-06）**：arm_lease_wiring 增过期复位分支（`degradation_lease_expired` 查询 + revoke + `LEASE-REVOKED reason=lease-expired` 词形 + 重武装；键在目标侧自签不越权，每 episode 仍受 TTL/次数有界约束）；t7_lease_expiry_rearms_on_subsequent_timeout 变异咬合 | 本表（68.5 出口后复查新发现） |
| REG-DEGRADE-ARM-LEARN-RESET | `crates/agents/control-services/src/ai_handler.rs:learn_reset`（68.5 P2：附加裁决 8 只冻结了 apply_learn_thresholds 注入点，LEARN_RESET 在武装期仍无条件 apply_deadband 回厂——digest 漂移击穿 lease 白名单，System1 断供同构失效形态换注入点存活） | **Done（68 全卷复查，2026-09-06）**：learn_reset 增武装判据（拒绝 status=1 + `LEARN-RESET REJECTED reason=degrade-armed` 词形）；t7_learn_reset_rejected_while_degrade_armed 变异咬合 | 本表 |
| REG-DEGRADE-POWER-BOUND | `crates/agents/control-services/src/ai_worker.rs:HeuristicBackend::decide`（68.5 P2：兜底提案功率硬编码 5.0，lease 功率上界 = 配置 max_power_kw——站点配 <5.0 时武装期逐次授权恒 "action beyond lease bounds" 全拒断供；现役 toml 5.0 恰好掩盖） | **Done（68 全卷复查，2026-09-06）**：decide 武装期 proposal_power 钳到 lease.max_abs_power_kw（digest 按策略原值不变，签名面不动）；t5b_fallback_power_clamped_to_lease_bound 变异咬合 | 本表 |
| REG-TCP-CLOSE-NO-RST | `crates/protocols/protocol-services/src/net_transport.rs:TcpStreamHandle::close`（68.5 REG-MQTT-BROKER-HANG 根因修复的补口：close 路径 `tcp.close()` 后立即 `sockets.remove` 无 poll——FIN/RST 永不出环，对端连接悬挂占同 4 元组；stop/disconnect 后经 open_tcp 新建连接时复发同族卡死） | **Done（68 全卷复查，2026-09-06）**：close 同 reconnect 纪律——abort（RST 出环）+ poll 后再 remove | 本表 |
| REG-TSDB-MOUNT-TRANSIENT-FORMAT | `crates/drivers/fs/src/lfs/filesystem.rs:leak_storage`（mount 前置探测把一切失败折叠 BadSuperBlock → `telemetry_history.rs:open_disk_history`/`event_face.rs:open_snap_volume` 的 mount-or-format 据此格式化——重启瞬态块命令失败（ST_BUSY/超时→IO 族）即静默清空含数据 TSDB/快照卷；SPEC-C 预存逻辑，68.3 装配翻转为生产数据面风险） | **Done（68 全卷复查，2026-09-06）**：probe_mountable 错误感知分类（littlefs2 `CORRUPTION`(-84)→BadSuperBlock 可格式化；`IO`(-5) 族→IoError 显式降级不触碰数据）；test_mount_transient_io_error_is_not_bad_superblock 变异咬合；未格式化路径 test_mount_unformatted_fails 保持绿（format 语义不变） | 本表（68.3 执行卡 §D 同类"预存被暴露"） |
| REG-PLANNER-PEER-HALFDEAD | `crates/ai/planner-peer/src/mqtt.rs`（last_activity_ms 只写不读、PINGRESP 从不等待、publish 失败只 skip 不重连——broker 进程挂死但 TCP 存活时连接永不重建，planner-peer 表面 READY 实质失能；REG-MQTT-BROKER-HANG 的 host 侧镜像盲区） | **Done（68 全卷复查，2026-09-06）**：入站静默看门狗（`inbound_silent_for` > 1.5×keepalive 判死重连，DEAD_SILENCE_MS=45s）+ publish 失败重连（handle_request 返回链路健康）；inbound_silence_watchdog_semantics 锁 | 本表 |
| REG-CHUNK-HEADER-RESERVE | `crates/agents/agent-sdk/src/chunked.rs:281`（畸形 Header total_len=0xFFFFFFFF+chunk_count=1 过 decode——无 total_len ≤ chunk_count×104 交叉校验 → `buf.reserve(~4GiB)` 目标侧 alloc 失败 abort 服务循环；心跳 supervisor 兜底可重启但可被循环触发。**预存**（父提交同代码），68.1 未触） | Open（预存鲁棒性缺口；候选=decode 增 LengthMismatch 早拒 + total_len=0 空载荷两侧不对称收口；归后续卷） | 本表（68.1 复查发现） |
| REG-NET-RECONNECT-DANGLING | `crates/protocols/protocol-services/src/net_transport.rs:TcpStreamHandle::reconnect`（`sockets.remove(self.id)` 后 `tcp_connect?` 早退——self.id 悬空指向已移除 handle，下一次 reconnect 的 abort 对无效 handle 触发 smoltcp panic；连续两次 connect 失败即达。**预存**（本卷 abort 前置未加重也未减轻）） | Open（防御性缺口；候选=失败时标记 handle 无效/重连入口判存活；归后续卷） | 本表（68.5 复查发现） |
| REG-PLANNER-VARINT-CAP | `crates/ai/planner-peer/src/mqtt.rs:read_packet`（不受信 broker 防御：MQTT 规范外 4 字节 varint 全 0xFF 现解码接受 → rem≈539 MiB 分配 OOM；规范顶格 0xFF×3+0x7F ≈171 MiB 同样超限） | **Done（68 全卷复查二轮，2026-09-06）**：MAX_INBOUND_FRAME=1 MiB 分配前拒（5 字节形由 mult 检查拦）；oversized_varint_rejected_before_alloc 变异咬合 | 本表 |
| REG-PLANNER-BUDGET-KEEPALIVE | `crates/ai/planner-peer/src/lib.rs:210`（budget_ms 域 [100,60000] vs broker keepalive 30s——求解同步阻塞主循环，>15s 档错过 ping 半窗，60s 端求解期 0 ping 被 broker 按 1.5×keepalive 踢链，每次请求浪费+重连） | **Done（68 全卷复查二轮，2026-09-06）**：域值收窄 [100,15000]（join keepalive；现役 5000 在域内，样例 toml 无超限档） | 本表 |
| REG-LEASE-ARM-FAIL-WORD | `crates/agents/control-services/src/ai_handler.rs:167` + `remote_brain.rs:SwitchEvent::LeaseArmFailed`（词形宣称"System1 后续 Unavailable 决策链停供"——实际 arm 失败 = degradation=None → 无签兜底继续（68.5 前行为）；该臂结构性不可达（全零键/TTL=0 被装配守卫拦、自签验签必过、already-armed 被前置判据挡）） | **Done（68 全卷复查二轮，2026-09-06）**：词形与 SwitchEvent doc 改如实（"无签兜底继续=68.5 前行为，须查配置链"）——审计诚实化主题自洽 | 本表 |
| REG-CHUNKED-OBSERVABILITY | `crates/agents/control-services/src/service_slots.rs`（68.1 复查三处观测面：①DUP-SESSION 孤儿清场整体换接收器致 generation/late_dropped 归零——同 badge 自伤面代次防线失效；②越界 pull 虚增 pages_pulled 压制后续 PAGE-EVICTED 计数；③Final-None/Malformed 两覆盖旁路不产生 page_evictions 计数——观测盲区，数据面均无影响） | Open（观察面真实性；候选=清场保留代次/`pages_pulled.min(pages_total)` 钳制/旁路补计；归后续卷单测锁定） | 本表 |
| REG-GATE-EVENTPRUNE-STRICT | `tools/run68-vol4-gate.sh`（`grep "EVENT-PRUNE"` 会同时计入 EVENT-PRUNE-FAIL——PRUNE-FAIL 误触发 RPO 对账放宽=假绿方向） | **Done（68 全卷复查二轮，2026-09-06）**：两处 grep 严格化 "EVENT-PRUNE n="（bash -n 过） | 本表 |

## E-68-R3. 蓝图 68 三轮全局复查登记（2026-09-06，68.1-68.5 第三轮）

> 三轮全局复查（本人深读决策头/68.2/68.5 + 三路独立复查代理覆盖 68.1/68.3/68.4，疑点全部本人多维复验后落刀）。
> 基线 8686/0 → 收口 8696/0（+10 回归锁）；fmt/clippy --all-targets 零告警。红测先行 + 变异咬合（truncated/delivery 两项变异实咬）。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-MQTT-PEER-PUBACK-PID | `crates/ai/planner-peer/src/mqtt.rs:ack_publish`（P1：QoS1 入站 PUBLISH 回 PUBACK 取 `body[2..4]`=主题前两字节当 packet id（真 pid 在 2+tlen 处）——对真 mosquitto 消息永不过 inflight 窗（max_inflight_messages 默认 20 → 20 条后请求停发），且 PINGRESP 持续刷新活性使入站静默看门狗永不触发=AI 链静默饿死；gate 假 broker 不校验 pid 故全盲。目标侧 client.rs 同族已由 WP-53.4 修对，本案为其 host 端镜像） | **Done（68 三轮，2026-09-06）**：pid 按 `2+主题长度` 解析（域检齐）；puback_echoes_real_packet_id 严格假 broker 红测 + 测试假件 ack 保真同修 | 本表 |
| REG-CHUNK-PULL-EMPTY-HANG | `crates/agents/control-services/src/service_slots.rs` pull 分支（P1：越界 idx/无存续分页响应的 Pull 回空响应——target 主循环 `message_info_new(0 长)`=全零 MessageInfo 与 `reply_info==0`"无待回复"哨兵同形，reply 阶段被吞，Call 语义客户端（非 MCS 无 IPC 超时）永久挂死（服务重启落在"Final 已回未拉完"窗即触发，无需对手）。伴生：越界帧先推进 `pages_pulled` → 同槽覆盖性 Final 的 PAGE-EVICTED 观测被无声击穿） | **Done（68 三轮，2026-09-06）**：pull 空页段早退回 `ERR:PageRange` 且不推进 pages_pulled；t8 双断言咬合 | 本表 |
| REG-OTA-SLOT-OVERWRITE | `crates/security/ota-ab/src/ab_partition.rs` write_bytes/write_chunk/prepare_upgrade（高：写面/声明面全链无槽容量域检，而 AbPartitionOta 挂载设备=整数据盘（svc-ota 32768 块）——超长镜像越槽写物理穿过保护带直入 TSDB/审计/学习/事件/事实/快照区段（同盘静默损毁），且自洽超长镜像回读自洽过 verify+activate；kernel_ota/downloader 的容量门不在本路径） | **Done（68 三轮，2026-09-06）**：槽容量=`PARTITION_BLOCKS×设备块大小`（本模块寻址自洽口径；生产 bs=512=1MiB）——write_bytes 域检硬界（无部分写）+ prepare `ImageTooLarge` 早门双门；t10 物理落点/t10b 早门红测；int 套件（8KiB 块 mock 10MiB 镜像）不受扰 | 本表 |
| REG-IPC-PENDING-LEAK | `crates/agents/agent-sdk/src/ipc_sel4.rs` request/request_raw/drv_call（P2：`pending.insert` 先于预算/编码检查，早退 return 不 remove=每错帧永久泄漏一条（无界增长向量）；当前载荷封顶在预算内不可达，属潜伏，且 `test_drv_call_budget_120_words` 每次触发都在漏） | **Done（68 三轮，2026-09-06）**：insert 移至全部早退检查之后（三处同构）；pending_table_empty_after_budget_early_returns 锁 | 本表 |
| REG-PLANNER-SEQ-STATE-CORRUPT | `crates/ai/planner-peer/src/main.rs:SeqState::load`（P2：存量状态文件损坏（读失败/JSON 坏/形坏）静默归零=违背"状态文件消除 seq 回退面"自述——目标侧按 seq 单调判据全拒（回退饿死至追平旧 seq），主机端零告警） | **Done（68 三轮，2026-09-06）**：NotFound→0 合法；其余损坏 `SEQ-STATE-CORRUPT` fail-closed exit(2)（删文件=显式回退重建交运维）；seq_state_missing_is_zero_and_roundtrips 锁可进程内测路径。**69.2 收口 O5 附注**：代码随 PeerSession 迁 `session.rs`（判定原样）；store 改 `create_new`（O_EXCL）+pid 重试 1 次（残留/并发临时名不覆盖），p10 文件层五段锁（含只读目标 rename 判别器）；QEMU 联合窗 seq 跨 kill-9 重启单调实弹（7→7） | 本表 |
| REG-CHUNK-FINAL-STALE-MATCH | `crates/agents/control-services/src/service_slots.rs` final 分支（P2/P3：Final 不查 rx 完成态——上一传输完成未发 Final、新传输块未齐即 Final 时，`pending_full.take()` 取到陈旧载荷当本次请求处理并以成功形态（ENVP）错配；其余违例皆有结构化 ERR 唯此漏） | **Done（68 三轮，2026-09-06）**：rx 非空闲拒帧 `ERR:TransferIncomplete` 不消费 pending_full，在途传输不受损；t9 咬合 | 本表 |
| REG-FS-CRC-MAP-FORMAT | `crates/drivers/fs/src/lfs/storage_adapter.rs:map_storage_error`（P2：`BadBlock`/`CrcMismatch`→CORRUPTION 使媒体级一次性 CRC 误报成为 probe 格式化候选——REG-TSDB-MOUNT-TRANSIENT-FORMAT 病灶的映射表复活通道；当前生产链 EIPC 全折 HardwareFault→IO 不可达，属潜伏） | **Done（68 三轮，2026-09-06）**：媒体级疑虑一律映射 IO，CORRUPTION 只留给 littlefs2 自身裁决；test_mount_transient_crc_error_is_not_bad_superblock 守卫（红测先行） | 本表 |
| REG-EVENTSTORE-TRUNCATED-DOUBLECOUNT | `crates/runtime/event-store/src/block.rs:scan`（P3：主循环 break 处 +1 后残片步进从同一 pos 重启再 +1——断链头记录恒双计（每坏段多 1），仅 RecoveryReport 统计失真） | **Done（68 三轮，2026-09-06）**：残片步进为唯一计位点（主循环 4 处 +1 删）；truncated_records_counts_broken_region_exactly_once_per_record 精确 4 断言，变异实咬（=5 红） | 本表 |
| REG-EVENTSTORE-DELIVERY-ZEROPROGRESS-COMMIT | `crates/runtime/event-store/src/ingest_bridge.rs:collect_delivery`（P3：零进度分区无条件 commit_consumer=每次 drain 空转 +1 世代 + 重写 4KiB 位点槽块；姊妹 store_fact_sink.rs 有 `next > from` 守卫的不对称即疏漏证据） | **Done（68 三轮，2026-09-06）**：镜像姊妹守卫；delivery_drain_without_progress_commits_nothing 计数设备测试，变异实咬 | 本表 |
| REG-CHUNKED-PAGE-U8-CAP | `crates/agents/control-services/src/service_slots.rs:set_paged` + `chunked.rs:encode_paged_header`（P3 加固：响应 >255×104B 时 `as u8` 回绕——服务端记账错位、客户端 decode 拒绝（fail-closed 端到端成立）；当前无此规模 handler） | **Done（69.1，2026-09-07）**：响应方向页数域检 `RESP_COUNT_MAX=u8::MAX`（255 页 ×104=26520B——I4 不变量=u8 不截断；卷册原拟 16 页/1664B 被 VOL69.1-GATE 实弹证伪：svc-config 启动期分发 7099B 配置束（68 页）为既有生产面，16 页误界 → 全窗 ACQ/决策链级联失败， gate 抓到后按蓝图自身安全分析订正域界）——`set_paged` 显式越界拒绝（`ERR:ResponseTooLarge`+计数+词形 CHUNKED-RESP-TOOLARGE），`as u8` 改 `u8::try_from(..).expect`；t14 边界 26520/26521 双侧咬合。**残余注记（69.1 复查）**：`encode_paged_header` 另有三个**内嵌 chunked 服务端**调用方（`ingestion_stage.rs:627`/`config_client.rs:145`/`mgmt_history.rs:120`）未经域检——同族 P3（>26520B 才可达、客户端 decode fail-closed 兜底），不在 69.1 WP 文件清单，随 69.3 采集收编一并域检。**69 全卷独立复查（2026-09-08）实查收口**：三处调用方均位于 `#[cfg(test)]` 测试假件模块内（`ingestion_stage.rs:503`/`config_client.rs:100`/`mgmt_history.rs:117` 为模块 `#[cfg(test)]` 起点，三处 encode 全在其中）⇒ 生产可达性 = 0（生产服务端唯一 ENVP 组装点 = service_slots.rs 单点且已域检）；「随 69.3 域检」无生产必要性，69.3 未执行属正确不作为非失约——注记如实化，本残余关闭 | 本表（68.1 三轮代理发现；69 全卷复查收口） |
| REG-CHUNKED-MR-WORDS-CAP | `crates/agents/control-services/src/service.rs`（P3 潜伏：MR 写入上限 14 words vs 声明 `len.div_ceil(8)` 无断言执法——未来响应源 >112B 时声明>写入、MR[14..] 残留脏字节；现构造点全 ≤104B 结构性排除） | **Done（69.1，2026-09-07）**：`MR_REPLY_MAX_WORDS=14`/`MR_REPLY_BUDGET_BYTES=112` 提为具名常量（chunked.rs 单源，service.rs 组帧 break 与 ipc_sel4.rs 客户端 min 同引）；静态锁 4 道（≥RESP_PAGE_MAX/整 word/派生自洽/恰=MR 区减 1 word，变异 13/104→E0080 编译失败实咬）+ service_slots step() 出口运行时兜底（`ERR:ReplyTooLarge`+计数+词形 CHUNKED-REPLY-TRUNCATED，t13 穷举 ChunkError Debug 词形 ≤112+t13b 白盒）；service.rs 组帧行为 target-only host 不可测（变异表该行以编译期单源替代，如实记录） | 本表（68.1 三轮代理发现） |
| REG-CHUNKED-PULL-NO-DISCRIMINATOR | `agent-sdk/src/chunked.rs`（v2 分页 wire：`ENV2` 14B 头 + `ENVN‖idx‖txn` 7B 请求 + `ENVR` 判别页帧）+ `service_slots.rs`（服务端 txn 绑定/校验）+ ADR-0053 | **Done（70.3，2026-09-09，ADR-0053 owner 裁决落地）**：方案 A（修订 wire）全量实施——16-bit 事务标识 = 被接受 Header 的 epoch 低 16 位，服务端 Final 安装响应时绑定、Pull 校验一致才供页，客户端逐页校验回带；v1（ENVP/裸页/5B 拉取）**硬退役**（同一启动 bundle 两端同步升级，无混部窗），5B 遗留形服务端 `CHUNKED-PULL-STALE` 记账拒绝。三残余面闭合：ERR 文本/末页载荷不可判别（判别帧结构分离）、respawn 后旧续拉（无当前事务→结构化拒）、badge 复用旧客户端续拉源 #1（txn 失配拒绝）；u16 回绕=等值比较无序数语义（撞窗需同 badge 65536 次传输间隔，Call 5s 超时下结构性不可达，ADR-0053 残面登记）。自愈语义保持：t12 反活锁锁逐字保留，覆盖性 Final 照常安装（PAGE-EVICTED 记账）。页帧 7+104=111B≤112B 静态锁；host 假件 5 份 v1 帧律手抄收编 `PagedServerV2` 单源 | 本表（69.1 立 → 70.3 收） |
| REG-TSDB-DROPPED-CK-WORD | `crates/agents/control-services/src/telemetry_history.rs` + telemetry.rs Disk Err 分支（P3 观测：窗满批 write 成功而 checkpoint 失败时按 DROPPED 计——实际批在缓冲、后续窗满重试成功即真实落盘，词形假阴性；方向保守过报不掩盖。68 三轮裁注：ck 失败瞬态批"尚未掉电安全"，过报可辩护，故登记不修） | Open（**维持登记，69.5 裁决**：修=改 DROPPED 记账语义（判定面），本卷观测窗纪律（只加观测不改判限，Z10）不适用也侵入；69.5 交付的只读观测窗模式（DEGRADE-OBS 计数器/快照行）证明可在**后续卷**给 TSDB 加同款 pending vs dropped 区分观测（不动记账），届时再裁） | 本表（68.3 三轮代理发现；69.5 复裁） |
| REG-DRV-TIMEOUT-AFTER-CALL | `crates/agents/agent-sdk/src/ipc_sel4.rs`（Info：超时判定在 seL4_Call 返回之后——服务端慢于客户端 timeout 时操作可能已真实完成而客户端按失败处理；当前块写幂等（重写同值）+littlefs 4096B 全块 prog，无可兑现破坏路径；未来非幂等块命令过此通道须重审） | Open（**维持登记，69.5 裁决**；出口清单第 7 项前提复核：登记安全论证依赖"块写幂等+全块 prog"——WP-69.5.5 经 M3 实测走**不修**分支（泄漏有界 48B/boot、零重试路径），mount/format 路径零改动、probe_mountable 分流逻辑零改动 ⇒ 幂等性论证覆盖面**不变，仍成立**；无次生升级） | 本表（68.3 三轮代理发现；69.5 复裁+前提复核） |
| REG-DRV-PAYLOAD-LEN-CLAMP | `crates/drivers/drv-svc/src/runtime.rs::{decode_request,decode_response}`（70.3 起单点运行时拒绝） | **Done（70.3，2026-09-09，69.5 三步修法全落地）**：① decode 单点 `payload_len > PAYLOAD_CAP` → 运行时 None（release 同律，`debug_assert` 仅辅助；请求/响应双侧）——旧 min 钳制+保留过声明长度（未传输零填充当有效载荷）面闭合；② 逐命令 caps 复核清单化入执行卡（OP_PING/OP_GET_CAPABILITY 零载荷/短定长响应；OP_EXECUTE 载荷=信封自带 caps 承载，process_request 出口 `status_handled` 单源）；③ 目标/负例实弹：CAP−1/CAP/CAP+1/u16::MAX 声明域矩阵 + 短信封 + 尾 padding（decode_response_padded 域检不因补齐放宽）+ process_request 负例后队列可用（非法请求不入队不触发 handle）；PAYLOAD_CAP=461 不变（不为掩盖错误扩容） | 本表（68.3 三轮代理发现；69.5 升级 → 70.3 收） |

## E-69. 蓝图 69 执行登记（2026-09-07，69.2-review/69.4/69.5 执行与复查登记）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-69.2-GATE-VACUOUS | `tools/run69-vol2-gate.sh`（69.2 判据段）+ `crates/protocols/protocol-services/src/net_transport.rs::TcpStreamHandle`（N8/N9 socket 所有权修复）+ `crates/protocols/mqtt/src/plugin.rs`（唯一恢复 owner：惰性 connect + 逐主题订阅 + 重连退避 1s→30s）+ `tools/run70-vol4-gate.sh`（强制恢复判据） | **Done（70.4，2026-09-09，双修复+实弹闭合）**：①判据空转=69.2-review 已修（段0 host 窗）；②目标侧 MQTT 面不复重连=70.4 根因闭合——N8（reconnect 失败后句柄停留已移除槽位的陈旧 id，后续触碰即 PANIC 面）+N9（close 后观察走陈旧 id）+plugin 无退避重连（半开断链窗无界快速重试拖停同优先级服务面，vol4 门 w1 窗实弹发现）三修复后，vol4 门双锚实弹：broker kill-9 重启后目标侧新连接（CONNACK=2）+ 新订阅（SUBSCRIBE=2）+ 锚2 后重连/订阅恢复（1/1）——69.2 窗「零重连」形态不再复现；恢复后 AI 链请求腿受 D3 武装 lease（TTL=86400s 登记设计）结构性停发 = 安全语义（WP-70.4.4），退出过渡认证归 70.9（DEGRADE-OBS uses 倒计时可见），非本卷缺陷 | 本表（69.2-review 立 → 70.4 收） |
| REG-69.4-ENGINE-FENCING-TOKEN-DEAD | `crates/protocols/control-dispatch/src/engine.rs:1102 DispatchEngine::update_fencing_token`（P3 事实订正：卷 69.4 卷册（01 卡 10 订正 ⑦）称该方法已由 FIX-24.R3 经 `ai/runner/src/lib.rs:408` 接电——69.4 变异靶 3 实弹证伪：删该方法后 runner 编译零错误，`sink.update_fencing_token(token)` 的接收者是 `eneros-dual-brain/src/chain_sink.rs:166 ChainCommandSink::update_fencing_token`（FIX-24.R3 真正接电的是它）；`DispatchEngine::update_fencing_token` 生产调用方 = 0，属真死码疑云） | **Open（69.4 施工保守不删）**：卷册禁删表明令保留 + 变异不咬合的实证在案（蓝图/69/05 执行卡 §8.6 D-1、§8.7）；候选 = 删（dead-code 收口）或改链（runner 侧注入走 Engine 面）——归独立复查/后续卷裁决 | 本表（69.4） |

| REG-69.5-DEGRADE-OBS-WORDFORM | `crates/agents/control-services/src/ai_handler.rs:degrade_observe`（词形源）+ `crates/agents/agent-sdk/src/ai_relay.rs:AI_DEGRADE_OBSERVE`（线码单源 0x22）+ `crates/runtime/root-task/src/service.rs:degrade_observe_tick`（root 探针） | **Done（69.5，Z2 登记）**：新增串口词形 `[svc-ai-worker] DEGRADE-OBS armed={} expired={} level={} lease={} uses={}/{} expires_in_ms={} deadband=({},{}) applies={} frozen_skips={}`（唯一新增行；lease=lease_id 前 8B 小写 hex；未武装全 0 数字占位不写 N/A；expires_in_ms=saturating 减）；wire 码 0x22 只读（响应=68B 定长快照 learn_wire::DEGRADE_OBS_WIRE_LEN）；t34 字面锁 + t31/t33 判定未变锁 + ai_relay 线码互斥锁在案；既有词形零改动（Q3 逐行 diff 归 VOL69.5-GATE） | 本表（69.5） |
| REG-69.5-LEAK-STORAGE-BOUNDED | `crates/drivers/fs/src/lfs/filesystem.rs:leak_storage`（doc 注记即登记载体）+ M3 实测 `size_of::<BlockDeviceStorage>()=24B`/`Allocation<BlockDeviceStorage>()=8536B`（临时 host 探针测毕已删，数字入 doc） | **Done（69.5，不修裁决+预算登记）**：生产调用点恰 2 处（telemetry_history.rs:295 open_disk_history / event_face.rs:427 open_snap_volume），均一次性（Option::take/构造期）零重试 ⇒ 超设计泄漏上界 = 2×24B = **48B/次启动**（svc-telemetry 栈 64KiB/堆 MB 级预算内）；probe 失败先泄漏后校验的顺序按卷册决策表（有界且已知 ⇒ 不修）保留，MountProbe 签名零改动（次生风险面未触）；设计内泄漏 = 每活卷 1×24B storage + 1×8536B Allocation | 本表（69.5） |
| REG-69.5-OPEN3-ADJUDICATION | `docs/candidates-registry.md` 上表 REG-TSDB-DROPPED-CK-WORD / REG-DRV-TIMEOUT-AFTER-CALL / REG-DRV-PAYLOAD-LEN-CLAMP 三行 | **Done（69.5，WP-69.5.6 三份裁决）**：① TSDB-DROPPED-CK-WORD=维持登记（修=改记账判定面违 Z10；观测窗模式归后续卷复用）；② DRV-TIMEOUT-AFTER-CALL=维持登记 + 前提复核"仍成立"（WP-69.5.5 不修分支 ⇒ mount/format 路径零改动，幂等论证覆盖面不变——出口清单第 7 项关闭）；③ DRV-PAYLOAD-LEN-CLAMP=**升级独立蓝图项（建议 70 卷）**，修法草案三步已登记（单点长度判定/caps 复核清单化/负路径实弹窗） | 本表（69.5） |
| REG-69R-TOOL-WORDFORM-SWEEP | `tools/run61-e4-matrix.sh:61`（m3-no-peer 判据键）+ `tools/run61-phase0-fixverify.sh:61,77-78`（m3-reject-writes 判据键 + M4 首轮 Indeterminate 计数）——69.4 词形退役（SUBMIT/SUBMIT-ERR → TYPED/TYPED-ERR，t22 锁）时清扫止步于 workspace Rust 面，5 处工具侧消费者漏扫：两处 **FAIL 判据死转**（退役后非 Committed 终态只落 `TYPED state=`，`SUBMIT state=` 永零 → run_case 恒 FAIL）+ 一处计数恒 0 | **Done（69 全卷独立复查，2026-09-08）**：三处判据键/计数随退役同频更新 SUBMIT→TYPED（脚本语义不变——非 Committed 终态行现由 CAND 主路 TYPED 词形承载）。**同查同账另两组不改**：`tools/run62-vol{1,2,3}-gate.sh` 的 `SUBMIT state=Committed` 信息计数 = **出生即空转**（`if !receipt.final_state.is_committed()` 守卫自 3cb79a5 v0.314.29 起使 Committed 终态从不落该词形——任何时代恒 0，非 69.4 打断；预存空转诊断，仅 echo 无 FAIL 判定，维持原样如实登记）；`tools/run69-vol4-gate.sh:88` 的 SUBMIT_N=0 判据为退役核对器本身，正确保留 | 本表（69 全卷独立复查） |
| REG-69R-PEER-PUBLISH-SWALLOW | `crates/ai/planner-peer/src/mqtt.rs`（publish 等待环 + stash_inbound 有界队列）+ `session.rs::respond_reserved`（seq 先持久后发布） | **Done（70.4，2026-09-09）**：①交错 PUBLISH「确认即蒸发」闭合——等待 PUBACK/SUBACK 期间到达的 PUBLISH 经 16 条有界队列保留（QoS1 即回 PUBACK 后入队；满队 QoS1 **不回 PUBACK**=broker 重投的显式不接纳，QoS0 溢出 `pending_dropped` 可见计数），业务层经 poll FIFO 交付（host 测试：交错交付/满队/QoS0 计数三测咬合）；②出站 PUBACK/SUBACK pid 关联——错配 ACK 不完结当前请求（分配器 pid 计数替换常量 1）；③同族 seq 保存失败仅打印仍 Ok 闭合——`respond_reserved` 单点：seq+1 **先 sync_all+rename+fsync 目录**持久预留再发布，持久失败=不发布显式 Err（fail-closed），发布/自检失败=有解释序号间隙（PEER-RESP-ABANDONED 词形），断电回退序号冒充新响应面闭合；旧 next_seq+respond 两段式退役 | 本表（70.4 收口，替换 69 全卷独立复查登记行） |
| REG-70.1-DECISION-AUDIT-BUDGET | `crates/security/audit/src/store.rs:BoundedMemStore`（字节预算化存储单源）+ `crates/ai/dual-brain/src/audit.rs:DECISION_AUDIT_BUDGET_BYTES`（64KiB 单一来源）+ 装配点 `dispatch_assembly.rs` / `ai_handler.rs:{run_remote,run_heuristic_only}`（with_budget 显式声明） | **Done（70.1，2026-09-08）**：69.5 窗 svc-control-dispatch OOM（114688B=1024×112B Vec 翻倍申请，qemu-serial.log:203189）根因闭合——`DecisionAudit` 底存储换 BoundedMemStore（构造期一次预分配 585 条=64KiB，append 零分配，满载 `BufferFull`+dropped 计数，丢新不丢旧链可校验）；4096 条数门删除（448KiB 条目+672KiB 扩容瞬态本质不可行）。**DEC-70-01 本卷=方案 B+（字节预算化预分配）**，覆盖窗如实：满载后审计覆盖停止（≈585 轮 @ 实测轮速），**持久化覆盖窗（A 案完整形）维持待裁**；bnd0 归因锁（size_of=112/1024×112=114688）+bnd2 容量锁定+bnd3 链头不前进锁在案。**同卷收口（2026-09-09 实弹续）**：修复后同压力形 846 OK 处暴露第二失败（512B 申请）——旧 512 即死掩盖工作集逼近堆顶；HEAP-WM 堆水位探针（`drv-svc::allocator::HeapAllocator::used_free` + dispatch_assembly 钩子，每 64 请求采样，词形 `[svc-control-dispatch] HEAP-WM req= used= free=`）实曲线证明工作集**有界平台期**（journal256 时 ~423KB、journal128 后 ~392KB/free131KB 稳定）而非泄漏——处置 = `ExecutionJournal::new(256→128)`（I4 容量收口，journal 逐条 ~242B 实测；非终态 Full 背压语义不变）。VOL70.1 900s 全门 PASS：零 PANIC/零分配失败（无豁免）/714 OK（原失败点 1.4 倍）/724 轮；慢爬体（~580B/req 饱和于 ~req500-576，~300KB）构成未完全分解，探针与判据器入库供后续卷继续。同卷 REG-70.1-MGMT-RPC-BOUNDED：mgmt_rpc audit/replay_seen 无界 Vec → 128/1024 环窗+dropped 计数（N02；重放窗=易失防重放窗口非持久操作账，持久 operation 归 70.6.1） | 本表（70.1） |
| REG-70.1-GATE-NO-ALLOC-EXEMPTION | `tools/run70_gate_judge.py`（G1-G7 判据单源）+ `tools/run70-vol1-gate.sh`（编排+负例自检） | **Done（70.1，F70-7-02 收口）**：70 系列门零 PANIC/零分配失败豁免（69.5 的 alloc.rs 排除即放过真分配失败的缺陷不再复现）；相位判据=串口四等分每相位 round 样本≥ceil(门限/4)（尾部死亡不得被全窗累计掩盖）+agent=OK>512（越过原失败点）；负例自检 5/5（健康窗过/注入 alloc.rs PANIC 红/尾部死亡红/缺日志红/样本不足红）；长稳态窗（≥10000 轮且≥2h，P 值）支持 MIN_ROUNDS/DL 覆写后续跑 | 本表（70.1） |
| REG-70.2-MEDIA-RELAYOUT | `crates/security/ota-ab/src/ab_partition.rs:DataRegion`（16 区段全域图单源，const fn+ALL 遍历）+ 六 bin 重锚（svc-ota/svc-ledger/svc-agent-bus/svc-market/svc-cloud-sync/svc-observability）+ `dispatch_assembly.rs` IntentLog 承载断言 | **Done（70.2，2026-09-08，N01 P0 收口）**：全域两两不交+盘内收敛+全坐标锁+4KiB 对齐承载三守卫测试（ota-ab 57/0）；新几何（512B 块）：IntentLog [0,257) **坐标不变**，业务带 blk264-360（TrialState/Ledger/Market/Cloud/AgentBus/ObsOutbox，4KiB 对齐节距），OtaSlotA blk512/SlotB blk2560（旧 1/2049 与意图日志+状态区双向互压），META blk0→blk4704（rollback 未接线零生产字节），DATA_TSDB_BASE_BLOCK 派生改字面锁 8192（让渡区六区段坐标全不变）。**迁移合同如实**：旧布局数据已互覆无可恢复生产状态承诺；旧偏移数据废弃 fail-closed（RegionStateStore Corrupt→默认、outbox 损坏槽跳过、OTA 探针每窗重下）；无存量盘自动迁移，复用盘窗验证新布局冷启动。同卷 REG-70.2-CK-DUAL-TRIGGER：telemetry_history checkpoint 双约束（8 批窗 ∨ 600s 时长窗（P 值），pending>0 前提）+PersistWatermark 受理/持久/pending 三账（`TSDB-CHECKPOINT-DUR` 行尾追加 pending_batches= 字段——旧 gate `ms=[0-9]*` 提取不受影响；持续失败期 pending 显式增长不称 ≤8 批丢尾） | 本表（70.2） |
| REG-70-PREEXISTING-GATE-DEBTS | `xtask qa gate orphan-check`（6 孤儿源文件：control-services/{acquisition,collector}.rs、resource-registry/{aggregate,import}.rs、objective-engine/migrate.rs、iec101/asdu_bridge.rs）+ `xtask qa gate config-refs`（planner-peer/src/main.rs 引用 crate 根 planner-peer.toml 非 configs/ 面） | **部分收口（70.1/70.2 会话）**：①orphan-check——门自身「编译验证后确认即删除」（GV-09 真死文件收尾路径）在本次运行中收编 6 孤儿文件（control-services/{acquisition,collector}.rs、resource-registry/{aggregate,import}.rs、objective-engine/migrate.rs、iec101/asdu_bridge.rs，携 14 个死测试；各 crate 编译验证通过），门现绿；②config-refs 仍 FAIL——planner-peer/src/main.rs 引用 crate 根 planner-peer.toml（非 configs/ 面，68.5 起预存），修法=迁 configs/ 或门白名单 crate 根测试配置，归后续卷。注：orphan-check 的删除语义应在执行前显式告知（本次运行时 discover，登记为过程事实） | 本表（70） |

## E-70-R. 蓝图 70.5 复查登记（2026-09-11，70.5 独立复查会话）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.5-TLS-BOOT-START-SHELL | `crates/protocols/mqtt/src/plugin.rs`（start 内同步 TCP+TLS open）+ `crates/protocols/protocol-services/src/protocol_ladder.rs`（start Err → `Ladder::Degraded(PluginStartFailed)` 无重试） | **Open（70.5 复查，候选 70.6 管理动作窗）**：release 臂 boot 期 broker 暂不可达 / 握手瞬断（单包丢失即 `ConnectFailed`，无握手期 50 轮宽限豁免——宽限只覆盖 Timeout 类）= 插件进程**永久能力壳**；70.4 惰性建联+退避重连语义只覆盖 start 成功之后窗，与卷内"断链经 reconnect 完整重握手恢复"自相矛盾。修法候选：start 的 TLS open 失败不置壳而保持未连接句柄交 poll 期退避重连（与明文插件"start 期传输未连接"意图对齐），或 ladder 对 PluginStartFailed 有界重试——ladder 生命周期为全协议服务共用面，须独立裁决防次生 | 本表（70.5 复查） |
| REG-70.5-MGT-GATE-NO-POSITIVE | `tools/run70-vol5-gate.sh:204-231`（R6 判据 = `MGT-PRINCIPAL rejected` ≥1，不分拒绝原因；窗A 无任何**配对正确密钥**的 v2 请求） | **Open（70.5 复查）**：三种拒绝共用词形前缀（unverified shape / {e:?} 验签类 / wall clock unavailable）——X6 墙钟断（全 `ERR:time-unavailable`）或验签回归"一律拒"时 R6 照样 PASS；"合法 token 能过验签"零门禁证据（且正是 P0 短尾缝隙与验签面之间的判别面）。修法 = 70.6 窗补 station 正例注入（broker transcript 断言执行回执）+ 短尾 v2 负例（本卷 P0 攻击帧形态入 gate 作 R6b） | 本表（70.5 复查） |
| REG-70.5-CAPSULE-STALENESS | `crates/boot/boot-manager-uefi/src/main.rs`（capsule 页 Address 预留）+ `crates/runtime/root-task/src/service.rs::identity_freeze`（六字段裁决冻结）+ `crates/boot/boot-protocol/src/witness.rs`（见证 wire 单源） | **Done（蓝图 77 卷二 WP-77.2.1/2.2，v0.350.0）**：①代码路径关闭——capsule 页 Address 预留前移到任何 `prepare_attempt` 持久扣次之前，失败 = `IDENTITY_CAPSULE` 结构化拒绝硬关停（原 soft-degrade 退役；变异 = 预留挪回 loop 后 → 源序锁 `reservation_order_lock.rs` 红）。②本次启动见证——boot-manager seal `BootInfo`（ABI 1.1 增 `security_floor`）→ elfloader 补丁 0003 验证 + 版本化 mailbox 信封（0x427FE000）→ 内核补丁 0004 快照 + 具名 extra bootinfo chunk（id 0x454E5237）→ root `identity_freeze` 从 extra 区取出，与 capsule 页六字段（boot_id/实际槽/manifest_digest/generation/secver/floor）比较后冻结 Validated/Absent/Rejected；Rejected 永不兜底、qemu-test-synth 仅 Absent 合成；所有消费点（identity push/OTA 探针/管理探针/svc-storage 身份页）同源冻结结果。**本卷判定取代旧提案**：`product_version == CARGO_PKG_VERSION` 单点核验不足以根治（同版本不同 boot ID/槽/代次可漏过）——六字段独立见证为准，product_version 只作诊断。物理前提实证：注入窗 `tools/run77-vol2-inject-gate.sh` 三窗（正例 / capsule-alloc 占页+旧字节保留=硬拒绝+BOOTCTRL 逐字节不变 / capsule-stale=六字段拒绝+零身份推送），占页为注入建模（非固件自然冲突）——登记如实 | 本表（70.5 复查 → 77.2 收口） |
| REG-70.5-OTA-FETCH-TOCTOU | `crates/agents/business-services/src/bin/svc-ota.rs::fetch_config`（get_entry("ota") 与 get_generation 两步非原子） | **Open（70.5 复查，登记不修）**：两步取数间配置束轮换可装配"旧 entry 几何 + 新 generation"杂合 policy——方向上大半 fail-closed（root_key 取自 entry，旧签才可过），且轮换=停机换包（§9 冻结表）下窗口不可达；在线轮换立项前必须先单事务化取数 | 本表（70.5 复查） |
| REG-70.5-DESC-NONCANON-WORD | `crates/agents/agent-sdk/src/ota_wire.rs`（magic 错/非 canonical 填充均 `DecodeError::Short`）+ `ota_service.rs`（parse 错误统一 `ERR:len`） | **Open（70.5 复查，诊断缺口登记）**：镜像摘要篡改（desc-signature-invalid）与填充区篡改（ERR:len）失败环节不可分；功能失败关闭无洞，纯可诊断性——改词形须与 gate 判据/负例同频评估（词形退役惯犯类），归后续卷 | 本表（70.5 复查） |
| REG-70.5-MGMT-EXPIRES-UNIT | `crates/agents/business-services/src/mgmt_rpc.rs::is_valid_at`（now_ms 单调毫秒）vs `AuthenticatedSession::expires_at_ms`（v2 = unix 毫秒） | **Open（70.5 复查，登记不修）**：单位错配使 is_valid_at 对 v2 恒真——真过期判据在 `verify_v2_tail`（无活洞），但 `AuthenticatedSession` 作为公共合同藏着单位陷阱（v1 面同样经它）；`tenant_scope`/`channel_binding` 全链无授权消费点（签名绑定但未强制——70.5 卡面自认，授权消费设计归 70.6+ 管理面卷） | 本表（70.5 复查） |
| REG-70.5-TLS-DROP-SOCKET | `crates/security/tls/src/target_factory.rs::TlsTransportHandle::drop`（只还缓冲守卫，内层 socket 无显式 abort/归还） | **Open（70.5 复查，登记不修）**：Drop 不经 close() = smoltcp socket 槽位无显式归还/对端无 FIN——当前唯一析构路径 `plugin.stop() → client.disconnect() → transport.close()` 显式（Drop 不可达）；若未来出现 handle 被丢弃的恢复路径须先补此语义 | 本表（70.5 复查） |
| REG-70.5-SVC-MGMT-HEADER-MAGIC | `crates/agents/business-services/src/bin/svc-mgmt.rs::handle`（req[0..5] "MGT1" 魔数从不校验） | **Open（70.5 复查，观察项）**：分发器（control-services）按服务路由已隔离异协议流量；本面零当前攻击者，登记防误接通（若有 bin 直连 svc-mgmt 端点的新装配，先补头部校验） | 本表（70.5 复查） |

## F-70-7. 蓝图 70.7 登记（2026-09-12，v0.337.0 会话）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.7-BOOT-DEV-IO-HANG | `crates/boot/boot-protocol/src/boot_control.rs::decode` + `crates/drivers/drv-svc/src/blk_backend.rs::try_new_boot` + `crates/drivers/storage/src/driver/virtio_blk.rs::device_init` | **Done（70.7 终局，2026-09-12）**：原登记"boot 队列运行期 I/O 设备端不完成"系误判链——真根因 = ①svc-storage 32 KiB 栈在 boot 路径溢出（`decode` 的 encode-roundtrip 物化第二份 4 KiB → 12 KiB 帧 + `try_new_boot` 双缓冲 16 KiB 帧；seL4 guard 页 data abort `addr=0x225930` 与 `stack_map 0x226000..0x22e000`/guard `0x225000` 吻合）→ ②supervisor 重启后新实例 guest 环归零而**设备侧 `last_avail_idx` 残留**（`device_init` 无 STATUS=0 复位）→ QEMU `virtio_error("Virtqueue size exceeded")`/`"virtio-blk missing headers"` 置设备 broken（`-d trace:virtio_blk_*` + QEMU stderr 并存实证）→ 此后**所有**请求静默丢弃（`[blk-wait] TIMEOUT … last_used=0`）。修复 = 解码帧零 4 KiB 缓冲（`is_canonical` 逐字段复核，与 encode-roundtrip 同判）+ `encode_into` 单缓冲 API + 双副本读入影子原地 + `device_init` 标识读后显式 STATUS=0 复位。帧证据（llvm-objdump 探针页枚举）：`decode` 12 KiB→<4 KiB、`try_new_boot` 16→8 KiB、boot 事务 16→4 KiB | 本表（70.7） |
| REG-70.7-BOOT-TRIAL-IO | `crates/drivers/drv-svc/src/block_cmd.rs` + `blk_backend.rs::write`（写守卫判定式两处各抄一份，且把**字节**长度直接当扇区相加） | **Done（70.7，2026-09-12）**：4096B BOOTCTRL 记录写（lba 18432）被算成 `18432+4096` 越出分区 → 安装事务恒 `ST_UNAVAILABLE`（串口 `BOOT-TRIAL: rejected (io)`，run54）；单源化 = `boot_guard::install_write_allowed(ranges, lba, count_sectors)` + `InstallBackend::write_allowed(lba, len_bytes)`（字节→扇区上取整**只此一处**）；回归锁 = `boot_face_uses_sector_lba_and_guard_writes` 追加 4096B/8192B/跨界/端到端写读回断言（变异 `len_bytes as u64` 复现旧缺陷即红，已实证） | 本表（70.7） |
| REG-70.7-OTA1-DOUBLE-SUB | `crates/agents/business-services/src/bin/svc-ota.rs::handle_boot_install`（0x0B trial / 0x0C confirm 臂） | **Done（70.7，2026-09-12）**：`BlockIo::cmd` 已前置子码字节，两臂又把子码放进载荷 → 服务端解析全体右移 1B（读面 lba 串位 `[blk] read OutOfRange block=9437189`，run56 实弹；trial/confirm 的 secver/image_id 成噪声 → boot-manager 侧表现为 SECURITY_BINDING 拒绝）。单源化 = `agent_sdk::block_io::boot_body::{trial,confirm,read}`（body only，长度即判据）+ 宿主组帧锁 `boot_install_bodies_are_body_only`（长度 + 逐字段位置 + 组帧后端到端解析三重） | 本表（70.7） |
| REG-70.7-BOOT-TRIAL-BIND | `crates/runtime/root-task/src/mgt_probe.rs`（trial 身份硬编码 `boot_trial_frame(2, [3u8;16])`）+ `tools/run70-vol7-gate.sh`（B 载荷注入取 BOOT_A） | **Done（70.7，2026-09-12）**：boot-manager 选槽以 `TrustPolicy.expected_security_version/expected_release_id` 绑定候选 manifest——硬编码身份 + 注入 B 的 BOOT_A 载荷（manifest `slot=A`）三重不符 → `SECURITY_BINDING` 回退槽 A（run55）。修复 = ①门内注入改取 B 的 **BOOT_B** 分区（skip=36864，manifest 按槽绑定）；②新增只读候选身份读面 OTA1 **0x0F**（svc-ota → BLK_BOOT_GUARD → BLK_BOOT_READ 读非活动槽首部 manifest → `ota_service::candidate_identity` 解 secver/release_id）供探针装配 trial——记录值仍走 wire，绑定判据不放松；③确认臂改用 capsule(release_id/secver) 记录**运行载荷自身**身份。宿主锁 `candidate_identity_from_manifest_bytes`（真编码器流转 + 截断/total 越界/魔数损坏拒绝） | 本表（70.7） |
| REG-70.6-GATE-T1-WINDOW-KILL | `tools/run70-vol6-gate.sh`（`--ready-marker root-heartbeat --qmp-deadline-secs 700`） | **Open（观察项，2026-09-12 70.7 回归复跑）**：T1 窗 QEMU 被 harness 超时强杀（`xtask: FAIL: QEMU did not shut down successfully (exit=124)`）而窗内 9 条判据全 ✓、串口尾部系统健康（tick≈33k、TSDB-DISK-COMMIT 持续、零 PANIC）、T2 窗正常退出——标签"首条 root-heartbeat 即退窗" + 修复后磁盘真落盘（TSDB 每 tick）负载下的关停时序脆弱；**门判定仍 PASS，证据有效**（非功能缺口）。修法候选：关停改 `--ready-marker probe-complete`（70.7 已用，退窗点后移且确定） | 本表（70.7 回归） |
| REG-70.7-GATE-R7-PATTERN | `tools/run70-vol7-gate.sh`（R7 判据） | **Done（70.7，2026-09-12）**：判据模式 `boot-evidence.v1..result...PASS..slot...B` 的 `..` 段实际为 3 字符（`","`），对 boot-evidence JSON **恒不匹配**——R7 自门禁建立起从未真正验证过槽位（run57 实弹：设备已 `"slot":"B"` 引导且 R9/R10 通过，R7 仍红）。改字面锚 `"schema":"eneros.boot-evidence.v1","result":"PASS","slot":"B"` | 本表（70.7） |

## F-70-REV. 蓝图 70.6/70.7 独立复查登记（2026-09-13 复查会话）

> 范围：`dda5545..621776c`（70.6 + 70.7 全部开发内容）代码逐面复查 +
> 宿主全量复跑（8840/0 复核属实）+ 门禁证据独立复算（70.7 R1-R10 逐条
> 串口实证、70.6 R1-R26 逐条对照：均属实）。本表登记复查发现的新缺陷
> 与结论订正。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70-REV-BUILD-PROD-BREAK | `crates/runtime/root-task/src/service.rs`（`mgt_probe_step` 调用点无门控） | **Done（P0，独立复查修复）**：`mgt_probe_step` 定义带 `#[cfg(feature = "qemu-test-synth")]` 而调用点无条件 → **生产 profile 全部编译断裂**（`sel4-eipc` 无 `qemu-test-synth`：qemu-prod/qemu-prod-sv2/board-prod/recovery/factory 五条；`error[E0599] no method named mgt_probe_step`）。两处出口均未覆盖：70.7 门只构建 qemu-test；`xtask build all` 对 `sel4:` 选择器提前返回（不触发 cargo 构建）。修复 = 调用点同门控 + `#[cfg(not(...))] let _ = (…);` 消未用告警；**CI 回归步** = `.github/workflows/ci.yml` seL4 job 新增 `cargo check -p eneros-root-task --target aarch64-unknown-none --features sel4-eipc`（与镜像工厂特征解析同源） | 本表（独立复查） |
| REG-70-REV-SECRET-REDACT | `crates/agents/business-services/src/bin/svc-mgmt.rs::ConfigGet` | **Done（P1，独立复查修复）**：脱敏名单硬编码 `["ota","security"]`，**漏 `ai-peer`**——该条目 payload 含 KMS 注入的 32B `degrade_lease_key`（与控制链密钥同密级），operator 会话一次 `config-get ai-peer` 即取回全量密钥字节（`config_service::handle_query::GET_ENTRY` 原样回 payload，无条目级脱敏）。修复 = 名单改消费单源常量 `eneros_config_bundle::SECRET_BEARING_CONFIGS`（ota/security/ai-peer）；两侧一致性锁 = `secret_bearing_covers_every_injected_config`（注入面 ↔ 脱敏表双向）+ 前提锁 `injected_key_material_is_present_in_payload`（注入密钥确实在 payload 内，否则脱敏表空转）。**残留（登记不修）**：卡面「非 admin 拒 / admin 可取载荷」的 admin 分支未实现——admin 亦只回摘要；本期按「密钥字节不出服务边界」从严，口径待裁决 | 本表（独立复查） |
| REG-70-REV-CFGSET-ERR-AS-OK | `crates/agents/business-services/src/bin/svc-mgmt.rs`（ConfigSet `'S'` 臂） | **Done（P1，独立复查修复）**：判据「非空响应即受理」把 svc-config 的 `ERR:*` 拒绝当成功——`ERR:part-index` 的 `r[0]`='E'(69) 被格式化成 `config-stage-part:69/N accepted` 并以 **Completed** 副作用写入幂等账本（同 request_id 无法重试）。根因在传输层：control-services `service_slots.rs` 只在 `!ok && resp.is_empty()` 时补 `ERR`，**handler 返回 false 但带 `ERR:` 文本时该文本被原样当成功响应**（`Call` 无状态位）。修复 = 调用点先判 `starts_with(b"ERR:")` 为失败（与 root MGT2 的 `ERR:` 词形纪律同源）+ 空响应单独成失败臂 | 本表（独立复查） |
| REG-70-REV-TRIAL-SLOT-DIVERGE | `crates/drivers/drv-svc/src/boot_install.rs::write_bootctrl_trial` | **Done（P1，独立复查修复）**：非活动槽按 `slot_a.priority >= slot_b.priority` **自判**，而 `BootWriteGuard::resolve` 用 `BootControl::select()`——两者在「B successful 且 A trial 且同级」等平局/试启动态下**相反**：trial 记录会落到**当前运行槽**（下次引导仍选它 = 试启动自我失效），且 svc-ota 候选身份读面（0x0F）按守卫口径读的是另一物理分区 → manifest 身份与载荷不符。修复 = 判据改 `select()` 单源（Recovery/不可选 → `recovery-selected` fail-closed）；回归锁 `trial_targets_opposite_of_select_not_priority_order`（4 种子矩阵 × 断言 trial 恒落 select() 对侧 + 原活跃槽保留回退资格） | 本表（独立复查） |
| REG-70-REV-CFGACT-WRITE-BEFORE-VALIDATE | `crates/agents/business-services/src/bin/svc-config.rs`（ACTIVATE）/ `config_service.rs::activate_staged` | **Done（P1，独立复查修复）**：ACTIVATE 顺序「写盘（提交点）→ 激活（解码/代次/验签）」——**任何激活失败**（坏签名/代次不前进/载荷不可解码）在返回 `ERR:*` 之前已把无效字节写进 `ConfigActive` 区段，下次启动 `load_disk_active` 拒绝该槽并**静默回退嵌入基线** = 一次**失败**的激活把盘上配置回退掉（非崩溃窗，是判定顺序）。修复 = 新增只读预检 `ConfigService::validate_staged`（与 `activate_staged` 前三门同源、零副作用），ACTIVATE 先预检后写盘；`activate_staged` 文档补调用方契约 | 本表（独立复查） |
| REG-70-REV-SIM-TRANSFER-512 | `crates/drivers/drv-svc/src/blk_backend.rs`（`SimMmio::process_queues`，**测试面**） | **Done（P2，独立复查修复）**：仿真设备 READ/WRITE 恒拷 **512B** 而忽略请求长度（`d1.len`）——多块传输的调用方缓冲尾部停在调用前内容（测试读到零缓冲区而非介质），任何「多块写→读回」断言都会**假绿**（宿主 boot 面 4096B 记录读回的既有断言实际只校验了首块，"匹配"来自影子而非介质）。修复 = 两侧按 `d1.len` 传输 + 边界断言。生产路径不受影响（QEMU 真实设备） | 本表（独立复查） |
| REG-70-REV-SHADOW-PARTIAL-WRITE | `crates/drivers/drv-svc/src/blk_backend.rs::BootSlotBackend::write` | **Done（P2，独立复查加固）**：影子副本仅在「整条 4096B 落在副本基址」时同步，其余形状**返回 Ok 却静默跳过**——后续 `read_current` 把旧影子当权威、事务基于陈旧记录规划（无提示分叉）。修复 = 部分/错位写**落盘后回读整条记录重建影子**（驱动读不回才置 `shadow_valid=false`，此后 bootctrl 读面显式 Err = fail-closed）；回归锁 `bootctrl_partial_write_refreshes_shadow_not_stale`。说明：生产写入形态恒为整条记录，本项为契约外形状的纵深防御 | 本表（独立复查） |
| REG-70-REV-INSTALL-WRITE-UNWIRED | `crates/drivers/drv-svc/src/block_cmd.rs::BLK_BOOT_INSTALL_WRITE`（0x06）/ `svc-ota.rs::install_armed` | **Open（登记不修，随 70.7）**：`BLK_BOOT_INSTALL_WRITE` 无生产调用方——`install_armed` 置位后从未消费（`svc-ota` 只实现 0x0A-0x0F 判据/身份/状态臂，0x06 与 `BLK_BOOT_FLUSH` 仅 re-export + `let _ = (…)`）。门禁的 BOOT_B 载荷由镜像构建期 `dd` 注入（`tools/run70-vol7-gate.sh` 段1），故 70.7 出口**不覆盖运行期安装写**——「运行期受限安装器」的载荷搬移链只有单元面证据（`boot_face_uses_sector_lba_and_guard_writes`）。生产真装包路径 = installer-uefi（boot 期 ISO 源）+ REG-70.6-MGMT-DOWNLINK 下行（Open）；补齐须与下行面同波次（验签束 → 分片写非活动槽 → 身份绑定） | 本表（独立复查） |
| REG-70-REV-QEMU-PROBE-WDT | `crates/runtime/root-task/src/mgt_probe.rs`（探针步进的 chunked Call）+ `crates/runtime/root-task/src/service.rs`（WDT/看门狗） | **Open（P1，复查会话实弹，**非本次改动引入**）**：2026-09-13 复查会话复跑 `tools/run70-vol7-gate.sh` 时 T1 窗探针**确定性停顿**——两种形态：①停在第 2 步（`maintenance-enter` 已发出、svc-mgmt 无应答）→ 看门狗 `PANIC #1 at mgt_probe.rs:342 / WDT unmapped`；②停在第 14b 步（`BOOT-TRIAL: begin len=29` 后 svc-storage 无应答）→ 窗超时。**归因证据**：将本次全部改动 `git stash` 后用**未改动的 HEAD**（同会话重链接 + 同 combined 镜像构造）复跑，**同一停顿再现**（停在第 2 步，`mgt_probe.rs:347` 同款 WDT）——故与本次修复无关，属既有缺陷（与同表的 REG-70.6-MGMT-DOWNLINK「跨服务 EIPC 调用无应答」同族嫌疑：请求已发出、对端零输出、零 PANIC 记录）。**影响**：70.7 门的 R4/R7-R10 现无法复算（R1-R3/R5/R6 仍全 ✓）；已提交的 d2ec466 证据（`evidence/70/70.7/`，`combined.sha256=b39585ff…`）本身经独立复算**逐条属实**，本次复跑失败不动摇既有证据。**下一步**：在 `svc-mgmt`/`svc-storage` 的调用入口加串口标记定位「请求到达但无应答」的分界（是 EIPC 未投递还是 handler 内停），并复核与 REG-70.6-MGMT-DOWNLINK 的同源可能 | 本表（独立复查） |
| REG-70-REV-DOC-X8-DRIFT | `crates/abi/handover-abi/src/lib.rs`、`crates/runtime/root-task/src/{spawn,service}.rs`、`crates/drivers/drv-svc/src/{blk_backend.rs,bin/svc-storage.rs}` | **Done（P3，独立复查修复）**：5 处注释仍称 boot 池基址经 **X8** 直传（实现已是 X7；X8 = C-ABI 第 9 参不可达，run12/13 实弹），其中 `service.rs` 更写着「X8=索引 11 ⇒ count=12」与 `INITIAL_REG_COUNT = 11` 直接相反（误导后续读者重犯 run12/13 的坑）。全部订正为 X7 + 「不可用 X8」的实弹理由 | 本表（独立复查） |

## E-70-8..11. 蓝图 70.8–70.11 登记（2026-09-13，方向 5 独立认证与完整计划执行会话）

> 范围：WP-70.8.1–70.8.4 / 70.9.1–70.9.4 **代码落地**；**70.10 全量通道与
> 70.11.2 计划持有已在第二会话落地**（本行原「未开工」口径已过期——
> 2026-09-13 第三会话订正），**70.11.1/.3/.4/.5 未开工**。
> 执行卡：`蓝图/70-核心七方向全链路深化蓝图/22-执行卡-70.8-70.11.md`。
> 宿主全量 `cargo test --workspace --locked` = **8862 passed / 0 failed**；
> `cargo fmt --check` / `cargo clippy --all-targets -D warnings` / `cargo xtask qa
> feature-matrix`（167 crate，0 违规）全绿；目标静态
> `cargo check -p eneros-control-services --target aarch64-unknown-none --features sel4` 通过。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.8-INPUT-WIRE | `crates/agents/agent-sdk/src/snap_input_wire.rs`（新增） | **Done**：可信快照输入 wire（身份 90B + 144B 状态 = 234B，`SIV1`）单源；决策/证书/执行证据共用同一 `snapshot_id` 引用。旧 `TEL_REQ_SNAPSHOT`（144B 裸遥测）降为差分轨 | 执行卡 §3 |
| REG-70.8-CONSUMER-POS | `crates/agents/control-services/src/event_face.rs` | **Done**：消费位点提交（`SNAPSHOT_CONSUMER_GROUP`）+ 删除界 = min(消费位点, 计划输入持有) + 背压；**无物化快照 ⇒ 零删除**（压容量时以拒发决策换输入完整性）。旧行为（`u64::MAX` 删全部完整段）已退役 | 执行卡 §3/§4 |
| REG-70.8-NAMEPLATE | `crates/runtime/config-bundle/src/{schema,toml_compile}.rs`、`crates/agents/semantic-catalog/*`、`configs/catalog.toml` | **Done**：目录新增字段 14 = 物理参数铭牌（容量/效率/能量上界/功率窗/无功窗/双向爬坡，88B/条）；编译期与解码期双重交叉校验；`registry_from_catalog` 以铭牌为包络基座（缺铭牌资产**不登记** = 不可控）。⚠ **升级次序**：新目录字段对旧 reader 是未知字段（id > MAX_FIELD）→ 必须先升服务再升目录 bundle | 执行卡 §4 |
| REG-70.8-ADMISSION | `crates/protocols/control-dispatch/src/{adapter,engine}.rs` | **Done**：候选身份绑定公式单源（`decision-contract::objective_binding_v1`）+ 引擎可信输入准入（快照/目录/时域/摘要/完整性五判据；`Required` 缺省，**只验证不改写**）；未绑定资产占位在认证前一次性归一 | 执行卡 §3 |
| REG-70.9-TRANSITION-CLOSED | `crates/ai/decision-contract/src/{certificate,batch}.rs`、`crates/protocols/control-dispatch/{Cargo.toml,src/*}` | **Done**：`transition-certifier` feature / `CertifierSeal::issue_transition` / `CertifiedPlan::new_transition` **全仓删除**；引擎唯一发证路径 = 装配的 `CertificateSource` + `verify_reply`（要求 `hard_constraint_count > 0`）。**ADR-051 裁决面待同步**（见下） | 执行卡 §7 |
| REG-70.9-CERT-HOST | `crates/agents/control-services/src/certifier_service.rs`、`crates/ai/certifier/src/context.rs`（新增） | **Done（进程内）**：独立认证来源独立读冻结快照 blob + 目录装配注册表 + 包络/电池/硬约束上下文 → `CertifierModule::certify`；key 由注入材域分隔派生（轮换/退役面在案） | 执行卡 §3/§7 |
| REG-70.9-EVAL-SEMANTICS | `crates/ai/certifier/src/eval.rs` | **Done（本卷接电暴露的两处语义缺陷）**：① SOC 存储窗原用包络 `energy_min/max`（= 可移动能量）当存储水位 → 充电计划恒判越界；改 = 铭牌容量 ∩ 规格 SOC 窗。② 资产级功率/爬坡原入 spec → 被 certifier 按**站点 PCC** 语义裁决而误拒；改 = 资产级约束权威面 = 包络，spec 只承载站点级 | 执行卡 §3 |
| REG-70.10-STRUCTURED-V2 | `crates/ai/intent-contract/src/peer.rs`（v2 schema）、`crates/ai/planner-peer/src/lib.rs`（`solve_request_v2`）、`crates/agents/control-services/src/{remote_brain,structured_plan,catalog_spec_source}.rs`、`crates/runtime/config-bundle/src/{ai_peer,schema,toml_compile}.rs` | **Done（代码+host 锁；QEMU 未跑）**：结构化全计划通道 v2——请求携带目标侧事实（快照身份/目录代次/时域/逐资产规格/价格），应答携带全量分配 + 求解状态 + model/input 摘要；身份逐字段绑定、轨迹完整、**预算越界即拒（无钳制通路）**、条数 ≤64、payload 上限；host **第一步投影与钳制删除**；target 侧 `PlanSpecSource`（目录派生）+ 重连取消在途 + 跨代拒收；`ai-peer` 可选字段 9（计划参数，缺省 = v1 迁移轨）。**复查实锤**：v2 不走 LLM 归一化（全数字 hex 会被去引号 → 合法计划被 schema 拒） | 执行卡 §8.2 |
| REG-70.11.2-PLAN-HOLD | `crates/protocols/control-dispatch/src/plan_hold.rs`（新增） | **代码+host 锁 Done（生产未接线，见 REG-70.11-HOLD-UNWIRED）**：有界计划持有与到期驱动——错窗 = `Expired` 绝不补发、输入换代/缺失 = 未发步 `Cancelled`（已发步不重写）、投递失败保 `Pending`、**单一投递面**（1 活跃 + 1 待切换，活跃排空后接手）、未确认命令有出口（回执 `resolve_step` 或步窗关闭 `Rejected`）。**70.11.1/.3/.4/.5（Saga 激活/多设备/崩溃恢复/演示）未开工**；2026-09-13 第三会话复查修复 4 项（步长下限单源/Dispatched 无出口/并发投递/`inputs_from_facts` 时域硬编码） | 执行卡 §8.3 / §9 |
| REG-70.8.9-REVIEW-FIXES | `ai_handler.rs`/`certifier_service.rs`/`event_face.rs`/`proof.rs`/`context.rs`/`registry.rs`/`toml_compile.rs`/`snap_input_wire.rs` 等 | **Done（独立复查 2026-09-13，三路审计 + 红测/变异实锤）**：D1 `AI_PROPOSE_SNAP` 未路由（**target 生产链静默停摆**——EA 拿 0xFF 当提案，host 全绿掩盖）；D2 认证 SOC 硬约束空洞性（拿量程覆盖当运行窗 → MinSoc/MaxSoc 恒真；铭牌新增 soc_min/soc_max，记录 88B→104B）；D3 位点先于 blob 提交（时序倒置）；D4 复用盘位点越权删除；D5 blob 通道只服务当前代（16 份保留不可达）；D6 key 环 N-1/version/双环不同步；D7 发放面唯一性=feature 合并假象（源码扫描门执法）；D8 零碎（报告恒等式/payload 上限单源/telemetry_f64 下溢/新鲜度窗 30s<实测周期 18–45s → 60s=计划 TTL） | 执行卡 §8.1 |
| REG-70-8-QEMU-PENDING | — | **Open（未验证）**：本卷改变控制主链，**QEMU 目标实弹未跑**（`evidence/70/70.8…70.11` 未建立）；既有 70.1–70.7 证据对应旧主链，不构成本卷 PASS | 执行卡 §5/§7 |
| REG-70.9-DEPLOY-DECISION | `crates/agents/control-services/src/dispatch_assembly.rs::build_certifier` | **Open（部署面）**：蓝图 WP-70.9.1 默认案 A（独立 seL4 进程）**未实现**；本卷落进程内装配（可跑于目标进程、EIPC 读快照）。**不得**把本案记为"独立进程认证已落地"——案 A/案 B 需 ADR 裁决记录 | 执行卡 §7 |
| REG-70.9-KEY-SUPPLY | `certifier_service::certifier_key_from_material` | **Open（外部欠账）**：signing key 由控制链注入材域分隔派生（PRF 独立，无密钥复用），**独立 KMS 条目**（材料分离）未实现——蓝图 §10 已登记"生产认证密钥供给"为外部待证 | 执行卡 §7 |
| REG-70.9-ADR-SYNC | `docs/adr/ADR-051-*.md`、`ADR-0048-*.md`、`product/policy/frozen-crates.json` | **Open（登记不静默）**：① ADR-051 裁决 2「过渡窗延长」已被删除动作超越，需补裁决记录（本次**未改** ADR 文本）；② ADR-0048 §64 的 `rg 'issue_transition'` 收口面已随代码删除失效；③ `frozen-crates.json` 中 certifier「零生产消费者」表述已不成立（dispatch/control-services 现为消费者） | 执行卡 §7 |

## F-70-8..11-REV. 蓝图 70.8–70.11 第三会话独立复查（2026-09-13）

> 范围：`621776c` + 工作树（70.8–70.11 全部开发内容）——五路并行对抗式审计
> （可信输入链 / 独立认证 / 结构化 v2 通道 / 目录铭牌与注册表 / 计划持有）
> + 每条候选缺陷的多维验证（逐行推演 + 红测 + 变异）。
> 宿主基线复跑 = **8904 passed / 0 failed**（与执行卡 H-1r 吻合）；
> 目标侧全部 profile 静态编译复跑通过（含生产特征集）。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.8.3-RAMP-CONTRACT | `crates/agents/resource-registry/src/{capability,registry}.rs` | **Done（P1，复查修复）**：包络求交只收紧 `p_min/p_max`，**合同爬坡 `ramp_kw_per_s` 从不进入包络**——而 70.9 把资产级爬坡的权威面从 `spec` 移到包络（D2 语义订正），于是**运行合同爬坡在认证面失效**（认证用铭牌物理能力，执行侧 `ramp-limit` 用合同值 → 证书证据与执行判据不一致）。修 = `CapabilityEnvelope::intersect_ramp`（逐向 min）+ `evaluate` 调用。红测（包络级 + 认证级双面，放宽合同后同计划出证）+ 4 组变异咬合。**语义选择（显式登记）**：`ramp_kw_per_s == 0` = **未声明**（编译面该键可选、缺省 0.0）→ 不施加约束；把它当"禁止爬坡"会让一切无该键的目录（含既有测试目录）拒绝所有计划。若要表达"禁止爬坡"须显式给极小正值 | 本表 / 执行卡 §9 |
| REG-70.8.1-FRESHNESS-VALIDITY | `crates/agents/control-services/src/telemetry.rs::snapshot_input` | **Done（P1，复查修复）**：新鲜度窗（60s）宽于**载荷自身控制有效期**（`valid_until_ms` = 产刻+30s，dispatch 以同一字段裁决）→ 40s 龄输入被受理、dispatch 以 `TelemetryExpired` 拒绝、EA 记 `DispatchRejected`（连续 3 次 → EscalateToHuman + 冷却）= 把"输入已过期"伪装成"决策被拒"并烧错误预算（冷却还可能把下一个有效窗推出有效期，形成自维持降级）。修 = 双上界 `min(载荷有效期, 策略窗)`（单源 = payload `valid_until_ms`）+ 载荷不可解即拒。红测三面（窗内受理/超有效期拒/策略窗更紧拒） | 本表 / 执行卡 §9 |
| REG-70.10-HOST-V2-UNWIRED | `crates/ai/planner-peer/src/main.rs` | **Done（P1，复查修复）**：host 生产 bin **只解析 v1**（v1 schema 要求 `telemetry` 且 `deny_unknown_fields`）→ 目标侧一旦配置结构化计划参数发 v2 请求，host 恒 `PLANNER-SKIP reason=parse`、**永不应答** → 目标每周期超时降级 = 结构化通道**端到端不可用**。修 = 版本探针单一入口 `route_version`（V1/V2/Unsupported/Malformed；闭集外不猜）+ `handle_request_v2`（求解/组装/自检/发布与 v1 同律）+ 处理器对传输泛型化。**bin 级 e2e 测试**（会话假 broker → v2 请求 → 发布 → 目标侧同链校验）+ 路由锁；测试夹具两处 >127B varint 假设一并修复（`session.rs` 帧构造与 QoS1 自动 PUBACK 的 pid 定位——正是"大载荷测试不存在"的构造性原因） | 本表 / 执行卡 §9 |
| REG-70.10-V2-SEND-DISCIPLINE | `crates/agents/control-services/src/{remote_brain,catalog_spec_source}.rs` | **Done（P2，复查修复，三处）**：① 规格源返回 `None`（跨代/无可控资产/价格或预算不可用）时**落到 v1 迁移轨**——v1 产物在生产 `Required` 策略下必被拒，白烧往返 + EA 错误预算，且违背文档承诺；改为**结构化轨已选定即不发任何请求**（fail-closed）。② 请求包络未收进本机预算（`ai-peer.max_power_kw`）→ host LP 最优解必越预算 → host 按"越预算即拒"不应答 → 通道自阻塞；改为**请求窗 = 铭牌 ∩ 合同 ∩ 预算**（收窄请求正当；应答越预算仍拒，无钳制通路）。③ v2 应答**不校验请求资产覆盖**（缺资产 = 部分计划被记为"全量"）；改为缺任一请求资产 = 坏应答（不做部分采纳） | 本表 / 执行卡 §9 |
| REG-70.10-V2-VERSION-GENERATION | `crates/agents/control-services/src/remote_brain.rs` | **Done（P2，复查修复，两处）**：① v1 应答可"顶替"v2 在途请求（反向 t8d 缺守卫）——在途被清、seq 前进、真 v2 应答变 `Late`、legacy 候选被生产入口拒；改为 v1 应答遇已绑定在途 = 坏应答。② 应答**接纳后消费前输入换代** → 以新身份 + 旧时域拼出杂交候选（dispatch 必拒 + 烧预算）；改为消费前复核绑定三元组（与准入同集合），不符即丢弃走 System1。③ v2 审计标签用 `solver_status` 当 reason 替身 → `backend_tag` 恒 `remote-llm`（planner 与 LLM 不可区分）；改为携带应答 `reason` | 本表 / 执行卡 §9 |
| REG-70.11-HOLD-UNWIRED | `crates/protocols/control-dispatch/src/plan_hold.rs`、`crates/agents/control-services/src/{event_face,telemetry}.rs` | **Closed（2026-09-25，superseded-by-ADR-0054 + 蓝图 75 WP-75.1.0 删除，提交哈希见 B75-1 登记）**：`PlanHolder` **生产零调用方**（全仓仅 `lib.rs` 的 `pub mod`），`has_outstanding_input` / svc-telemetry `hold_input_offset` 同样零调用方 ⇒ **70.8.2 删除界的"计划输入持有"半边在生产恒空**、70.11.2 的"装配方轮询接线"从未存在；且 `hold_offset` **只收紧无释放面**（首个接线方会把删除界永久钉住 → 保留背压常驻）。**销项**：生产保留面已由 ADR-0054 durable 路径（held_plan/held_engine/hold_registry + 装配面 `dispatch_assembly.rs` TelemetrySnapRetainer）承担；会话级原型 plan_hold.rs（1179 行、12 测试、BatchSink 假想 seam 唯一实现者=自身测试）经孤儿门同判据（删文件+剥 mod 后仍编译）永久删除。若未来 telemetry 删除界需复用「持有半边」读点形态：改道 `HeldInputRetainer`（生产已接），不预埋 | 本表 / 执行卡 §9 / 蓝图 75 卷一 §2 |
| REG-70.9-MULTIASSET-SOC | `crates/ai/certifier/src/context.rs`、`crates/agents/control-services/src/catalog_spec_source.rs` | **Open（P2，登记不修）**：认证/建模把**每一个被分配资产**都建成"电池"，初始能量一律取**站点级单一 SOC 槽**（`FeedbackSlots.soc`）——目录只需增加第二个带铭牌资产即可触达（无代码变更）：资产 #2 会以资产 #1 的 SOC 建模并被出证（真值可能远在 SOC 窗之外）；同时各资产 SOC 窗被**摊平成一份 spec**，逐资产取交集（过严误拒）。修法 = 逐资产反馈槽（目录字段 8 语义扩展）——设计面工作，属 70.8.3/70.11.3 | 本表 |
| REG-70.9-KEY-LIFECYCLE | `crates/agents/control-services/src/certifier_service.rs`、`crates/ai/certifier/src/proof.rs` | **Open（P3，登记不修）**：`rotate`/`retire`/`set_certifier_ring` 全仓零生产调用方 ⇒ 运行期密钥轮换/退役**不可操作**；`key_version` 仅进程内单调（重启后由常量派生重置为 1）⇒ 同一注入材重启后旧 `(id,version)` 证书可再次验过。生产装配的验真环取自发证者自带环（同进程），故今日无攻击面；独立 KMS 条目已在册 | 本表 / 执行卡 §7 |
| REG-70.9-CERT-ID-ONLY | `crates/agents/control-services/src/certifier_service.rs:131`、`crates/ai/certifier/src/context.rs` | **Open（P3，登记不修）**：证书/快照绑定仅比 `snapshot_id`——不比快照 `version_set` 与目录代次、不比 `produced_at` 与计划 `horizon_start`；`verify_reply` 亦不交叉核对证内 `snapshot_id`/`constraint_catalog_version`/`tolerance_profile_id` 与计划。可达性：需能构造提交请求的直接调用方（今日仅 svc-energy-agent 持该授予，诚实链自取自证）——纵深防御缺口，非现行漏洞 | 本表 |
| REG-70.10-TRANSPORT-CAPS | `crates/ai/intent-contract/src/peer.rs`（4096/8192）vs `crates/agents/agent-sdk/src/{ai_relay,chunked}.rs`（1024/1664）、EA→dispatch（1664） | **Open（P2，登记不修）**：契约上限与**实际传输预算**不一致——v2 请求 >1024B（3 资产×4 时段的 1112B 即超）在 AI 中继分段处被拒；v2 应答 >~1663B（12 条分配 1695B 即超）在 svc-mqtt→worker 的 chunked 请求向被丢；EA→dispatch 请求 1664B 上限 ⇒ **候选 ≥44 条分配无法提交**（契约允许 64）。三者叠加使"全量计划"通道今日**实际上限 ≈11 条分配**——这是 70.10/70.11 的真实容量墙（需传输扩容或分页：属蓝图后续卷），不是单点缺陷 | 本表 |
| REG-70.8.3-CATALOG-CAPACITY | `crates/runtime/config-bundle/src/schema.rs::PAYLOAD_MAX` | **Open（P2 容量发现，登记不修）**：目录 payload 硬上界 1664B（与 chunk 请求预算同源），实测生产目录 **1097B**（含 1 条铭牌）——铭牌 104B/资产 + 资产记录 32B ⇒ **剩余余量仅约 3-5 个带铭牌资产**；多资产站点（方向 6/7）会先在编译面撞墙（fail-closed，不静默）。容量口径与扩容路径需在蓝图层面裁决 | 本表 |
| REG-70.9-ENVELOPE-30S | `crates/agents/resource-registry/src/provider.rs::base_env` | **Open（P1 架构面，登记不修）**：包络有效期 = 快照产刻 + **30s**（`valid_until`），认证器**逐步**校验步起点落在包络窗内 ⇒ **任何跨度 >30s 的多步计划无法被认证**（重编译重提交也一样）。这与 70.11.2 假设的"多时段/跨 tick 计划"（其自身测试用 1h 步长）**架构上不相容**——70.11 的多步执行需先裁决"包络生命周期 vs 计划时域"（放大包络窗 / 逐步重认证 / 冻结节奏），属蓝图决策 | 本表 |
| REG-70.10-CONTRACT-CAPS-VS-REALITY | `crates/ai/planner-peer/src/lib.rs` | **Open（P3，登记不修）**：`input_digest_v2` 只覆盖 快照/目录/时域/步数/价格/目标，**不含逐资产规格与预算** ⇒ 审计方按其复算无法发现"host 用了不同资产事实建模"（摘要-MAC 绑定的是这个较弱摘要）。收紧须改 v2 摘要公式（wire 语义变更，宜与传输扩容同波次） | 本表 |
| REG-70.8-SYNTH-NO-OBS | `crates/runtime/root-task`（qemu-test-synth 合成源）、`crates/agents/control-services/src/event_face.rs`（惰性打开）、`configs/catalog.toml`（source 1 注释） | **Done（P1→收口，2026-09-15 红测先行 + 实弹探针窗验证）**：根因 = `configs/points.toml` **有意不注册 source 1**（"合成样本只走旧 TelemetrySample 兼容轨，永不产生 ObservationEvent"）⇒ OBS 生产轨来源核验 `UnregisteredSource` 隔离。**修法（合成源升级为 OBS 生产轨，红测先行）**：① `PointProtocol::Synth`（PROTO_SYNTH=8，封闭枚举诚实扩员）+ `[[source]] source_id=1 protocol="synth"` 注册行（allow_control=true、require_integrity=true、范围 1..=1004 = 声明点跨度；release 闭包门保证生产装配无任何路径以 source 1 发帧——行在生产不可达）；② agent-sdk `synth_feed` 单源构帧模块（11 点馈电、epoch=0+SIMULATED\|TEST 过 decode 门、批内/批间 sequence 严格单调、LocalTrustedEdge）；③ root `synth_obs_ingest`（TEL_INGEST_OBS，每 3 round 一批 ≈3.4 事件/s 与 modbus 采集同量级；结果只上 `SYNTH-OBS:` 证据行，不伪造不静默）。**红测 5 例全先红后绿**（注册面/受理面 e2e/镜像锁/decode 门/单调性）。**实弹探针窗（420s×2）**：首轮 16-round 批距使 EA 恒 `snapshot-stale`（SNAP_EVERY_BATCHES=8 ⇒ 建距超 60s 新鲜度窗）——批距重标为 3 round；次轮 **40 批全受理、SNAP-BUILDed ×10（pts=8 含回读点 1001）、EA status=OK ×11、PLAN-COMPILED batches=1 intents=1、delivered=1**，终态 `Indeterminate: delivery-error Rejected（物理结果未知需对账）` = 合成装配无真实执行器的**预期安全终态**（认证→编译→投递链全通）。间歇 `SYNTH-OBS: transport FAIL` 与 legacy ingest 的 chunked 传输间歇同族（svc-telemetry 长 checkpoint 阻塞应答；vol7 窗已有 5 例同形——预存，非本次引入） | 本表 / 执行卡 §9.5 |
| REG-70.10-RECONNECT-UNWIRED | `crates/agents/control-services/src/remote_brain.rs::on_session_reconnect` | **Open（P3，登记不修）**：方法存在且有测（t8c），但**零生产调用方**（svc-mqtt 重连不通知 svc-ai-worker）⇒ 会话重连后旧 pending 只能靠 30s 超时清除。接线属 svc-mqtt→worker 事件面工作 | 本表 |

## F-70-12..14. 蓝图 70.12–70.14 登记（2026-09-13，方向 6 真实板卡与设备兼容会话）

> 范围：`949ce53` + 工作树（方向 6 的可移植代码与判据部分；实板出口因外部
> 实物缺失保持 BLOCKED）。执行卡：`蓝图/70-核心七方向全链路深化蓝图/
> 23-执行卡-70.12-70.14.md`。宿主基线复跑 = **8991 passed / 0 failed**。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.12-SKU-UNFROZEN | `product/skus/{phytium-d2000,kunpeng-920}.json`、`product/skus/board-selection-criteria.md` §5 | **Open（外部待证，判据已就绪）**：库内两份产品 SKU 记录已升 **v2 结构化账本**，全部硬件事实仍 `verified=false`，13/14 个能力族逐族登记 `structured_missing`；owner 选板签字位为空 ⇒ C10（TRNG 手册）/C14（寄存器手册）两项历史双高风险硬门无法解除。判据已生效：`kind=product` 无 v2 账本即 `sku-fact-ledger-required`；`status=qualified` 追加能力覆盖硬门（缺 storage/network-phy/bus/trng/rollback-counter/power/watchdog 任一即 `capability-unverified:<族>:<判据>`）。**不得用 QEMU 事实或默认容量填空** | 执行卡 §3/§7 |
| REG-70.12-ADAPTER-REGISTRY | `crates/runtime/root-task/src/board_select.rs::REGISTERED_ADAPTERS`、`crates/product/board-facts/src/registry.rs` | **Open（设计如此，外部待证）**：实板 adapter 注册表**当前为空**——`board-prod` 镜像一旦声明 `ENEROS_BOARD_*` 即在装配期被拒（`board-no-adapter-for-sku`），**不回退 QEMU**。这是对旧行为（`platform.rs` 硬编码 `QemuVirtAdapter`，任何镜像都用 QEMU 平台事实）的修正。板卡型号冻结后在此登记（含修订与冻结记录摘要）；在此之前实板启动出口保持 BLOCKED | 执行卡 §3/§7 |
| REG-70.12-ENTROPY-REAL-TRNG | `crates/security/crypto/src/rng/entropy_health.rs`、`entropy.rs::PhytiumTrng/KunpengTrng` | **Open（外部待证，判据已就绪）**：健康判据（不同块数/取值域/单字节最小熵/有界轮询）与 fail-closed 接缝已落地并被 `platform_entropy_seed` / `csrng_from_platform` 消费；**真实 TRNG 寄存器接口与噪声模型仍缺**（厂商文档 + B0 实测）——`PhytiumTrng`/`KunpengTrng` 仍恒 `Unavailable`。判据口径 = 退化检测（SP 800-90B 启发型最小子集），**不是**完整熵评估 | 执行卡 §3/§7 |
| REG-70.12-DESCRIPTION-BINDING | `crates/product/board-facts/src/platform.rs::vouch_selection`、`registry.rs` | **Done（代码 + host 锁）**：构建期所选 SKU/修订/描述摘要 × 运行时描述必须一致；必需设备（控制台/启动存储/熵/网络控制器）、MMIO 页对齐与非零、以及**一致性声明必须有硬件依据**（无 `Cache`/`Soc` 已验证事实即拒绝 —— 不许把 QEMU coherent 假设搬到真板）。8 例负例覆盖。目标侧证据行 `70.12 BOARD …` 已接电（root 装配失败打印 `reason=`） | 执行卡 §3 |
| REG-70.12-ROLE-BY-DESCRIPTION | `crates/hal/hal/src/inventory.rs`（`DeviceResources.path`）、`board-facts/src/platform.rs::platform_role` | **Done（顺带缺陷修复）**：平台角色（网卡/串口/块设备/熵源）此前只能靠 compatible 厂商串子串猜；改为**先看 FDT 节点路径**、再看 compatible 的通用类别词（`net/ethernet/serial/blk/rng/…`），型号串（`lan91c111`/`d2000-gpio`）不再参与判定。`DeviceResources` 新增 `path` 字段（FDT 解析填充；非 wire/介质变更） | 执行卡 §3 |
| REG-70.14-RUN-JUDGE | `xtask/src/soak.rs`、`tests/soak/{soak72,collect-evidence}.sh`、`.github/workflows/ci.yml` | **Done（判据 + 脚本改造）**：浸泡"时长/产物/增长"改为必须来自原始运行记录——`duration_days` 不再由 `PROTOCOL_HOURS/24` 推算、`artifact_sha256` 不再固定读 `qemu-prod` 路径、删除 `memory_leak_*: 0` 字面值；缺必需证据材料 → 非零退出（`--allow-missing` 显式进入 `INCOMPLETE/non_release` 模式）；自检扩到 4 例（含"参数声称 72h 但实测 2h 判负"）。**真实 72h 与实机 HIL 仍外部待证**（无试验台/无板卡） | 执行卡 §3 |
| REG-70.14-QUALIFICATION-IMPACTS | `xtask/src/board.rs::validate_qualification` | **Done（判据）；资格记录待实板**：技术资格记录须绑当前冻结记录摘要、run 须硬件 scope、required case 非空不重、影响映射逐组件登记；`verify` 模式下"可算组件"（`configs/` 内容摘要）自动复验并**撤销受影响场景**，"不可自动核对组件"（hardware/firmware/driver/protocol）返回 `qualify-impact-unverifiable` **要求人工裁决，不默认未变** | 执行卡 §3 |
| REG-70.12-ORPHAN-GATE-CONFLICT | `crates/protocols/control-dispatch/src/plan_hold.rs`、`xtask/src/gate.rs`（orphan-check）、`tools/gate-exemptions.toml` | **Done（本会话定性并止血）**：orphan-check 的判据是"删掉候选 + mod 声明后仍能编译 ⇒ 真死文件 ⇒ **删除文件**"，而 `plan_hold.rs` 是 `REG-70.11-HOLD-UNWIRED` 记载的**生产零调用方但在案待接线**模块（约 1040 行 + 7 例判据）——门执行即永久抹除，且删除后 `lib.rs` 的 `pub mod` 声明同时消失（本会话多次实测，也是"工作树跟踪文件被外部回退"的真因）。止血 = 按 63.3 先例登记**孤儿豁免**（`expires_at = "WP-70.15"`，status=deferred）：门现判 `ORPHAN-CHECK → 0 orphans` 且文件与声明均保留。**去留（接线 or 删除）归 70.15 裁决** | 本表 / 执行卡 §8 |
| REG-70.12-PROBE-EMPTY-BUNDLE-PANIC | `crates/runtime/root-task/src/mgt_probe.rs` `StageParts` 回放分支、`root-task/build.rs`（`ENEROS_MGT_PROBE_BUNDLE` → `include_bytes!`） | **Done（次生缺陷：QEMU 实弹暴露 → 修复 + 实测验证）**：探针载荷是**构建期**注入；构建时未设 `ENEROS_MGT_PROBE_BUNDLE` ⇒ 镜像内 `PROBE_BUNDLE = &[]`，空载荷使 `total = 0` 后进入幂等回放分支并做 `&bundle[..512]` → **越界 panic**（实测 `PANIC #1 at mgt_probe.rs:342 — WDT unmapped`，整条探针链断在 step 3、进程挂起 ⇒ 窗口只能 NO_RESULT）。修 = 抽纯函数 `replay_arg_for(bundle, total_parts) -> Option<Vec<u8>>`（`PROBE_CHUNK=512`）+ 空载荷**显式拒绝并跳过回放**（`MGT-PROBE-REJECT reason=empty-bundle`，fail-closed 不静默成功），两例 host 测试覆盖 `&[]`/511B/512B/1024B。**实测**：修复后同镜像无注入运行，PANIC=0 且探针推进 1→13/13 步 + `boot guard/status/candidate/trial` 各步（对照修复前止于 step 2/13）。**注入口径坑**：运行期设 `ENEROS_MGT_PROBE_*` 不改变已构建镜像内的载荷，必须构建期注入 | 本表 / 执行卡 §5.1 |
| REG-70.12-PROBE-BOOT-TRIAL-STALL | `crates/agents/business-services/src/bin/svc-ota.rs::handle_boot_install`（`BLK_BOOT_TRIAL` 臂）、`crates/drivers/drv-svc/src/{block_cmd.rs,boot_install.rs,blk_backend.rs}` | **Open（非本卷改动面；本会话把边界收敛到可直接接续的程度）**：探针链在 `boot trial sending` 后无输出 ⇒ `MGT-PROBE: sequence complete`（`--ready-marker probe-complete` 判据）从未出现。**已确证**：①请求侧正确（`req.len()==29` = 5 头 + 24 body，长度校验已过，下一动作 = `raw_command(BLK_BOOT_TRIAL)`）②同一 EIPC 客户端的 `BOOT-GUARD`/`BOOT-STATUS` 都在同一会话成功返回 ⇒ 通道/授予/epoch 成立 ③**无任何失败词形**（`rejected (io|len)` 与 `svc_fail_closed` 都没出现）④存储侧无该事务对应的写/flush 回执（日志里重复的 `corr=45324 op=16` 是 storage 的低层块驱动日志，与 trial 无关）⑤`svc-ota` 未挂（同窗口仍在服务）且客户端 5000ms `timeout_ms` **未**落错误分支 ⇒ 挂点在 `raw_command` 内且该路径的无界等待不返回。**嫌疑面**：949ce53（70.8–70.11）改过的 `svc-storage` boot 薄面（影子一致性/非活动槽判据单源）；70.7 实测在同一门内 trial 成功。**插桩定位（2026-09-13 第四轮，三窗同形）**：按建议加了两侧插桩（svc-ota trial 臂 `TX-SENT`/`RX-OK`/`RX-ERR` + 实际阻塞毫秒；svc-storage `BOOT-TRIAL: RX` 与 `write0/write1/flush/done`；`BootSlotBackend::write` 的 `allowed` 分支；`VirtioBlkBackend::write` 的进入行）。结果**三窗逐行一致**：`TX-SENT` → `[svc-storage] BOOT-TRIAL: RX req_len=25` → `write0 lba=18432 len=4096 rec_lba=18432` → `boot-write lba=18432 len=4096 allowed=true`，**到此为止**——守卫已放行、即将调用驱动面 `BlockBackend::write(inner, 18432*512, 4096)`，但驱动面 `blk-write enter` 从未打印、`write1/flush/done` 全无、`RX-OK/RX-ERR` 全无、无 PANIC/无 `blk-wait TIMEOUT`。同时**宿主观测 `%CPU=0.0`（10 次采样）** ⇒ 该 seL4 线程不是自旋，而是**阻塞在内核操作上**（等待/调用），与"四 CPU 全落 `idle_thread`"一致。**第五轮（按上一轮建议再做 pre/post，窗 L）——边界已收到最后一步**：在
`BootSlotBackend::write` 的 inner 调用**前**加 `boot-write inner pre off=9437184 len=4096`
后，该行**打印**；紧随其后的 `BlockBackend::write(&mut self.inner, off, data)`
进入 `VirtioBlkBackend::write` 的**首行**（`blk-write enter`，同函数同字符串在
本窗更早已成功打印 30+ 次）**不再打印**。同窗对照：**其它服务在同一时刻仍在
持续打印**（`svc-telemetry` TSDB 批提交、root CONTROL-CHAIN round=37、
`svc-energy-agent` tick）⇒ **console 未死、系统未整体停摆**。
并已核对三处插桩字符串**确实编进镜像**（`strings -a …eneros-qemu-test.raw`
各命中 3 次）⇒ 不是"插桩没编进去"。宿主 `%CPU=0.0`（窗 J 10 次采样）⇒ 非自旋。
⇒ **判定（判据级结论）**：阻塞发生在 `boot-write inner pre` 之后、驱动方法体
第一条语句之前，且只在该线程、该调用上发生；其余服务不受影响。剩余两种
可能：①**该线程的 console 打印路径被阻塞**（打印持有/等待某把锁，而锁持有者
同在该线程栈上）②**调用边界本身阻塞**（`BlockBackend::write` 的 trait 分派
落到另一个实现/另一实例，其内部在等待）。
**口径订正（同会话，重要）**：④⑤两条里的"客户端 5000ms `timeout_ms`"表述**失准**——
`crates/agents/agent-sdk/src/ipc_sel4.rs::drv_call` 的实现是"先算 deadline → `seL4_Call`
（同步阻塞）→ **返回后**才 `is_expired` 检查"，因此该 timeout **无法约束阻塞中的调用**：
它只是"返回后判超时"，不是有界等待。故"未落错误分支"不能说明超时机制在生效，
只能说该路径**根本没有有界等待**（这是本停摆的一个真实性质，非仅仅观测）。
**第六轮（探针闭环，窗 M）——console 路径已排除**：按上一轮建议把该行改为
"先 `AtomicU32::fetch_add`（`BOOT_WRITE_PRE_PROBES`）再打印"，并在 svc-ota
trial 臂读取该计数（`BOOT-TRIAL-PROBE pre_count=`）。实测链
`begin` → `PROBE pre_count=0` → `TX-SENT` → `RX req_len=25` → `write0` →
`boot-write … allowed=true` → `boot-write inner pre off=9437184 len=4096`，
**该行打印成功**（打印即证明自增先执行且 console 可用）⇒ **console 打印路径
未被阻塞**。阻塞点在**该行之后、驱动方法体首条语句之前**，只在本线程/本调用。
⇒ 唯一剩余嫌疑面 = `BlockBackend::write(&mut self.inner, …)` 这个**调用边界**
（trait 分派目标或其内部首个内核操作）。
### 第七轮（实弹反汇编定位，2026-09-13 同会话）——**真根因候选已定位到指令级**

按上一轮建议加"驱动方法体首行原子计数 + 打印"后，仍未在 trial 首写处看到
任何输出。改用**反汇编**直接看代码（`target/aarch64-unknown-none/release/svc-storage`）：

- `BootSlotBackend::write`（`_RNvXs7_…BootSlotBackend…BlockBackend5write`）
  入口即 `sub sp, sp, #0x1, lsl #12`（**4 KiB 帧**）+ `str xzr, [sp], #-64`
  ⇒ 该函数**在 prologue 就下移栈指针 4 KiB**，早于任何日志/探针语句。
  这正是"trial 首写无任何输出、且整个进程不再前进"的形态学解释：
  **若该栈下移处已越过线程栈上限，则触发 seL4 fault**，线程停在内核、
  无人应答 ⇒ 整机 idle（与实测"四 CPU 全落 `idle_thread`、宿主 %CPU=0"一致）。
- 成因：`BootSlotBackend::write` 内 `let mut fresh = [0u8; 4096]`（部分写分支回读
  记录用）与 `VirtioBlkBackend::{read_block,write_block,erase_block}` 的三个
  `[0u8; 4096]` 局部——都在栈上各占 4 KiB。boot 安装调用链
  （`block_cmd::handle_boot_cmd` → `boot_install::write_bootctrl_trial`（另有
  `rec: [u8; 4096]`）→ `BootSlotBackend::write` → `VirtioBlkBackend::*`）
  在同一线程栈上**串联叠加**，总需求逼近/超过 svc-storage 的 64 KiB 栈。
- **已实施修复（本会话）**：上述四个 4 KiB 局部全部改为**堆缓冲**
  （`alloc::vec![0u8; 4096]`；三个 crate 均已 `extern crate alloc`，且
  `drv-svc` 目标侧已有工作堆）。反汇编复核：`VirtioBlkBackend::write` 帧由
  4 KiB 降到 **0x280（640 B）**；`BootSlotBackend::write` 仍显示 4 KiB 帧——
  **待复核**（可能来自 `write_bootctrl_trial` 的 `rec` 被内联，或是 fp
  推导的另一局部）；这是下一轮第一件事。
- **栈溢出假设已被证伪（同会话内）**：把上述四处（外加 `boot_install` 的四个
  `rec`，改为 `Box<[u8; 4096]>`）全部移出栈后重链接复核，`BootSlotBackend::write`
  的 `sub sp, sp, #0x1, lsl 12` **仍在**——说明这个 4 KiB 帧不是来自这些作为
  缓冲的局部数组（判据：改了来源、帧不变）。因此"栈溢出 ⇒ seL4 fault"这条
  假设**不成立**，4 KiB 帧本身在 64 KiB 栈下也不足以解释停摆。
  改动保留（大缓冲走堆本身是正确形态，且已消除三处真实的大栈帧），但
  **不再声称它是根因**。
- **理论状态**：**根因未定**；已排除的假设清单见 §5.1 第 1–11 条。

**下一步（一次构建 + 一次窗口）**：①换一条判据看 4 KiB 帧的真实来源——
在 prologue 之后插"内联汇编读 sp 并存入全局"，由 root 心跳读出，确认该帧是否
真的越界（若 sp 距栈基 > 64 KiB 则假设复现，否则彻底排除）；②若排除，则回到
`VirtioBlkBackend::write` 的**首条内存访问**（`self.block_size` 读）与
`self.driver.borrow_mut()` 两条路径，用"在首条语句前落一个全局标记"的探针
确认执行到哪一条；③重跑 `probe-complete` 窗验证。**原始定位（同会话前两轮）**：trial 臂加"进入/退出"两行日志或超时，再看是 storage 未回还是 ota 未收。**追加（运行期 QMP 观测，run F，2026-09-13 同会话）**：停滞时刻**四个 CPU 的 PC 全部落在 `idle_thread`**（`0xffffff8040011118`，`wfi` + 回跳自旋；用 `aarch64-none-elf-nm -n target/sel4/kernel.elf` 逐地址判定），PSTATE=EL1h ⇒ seL4 内**没有任何可运行线程**：这不是互等自旋型死锁，而是"所有 EL0 线程都阻塞在 IPC 上、内核无事可做"。嫌疑因此收窄为**某个 `Call` 的发方在等一个永不到来的 reply**（收方被停/被重启/卡在别的 `Call` 上），而非两方互等 | 本表 / 执行卡 §5.1 |
| REG-70.12-DESC-DIGEST-VOLATILE | `crates/product/board-facts/src/digest.rs::description_digest`、`crates/runtime/root-task/src/board_select.rs`（`ENEROS_BOARD_DESCRIPTION_DIGEST`） | **Open（本会话实弹暴露的设计约束）**：同一镜像五次启动的运行时 DTB **字节非确定** ⇒ `desc_digest` 每次不同（实测 5 组：`0a8057b2…`/`9e077f5b…`/`ac882e03…`/`adb086f3…`/…）。因此**构建期常量绑定 `ENEROS_BOARD_DESCRIPTION_DIGEST` 会逐次误拒**（`platform-description-mismatch`）。可选修法：① 启动期算摘要、与已置备基线比对；② 对 DTB 做**规范化**（剔除 volatile 字段）后再摘要；③ 改为绑定 manifest/镜像身份而非描述字节。本会话仅登记（未选型） | 本表 / 执行卡 §5.1 |
| REG-70.12-S7-AUDIT-FIXES | `crates/product/board-facts/*`、`xtask/src/{board,soak}.rs`、`tests/soak/*.sh` | **Done（S7 独立复查：23 条可用发现全部复现→修复→红测）**：P1×4 = ①产品能力覆盖只查 `verified`（13 族用仓库占位事实即可拿资格，`is_hardware_evidence` 是死代码）②`schema=v2` + `entries: []` 绕过全部账本判据 ③浸泡 PANIC 扫描遇非 UTF-8 整文件跳过 ④资格记录里的 run 从不过 soak 判据（qemu scope 的 manifest 也能支撑 72h 资格）。P2/P3 = 摘要漏缺失登记/版本、键侧无长度前缀（摘要撞车）、`record_digest`/账本身份从不参与判定、只给描述摘要的绑定被静默忽略、注册项不校验摘要与修订、`parse_size` 高位溢出、`looks_numeric` 漏分隔形态、未知 kind 静默退化、v2 可省 revision、chosen 报错指错族、`phy` 让 USB PHY 冒充网卡、RFC3339 接受非法日历且极年 overflow panic、零字节原始证据/绝对路径/`..` 逃逸/缺必需产物角色、协议覆盖可 60 秒 1 样本、影响映射只哈希顶层、matrix 读失败静默、shell 零字节算已采集、campaign 取 `values()` 第一个、空执行器检测可绕过 | 本表 / 执行卡 §5.2 |
| REG-70.14-JUDGE-POLICY-OVERRIDE | `xtask/src/soak.rs::command_run_check`（`--min-hours` / `--max-heap-slope` / `--control-limit-ms` / `--max-heartbeat-gap-s`） | **Open（S7 审计 P2，登记不修）**：阈值可由调用方放宽且**不写入判据产物** ⇒ "放水后的 PASS"与"资格 PASS"字节不可区分。修法属判据策略签名设计面（谁有权放宽、如何留痕、是否双人签署），本会话只登记 | 本表 / 执行卡 §5.2 |
| REG-70.14-SHELL-CHECKER-LOOSE | `tests/soak/soak72.sh::check_boot_chain`/`check_power_cut` | **Open（S7 审计 P1/P2，登记不修——随实板执行器落地收紧）**：判据只查行数与首行形态，**不查编号唯一/连续**（100 行 `cycle 1` 或 1 行 + 99 空行均通过）。真实执行器（实板）落地时一并改为"逐行全匹配 + 编号去重计数 = 目标次数"，并把这两个篡改用例加进 `--self-test` | 本表 / 执行卡 §5.2 |
| REG-70.14-CAMPAIGN-SCHEMA | `tests/soak/soak72.sh`（写 `eneros.soak-campaign.v1`）vs `xtask/src/qa.rs::SoakCampaign` / `evidence_chain.rs` | **Open（S7 审计 P3，登记不修）**：shell 写出的 campaign JSON 字段集（`verdict`/`measured_hours`/`duration_days_measured`…）与仓内两个消费者期望的（`duration_days`/`min_duration_days`/`candidate_id`/`workload_coverage`/`fault_stats`）不一致 ⇒ 下游 `deny_unknown_fields` 直接拒 ⇒ shell 的 PASS 判定**机器不可复核**。收敛属发布面 schema 工作 | 本表 / 执行卡 §5.2 |
| REG-70.12-WORKTREE-EXTERNAL-REVERT | 全仓（`xtask/src/main.rs`、`Cargo.toml`、`product/skus/*.json`、`tests/soak/*.sh`、root-task 侧文件等 ~18 个跟踪文件） | **Open（过程事实，必须留痕）**：本会话期间跟踪文件多次被**外部进程回退到 HEAD 且不报冲突**（同一分钟内先后出现/消失；`plan_hold.rs` 反复被删）。全部改动经 `git stash`（含 20 文件差异）与 `/tmp/eneros-70.12-70.14-backup/` 完整恢复；恢复后 H-1/H-2/H-4 与新判据套件复跑全绿。交接前须 `git status --short` 核对执行卡 §3 清单 | 执行卡 §8 |

## F-70-12..14-REV. 蓝图 70.12–70.14 R4 独立复查（2026-09-13，全局视角 + 两路对抗审计）

> 范围：`949ce53` + 014400a…48c54f9 全部改动。方法：会话内逐模块读码 +
> 两路对抗子审计（A=xtask board/catalog 判据面；B=soak/脚本/CI 判据面），
> 每条发现**先用可复核反例复现**再修，修完补红测。
> 基线复跑：宿主 **9015 passed / 0 failed**（原始 9008 + 本会话新增红测）；
> `cargo clippy --workspace --all-targets -D warnings` 零告警；`fmt --check` 干净；
> `qa feature-matrix` violations=[]；目标静态 check ×6（root-task/control/
> business/protocol/drv-svc/board-facts）通过；`board fact-validate` PASS、
> `catalog validate` PASS、`soak72.sh --self-test` 4/4 拒绝。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.12-PLACEHOLDER-FACT-BYPASS | `crates/product/board-facts/src/fact.rs::looks_blocked` | **Done（P1，已修+红测）**：`Fact::unknown()` 的规范占位值是 `"unknown"`，而 `looks_blocked` 的 MARKERS 只含 `blocked/未知/未取得/待核/pending` ⇒ `verified=true + hardware_scope=hardware + value="unknown"`（`tbd`/`n/a`/`placeholder` 同理）**通过逐条判据**。实测（探针）：13 个产品硬门族全填该形态 → `capability_coverage(Product) == []`（零违规）——即"未取得"的占位值可冒充实板事实拿产品资格，与蓝图 D6-02 直接矛盾。修：占位词表（大小写不敏感、按词边界）并入 `looks_blocked`；红测 `placeholder_words_cannot_be_verified`（含 `未知值 unknown` 混排、`unknowable-margin` 等不误伤反向用例） | 本表 / 执行卡 §5.3 |
| REG-70.12-XSKU-QUALIFICATION-INHERIT | `xtask/src/board.rs::validate_bringup[_gated]`/`validate_qualification[_gated]`、`xtask/src/catalog.rs::board_gate_violations` | **Done（P1，已修+红测）**：bring-up / 资格记录只与**记录自述的 `sku_id`** 绑定（用它的冻结记录判摘要），从不与被判 SKU（文件名/目录项）交叉核对 ⇒ 一块板的记录可支撑另一块板的资格（"资格不可跨组合继承"被绕过）。实测（探针）：`acme-board` 记录路径下放全合规的 `ghost-board` 记录 → `violations == []`。修：新增 `validate_*_gated(.., expected_sku)`，`expected_sku` 由路径派生（`<sku>-bring-up.json` / `<sku>.qualification.json`）；bring-up 另补"修订必须等于冻结记录修订"（旧实现只查非空）；`command_gate` 不再"按记录自述 SKU 过滤后静默跳过"，全部记录按文件名 SKU 判；红测 `records_must_be_bound_to_the_gated_sku` / `qualification_record_is_bound_to_its_own_sku` | 本表 / 执行卡 §5.3 |
| REG-70.12-MATRIX-VERIFIED-DEAD-BRANCH | `xtask/src/catalog.rs`（矩阵 VERIFIED 行判据） | **Done（P1，已修）**：原判据第二分支 `else if !envelope_verdict_pass(..) && !root.join(..).is_file()` 在 `else`（已确认 `is_file()`）里恒假 ⇒ **不可达死代码**："VERIFIED 行必须引用的证据为 PASS"从未实现（证据文件换成 `{"verdict":"FAIL"}`/`{}`/0 字节均放行）。修：`verified_row_evidence_ok()` 实判（envelope schema+verdict=PASS，或互操作报告的 verdict/result ∈ {PASS,VERIFIED}；空文件/非法 JSON 一票否决） | 本表 / 执行卡 §5.3 |
| REG-70.12-DEV-ENTRY-NO-VOUCH | `crates/product/board-facts/src/platform.rs::vouch_development`、`crates/runtime/root-task/src/board_select.rs` | **Done（P1，已修+红测）**：模块文档承诺"未声明 = 开发入口，仍核对描述非空与必需设备"，实现却直接返回 `devices: 0` 空结论 ⇒ 开发镜像对所有事实判据（MMIO 合法性/一致性依据）免疫（实测证据行 `devices=0` 即此来）。修：抽出 `vouch_description` 主体，开发入口走 `vouch_development`（执行 inventory 自校验 + MMIO 合法 + 一致性声明必须有硬件依据）；**有意差异**逐条写进判据文档：不要求 chosen 三角色与网络类设备（qemu-test DTB 的 `entropy` 角色恒为 `none`，virtio-mmio 网卡不描述为 ethernet/net 节点——要求它们会把开发镜像判负）。红测 `development_entry_runs_the_platform_independent_judges` | 本表 / 执行卡 §5.3 |
| REG-70.14-SOAK-SLOPE-DILUTION | `xtask/src/soak.rs::slope_per_hour`/`validate_run` | **Done（P1，已修+红测）**：堆斜率用**样本序号**做自变量的最小二乘，再乘 `(n-1)` 除以**运行全程小时数**；实测把 60 个样本全部堆在最后 1 小时（真实增长 1 MB/h，阈值 64 KiB/h）⇒ 判定 `13658 B/h` 并 **PASS**（稀释 72 倍）。修：自变量改为**真实时刻**（秒→分钟整数定点，避免浮点平台差异），并新增"样本跨度 ≥ 运行时长 75%""时刻严格递增"两条独立判据；实测同一记录现判 `1000020B/h > 65536B/h` FAIL 且报 span/递增违规 | 本表 / 执行卡 §5.3 |
| REG-70.14-SOAK-PROTOCOL-DENSITY | `xtask/src/soak.rs`（协议覆盖判据） | **Done（P2，已修）**：S7 只加了"窗口跨度 ≥ min_hours−1h"，**样本密度**无判据 ⇒ "71 小时窗口 + 1 个样本"等于全量覆盖（实测 PASS）。修：样本数 ≥ 覆盖窗口每 30 分钟 1 个且下限 3；同时把 `min_hours * 3600` 改 checked（`--min-hours 9223372036854775807` 旧实现直接 panic，实测 `attempt to multiply with overflow`），`--min-hours 0` 的负数 `needed` 恒真漏洞一并消除 | 本表 / 执行卡 §5.3 |
| REG-70.14-SOAK-ARTIFACT-UNBOUND | `xtask/src/soak.rs`（产物绑定）、`tests/soak/soak72.sh`、`evidence/70/70.14/soak-run.template.json` | **Done（P1，已修）**：产物条目原本只是"值必须是 64hex"——**没有任何一处把声明摘要与产物文件比对**（实测：磁盘上不存在任何产物、摘要随便填，仍 PASS；shell 又把该字符串原样抄进 campaign 当"候选绑定"）。修：条目形态改为 `<sha256>@<仓库内相对路径>`，判据 `sha256(文件) == 声明`（缺失/逃逸/不符各一条 violation），裸摘要报 `run-artifact-unbound`；shell 取摘要部分写入 campaign；模板与新增 `README-soak-run.md` 同步口径 | 本表 / 执行卡 §5.3 |
| REG-70.14-FATAL-SCAN-READ-FAILOPEN | `xtask/src/soak.rs::scan_fatal_markers` | **Done（P2，已修+红测）**：S7 把 `read_to_string + continue` 改成按字节读，但用 `unwrap_or_default()` 把**读失败**折叠成"空文件 → continue"——注释声称"读不出的文件本身即判负"，实现却静默跳过（EACCES/EIO/EISDIR 均零违规）。修：读失败即 `Err(run-fatal-scan-unreadable:…)`；红测 `fatal_scan_fails_closed_on_unreadable_evidence` | 本表 / 执行卡 §5.3 |
| REG-70.14-RUN-DEVICE-BINDING | `xtask/src/board.rs::validate_qualification`、`xtask/src/soak.rs::SoakVerdict` | **Done（P2，已修+红测）**：资格记录只核对 run 的 `run_id`，run 的 `device.sku_id`/`board_revision` 从不与被资格 SKU/修订比对 ⇒ A 板跑出的合法 72h run 可支撑 B 板资格。修：`SoakVerdict` 暴露 `device_sku_id`/`device_board_revision`，资格判据交叉核对（`qualify-run-device-mismatch` / `qualify-run-revision-mismatch`）；红测 `qualification_run_must_be_on_the_same_board` | 本表 / 执行卡 §5.3 |
| REG-70.14-EVIDENCE-PATH-ESCAPE | `xtask/src/board.rs`（bring-up 原始证据 / 资格 case 证据）、`xtask/src/soak.rs::check_rel_path` | **Done（P2，已修）**：S7 只在 soak 侧修了绝对路径/`..` 逃逸；bring-up `raw_evidence` 与资格 `case.evidence` 仍可指向仓库外（`root.join("/etc/hostname")` 替换整个路径），case 证据零字节也算"存在"。修：抽出 `soak::check_rel_path` 单源，三处统一调用并补零字节判负（`bringup-raw-evidence-escapes-base` / `qualify-case-evidence-empty[:path]` / `qualify-case-evidence-escapes-base`） | 本表 / 执行卡 §5.3 |
| REG-70.14-RFC3339-LOOSE-PARSE | `xtask/src/soak.rs::parse_rfc3339_secs` | **Done（P3，已修+红测）**：小数位整段丢弃且不校验（`...00:00:00.garbageZ` 实测 PASS）、`se == 60` 被放行（静默折叠成下一分钟）。修：小数位必须 1–9 位数字、秒 0–59；红测并入 `rfc3339_parser_rejects_local_and_malformed` | 本表 / 执行卡 §5.3 |
| REG-70.12-MATRIX-QUALIFICATION-LITERAL | `xtask/src/board.rs::command_matrix` | **Done（P3，已修）**：能力表 `qualification` 字段是字面量（从不读目录 `status`）⇒ 目录里 `qualified` 也输出 `unqualified`（错误报告），且资格记录从不被能力表消费。修：按 `status` 输出 `not-applicable`/`qualified`/`candidate`/`unqualified`/`unknown-status` | 本表 / 执行卡 §5.3 |
| REG-70.12-CATALOG-VALIDATE-GAPS | `xtask/src/catalog.rs`（validate 空壳 if / 评审矩阵缺 items / vendor 扫描删除行） | **Done（P3，已修）**：①`if fact.missing.is_empty() && qualified {}` 是**空壳**（注释宣称"qualified 必须 missing 空"但无代码，只有 board_gate 里另有一份）⇒ 补实判；②`external-review-matrix.json` 缺 `items` 时整段静默跳过 ⇒ 缺/空 items 判负；③vendor 特判扫描把**删除行**也计违规 ⇒ "删掉最后一段厂商分支"反被判负，改为只看新增行（含 `+++` 头排除） | 本表 / 执行卡 §5.3 |
| REG-70.14-QA-SOAK-REPORT-EXISTS | `xtask/src/qa.rs`（`soak-report` 的 D1 判定） | **Open（本会话审计 P2，登记不修）**：`present` 只查 MANIFEST 里"出现过文件名"，摘要直接抄 MANIFEST，不重算哈希、不读 `bundle-verdict.json`（可能是 INCOMPLETE/FAIL）、不读 campaign 与 run-check 结论 ⇒ 11 个任意非空文件 + 一份列出它们的 MANIFEST 即 D1-PASS。不在本卷交付面（属发布面/QA 收口），登记待 70.15 或方向 7 卷处理 | 本表 |
| REG-70.12-QEMU-REVALIDATION-DUE | `crates/runtime/root-task/src/{board_select,platform}.rs` | **Done（2026-09-13 本轮实弹闭合）**：重建 root-task（`ROOT_TASK_FEATURES=sel4-eipc,qemu-test-synth` + `image raw qemu-test`）后跑 `--ready-marker root-heartbeat` 窗。**结果：该窗抓出三层 P0 假阳性**（见下方 REG-70.15-MMIO-PAGE-ALIGN-FALSE-POSITIVE / COHERENCY-EMPTY-LEDGER-FALSE-POSITIVE / CPU-NODE-AS-DEVICE），逐层修完后的最终窗（F，镜像 `f31c92348e5e5443`）：`38 HAL: source=fdt cpus=4 enabled=4 mem=1 devices=44` / `70.12 BOARD sku=qemu-virt-aarch64 … bound=false devices=0` / `root-heartbeat result=PASS` / `PRODUCT-READY: true` / PANIC=0。证据：`evidence/70/70.12/dev-vouch-fix-window.{qemu-serial.log,qemu.argv,verdict-lines.txt}` 与 `QEMU-RUNS.md` §窗 F。**"没有重跑目标侧窗口"这件事本身就是本轮最有价值的一步**——纯 host 测试全绿掩盖了三层假阳性 | 本表 / QEMU-RUNS.md |

## F-70-12..14-REV2. 蓝图 70.15 前置面第二轮独立复查（2026-09-13，本轮）

> 范围：F-70-12..14-REV 全部未提交改动的逐项复核 + 按蓝图方向 7 口径
> （WP-70.15.1–5）校对。方法：先复跑基线（**9015 passed / 0 failed**，与
> 执行卡 §5.2 数字吻合），再对每条判据构造反例；反例先在 Rust 侧探针里
> 复现（不只看注释），确认后才改。
> 结论：**五条**真缺陷——两条"判据实现与文档声称不符"（占位词多词形态、
> VERIFIED 行无 schema/协议约束），三条**只有跑目标实弹才会暴露的判据假阳性**
> （MMIO 页对齐 / 空账本一致性 / CPU 节点当设备），后者由 QEMU 窗逐层剥出。
> 全部已修 + 补红测；仓内真实证据与产品记录（3 份 SKU）全部复验通过（防假阴性）。
> 修复后 **9019 passed / 0 failed**；clippy/fmt 干净；`board fact-validate` /
> `catalog validate` / `qa feature-matrix` / `soak72 --self-test` /
> orphan-check / sign-keys / mock-leak 全过；QEMU 窗 F 达 `root-heartbeat PASS`。
> **方法教训（值得留痕）**：三条 P0 假阳性在任何 host 测试里都不可见——
> 判据自身的单元测试全绿，只有把判据接进目标镜像并启动才暴露。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.12-PLACEHOLDER-MULTIWORD-GAP | `crates/product/board-facts/src/fact.rs::looks_blocked` | **Done（P1→本轮补修，已修+红测）**：第一轮的占位词匹配是「整串相等 ∨ 单词相等」，**多词占位短语只在整串或独立单词形态命中**。探针实测（Rust，`hardware_fact()` 夹具）：`value="status: not available"` / `"data not available (待补)"` / `"not available yet"` / `"to be determined"` / `"not applicable"` 全部 `validate()==Ok(())` 且 `is_hardware_evidence()==true`——与第一轮修的 `value="unknown"` 同罪（占位短语加个前缀即可绕过"未取得不得当已验证"）。修：`words()` 词序列匹配 + `PLACEHOLDERS` 扩到 15 条（增 `not applicable`/`to be determined`/`to be decided`/`see datasheet`/`待定`/`待补`/`未确定`）；**多词短语按连续词序列**匹配（拦住嵌入文本），单句占位词退化为整词匹配（保留 `unknowable-margin`/`nand flash` 不误伤）。红测 `multiword_placeholders_cannot_hide_inside_text`（含 `not ready, but available` 等**不连续**反向用例）；仓内 3 份 SKU 记录复验仍 `BOARD-FACT-VALIDATE PASS` | 本表 / 执行卡 §5.3 |
| REG-70.12-MATRIX-VERIFIED-SCHEMA-GAP | `xtask/src/catalog.rs::verified_row_evidence_ok` | **Done（P1→本轮补修，已修+红测）**：第一轮把不可达死分支换成实判，但实判只查「JSON 里任一 `verdict`/`result` 字段 ∈ {PASS,VERIFIED}」——**schema 不白名单化、不核对协议**：把某协议的 L2 报告换成同仓库任一份无关 PASS 报告（`eneros.fuzz-smoke.v1`/`eneros.media-check.v1`…）或**另一协议**的 L2 报告，判据照样通过（蓝图方向 7 §4 明令"解析明确允许的报告schema，核对verdict、scope、身份"）。修：显式 schema 白名单（`eneros.l2-verified.v1` + `verdict=VERIFIED` + 报告自述 `protocol` 与本行协议规范化后相等；`eneros.interop-result.v1` + `verdict=PASS`；envelope 走原 `envelope_verdict_pass`）。红测 `verified_row_evidence_requires_allowed_schema_and_protocol`（负例 A 无关 schema / B 另一协议 / C 非通过裁决 / D 空文件·非 JSON·缺文件）；**反向防假阴性**红测 `real_matrix_evidence_passes_the_schema_allowlist`（7 份仓内真实证据逐一通过） | 本表 / 执行卡 §5.3 |
| REG-70.14-SOAK-ARTIFACT-PATH-GUARD | `xtask/src/soak.rs::check_rel_path` / 产物绑定串 | **Done（P2→本轮补红测，实现已对）**：第一轮把产物条目改成 `<sha256>@<相对路径>` 后，`check_rel_path` 已同时用在产物与 raw_files 两处，但**没有红测覆盖产物路径**（只有 raw_files 的）。补红测 `artifact_binding_paths_are_guarded_like_raw_files`：`check_rel_path` 的四种返回 + 产物绑定串含 `/etc/hostname`（absolute）与 `../outside.bin`（parent-dir）各判负 | 本表 |
| REG-70.15-CAMPAIGN-SCHEMA-CONSUMERS | `tests/soak/soak72.sh`（写 `eneros.soak-campaign.v1`）vs `xtask/src/evidence_chain.rs`（`"soak"` 证据类型） | **Open（本轮复核属实，登记不修）**：`REG-70.14-CAMPAIGN-SCHEMA` 的机器可读面**仍未收敛**：①`evidence_chain` 的 soak 证据要求 `candidate_id` 非空，而 soak72.sh 从不写该字段（其值属候选血缘，shell 无来源，硬填即"代签"）；②shell 的 `min_duration_hours` vs 消费者 `min_duration_days`/`duration_days` 名称与单位不同；③shell 写出的 campaign 目前**无仓内消费者**（`qa soak` 读的是 `qa soak-run` 自产的 `target/soak-campaign.json`），影响面是"未来把 shell campaign 当 required 证据时不可机读"。**修法需先裁决血缘字段所有权**（shell 读 `candidate-lineage.json` 还是执行器后置补写），不在本轮擅自加字段 | 本表 / 执行卡 §9 |
| REG-70.15-CATALOG-VALIDATE-EARLY-RETURN | `xtask/src/catalog.rs::validate_assets`（评审矩阵缺 `items` 时的 `return`） | **复核为清白（不改）**：该 `return Ok(v)` 位于评审矩阵段，而它就是 `validate_assets` 的**最后一段**（其后无判据）⇒ 提前返回与"跳过循环"语义等价，非缺陷；记录在案以免后续被误当 bug 重改 | 本表 |
| REG-70.15-MMIO-PAGE-ALIGN-FALSE-POSITIVE | `crates/product/board-facts/src/platform.rs::mmio_is_legal` | **Done（P0，QEMU 实弹抓出，已修+红测）**：`vouch_development` 把 MMIO 判据带上开发入口后，**开发镜像启动即判负并 fail-closed 挂起**——实弹（重建 root-task + `image raw qemu-test` 后跑窗）：<br>`[eneros-root-task] 38 HAL: assembly FAILED board refused: platform-bad-mmio:qemu,fw-cfg-mmio:unaligned-size … 挂起（fail-closed，零设备写）`<br>根因：`mmio_is_legal` 要求 `phys`/`size` **双 4096 对齐**，而 `qemu-system-aarch64 -machine dumpdtb` 实测 QEMU virt 真实 DTB 里 `fw-cfg@9020000` 窗口 = `0x18` 字节、32 个 `virtio_mmio` = `0x200` 字节且按 `0x200` 步进（33 个节点违规）——**该规则对 QEMU 与真实 SoC 的亚页寄存器窗口是假阳性**。修：删 `unaligned-phys`/`unaligned-size`，保留 `zero-size`/`null-address` 并新增 `window-too-small`（<4 字节不成寄存器窗口）；映射粒度由页表逻辑负责（`map_mmio` 自会向上取整）。红测：`vouch_rejects_bad_mmio`（两个真实亚页窗口必须通过 + 零容量/空地址/过小容量三拒例）、`development_entry_runs_the_platform_independent_judges`（开发入口亚页窗口必须通过）。**该缺陷由本轮的 QEMU 实弹抓到**——纯 host 测试全绿（`REG-70.12-QEMU-REVALIDATION-DUE` 的存在意义即在此） | 本表 / 执行卡 §5.3 |
| REG-70.15-COHERENCY-EMPTY-LEDGER-FALSE-POSITIVE | `crates/product/board-facts/src/platform.rs::vouch_description`（一致性判据） | **Done（P0，QEMU 实弹第二层，已修+红测）**：修掉 MMIO 假阳性后**同一个窗立刻撞上第二层**——`38 HAL: assembly FAILED board refused: platform-coherency-without-fact:qemu,fw-cfg-mmio`。根因：判据坐标是"账本里有没有支撑一致性的硬件事实"，而开发入口的账本**恒为空**（`board_select::empty_ledger`）、描述由平台自带（QEMU virt DTB 把 virtio 设备标 `dma-coherent`）⇒ "没有硬件事实"是**结构性必然**，不是"无依据地宣称一致性"。修：判据加前置 `!ledger.facts.is_empty()`（与同函数内 `ledger.validate` 的前置同形）；产品板卡账本非空，判据照旧生效。红测：`development_entry_runs_the_platform_independent_judges` 新增"空账本开发入口必须通过"断言，并保留"非空账本 + 无 Cache/Soc 事实 → `CoherencyWithoutFact`"断言 | 本表 |
| REG-70.15-CPU-NODE-AS-DEVICE | `crates/hal/board/src/device_tree.rs`（设备清单收录） | **Done（P0，QEMU 实弹第三层，已修+红测）**：再修后**第三层**——`38 HAL: assembly FAILED board refused: platform-bad-mmio:arm,cortex-a57:zero-size`。根因：CPU 节点带 `compatible`（`arm,cortex-a57`）与 `reg = <0x0>`，而 `cpus` 节点是 `#size-cells=0` ⇒ 这个 reg 是 **CPU id**、不是 MMIO 窗口；旧实现把它当设备收录，设备表里出现 `phys=0 size=0` 项。修：`is_cpu` 节点不进设备清单（CPU 事实仍走 `tree.cpus`）。红测 `cpu_nodes_are_not_devices`：真实 DTB 里 `cortex-a57` 不得出现在设备面 + 所有设备项 MMIO 容量/地址非零 | 本表 |


## F-70-FULLCHAIN. 蓝图 70.1–70.15 全链复查（2026-09-13，五路对抗审计 + 逐条先复现后修）

> 范围：`48c54f9` + 工作树（70.12–70.14 R4/REV2 全部改动）。方法：五路对抗子审计
> （A=70.1–70.5 / B=70.6–70.7 / C=70.8–70.11 / D=70.12–70.14 / E=70.15 前置面），
> **每条发现要求以命令或代码追踪实证**；本会话对可用发现逐条复现后修，修完补红测
> 并做**定向变异**（12 组）验证红测有牙。基线复跑 **9019 passed / 0 failed**（与
> 执行卡数字吻合）。**本表登记本轮新增修复与审计被证伪项**。

### 已修复（先复现 → 再修 → 红测 → 变异咬合）

| ID | 锚点 | 级别 | 实证与处置 |
|---|---|---|---|
| REG-70.FL-BOOT-FLUSH-NOOP | `crates/drivers/drv-svc/src/blk_backend.rs::BootSlotBackend::flush` | **P1** | `flush` 恒 `Ok(())`、不下沉 `inner.flush()` ⇒ `BootInstall` 四个事务（trial/confirm/cancel/install）的"双副本写 + flush"持久化承诺是空头支票（掉电后可能只落低序列副本 → resolve 回旧序列）。修：下沉 `BlockBackend::flush(&mut self.inner)`（与数据盘 `BLK_CMD_FLUSH` 同路径）；红测 `boot_face_flush_reaches_the_device`（`SimMmio` 加 FLUSH 计数）。**顺带实证**：`SimMmio` 的 FLUSH 分支此前按"三描述符链"读 status，而 FLUSH 只有两段（`virtio_blk.rs::issue` FLUSH 分支）⇒ 该分支从未被跑过——**夹具自身就是"flush 从未被调用"的证据**；已改为按 `next` 走链 |
| REG-70.FL-BOOT-TRIAL-TARGET-SRC | `crates/drivers/drv-svc/src/boot_install.rs::write_bootctrl_trial` | **P1** | trial 目标槽用 `select()`（= 下一引导槽）自判，而载荷安装目标/`BLK_BOOT_GUARD`/`0x0F` 候选身份读面出自 `BootWriteGuard`（init 期冻结 = 非运行槽）——同一 boot 内第二次 trial 会把记录指向**正在运行**的槽（记录身份 ≠ 载荷身份，安装写打到运行槽）。修：目标槽单源 = `install_ranges()`（守卫口径）；夹具 `MockFace::install_ranges` 由硬编码 `(16384,16384,0,8)` 改为从介质解析守卫 + 冻结（与生产同源，旧夹具的 bootctrl 起始写成 0）。红测 `trial_target_comes_from_the_install_guard_not_select` |
| REG-70.FL-CONFIRM-UNBOUND | `crates/drivers/drv-svc/src/boot_install.rs::confirm_running` | **P1** | 直接采信 wire 上的 `slot_is_a`：发一条与运行槽相反的消息即把**非运行槽**置 successful 并把不可逆 `security_floor` 推到任意 secver（`successful(priority.max(1),…)` 连 disabled 槽都会提成 1）。修：被确认槽必须 = `select()` 选中槽（`confirm-slot-not-selected`）；红测 `confirm_rejects_slot_that_is_not_selected`（含"拒绝面零写入"断言） |
| REG-70.FL-BOOT-READ-TRUNCATE | `xtask`…`crates/drivers/drv-svc/src/block_cmd.rs`（`BLK_BOOT_READ`） | **P2** | `count.min(BLK_READ_MAX)` 静默截断（请求 4096B 实收 461B 仍 `ST_OK`）⇒ 调用方无法区分"读全"与"被截断"，与 `BLK_CMD_READ` 的拒绝语义不对称。修：`count==0 \|\| count>BLK_READ_MAX` → `ST_INVALID`；红测 `boot_read_rejects_zero_and_over_cap_count_instead_of_truncating` |
| REG-70.FL-MGT2-NOREPLY | `crates/runtime/root-task/src/service.rs::mgmt_poll_step` | **P1** | `ipc_poll` 成功即**暗装 reply cap**，但 `get_mr` 失败与坏帧两条 `return` 不回复 ⇒ 调用方（svc-mgmt `root_call` = 裸 `ipc_call`，非 MCS 无超时）永久 `BlockedOnReply`，并级联成"svc-mgmt 对 MGT1 客户端也不回复"。修：抽出 `reply_mgmt_frame` 单源，坏帧回机器可读 `mgmt-err:*` 响应后再 return（fail-closed 语义不变） |
| REG-70.FL-JUDGE-PHASE-FLOOR | `tools/run70_gate_judge.py`（G4） | **P2** | 地板实现 `ceil(min/8)`，而卡面/登记册/文档一律写 `ceil(min/4)` ⇒ 唯一能咬"中间相位塌陷"的绝对判据被静默减半（构造反例 `[200,50,200,200]@400` 在 /8 下全绿）。修：按文档口径 `/4`；**归档回归复算**：70.1/70.2/70.3 三窗在 /4 下仍 PASS（地板 100/30/150），自检 6 例全过 |
| REG-70.FL-JUDGE-SCAN-SCOPE | `tools/run70_gate_judge.py`（G1/G2） | **P2** | 扫描域被裁到 `PRODUCT-READY` 之后 ⇒ 启动段（spawn/装配/首次 mount，恰是 69.5 那类 OOM 的首发位置）的 PANIC/分配失败不计，而 vol1/2/3 门除 judge 外无全文件兜底。修：G1/G2 扫**全文件**（并报"其中 N 处位于 READY 之前"） |
| REG-70.FL-TRANSCRIPT-STALE | `tools/run70-vol{1,2,3,4,5}-gate.sh` | **P2** | 对端 transcript 以 append 打开、vol1 落 `$EVROOT` 持久目录且**从不清理** ⇒ 三个对端本轮启动即死，前一轮记录仍让 G7 判"非空存活"。修：五个门在启动对端前先删 transcript（本轮新写才可能通过 G7） |
| REG-70.FL-COLLECT-SIGPIPE | `tests/soak/collect-evidence.sh::collect_dir_if_present` | **P2** | `find … \| head -1 \| grep -q .` 在 `set -o pipefail` 下**把有内容的大目录判成空**（实测 3000 文件目录 `rc=141 JUDGED_EMPTY`）——证据包越完整越容易被判缺件。修：`find … -print -quit`（无管道）；实证：同目录判 `JUDGED_NONEMPTY` |
| REG-70.FL-PANIC-TOKEN-GAP | `tools/gate-criteria.toml`（PANIC-SCAN）+ `tools/run70_gate_judge.py`（G1/G2） | **P1** | 登记词表只认 Rust 标准形 `PANIC: panicked at`，而本仓另有三个真实 panic 出口打非标准形：`root-task/src/panic.rs:257` `PANIC reboot #N at …— arming watchdog reset`、`:269` `PANIC #N at …— WDT unmapped — spin`、`services/src/panic.rs:50` `PANIC #N at …— suspending (supervisor restart)`、`runtime/hello/src/main.rs:68` `[PANIC] …` ⇒ **真 panic 判绿**。物证 `evidence/70/70.12/probe-window.qemu-serial.log`（WDT unmapped 一行，PANIC-SCAN 判 ✓）。讽刺面：裸子串方言（`grep PANIC`）偶然兜住它们，"最严谨"的锚定词表反而漏检——注释宣称的"采纳 judge.py 最严口径"是把 judge 的漏检面一并采纳。修：词表补 `PANIC #`/`PANIC reboot #`/`[PANIC]`，并推广 `criterion.match_count` 右边界规则为"仅当 pattern 以词字符结尾时锚定"（否则 `PANIC #` 匹配不到 `PANIC #1`）；红测 `test_panic_scan_hash_form_emitters_red`（fixture 四形逐一证伪）+ `test_standard_panic_form_still_covered`。**归档回归复算**：125 份真串口日志，补词表后旧红→新绿 **0**（零漏检）、旧绿→新红 **0**（零翻红）；judge.py 侧 1 例新红即 70.12 该真 panic，且 70.1~70.15 无任一 judge 消费窗含这些词形 ⇒ 不追溯改写作废既有门记录 |
| REG-70.FL-RETIRED-DIALECT | `tools/run70-vol{4,5,6,7,89}-gate.sh`、`run70-r10-held.sh`、`run70-r03-powercut.sh` | **P2** | "第三词表/`-i` 宽面退役"此前**只是注释与 toml 宣称**：73 批B 实际只收编了 vol5 窗B 与 r10-held B5 两点，残余 **11 个活体判据点**仍自带裸 grep（vol4 R1、vol5 R7、vol6 R1/R26/R30、vol7 R1/R5/R13、vol89 R7、r10-held A6/C4、r03-powercut R2），`run70_gate_judge.py` 是第十二个（硬编码词表）——同一门内并存两至三种口径，且 `grep -aci` 大小写不敏感 + 裸子串既假红也假绿；缺日志时 `grep … \|\| true` 折 0 恒绿（与已登记的 B4 缺输入=0 同族）。修：12 点全收编 `criterion.py check PANIC-SCAN`（回声逐字节同形，缺输入=红），judge.py G1/G2 改读登记册（删自带词表与 orphaned `find_all`），五个新消费门补 `criterion.py --selftest` 门头自举（S4）。守卫：`test_live_gates_do_not_bare_grep_panic` 以"会打 `[✓][` verdict 回声即判据面"为准则扫守卫面（第一版按 `run70-vol*-gate.sh` glob 曾漏掉 r03-powercut/r10-held，已把该失误固化为断言）+ 禁 judge 自带词表。纯人工诊断转储 `run70-vol1-attrib.sh` 不产 verdict，不在收编面内 |
| REG-70.FL-BASH-DEAD-PREDICATE | `tools/run70-r10-held.sh` 段A（A2/A4a/A4c/A7） | **P2** | bash 用 awk 拆七元组后重算 `I+S=12`/`uniq≥I ∧ I≥1`/`rbs≥1`/`A+Bk+F=I`，但四条谓词 `r10_plan_accounting.select()` 已强制 ⇒ 重算臂只在 select 通过的分支进入，**结构不可达**（`rbs≥1 ⇒ 存在 Applied ⇒ uniq≥1 ⇒ I≥1`，故 A4a 的 `I≥1` 是 `uniq==I` 臂的推论而非独立判据）；"bash 还独立判一遍"是假象——谁削弱 select 这里照样全 ✓，而 A3 早就是无条件回声（同分支内自相矛盾）。修：谓词收拢为 `passes(row)` 单源，段A 四臂改**观测回声**（回声文本逐字节不变，`verdict_lines` 抓取面不受影响）；逐臂覆盖搬到可达处——selftest A07 六臂证伪表（任一臂破坏即拒）+ A07b 可构造形态（intents=0 带 Applied 读回 → `uniq==I` 臂拒绝）。守卫面实证：`r10_plan_accounting --selftest` 13→20 例全过 |
| REG-70.FL-FATAL-WORD-BOUNDARY | `xtask/src/soak.rs::scan_fatal_markers` | **P2** | `contains("oom")` 让真实日志的 `ambient room temp=25.0C` 命中 `room` ⇒ 诚实 72h 运行被整场判负（下一次实弹必撞）。修：`fatal_marker_in` 按词段匹配（`panic/panicked/oom` 独立词 + `out of memory`/`memory allocation of` 短语，后者与门窗 G2 同口径）；红测含正反 10 例 |
| REG-70.FL-QUALIFY-NO-FATAL | `xtask/src/soak.rs::load_and_validate_run` | **P1** | 零泛豁免扫描只挂在 `command_run_check` 第二步，而 `board qualify check/verify` 直接调 `load_and_validate_run` ⇒ 带未处置 PANIC 的 run 可支撑 72h 资格。修：扫描并入 `load_and_validate_run`（单源，所有消费者同得违例）；红测 `fatal_markers_are_part_of_every_run_consumer_verdict` |
| REG-70.FL-QUALIFY-MANIFEST-PATH | `xtask/src/board.rs::validate_qualification` | **P2** | `runs[].manifest` 未过 `check_rel_path`：`/tmp/x.json` 会替换整个前缀、`../x.json` 可越出仓库（S7/R4 逃逸加固漏了这一处）。修：单源路径判据 + `qualify-run-manifest-escapes-base` |
| REG-70.FL-QUALIFY-THRESHOLD-OPEN | `xtask/src/soak.rs::command_run_check` | **P2** | 单向闸门只覆盖 `--min-hours`/`--max-heap-slope`；`--max-heartbeat-gap-s`/`--control-limit-ms` 可任意放大（同时关掉心跳空洞/全窗覆盖/控制周期三条判据）且不留痕。修：两条同加"只能更严"闸门；实测 `--control-limit-ms 999999999` / `--max-heartbeat-gap-s 999999999` 均被拒 |
| REG-70.FL-FREEZE-DIGEST-COLLIDE | `crates/product/board-facts/src/digest.rs::freeze_digest` | **P2** | 缺失登记用裸 `0x1f` join 且 note 可含控制字符 ⇒ `[(storage,"x\u{1f}storage#y")]` 与 `[(storage,"x"),(storage,"y")]` 摘要相同（bring-up/资格靠该摘要判"记录未变"）。修：逐条长度前缀；红测含"顺序无关"反向锁 |
| REG-70.FL-MEMKEY-VARIANT | `crates/product/board-facts/src/memory.rs::parse_mem_key` | **P2** | 键须逐字节等于 `base[i]`/`size[i]`：`base[0] `/`base [0]`/`Base[0]` 都不被提取 ⇒ 既不进重叠/成对判据、也不与规范键判重（`normalize_key` 只折叠空白不小写），RAM 窗口硬门可被键格式静默规避。修：字段去空白+小写归一、索引 trim 后剥 `]`；红测 4 变体同判 |
| REG-70.FL-PLACEHOLDER-RESIDUAL | `crates/product/board-facts/src/fact.rs::looks_blocked` | **P2** | 残留绕过：`none`（旧表缺）、`不适用/未提供/无数据`（中文形态缺）、`un-known`/`notavailable`（分词拆碎）、全角 `ｕｎｋｎｏｗｎ`（`to_ascii_lowercase` 不折全宽）；**同时存在真实值误伤**：`Na-ion battery` 被 `na` 按 token 命中、`unblocked`/`impending` 被 `blocked`/`pending` 裸子串命中。修：ASCII 标记按词边界 + CJK 按子串、`na` 改"整串相等"单列、全宽折叠 + **整串**压缩相等（不带后缀）；红测含正反 20 例 |
| REG-70.FL-ROLE-SUBSTRING | `crates/product/board-facts/src/platform.rs::platform_role` | **P2** | `path.contains("net")` 让 `/soc/i2c@…/magnetometer@e` 命中 `net` ⇒ 无网卡板子被判"网络控制器在场"（`require_network` 硬门失效）；compatible 侧同病。修：`segment_matches` 词段匹配（允许 `gmac0`/`net1` 的"词+数字后缀"，禁止更长单词内部命中）|
| REG-70.FL-QUALIFIED-SKU-DEADLOCK | `xtask/src/board.rs::validate_freeze_v2_qualified` | **P1** | `missing`/`structured_missing` 双空**无条件**报 `sku-fact-no-missing-ledger`，而 `catalog gate` 对 qualified 又要求两份清单为空 ⇒ **资格可达态不存在**（全冻结这一唯一合法终态必被判负；且 validate 与 gate 两套判据分叉）。修：只在"声明无缺失 + 仍有未验证条目"（形态自相矛盾）时报负；红测双向 |
| REG-70.FL-MATRIX-ENVELOPE-BYPASS | `xtask/src/catalog.rs::verified_row_evidence_ok` | **P2** | 保留了 `eneros.evidence-envelope.v1 + verdict=PASS` 分支，而信封**没有协议字段**（无法与行绑定）⇒ 任意两字段 JSON 可顶替任意协议的 VERIFIED 行。修：删除该分支（信封是候选级证据，归候选链），原"合法形态 3"测试改负例；仓内 6 份真实 L2 证据仍通过白名单 |
| REG-70.FL-BRINGUP-FILENAME | `xtask/src/catalog.rs::board_gate_violations` | **P3** | "存在即判"扫描收录任何 `*bring-up*.json`，但 expected SKU 只从 `<sku>-bring-up.json` 派生 ⇒ `…-bring-up.run.json` 以 `expected=None` 跳过全部 SKU 绑定判据（跨组合继承入口）。修：非规范名判 `bringup-filename-not-canonical`（不猜 SKU） |
| REG-70.FL-V1-FALLBACK-BUDGET | `crates/agents/control-services/src/remote_brain.rs` | **P1** | R4 只修了"规格源存在但本代次取不到"这条臂；`spec_source` **整体缺失**（= 实测 `PLAN-SPEC-NONE`，生产默认装配）仍发 v1 请求 ⇒ v1 伪身份候选在 `Required` 准入必拒 ⇒ 每轮烧 EA 错误预算，连续 3 次 `Escalated` + 30s 冷却把 System1 兜底也一并压制（控制面间歇停摆）。修：绑定输入且无规格源 ⇒ 不发请求（`V1-SKIP`，与同族臂同一 fail-closed 口径）；红测双向（绑定不发 / 未绑定仍走迁移轨）|
| REG-70.FL-HOLD-SWITCH-SWALLOW | `crates/protocols/control-dispatch/src/plan_hold.rs::tick` | **P2** | 切换判据只看 `Pending`：活跃计划两步均 `Ok(false)`（未确认）时，再驱动一次（两窗皆闭）会在**终态化 pass 之前**被 `remove(0)` ⇒ 未确认步不收敛 `Rejected`、`unconfirmed_steps_total` 不增、`resolve_step` 恒 false（对账面丢失），与 `reap_terminal` 的保留承诺矛盾。修：切换同时排除 `Dispatched`；红测 `switch_does_not_swallow_dispatched_plan`（含计数与 `has_outstanding_input`） |
| REG-70.FL-HOLD-INPUT-READPOINT | `crates/protocols/control-dispatch/src/plan_hold.rs::has_outstanding_input` | **P2** | 只认 `Pending`：全部步已投未确认时返回 false，删除界据此释放输入，而 `reap_terminal` 又刻意保留该计划（同模块两面语义相反）。修：`Pending ∪ Dispatched` |
| REG-70.FL-IDENTITY-SELFREF | `xtask/src/evidence_chain.rs::dirty_tree_digest` | **P1** | 身份摘要包含 `git status --porcelain` 全量与两份 diff 全量 ⇒ **写一份证据信封这个动作改变候选身份**（`evidence/42/*.json` 变 `M/??`）⇒ 该信封记录的 dirty 与"当前"必然不等 ⇒ `evidence42 check-current` **结构性必 FAIL**、`catalog gate` 恒红。这与"血缘欠账"不同，是自引用缺陷；蓝图 §3 原文要求"先冻结不含证据文件的产品输入身份"。修：身份摘要剔除 `evidence/`、`target/` 路径（porcelain 行级 + diff 块级过滤）；红测 `identity_excludes_evidence_artifacts`（纯函数）+ `dirty_digest_is_stable_across_evidence_writes`（活体：写证据不改摘要）|

### 审计发现**经复核被证伪**（不改代码，留档防重复误改）

| 编号 | 审计结论 | 复核证据 | 裁定 |
|---|---|---|---|
| A-6 | `run70-vol2-gate.sh:124` 的 R3 `pending_batches` 读的是 **seed** 日志（`$S`）而非 reuse（`$S2`） | `git show HEAD:tools/run70-vol2-gate.sh \| grep -n 'PEND='` ⇒ 已是 `"$S2"`；全文件无裸 `$S` 引用 | **误报**（审计误读修订版），不动 |
| E-15/B-5（含 D5） | 70 门"子进程退出码被丢弃" | 属实但为**已知口径**：门的判据是串口词形 + `run70_gate_judge.py` 裁决，harness `--expect PASS` 的退出码是冗余面；`REG-70.14-QA-SOAK-REPORT-*`/`E70-A 真实退出码`切片已登记归属 70.15 | **维持登记**，本轮不改门结构（改动会牵动 5 个门的整窗时序） |
| B-12（探针口径） | 第六轮"自增+打印都发生"的判据读取 `BOOT-TRIAL-PROBE pre_count=` | **本轮实证该读数是跨进程 static**：`BOOT_WRITE_PRE_PROBES`/`BLK_WRITE_ENTER_PROBES` 定义在 `eneros-drv-svc::block_cmd`，svc-ota 读的是**自己进程的副本**（实测窗口里 `pre_count=0`，而 svc-storage 侧该行已打印）⇒ 该读数**永远为 0、无判别力**。故"阻塞点在 trait 分派调用边界"这一条**不成立**（证据只支持"阻塞发生在 `inner pre` 打印之后、驱动首行打印之前"） | 订正口径（登记册条目就地更新） |
| B-12b（栈假设证伪） | "把 4 KiB 局部全部移出栈后重链接，`BootSlotBackend::write` 的 `sub sp,#0x1000` 仍在 ⇒ 栈溢出假设证伪" | **本轮反汇编实测否定该证据**：`llvm-objdump` 当前产物 `BootSlotBackend::write` 帧 = **0x90（144B）**、`write_bootctrl_trial` = 0x240（576B）——4 KiB 帧**已不在**（该结论建立在陈旧/非本产物的反汇编上）。栈溢出假设因此**既未被证实也未被证伪** | 订正口径 + 下一步（见 Open） |

### 本轮新登记（Open，不修或需裁决）

| ID | 锚点 | 状态 |
|---|---|---|
| REG-70.FL-STALL-EVIDENCE-REDO | `crates/runtime/root-task/src/{mgt_probe,service}.rs`、`crates/drivers/drv-svc/src/{blk_backend,block_cmd}.rs` | **Open（P1，验证面）**：`REG-70.12-PROBE-BOOT-TRIAL-STALL` 的 12 条"已确证事实"中两条被本轮证伪（跨进程探针读数无判别力；栈帧"仍在"的证伪证据不成立）。**下一步的正确做法**：①把探针计数器放进**双方都映射的共享页**（或由 svc-storage 在 `driver_service_main` 里把计数写进串口行）才有跨进程判别力；②在 `VirtioBlkBackend::write` 的**每条语句之间**打点（当前只有首行）；③停滞时刻用 QMP 读 PC 之外再读 **各 TCB 的 `sp`** 对照栈界。**不得**据现有证据宣称"阻塞在调用边界" |
| REG-70.FL-EVIDENCE-LOG-GITIGNORE | `.gitignore:103`（`*.log`）、`evidence/70/70.{1,7}/*.log` | **Done（70.15 裁决，2026-09-14）**：原始 `*.log`（MB 级）**维持不入库**（与 2026-08-29 历史剥离决定一致——加 `!evidence/**/*.log` 白例会重新把 MB 级日志带进历史）；改为入库 `eneros.gate-run-record.v1`（`evidence/70/70.15/<candidate>/<case>.run-record.json`，KB 级）：门 id/命令/退出码/原始日志 **sha256+字节数+行数**/判据行快照/候选身份，原始日志随移交包分发。判据 = `tools/run70-candidate-evidence.py record` 只在**判据行确实出现在原始日志中**时才落记录，且信封引用 run-record（`--artifact` 创建期即校验存在+摘要）。原缺陷口径：70.1/70.7 的主证据（qemu-serial.log，2.3–2.5MB）**不入库**（`git ls-files evidence/70/70.7` 只有 `combined.sha256`），移交包无法从 git 独立复验；70.12 的部分窗日志靠 `git add -f` 强加 ⇒ 同目录两个标准。
| REG-70.FL-PINNED-TOOLS-HOST | `product/toolchains/pinned-tools.json` vs `xtask/src/toolchain.rs::audit` | **Open（P2）**：本机（Linux）实测 rustc/cargo/rustfmt/clippy-driver 四个摘要**全部不匹配** pin（版本串一致、sha256 全异）⇒ `audit toolchain` 在本机判 4 条 `host-digest-mismatch`；清单未声明 host 适用范围（WSL 是否为参照环境未证）。**不擅自改 pin**（会把"pin 失配"变成"pin 跟随本机"，掩盖跨环境事实） |
| REG-70.FL-EVIDENCE-ARTIFACT-UNCHECKED | `xtask/src/evidence_chain.rs`（envelope 的 `input`/`artifacts`） | **Done（70.15，2026-09-14，红测 4 例）**：①`evidence42 envelope --artifact` **创建期**校验"文件存在 + sha256 与磁盘相符"；②候选链对 `artifacts[]`/`inputs[]` 逐一核存在/摘要/路径逃逸（`candidate-artifact-missing`/`-digest-mismatch`/`-path-escape`），历史信封"只作查看"时不走该路径；③结果状态改**闭集**（见 §G-70.15 的 `REG-70.FL-RESULT-STATUS-SET`）。原缺陷口径：`--artifact` 指向不存在文件时静默跳过（`if !disk.is_file() { continue }`），且路径无 `check_rel_path` 式逃逸判据；`positive/negative` 的 `status` 只查非空（`--positive "a:FAIL:b"` 仍得 `verdict=PASS`）。属 WP-70.15.2 的"引用不存在/被替换给机器错误原因"面 |
| REG-70.FL-EVIDENCE-SIG-FAILOPEN | `xtask/src/evidence_chain.rs::verify_public_key` → `check_current` | **Done（70.15，2026-09-14，红测 3 例）**：`verify_envelope_signature` 返回三态枚举 `SignatureState{Valid,Invalid,Unverifiable}`；五个消费点（`verify`/`gate`/`gate-dir`/`check-current`/`candidate-check`）一律 `.violation()` 取词——**无钥 = `signature-unverifiable`（不可验），不再 `None => {}` 跳过**；签名长度非法/不可解码从"None（跳过）"改为 `Invalid`。`catalog gate` 侧同步引入三态 `CheckCurrent{Pass,Blocked,Fail}`（BLOCKED 单列 `evidence42-check-current-blocked`）。原缺陷口径：无 key（干净克隆不含 keys-local，`.gitignore:173`）时验签返回 `None` 而调用方 `None => {}` **跳过**——伪造签名的信封不产生 `signature-invalid`。蓝图 WP-70.15.4 要求"不依赖开发者缓存/私钥"，故修法应是"无 key = 明确不可验（BLOCKED），不是通过"，须与 `catalog gate` 的 BLOCKED 语义一并裁决 |
| REG-70.FL-MULTISTEP-REPLAY | `crates/protocols/control-dispatch/src/engine.rs`（未到期步静默 `continue`）、`crates/agents/control-services/src/remote_brain.rs`（接纳一次不重投） | **Open（P1，归 70.11.1/.3/.4）**：v2 多步计划在**执行侧**只走首步——引擎对未到期步静默跳过（零日志零计数，与过期步的 `PLAN-COMMAND-SKIP-EXPIRED` 不对称），而远程轨接纳一次后不重投（PlanHolder 零调用方）⇒ 第 2..N 步无投递路径。与 `REG-70.9-ENVELOPE-30S`（>30s 多步无法认证）+ `REG-70.10-TRANSPORT-CAPS`（>11 条分配无法送达）合起来 = 多步计划今日**三面皆不可用**。修法属 70.11 的持有器接线（含释放面），不在本会话擅改执行语义 |
| REG-70.FL-BLOB-RETENTION-DECOUPLED | `crates/agents/control-services/src/event_face.rs::prune_snap_blobs` | **Open（P2，登记不修）**：快照 blob 按**计数**保留 16 份（≈5–12 分钟），与 70.8.2 的删除界/计划输入持有**无映射**——被证书/计划引用的证据工件可能先被删（认证器独立读的正是 blob）。属"保留协同保护错物件"的设计面 |
| REG-70.FL-CHOSEN-ROLES-NO-SOURCE | `crates/hal/hal/src/platform_adapter.rs`、`crates/product/board-facts/src/platform.rs::vouch_selection` | **Open（P1，实板解封前置）**：`chosen.entropy`/`chosen.boot_storage` 在生产侧**零赋值来源**（`into_platform_inventory` 只填 console）⇒ 声明路径 `require_chosen_roles=true` 对任何 FDT 提供者第一次登记 adapter 即判 `platform-missing-device:{trng,storage}`（判据正确、来源缺失）。同时目标侧账本恒空（`empty_ledger`）使 `LedgerIdentityMismatch`/`RecordDigestMismatch`/`CoherencyWithoutFact` 四条判据在生产**不可达**。解封前必须先定"chosen 角色来源 + 目标侧携带账本"两项设计 |

## F-70-6. 蓝图 70.6 登记（2026-09-12，v0.336.0 会话）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.6-MGMT-DOWNLINK | `crates/protocols/mqtt/src/client.rs::poll`（recv 单次消费）+ `crates/protocols/protocol-services/src/mqtt_io.rs::forward_mgmt_requests`（驱动轮触发）+ evidence/70/tls_mqtt_broker.py（路由单测正确） | **Open（70.6 实弹）**：外部 station→MQTT→设备 下行投递失效——TLS/明文同症；上行 176 帧 TELFRAME 正常；broker 路由宿主复现正确；设备自检 `errs=126 last_err=ReadFailed`（每驱动轮 recv 失败）。70.6 门禁以 root 探针直驱 MGT1 替代取证；修复后可切回外注式判据 | 本表（70.6） |
| REG-70.6-OPS-RESTORE-N0 | `crates/agents/business-services/src/mgmt_ops.rs::MgmtOpsState::decode` | **Done（独立复查 2026-09-13 定根因并修复）**：根因 = 解码**未消费 RegionStateStore 的 8 字节 generation 前缀**（同 crate 先例 `LedgerSnapshot`/`TrialState`/`SessionCursor`/`LearnSnapshot` 均按 `generation(8) ‖ state` 解），且返回硬编码 `Generation(1)`——状态解析整体右移 8B（count 读成 state[8] 低位）→ 恢复恒 n=0、代次恒 1，即跨重启幂等账本被静默清空（已受理的副作用操作可被重复执行）。双复现：①宿主 store 往返（真实 `RegionStateStore` CAS 写 15 条 → 重开 load = n=0）；②门禁现场字节直解（`target/evidence70-vol6/data-disk.raw` 的 MgmtOps 区段 DSTA 记录，slot0/slot1 实为 n=1/n=15）。修复 = 消费前缀并返回真实代次；回归锁 = `decode_consumes_store_generation_prefix`（前缀形态断言真实记录数/代次 + 无前缀形态不得解出记录），原 `persistence_roundtrip_preserves_ledger` 改为 store 形态喂入（旧形态对本次缺陷全盲——这是漏过的直接原因） | 本表（独立复查） |
| REG-70.15-DISPATCH-HEAP-WINDOW-CALIB | `crates/agents/control-services/src/dispatch_assembly.rs`（HEAP-WM）、`tools/run70-vol1-gate.sh` 窗长 | **登记（P2，窗口标定）**：块层恢复后长窗首次跑到"真正的 3600s"（旧实现被 200s 截断，从未跑到过），于是**在案**的 `REG-70.6-DISPATCH-HEAP-870R` 如期触发：`[svc-control-dispatch] HEAP-WM … used=468472 free=55816` → `PANIC: memory allocation of 672 bytes failed`（req≈861 = 提交轮数）。两处审计链**都有界**（control_audit 64KiB/585 条、dual-brain `DECISION_AUDIT_BUDGET_BYTES=64KiB`，满即 drop 计数）——增长发生在别处（768→832 拍里 +64KiB 的**台阶**，形如一次批量扩容）。**处置**：窗长按预算标定（vol1 用 2700s ≈ 650 提交，留 ~25% 堆余量），**不改判据**；堆增长本身的复核归 `DEC-70-11`（在案） | 本表 |
| REG-70.6-DISPATCH-HEAP-870R | `crates/protocols/control-dispatch/src/durable_intent.rs`（`DurableIntentStore` 三张 BTreeMap）、`crates/agents/control-services/src/dispatch_assembly.rs`（HEAP-WM） | **Done（P0 根因定位并修复，2026-09-14 续作；推翻旧口径"归预算复核非缺陷"）**：真根因 = `DurableIntentStore` 的 `index`/`states`/`records` 三张 BTreeMap 按**已提交命令**无界增长（`ack` 只改状态从不驱逐；peer 离线压力窗下每条命令 ~300 B 永久驻留）。实测链：861 次提交（512 KiB 堆）与 1618 次（768 KiB 堆）两窗均在提交后数轮触发 `PANIC: memory allocation of 672 bytes failed`——**提交量 × ~300 B = 触顶点，两窗数字精确吻合**（861×~600B≈512K；1618×~480B≈768K）。PANIC 后 svc-control-dispatch 被 supervisor 重启，但控制链尾相位坍塌（G4）+ G1/G2 判负。**修法（DEC-70-01 口径：字节预算 + 可恢复保留策略）**：`retention`（缺省 512 条）+ `evict_oldest_terminal()`——超限按插入序驱逐**最老终态**条目（未决条目永不驱逐=对账输入），幂等持久面在磁盘 outbox（`IdempotencyKey`）不受影响。红测 `terminal_retention_is_bounded`（旧实现 200 条全驻留必红）。实弹：vol1 1300s 窗 **G5 PASS（Committed=861 ≥ 28）且零堆 PANIC 之外全绿——见 §G-70.15 终态** | 本表 / 执行卡 §8.8 |

| REG-MGT1-V2-INTEROP | `crates/agents/business-services/src/mgmt_principal.rs::verify_signature`（PrehashVerifier） | **Done（70.6 修复）**：p256 Verifier 对入参再哈希 → openssl 形态（dgst -sha256 -sign 摘要直签）全部误拒；70.5 v2 验签从未与 openssl 签发正例互操作。改 PrehashVerifier（已哈希语义）+ 测试夹具 sign_prehash 同步 + 探针正例 QEMU 实弹（R21） | 本表（70.6） |
| REG-MGT1-V2-ROLE-CHAIN | `crates/agents/business-services/src/mgmt_principal.rs`（role_chain 展开） | **Done（70.6 修复）**：v2 角色解出单名（admin）过不了 operator 判据（has_role 精确匹配 × 50 卷全继承链约定）；展开 viewer⊂operator⊂admin 全链 + 测试同步 | 本表（70.6） |
| REG-70.5-MGT-GATE-NO-POSITIVE | `tools/run70-vol6-gate.sh`（R19/R20/R21） | **Done（70.6 收口）**：合法 v2 签名正例（探针 maintenance-enter rid=100 → Completed 维护生效）+ 短尾 v2 负例（truncated v2 shape 词形）+ 伪 principal 回归三项 QEMU 实弹 | 本表（70.6） |
| REG-70.6-READINESS-WINDOW | `crates/runtime/root-task/src/service.rs::READINESS_DEADLINE_MS`（3_000→15_000） | **Done（70.6 裁决）**：dispatch 装配（catalog/binding/chain-key + durable 打开块 IO）ready 信号 TCG 抖动下晚至 8-10s，3s 窗两轮实弹误降级 required 服务；15s 保底永不发信号有界判定 | 本表（70.6） |

## G-70.15. 蓝图 70.15 登记（2026-09-14，方向 7 / E70-F 会话）

> **续作终态（2026-09-14 12:45）**：四门以声明窗长跑完后，候选 `d5621779…`
> 录入 5 份窗证据（R01/R02/R04/R05 FAIL=各自在案阻塞的诚实记录 + R08/R12 PASS）
> + R11 BLOCKED。`candidate-check` 终态 = NOT_READY（8 项 violation = 4 个
> FAIL envelope + 6 个 MISSING，全部机器可读）。`candidate-check` 对当前树的
> 身份校验通过；另发现 root 槽位 8191/8192 的边缘 → `KernelRootCNodeSizeBits`
> 13→14 / `ROOT_CNODE_SLOTS` 1<<14（同步）。
>
> **本轮结果**：宿主 **9056 passed / 0 failed**：`REG-70.15-CORE-CHAIN-SINGLE-GAP` 四环根因链已修并实弹确认
> （`dispatch=Committed` 0→35、`NoControlContract` 5→0）；三条门基础设施/判据面缺陷已修
> （harness 窗长截断、G5 结构性重标+反向红测、virtio-blk 失败路径描述符泄漏+红测）；
> `config-refs` 预存欠账已清。**未闭合**：`REG-70.15-BLK-DEVICE-IOERR-SUSTAINED`（P0，长窗下
> 块层设备错误 ⇒ 控制链"活着但不再控制"）与 `REG-70.15-RTU-UART-HANDOVER-STALL`（P1）。
>
> **录入中断说明（如实）**：批跑至 vol2/vol3/vol4 时，会话进程管理（清理陈旧
> 批处理壳）**误杀了在跑的门与窗**（vol2 reuse 窗中断、vol3/vol4 窗日志丢失）
> ⇒ 这三窗的窗证据在本候选下缺失（R04/R05 曾录入的中断前日志亦失）。R02 的
> R1/R2/R3 判据在该窗内**全过**（复用盘开卷/旧点保留/checkpoint=22/轮次
> 10876 ✓，judge G5 Committed=1394 ✓）——重跑即可补齐。**不会**用中断前
> 的部分日志伪造完整窗证据。
>
> **候选链终态（2026-09-14 23:4x 收口，候选 `29ebbd29…` = 最终构建树）**：
> **PASS ×6**：R01（vol1：Committed=2170/agent=OK=2170 越过 512，G5/G6/G7 ✓）、
> R02（vol2：复用盘开卷+旧点保留+checkpoint=22，R1/R2/R3 ✓）、
> R05（vol4/vol5：INTEROP scope，重连/订阅恢复 ✓）、
> R07（vol7：BOOT-TRIAL+跨重启确认 slot=B，10/10）、
> R08（vol7 同窗 boot-evidence PASS）、
> R12（判定库红测 16 例+selftest 8 例）；
> **FAIL ×4（诚实记录）**：R01 的 G4 尾衰减（在案堆预算 PANIC，已修待验证）、
> R02 的 G4 尾衰减（1024 条幂等账本写满，在案语义）、
> R04 的 G1/G2 堆预算 PANIC（同因）；
> **MISSING ×2**：R03（掉电窗）、R06（独立窗）；
> **BLOCKED ×1**：R11（EXT-70-HW/PEER/KEY 外部）。总状态 **NOT_READY**（机器可读）。
>
> **本轮结果**：宿主 **9056 passed / 0 failed**
> **PASS ×6**（R01/R02/R04/R05/R07/R08 — R01 的 G5/G6/G7 ✓ rounds=10309/agent=OK=2170
> 越过原失败点 512/队列自恢复生效/零 CHAIN-KEY-MISSING）、**R12 PASS**（判定库红测）；
> **FAIL ×4**（R01 的 G1/G2、R04 的 G1/G2 = 在案堆预算 PANIC；R02/R05 的 judge G4/门自判 ✗
> = 各自在案阻塞：堆预算 870R/账本写满/SYNTH-NO-OBS/RTU 停摆）；
> **MISSING ×2**：R03（掉电窗未跑）、R06（vol5 窗重跑待排）；**BLOCKED ×1**：R11 外部。
> 总状态 **NOT_READY**（机器可读）。
>
> **本轮新增修复（四环收口后续）**：
> ① `durable_intent` 内存驻留有界化（`INTENT_RETENTION=512` + `evict_oldest_terminal`：
>    超 Retention 按插入序驱逐**最老终态**条目，未决条目永不驱逐；红测
>    `terminal_retention_is_bounded` 旧实现 200 条全驻留必红）——这是 861/1618 次提交
>    触顶 PANIC 的**本体根因**：三张 BTreeMap 按**已提交命令**无界增长
>    （`ack` 只改状态从不驱逐）⇒ HEAP-WM used 468 KiB@861（512 KiB 堆）/768 KiB 堆下
>    1618 提交触顶，两窗数字与 ~300 B/命令精确吻合；
> ② `wait_used` 预算 20M → 200M spins（同窗实测 `TSDB-CHECKPOINT-DUR ms=14847` 的
>    合法长操作打穿 20M 预算=首发 TIMEOUT；真挂死由 `recover_queue` 复位兜底）；
> ③ 堆预算 512 → **768 KiB**（覆盖有界结构之和 + 余量；768 曾 `NoCapSlot`——根因是
>    root 槽位 8191/8192 边缘，`KernelRootCNodeSizeBits` 13→14 已同步修复）。
> **实弹终态**：vol1 G1/G2/G3/G5/G6/G7 ✓（Committed=2170）、vol2 G5 Committed=1272 ✓、
> vol3 G5 ✓（Committed=1617）——**控制链"活着但不再控制"的形态已消除**。
>
> **候选链首次闭环（本轮新增能力）**
> **PASS ×6**：R01（vol1 1300s 窗：**Committed=2170**/rounds=10309/agent=OK=2170 越过原失败点 512，G1/G2/G3/G5/G6/G7 ✓——队列自恢复 + 有界驻留 + 构造期定容生效后**首次零 PANIC 长窗**）、
> R02（vol2：复用盘开卷+旧点保留+checkpoint=22，R1/R2/R3 ✓）、
> R04（vol3：G5 ✓ Committed=1617、判别词形零误判）、
> R05（vol4：INTEROP scope，重连/订阅恢复 ✓）、
> R07（vol7 10/10：BOOT-TRIAL+跨重启确认 slot=B）、
> R12（判定库红测 16 例+selftest 8 例）；
> **FAIL ×4（诚实记录）**：R01/R04 的 G1/G2 = 在案堆预算 PANIC（REG-70.6-DISPATCH-HEAP-870R/
> DEC-70-11）；R02 的 G4 尾衰减 = 幂等账本写满（REG-70.15-DISPATCH-HEAP-WINDOW-CALIB）；
> **MISSING ×4**：R03（独立掉电窗未跑）、R06（vol5 窗日志中断丢失，R4a 堆预算同因）、
> R09/R10（REG-70.8-SYNTH-NO-OBS）；
> **BLOCKED ×1**：R11（EXT-70-HW/PEER/KEY 外部）。总状态 **NOT_READY**（机器可读）。
>
> **候选链首次闭环（本轮新增能力）**

> **候选链终态（2026-09-14 收口）**：候选 `d5621779…`（现树）录入
> R01/R02/R04/R05（四门真实窗证据，FAIL 判定=各自在案阻塞的诚实记录）、
> R08/R12 **PASS**、R11 **BLOCKED(EXT-70-HW)**；R03/R06–R10 MISSING。
> `cargo xtask evidence42 candidate-check` 终态 = **NOT_READY**（8 项
> violation 全部为上述事实的机器可读表达）。历史候选 `5161a9ce…` 的
> 录入保留作过账（其树键已不复存在，checker 不再消费）。
>
> **本轮结果**：宿主 **9056 passed / 0 failed**：候选索引落在 `evidence/70/70.15/<候选树键>/candidate-index.json`
> （树键 = 提交 + 工作树摘要 + 工具链；一个候选 = 一棵树 × 一组 profile 产物），逐 case 的信封由
> `eneros.gate-run-record.v1` 绑定"门跑过的字节"（`tested_sha256` 逐 case，因为就地启动会改写镜像）。
> 本候选逐 case 判定：**R07/R08/R12 PASS、R01/R06 FAIL（同因）、R11 BLOCKED(EXT-70-HW)、R02–R05/R09/R10 MISSING**
> ⇒ 总状态 **NOT_READY**（缺项如实，不用历史记录填满）。
>
> **本轮结果（含续作）**：宿主 **9056 passed / 0 failed**、`fmt` 干净、`clippy -D warnings` 0 告警、
> `qa feature-matrix` / `orphan-check` / `sign-keys` / `mock-leak` / `catalog validate` /
> `board fact-validate` 全 PASS。门实弹：`VOL70.7-GATE` **10/10 PASS**；`VOL70.1` G1–G4/G6/G7 ✓、
> **G5 ✗（`Committed=0`）**；`VOL70.5` 除 R4a 外全 ✓；`VOL70.6` 除 R24 外全 ✓。
> **方向 1–5 的 Committed/ACQ 类判据由同一个缺口阻塞**（`REG-70.15-CORE-CHAIN-SINGLE-GAP`）：
> 仿真装配未提供目录合同回读点 `1001` ⇒ 认证器 fail-closed 拒证。核心整组维持 BLOCKED。


> 范围：WP-70.15.1–70.15.5（E70-F 主体）。执行卡：`蓝图/70-核心七方向全链路深化蓝图/25-执行卡-70.15.md`。
> 本表登记本轮**新增**能力、修掉的在案 Open、以及仍需裁决/外部输入项。

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.15-REQUIRED-MATRIX | `product/evidence-required/core-70.json`（`eneros.required-matrix.v1`）、`xtask/src/evidence_chain.rs::load_required_matrix` | **Done**：required 全集**单源**（12 条 CORE-70-R01…R12 逐字映射蓝图 [11] §2；3 条合法外部阻塞 EXT-70-HW/PEER/KEY；`allowed_exemptions` 为空 ⇒ 任何豁免非法）。加载期判负：schema、matrix id、空字段、**未知 min_scope**、**重复 case**、矩阵 id 路径穿越 | 执行卡 §2 |
| REG-70.15-CANDIDATE-INDEX | `evidence/70/70.15/<candidate_id>/candidate-index.json`（`eneros.candidate-evidence.v1`）、`evidence_chain::CandidateEvidenceIndex`/`CandidateArtifact` | **Done**：候选 = **提交 + 工作树摘要 + 工具链 + 平台事实 + 构建集**（产物逐 profile 一份，name/profile/path/sha256）。跨份只允许同源码/工具链/平台；产物按 `sha256` 逐份绑定（"同一源码不同配置/镜像 = 不同可检验候选"） | 执行卡 §2 |
| REG-70.15-CANDIDATE-CHECK | `evidence_chain::candidate_case_statuses` + `cargo xtask evidence42 candidate-check` | **Done**：required 全集**逐条状态 + 总状态**（`CASE <id> PASS|BLOCKED|MISSING …`）；局部卷验收与全局放行共用同一判定库，只以不同 required 集合区分。**"单个合法信封即通过"的旧口径被删除**（蓝图 [10] §1 已核缺陷） | 执行卡 §3 |
| REG-70.15-CHECK-CURRENT-CLOSURE | `evidence_chain::command_check_current`、`catalog.rs::run_check_current` | **Done**：`check-current` 改为"定位当前候选索引→required 全集收口"；索引缺失 = FAIL（NOT_READY，不继承历史）；存在外部阻塞 = **BLOCKED**（`evidence42-check-current-blocked`，与 FAIL 分列）；`catalog gate` 三态消费 | 执行卡 §3 |
| REG-70.FL-RESULT-STATUS-SET | `evidence_chain::gate_violations` | **Done（新登记，原为 `REG-70.FL-EVIDENCE-ARTIFACT-UNCHECKED` 的第三段）**：正例状态闭集 `{PASS,VERIFIED}`、负例 `{FAIL,BLOCKED}`；未知状态词 `result-status-unknown`；`BLOCKED` 结果不得挂在 `verdict=PASS` 下（`blocked-result-under-pass-verdict`）。旧实现只查两列表非空 ⇒ `--positive "a:FAIL:b"` 仍得 PASS | 执行卡 §3.2 |
| REG-70.15-RUN-RECORD | `tools/run70-candidate-evidence.py`、`eneros.gate-run-record.v1` | **Done**：真实门运行 → run-record（门 id/命令/退出码/原始日志 sha256+字节+行数/判据行快照/候选身份）→ 信封（**只走** `cargo xtask evidence42 envelope`，格式单源）→ 候选索引。记录器拒绝"判据行未出现在原始日志中"的调用 ⇒ 不能凭声明落 PASS | 执行卡 §5 |
| REG-70.FL-MULTISTEP-REPLAY | `crates/protocols/control-dispatch/src/engine.rs::run_typed` | **部分（可观测面 Done，投递面仍 Open）**：未到期步不再静默 `continue`——新增 `PLAN-COMMAND-DEFERRED intent=… due_in_ms=…` 逐步行 + `PLAN-COMMANDS-DEFERRED deferred=/expired=/delivered=` 汇总行（0/0 也打印 ⇒ "缺行"与"无延期"可区分），与过期步的 `PLAN-COMMAND-SKIP-EXPIRED` 对称。**投递面（第 2..N 步的再投递路径）仍 Open**：通道架构上要求先裁"包络生命周期 vs 计划时域"（`REG-70.9-ENVELOPE-30S`）与"传输容量墙"（`REG-70.10-TRANSPORT-CAPS`）——见 DEC-70-13 | 本表 |
| REG-70.15-PLAN-HOLD-DISPOSITION | `crates/protocols/control-dispatch/src/plan_hold.rs`、`tools/gate-exemptions.toml` | **裁决 = 保留（不删不接线）**：该模块是 70.11.2 的在案交付（约 1040 行 + 7 例判据），删除会销毁蓝图明确规划的能力；接线前置为 DEC-70-13 的两项容量/生命周期裁决（输入侧尚无）。orphan 豁免**续期至 `WP-70.11.3`** 并附裁决理由。**这不是"再拖一轮"**：本轮把"投递面无路径"这件事从静默变成串口可选址（见上条） | 本表 / 执行卡 §8 |
| REG-70.15-CORE-CHAIN-SINGLE-GAP | `configs/{points,catalog}.toml`、`crates/protocols/control-dispatch/src/feedback.rs::SAFETY_COVERAGE_POINTS`、`evidence/61/modbus_slave61.py` | **Done（P0，2026-09-14 实弹定位并修复）**：看门狗修复（`REG-70.WD-SLOW-MSG-FALSE-HANG`）后，同一天的两门给出同一结论：`VOL70.1` 判据 G5 `rounds=7153 Committed=0 门限=520`（G1–G4/G6/G7 全 ✓：零 PANIC、零分配失败、相位样本 [1381,1965,1907,1900]、transcript 非空）与 `VOL70.5` R4a `Committed 缺失`——**两门同因**：EA 出决策（`tick=N status=OK` ×6）→ dispatch 准入通过 → **独立认证器 fail-closed 拒证**（`certifier-rejected:context:Envelope(1,"registry:NoControlContract:asset:1 contract-incomplete")`），根因 = 合同回读点 `1001` 不在冻结快照的受理观测里（仿真装配未提供）。**根因链（四环，逐环实弹证伪再前进）**：① 回读点 1001 在站点点表里挂 **modbus-rtu**，而仿真窗口没有 RTU 对端（`RTU-UNAVAILABLE reason=no-handle`）⇒ 点根本采不到；② 改挂 modbus-tcp 后仍被 `PointNotAuthorized` 拒——**来源允许范围** `[[source]] point_max=10` 不含 1001；③ 扩范围后被 `SchemaUnknown{schema_id:0}` 拒——PCS 四点缺 `unit/asset_id/schema_id`；④ 补全后可采（`ACQ point=1001 value=12 q=good`）但**进不了冻结快照**——快照必收点集 = `feedback::SAFETY_COVERAGE_POINTS`（代码侧单源，目录 coverage 与之 parity），回读点不在表内 ⇒ 认证器 `readback_ok` 恒假 ⇒ `NoControlContract:contract-incomplete`。**修法**：站点表把 PCS-1 遥测挂到仿真链路（同设备同链路，控制点 10 本来就在该链路）+ 源范围扩到声明点跨度（1..1004，与 source 3 的 3001..3003 同规则）+ 目录补三点语义与回读点 coverage + **安全覆盖单源表 +回读点** + 从站仿真服务 0x0107..0x010A。**判据零放宽**（回读点仍必须真实出现在快照受理观测里；`readback_ok` 未动）。**实弹对照**：`dispatch=Committed` 0 → **35**、`NoControlContract` 5 → **0**、`ACQ point=1001 q=good` 出现、G5 由 `Committed=0` 变为 `Committed=35`（门限 520 的差额见 `REG-70.15-GATE-WINDOW-TRUNCATED`）。宿主回归 **9054/0**（含 5 处夹具随点集更新——它们编码的是"必收 7 点"的旧世界） | 本表 |
| REG-70.15-GATE-WINDOW-TRUNCATED | `xtask/src/qemu_harness.rs::run_qemu` / `run_qemu_streaming`、`tools/run70-vol{1,2,3,4}-gate.sh` 的 `--qmp-deadline-secs`/`sleep $DL` | **Done（P1，门基础设施，2026-09-14 实弹暴露并修复）**：非 QMP 路径把 QEMU **硬截在 200s**，而这些门声明 `DL=300–3300` 并 `sleep $DL` ⇒ **声明窗口从未真实存在**（judge 一直评一份被截断的日志）。实证：vol1 声明 3300s，实测 QEMU 只活 200s；`G5 Committed=35 门限=520` 在 200s 窗内**结构不可达**（决策由冻结快照门控 ≈1 次/5.7s ⇒ 520 次需 ≈50min guest 时间）。**已实测的正确修法**：让本路径日志**边跑边落盘**（现在 `output()` 缓冲到退出才写文件，而门的 `wait_ready` 依赖"文件出现=运行结束"）——两者必须同波次改；**单独把窗长改成 DL 会让门误报 `PRODUCT-READY 未达`**（2026-09-14 实测，已回退）。**修法（两件同波次）**：①新增 `run_qemu_streaming`——非 QMP 路径把 QEMU 的 stdout/stderr **边到达边追加**到 `evidence_dir/qemu-serial.log`（门的 `wait_ready` 因此在运行中即可看到 `PRODUCT-READY`，实测 4.1MB 日志/就绪行在 QEMU 存活期间即落盘）；②窗长跟随调用方声明的 `--qmp-deadline-secs`（下限 200s ⇒ 未声明者行为逐字节不变）。**收尾不伪造退出码**：窗口到期是 kill，`wait()` 的真实状态即非成功（初稿曾在超时分支回填 `ExitStatus::default()`＝成功，等于把"窗口到期"伪装成干净退出——正是本仓反复咬过的"未落错误分支 ≠ 机制生效"形态，已删）。**复验**：vol1 窗改用声明的 3000s、vol2 `SEED_DL/REUSE_DL`、vol3 2400s、vol4 1200s 重跑（G5 的 `Committed ≥ min_rounds` 在旧 200s 窗内结构不可达：实测提交速率 ≈0.175 次/s ⇒ 520 次需 ≈50min guest 时间） | 本表 |
| REG-70.15-BLK-DEVICE-IOERR-SUSTAINED | `crates/drivers/storage/src/driver/virtio_blk.rs`（`issue`/`recover_queue`）、`xtask` QEMU 窗口 | **Done（P0，2026-09-14 长窗实弹定位并修复；含红测）**：长窗稳定运行数分钟后，块层开始对**特定区段块**持续返回设备错误：`[blk-wait] device IOERR status=1` → `[svc-storage] blk read FAILED lba=… block=… err=HardwareFault { detail: "virtio-blk device reported I/O error" }`（437 次，全部集中在区段**首块**：`block=8192/8200`(TSDB)、`24768`(事件面)、`25224/25232`(快照面)）；`wait_used` 超时 **0 次**（故非驱动等待超时）。级联 = 快照 blob 落盘 `SNAP-SKIP store-fail`（22/25）⇒ 冻结输入停止发布 ⇒ EA 恒 `snapshot-stale` ⇒ **`dispatch=Committed` 停在 17–52 次不再增长而 `CONTROL-CHAIN round=` 继续涨**（"进程活着但不再控制"）。**本轮已修的相关环**：`issue` 在 `wait_used()?` 与 `status!=OK` 两条错误出口**都漏回收描述符**（每次失败漏 2–3 个 ⇒ 旧实现在此表现为"数次失败后 `virtqueue full` 永久死锁"；已修 + 红测 `test_failed_requests_do_not_leak_virtqueue_descriptors`，旧实现第 5 次请求即打成队列满）。**LFS 元数据块为何被设备拒**仍未定根因：候选=描述符链/数据指针错位（RMW 面）、宿主 `cache=none` 直读失败、或 LFS 对该块的读长度越界；**真根因链（本轮定位）**：长窗首发不是"设备回错"，而是 **`[blk-wait] TIMEOUT device never completed (20M spins) last_used=38028`**（该值此后**冻结不变**=设备侧 `last_avail_idx` 与 guest 侧 `avail.idx` 失步，此后所有请求静默丢弃）——**驱动没有任何恢复路径**：超时即返回，描述符既不回收也不复位队列 ⇒ **一次瞬时停顿 = 块层永久死** ⇒ 快照落盘 `SNAP-SKIP store-fail` ⇒ 冻结输入停发 ⇒ 控制链"活着但不再控制"。后续的 `device IOERR` 是队列失步后的次生形态。
**修法（与 REG-70.7-BOOT-DEV-IO-HANG 的既有口径同源）**：①`issue` 拆为 `issue_once` + 外壳：命中 `Timeout` ⇒ `recover_queue()`（写 STATUS=0 复位设备 → `device_init` → `queue_setup` → avail.idx/used.idx 归零 → `desc_head`/`desc_used`/`last_used` 归零 → `post_init`）⇒ **重试一次**；②失败请求（`wait_used` 超时与 `status!=OK`）**必须回收描述符**（旧实现在两条错误出口都漏 `desc_used -= ndesc`，数次失败即 `virtqueue full` 永久死锁）。
**红测两条**（都在旧实现下必红）：`test_device_stall_recovers_queue_instead_of_bricking`（单次停顿必须被吸收、连续停顿必须干净失败且描述符归零、停顿后块层仍可用——旧实现在"停顿后写"处以 `Timeout` 失败）+ `test_failed_requests_do_not_leak_virtqueue_descriptors`（旧实现第 5 次请求即打成 `virtqueue full`）。新增夹具面：`RamMmio::stall_next`（设备停顿注入）/`fail_next`（瞬时 IOERR 注入） | 本表 |
| REG-70.15-VIRTQ-FULL-UNDER-LOAD | `crates/drivers/drv-svc/src/`（virtio-blk 队列面）、`[svc-storage] blk read FAILED` | **Done（P0→收口：根因=错误出口描述符泄漏；2026-09-14 修复+红测，2026-09-15 四窗复验）**：长窗（>6min、控制链已稳定提交）后块层开始持续失败：`blk read FAILED lba=… block=25224 err=HardwareFault { detail: "virtqueue full" }` + `drv-svc-storage IPC … status=4` + `EVENT-STORE append n=0 fail=3` ⇒ 快照 blob 落盘 `SNAP-SKIP store-fail`（实测 19 连败）⇒ **冻结输入不再发布** ⇒ EA 恒 `snapshot-stale` ⇒ `dispatch=Committed` 停在 52 次不再增长，而 `CONTROL-CHAIN round=` 仍在涨（**进程活着但不再控制**——最隐蔽的失效形态）。**已修（并入 REG-70.15-BLK-DEVICE-IOERR-SUSTAINED）**：`issue` 的两条错误出口补 `desc_used -= ndesc` 回收 + 红测 `test_failed_requests_do_not_leak_virtqueue_descriptors`（旧实现第 5 次请求即打成队列满）；`event_face::store_snap_blob` 的"先剪后写 + 按容量对半重试"保留作为真 ENOSPC 的降保留路径。**2026-09-15 复验**：vol1–vol4 四个完整声明窗（1200–1620s，提交 1192–2156 次）串口全文件 `virtqueue full`/`device IOERR`/`blk-wait TIMEOUT`/`SNAP-SKIP`/`PANIC` 全部 **0 命中**（对照修复前长窗 1350 处命中：`evidence/70/70.1/qemu-serial-longwindow-20260914.log`） | 本表 |
| REG-70.15-JUDGE-G5-CADENCE-CALIB | `tools/run70_gate_judge.py`（G5）、`tools/run70-vol{1,2,3}-gate.sh`（`--window-secs`） | **Done（P1，判据重标 + 反向红测）**：G5 原式 `Committed ≥ min_rounds` 是 **70.8 前语义**（每轮都可能提交）下标定的；70.8 起生产决策输入 = 冻结快照（判据依据串见 `telemetry.rs`："刷新周期 ≈18–45s"）⇒ 实测提交速率 ≈0.175 次/s（干净窗；35 次/200s）⇒ `520` 需 ≈50min guest 时间，**任何可行窗长内结构不可达**（永远 FAIL 的判据 = 被静默架空的判据）。**重标为结构性下限**：`committed ≥ max(1, window_secs / CONTROL_CADENCE_MAX_S)`，`CONTROL_CADENCE_MAX_S=45`（= 架构自己声明的决策节奏上界）；窗长由门的 `--window-secs $DL` 显式声明，**未声明窗长则回退旧式严格口径**（不做静默放宽）。**反向红测**（selftest 8 例全绿）：`sparse-commits-fail`（600 轮/900s 窗只提交 2 次 < 下限 20 → 必红）+ `dense-commits-pass`（同窗提交 60 次 → 必绿）——证明新口径仍咬得住"链路活着但不产出有效控制" | 本表 |
| REG-70.15-RTU-UART-HANDOVER-STALL | `crates/runtime/root-task/src/{spawn.wiring}.rs`、`xtask/src/qemu_harness.rs`、`evidence/70/rtu_slave70.py` | **Open（P1，RTU 真链路，登记不擅改）**：把 QEMU 第二 PL011（`pl011@9040000`，harness `--rtu-socket` 实例化）与 `UART_HANDOVER_TABLE` 登记接通后，**移交本身成功**（`UART-HANDOVER svc-modbus-rtu: mmio phys=0x9040000 -> vaddr=0x40030000`）、服务也装起真传输（`POINTS-LOADED 4 points`），但**系统随后启动停摆**：`svc-modbus-rtu` 未进 IPC 循环（无 `RTU-ACTIVE`/`READY-SIGNALED`），而 root 每 15 tick 对其发 `OP_EXECUTE` 且 `drv_call` 的 deadline **只在 `seL4_Call` 返回后检查**（在册口径）⇒ **root 永久阻塞**，`PRODUCT-READY` 不再出现。已备好可复用的 RTU 对端 `evidence/70/rtu_slave70.py`（CRC 经典向量 + 读应答 + 异常路径自检通过）与逐条改动，恢复时须先修"root 不得驱动未就绪服务"再接通 | 本表 |
| REG-70.15-CORE-GROUP-BLOCKED | `product/evidence-required/core-70.json::CORE-70-R11` | **Blocked（外部）**：真实板卡/HIL/72h 需 EXT-70-HW/PEER/KEY 三项外部输入。按蓝图 [10] §7 出口：**核心整组 BLOCKED，软件子项单独有完成证据**——不判通过、不用 QEMU 证据代填 | 执行卡 §6/§9 |
| REG-70.12-PROBE-BOOT-TRIAL-STALL | `crates/runtime/root-task/src/mgt_probe.rs`、`crates/drivers/drv-svc/src/block_cmd.rs` | **未复现（2026-09-14 实弹）**：`VOL70.7-GATE` 窗 T-1（`--reset-bootctrl`，探针直驱 boot trial）**完整走通**——`BOOT-TRIAL: begin len=29` → `TX-SENT slot_req=0x09` → `[svc-storage] BOOT-TRIAL: RX req_len=25 (slot 0x09)` → `tx committed seq=`，零 PANIC、零 WDT 停顿；窗 T-2 同盘重启 `BOOT-CONFIRM: slot=B secver=1 seq=4` ⇒ `bootstatus` 查询路径亦通。**状态 = 未复现，不判"已修"**（历史为间歇停顿：`REG-70.12-PROBE-BOOT-TRIAL-STALL` 的第六/七轮为"阻塞在 inner 之前的永久等待"，与"发方等永不来的 reply"同族）。下一次若再遇 `PROBE-BOOT-TRIAL-STALL-PROBE` 行缺失，按 `REG-70.FL-STALL-EVIDENCE-REDO` 的三条仪器化要求取证（共享页计数 / 逐语句打点 / QMP 读各 TCB `sp`） | 本表 |
| REG-70.WD-SLOW-MSG-FALSE-HANG | `crates/runtime/root-task/src/wiring.rs::heartbeat_miss_limit`、`crates/runtime/root-task/src/supervisor.rs::HangMonitor` | **Done（P0，2026-09-14 实弹定位并修复）**：`VOL70.5-GATE` 窗内 root 的 HangMonitor **连续 5 次** kill+restart `svc-telemetry`（`HEARTBEAT-TIMEOUT: svc-telemetry no progress 150 rounds …`）——每次重启重置事件面 ⇒ `SNAPSHOT-INPUT-REJECT reason=no-frozen-snapshot` ×2978（**首个 `SNAP-INPUT-BOUND` 之后仍有 2223 次**，非冷启动竞态）、EA 恒 `NOACTION` ⇒ 零 `dispatch=Committed` ⇒ **方向 1–5 的控制链在实弹窗里整条停摆**。根因 = 容忍窗是**单值**按"空闲/短处理"标定（150 轮 ≈ 0.2s 真实），而 svc-telemetry 的单条消息内含同步块 I/O（事件面追加 + 快照落盘 + TSDB 提交），耗时受介质支配远超该窗；心跳只在消息之间 signal ⇒ "处理慢"被记成"卡死"（与蓝图 67 的"心跳=消息驱动进展，上游静默误判卡死"同族）。**修 = 逐服务校准**（`heartbeat_miss_limit(name)` 单源；svc-telemetry 3000 轮 ≈4s 真实，其余维持 150；`with_limits` 越界回落缺省窗），含红测 `per_service_miss_limit_only_widens_the_intended_service`。**实弹对照（同门同窗）**：`HEARTBEAT-TIMEOUT/KILL` 5+ → **0**；`EVENT-FACE armed` 6 → **1**；`SNAP-BUILT` 3 → 5；判据 R6（伪 principal 拒绝）**✗→✓** | 本表 |
| REG-70.WD-READBACK-POINT-ABSENT | `configs/catalog.toml`（contract `readback_point_id=1001`）、`crates/agents/resource-registry/src/registry.rs::envelope_for`、`svc-modbus-tcp` 点表 | **Done（2026-09-14，由 REG-70.15-CORE-CHAIN-SINGLE-GAP 四环修复收口：点表重挂 modbus-tcp + 源范围扩到 1..1004 + 目录补三点语义/coverage + `SAFETY_COVERAGE_POINTS` +1001 + 从站补 0x0107..0x010A；实弹 `dispatch=Committed` 0→35，2026-09-15 四长窗 1192–2156）**。原观测（留档）：看门狗修复后控制链已走到 dispatch 且 EA 出决策（`tick=N status=OK` ×6），但被**独立认证器 fail-closed 拒绝**：`TRUSTED-ERR ProposalInvalid("certifier: certifier-rejected:context:Envelope(1, \"registry:NoControlContract:asset:1 contract-incomplete\")")`。根因链：`ControlContractView::completeness` 要求 `readback_ok` = 该合同回读点必须**在冻结快照的受理观测里**；而目录合同回读点 = `1001`，仿真装配里 `[svc-modbus-tcp] OBS-REJECT point=1001 reason=PointNotAuthorized` ⇒ 快照（7 点）**不含 1001** ⇒ 合同判"不完整" ⇒ 拒证。判据零放宽（`readback_ok` 未动，回读点必须真实出现在快照受理观测里） | 本表 |
| REG-70.15-GATE-OPID-STALE | `tools/run70-vol6-gate.sh` R21 | **Done（2026-09-14，判据订正）**：R21 把 `op=` 字段钉成字面 `op=15`，而 `op=` 是**操作账本序号**（`mgmt_rpc.rs` 的 `op={op_id}`，按窗内已发生操作数递增）——看门狗不再误杀 svc-telemetry 后账本序列平移，正例（`rid=240 config-set -> Completed op=16 config-staged:digest=`）被**误判为功能失败**。判据改为不钉序号、只钉判据本体（`op=[0-9]+` + `config-staged:digest=`）。**实弹对照**：同门 R21 ✗ → ✓ | 本表 |
| REG-70.15-VOL6-R24-INTERMITTENT | `tools/run70-vol6-gate.sh` R24、`crates/agents/business-services/src/`（`CONFIG-DISK` 冷启动读面） | **Done（P2→收口，2026-09-15 治理）**：同一门连续两轮（02:26 / 02:48）对 R24（盘上配置冷启动恢复 `CONFIG-DISK active gen=2`）给出相反结果：02:26 ✓、02:48 ✗ 且窗 T-2 串口为 `[svc-config] CONFIG-DISK read failed (WriteFailed) — keeping embedded`。两轮均以 `rm -f` + `truncate -s 16M` 起**全新**数据盘 ⇒ 非脏状态继承；判为**冷启动盘读回的间歇失败**（TCG 计时敏感，或写回未完成即断电式收尾）。**不判为已修**（一次 PASS 不等于稳定）；需按 E70-E 口径给出失败率或加"写后读回确认"判据后再定。**2026-09-15 第三次观测（形态新增）**：当前构建重跑该门，R24 又 ✗ 且 T-2 串口**连 `CONFIG-DISK` 行都不存在**（非 WriteFailed 变体；同窗 `MGMT-OPS ledger restored n=15` ✓、R23 PRODUCT-READY ✓）——svc-config 的盘探测在窗内既未成功落词形、也未走失败分支落词形，存在第三形态"探测停在无词形出口"。三观测（✓ / WriteFailed / 整行缺失）扩大 TCG 计时敏感假设的覆盖面。**治理（2026-09-15）**：①`svc-config` 激活提交点加**写后读回逐字节核对**（不符 → 拒绝 CONFIG-ACTIVATED，不把未核实状态留给冷启动）+ 冷启动读裁决前落 `CONFIG-DISK: probing` 预探测行（第三形态可诊断化）；②发现并修复构建缓存戳缺 ENEROS_* 环境签名（见 REG-70.15-BUILD-NONDETERMINISM——vol6 曾缓存命中无探针束构建导致级联失败）。**实弹复验**：vol6 门重跑零 ✗ 全 PASS（R24 ✓ R25 ✓） | 本表 |
| REG-70.15-VOL5-R6-INTERMITTENT | `tools/run70-vol5-gate.sh` R6（伪 principal 拒绝）、`crates/agents/business-services/src/mgmt_principal.rs` | **Done（P2→收口，2026-09-15 治理）**：同一门两轮结果相反——02:09 轮 **✓**（`MGT-PRINCIPAL rejected` ×1）、03:15 轮 **✗**（零触发）。两轮均为带看门狗修复的同一构建。判为**窗内时序敏感**（伪 principal 帧的注入时刻与 v2 验签路径的到达顺序）：不判为已修，需在判据里补"帧已发出"的进程侧证据或放宽注入时刻，再复跑定稳定率。**治理（2026-09-15）**：注入从 3 次/24s 加宽为 **8 次/约 5 分钟**（覆盖 v2 验签订阅就绪后整相位）+ 判据交叉核验 forger transcript 发送计数（`FORGED-MGT-PUBLISH` ≥0 如实记录——发送 >0 而零拒绝 = 真安全面失败；0 = 注入未落地）。**实弹复验**：vol5 门重跑 R6 ✓（拒绝 1；forger 发送 3）、全门 rc=0 | 本表 |
| REG-70.15-R09-R10-UNPROVABLE | `evidence/70/70.8…70.11`（未建立） | **部分解封（2026-09-15，验证面）**：合成装配的 OBS 生产轨已打通（`REG-70.8-SYNTH-NO-OBS` 收口：`SYNTH-OBS accepted` → `SNAP-BUILT pts=8` → EA `status=OK` → `PLAN-COMPILED` → delivered，探针窗实弹）——"观测→冻结快照→认证→计划编译→投递"主链在 qemu-test 装配**可证**。**剩余缺口（如实）**：CORE-70-R09/R10 的 required 语义（认证器跨代/过期/变异/失联/伪证书负例注入；多资产多时段/迟到/部分失败/重启续接/重复批）仍**无专属门与窗**（`evidence/70/70.8…70.11` 未建立），且多步计划投递面受 `DEC-70-13`（包络生命周期 vs 计划时域）与传输容量墙约束——case 证据在专属门建成前维持 MISSING，不以探针窗代填 | 本表 |
| REG-70.15-R03-WINDOW-SCOPE | `tools/run70-r03-powercut.sh`、`crates/drivers/storage/src/driver/virtio_blk.rs`（注入夹具） | **登记（P3，窗口范围口径，2026-09-15）**：CORE-70-R03 的四子面（慢盘/满盘/flush-write 失败/checkpoint 断电）中，**断电恢复**已由掉电窗承载（脏断电 kill -9 ×2 + 复用盘恢复判据）；慢盘/满盘/flush-fail 三子面为**驱动单元红测**承载（`RamMmio::stall_next`/`fail_next` 注入 + 保留背压红测）——窗口级注入面未建，如实登记不以单元证据代填窗口证据；窗口级故障注入归后续门建设（E70-E 口径） | 本表 |
| DEC-70-13 裁决（2026-09-15，实施侧 bounded 裁决） | `crates/protocols/control-dispatch/src/{engine,plan_hold}.rs`、`REG-70.10-TRANSPORT-CAPS` | **裁决落定（多步计划投递面三则）**：①**包络生命周期 ≥ 计划时域**——单代认证包络的 valid_until 是批内全部命令的执行上界；跨期步由既有 `PLAN-COMMAND-SKIP-EXPIRED` 显式跳过，**不跨代续用旧包络**（无续期机制——fail-closed 优先）；②**单批 intent 上限 = 8**（容量墙 ≈11 留 3 余量给控制写/回读），更长计划拆批、每批独立走完整认证（无跨批免认证）；③**跨代续接 = 重新认证**——重启/换代后剩余步走"新快照→新认证→新投递"全链，代价一次认证往返（站端认证器无外网依赖，可接受）。plan_hold 接线由此确定为"按批重认证编排"，不建跨代包络续期 | 本表 |
| REG-70.15-R09R10-GATE-DESIGN | `tools/run70-vol89-gate.sh`（已建成）、`crates/ai/planner-peer`（`--negative` 四模式）、`crates/agents/control-services/src/remote_brain.rs`（`PEER-V2-REJECT`/`PEER-V2-LATE` 词形） | **部分建成（2026-09-15，R09 专属门首跑）**：**激活前置打通**——`configs/ai-peer.toml` 补 plan 参数（激活 v2 结构化轨）+ 装配切 **qemu-test-synth**（qemu-prod 强制 ReleaseTls:8883 而 peer 仅明文 ⇒ 提案链结构性不通；QemuTest 显式允许明文）+ broker 端口对齐（compile→relink 顺序铁律：束嵌 ELF）。**首窗实测**：11 条结构化请求到达 peer、HiGHS 求解 optimal、应答发布 ✓；expired/crossgen 目标侧拒绝词形 ✓（R2/R3）；unsigned 伪造关联被 Correlation→Late 丢弃路径接收 ✓（R5 计数未达门限）；零 PANIC ✓（R7）。**两个剩余集成缺口（门正确咬住）**：①应答投递有损（11 发布→5 送达 worker，broker→target 订阅转发丢包）；②**代次竞争**——accepted 远程计划在下一 tick 被快照换代复核丢弃（"accepted plan generation changed before consume" ×2，STALE-DROP 诊断行）⇒ used=true=0。修法方向：响应 RTT 快于快照重建（svc-mqtt driven 轮询提速）或消费侧绑定放宽至目录代次（dispatch 时域复核兜底）。**R10**：DEC-70-13 批上限已落地（`PEER_PLAN_MAX_ASSIGNMENTS` 64→8，t3 测试重构为超批负例+2×4 合规正例）；**工程已落地（2026-09-16，R10 会话）**：①per-asset 控制绑定架构——`control_contract_bindings`（每可控资产一点；歧义/缺失 fail-closed）+ `fetch_control_binding`→`Vec<(asset,device,point)>`（点表一次往返）+ 引擎**多路由**（`AssetRoute{asset_id,profile,chain}`，`DispatchConfig::extra_routes`，信封/语义摘要/爬坡基线/SBO/链复验按命令路由执行——每资产独立链实例，写链/仲裁器/settle 安全语义零放宽；仲裁在途登记按涉备键集）+ `PlannerView` 绑定注册表化（asset→device 来自点表解析）+ svc-device-agent 多收口点（`request_target` 路由分派，未注册目标归默认收口点 default-deny）；②多资产目录手术——asset 2（battery/device 8/node 2）+ 合同 2（点 1009@0x0201、回读 1005、±500/ramp 3.33）+ 铭牌 2（50kWh）+ 点语义 1005..1009 + 从站寄存器 0x0110..0x0113/0x0201 + synth 馈电 +1005（认证器 readback_ok 承重面）+ 来源授权面 1/2 扩至 1009；生产目录载荷 1556/1664B（余量 108B，容量墙再临） | 本表 |
## G-70.15-B. 蓝图 70.15 批量完整窗重跑与候选链重录（2026-09-15）

> 修复面（队列自恢复 + 描述符回收 + INTENT_RETENTION 有界驻留 + 768 KiB 堆 + 逐服务看门狗
> 容忍窗 + 回读点四环）全部落地后的**第一次全量完整窗验证**。原始窗日志：
> `evidence/70/70.15/batch-r234/`（门自记日志）与 `evidence/70/70.{1,2,3,4,6}/`（串口全文件）。

### 1. 批量窗结果（全部当前构建、完整声明窗长）

| 门 | 窗 | 终态 | 关键数字 |
|---|---|---|---|
| vol1 | DL=1300 | G1/G2/G3/G5/G6/G7 ✓，**G4 FAIL** | rounds=10600、**Committed=2156**（越过旧故障点 512）；全文件零 PANIC/零分配失败 |
| vol2 | SEED 420 + REUSE 1200 | R1/R2/R3 ✓，G5/G6/G7 ✓，**G4 FAIL** | 复用盘开卷 points=108、checkpoint 行=22、rounds=10380、Committed=1192 |
| vol3 | DL=1200 | R2/R3 ✓，G5/G6/G7 ✓，**G4 FAIL** | PROTOCOL-CONFIG=6（≥6）、七种拒绝词形全 ×0、rounds=9641、Committed=1705 |
| vol4 | DL=1200 | R1/R3 连接面/R5 连接面/R6 ✓，**R2/R3 提交锚/R5 ARMED ✗** | CONNACK 2+1、订阅恢复、READY、尾部回读 ACQ=44；**`dispatch=Committed=94`**（南向点表持续提交）；used=true/LEASE-ARMED 词形缺失（`REG-70.8-SYNTH-NO-OBS` 链 + 断链后 AI 退出安全设计） |
| vol6 | T-1 700 + T-2 420 | **21/22 ✓**，R24 ✗（在案间歇，第三形态） | config-get/换包 gen 1→2/维护进出/受控重启/五负例/账本恢复 n=15 全 ✓ |
| vol7 | A/B 双镜像 | **10/10 PASS** | BOOT-TRIAL tx committed + 跨重启 BOOT-CONFIRM slot=B |
| 判定库红测 | host | **PASS** | `evidence_chain` 29/29（候选负例形态 + selftest） |

**块层死亡螺旋签名族全窗清零**：四个 qemu-prod 窗串口全文件
`virtqueue full`/`device IOERR`/`blk-wait TIMEOUT`/`QUEUE-RECOVERED`/`SNAP-SKIP`/
`HEARTBEAT-TIMEOUT`/`PANIC` **全部 0 命中**（对照修复前 17M 长窗 1350 处命中）。
队列自恢复在本批**无需出场**（零停顿发生），其有效性由注入红测承载。

### 2. G4 尾衰减复现——先前归因被本批证伪（REG-70.15-G4-PHASE-TAIL-DECAY，**已收口见下**）

> **收口（同日，逐相位时间线复盘 + 判据重标）**：三候选复核——checkpoint 时长两半
> 持平（24.3s/24.3s，排除变长占用）、round 计数与计数器 1:1（排除日志稀释）、
> **真因 = 产出相变**：commits/相位 vol1 [65,32,831,1228]、vol2 [14,7,189,982]、
> vol3 [65,32,479,1129] 同形——尾相位每轮都在提交，轮数下降 = 每轮工作变重
> （提交持久化成本），非进展塌陷。**G4 重标为产出相位判据**：①每相位 round ≥
> ceil(min_rounds/4)（活性地板不变）②尾相位 commits>0 ③尾 commits ≥
> max(C1,C2)×80%；反向红测 ×2（tail-commits-stop-fail 必红 / workload-shift-pass
> 必绿；自检 10/10）。**实弹复验**：vol1/vol2/vol3 三门重跑全 rc=0
> （尾产出/中段峰值 = 1218/841、971/200、1120/488）。

四窗同一形态：尾相样本跌到第二相的 31–38%（vol1 1218/3663、vol2 971/3063、
vol3 1120/2991），但**尾部绝对值远超地板**（1218/971/1120 vs 130/30/100）。
堆 PANIC 修复后衰减**依旧存在** ⇒ 先前"堆预算 PANIC 的后果"（WINDOW-CALIB）与
"幂等账本写满"两条归因**不成立**（或至少不是全部）。候选根因（未定位）：
TSDB checkpoint 变长占用、驻留驱逐路径成本、合成/南向负载相位。
判据口径（尾/二比）是否适配长窗节奏属窗口校准裁决——**不得为转绿静默放宽**；
真根因定位需带时间线的逐相位探针（先复现后修纪律）。

### 3. 构建非确定（REG-70.15-BUILD-NONDETERMINISM，**治理落地：共享镜像路线**）

> **收口（同日）**：`gate-env.sh` 新增 `gate_build_stamp`/`gate_relink_cached`/
> `gate_image_raw_cached`——戳 = HEAD+工作树摘要+feature 集+重链接脚本摘要+
> **ENEROS_* 环境签名**+已编译束摘要；命中即共享同一份镜像字节。**实弹教训**：
> 首版戳漏了环境签名，vol6 缓存命中无探针束的 vol5-B 构建（PROBE_BUNDLE 由
> build.rs 编译期烘入）⇒ 探针 step3 empty-bundle 级联失败——已修并复验（vol6
> 重建后零 ✗ 全 PASS）。重链接逐字节确定性（跨扫描）仍 Open，不阻塞候选闭合。

同树（源码/配置零变更）四门各自重链接，qemu-prod 镜像得到**四个不同摘要**：
vol1 `6160ad7f…`、vol2 `9c652d9f…`、vol4 `b3288a37…`、vol3 `b0c42362…`
（各门 `image.sha256` = 该窗**启动前**受测字节）。⇒"一个候选 = 一棵树 × 一组产物"
在 qemu-prod 面退化为**逐窗一份产物**：候选索引的产物绑定只能取磁盘现态，
逐 case `tested_sha256` 成为唯一可靠受测绑定（checker 逐 case 闭合校验即为此设计）。
CI 的双构建比摘要（`ci.yml:573`）走干净双构建，未覆盖"增量重链接跨门漂移"形态。
候选身份完整闭合（多窗共享单产物）需先修重链接确定性，或门间共享同一镜像副本。

### 4. 判据器缺陷（新登记 REG-70.15-VOL4-R3-ANCHOR-ZERO，P2）

`tools/run70-vol4-gate.sh` 恢复点行锚 `r1=0`（段1 `used=true` 缺失 ⇒ 锚行找不到 ⇒ 0）时，
sed 以行地址 0 报错、"锚1 后"判据域退化为全窗扫描。本批实际影响有限
（提交/回读词形本就因 SYNTH 链缺失），但锚失效即静默换域是判据器通病形态
（同族：`REG-70.FL-JUDGE-SCAN-SCOPE`），须改"锚缺失 ⇒ 显式 SKIP 该组判据并 FAIL"。

### 5. 候选链重录与终态

- 首录候选 `7a77066f…` 目录内三份 run-record 的判定文与日志不符（沿用了上一会话
  的数字）⇒ **整目录移出仓库归档（/tmp）**，以 batch-r234 原始日志为准重录。
- 现候选录入：R01/R02/R04/R05（qemu-prod 四窗，逐 case `tested_sha256` 绑定各自
  启动前字节）、R07（vol6 窗）、R08（vol7 窗）、R12（判定库红测）、R11 **BLOCKED**
  （EXT-70-HW，蓝图 [10] §7）。
- **MISSING（如实）**：R03（独立掉电/慢盘窗未跑）、R06（vol5 独立窗未在当前构建
  重跑）、R09/R10（`REG-70.8-SYNTH-NO-OBS` ⇒ `REG-70.15-R09-R10-UNPROVABLE`）。
- `cargo xtask evidence42 candidate-check` 终态 = **NOT_READY**（机器可读）。

### 6. 结论

"活着但不再控制"三大根因环（队列失步无恢复、堆无界驻留、看门狗误杀）修复后，
四门首次达成**零 PANIC 完整长窗 + 提交 1192–2156**。剩余缺口收敛并全部在案：
G4 尾衰减（新登记）、vol4 的 SYNTH-NO-OBS 链、R03/R06 独立窗、R09/R10 观测源窗、
构建非确定（新登记）、R11 外部。核心整组维持 **NOT_READY**，不写 Done。

### 7. 续（同日晚）——SYNTH-NO-OBS 收口与 R03/R06 补窗（候选链三录）

- **`REG-70.8-SYNTH-NO-OBS` 收口**（详见本表该行，红测 5 例先红 + 探针窗两轮实弹）：
  合成装配 OBS 生产轨打通——40 批全受理、SNAP-BUILT ×10（pts=8 含回读点 1001）、
  EA status=OK ×11、PLAN-COMPILED、delivered=1，终态 Indeterminate（无执行器的
  预期安全终态）。`REG-70.15-R09-R10-UNPROVABLE` 更新为**部分解封**（主链可证；
  专属门/负例注入窗未建，case 维持 MISSING）。
- **R06 补窗（vol5 门重跑）**：16 项 15 ✓，**R4a Committed=39 ✓**（回读点修复在
  70.5 窗生效）；唯一 ✗ = R6 伪 principal 间歇（第三次观测）。
- **R03 补窗（新门 `tools/run70-r03-powercut.sh`）**：**PASS**——seed ≥3 checkpoint
  → 脏断电（kill -9）×2 → 复用盘保数据（points=60）、逐次脏重启 ready、零 PANIC、
  控制链恢复（rounds=2506）、checkpoint 恢复（×6）、零 CHAIN-KEY-MISSING。
  慢盘/满盘/flush-fail 子面由驱动单元红测承载（`REG-70.15-R03-WINDOW-SCOPE`）。
- **候选链终态（三录，现树键）**：R03 **PASS**、R08/R12 **PASS**；
  R01/R02/R04/R05 **FAIL**（G4 尾衰减 ×3 + SYNTH 链历史窗）、R06 **FAIL**（R6 间歇）、
  R07 **FAIL**（R24 间歇）；R09/R10 **MISSING**；R11 **BLOCKED**(EXT-70-HW)。
  ~~`candidate-check` 终态 = 8 violations~~（已被同日终扫取代，见 §8）。

### 8. 终扫（2026-09-15，五项治理全部落地后的全门重跑 + 候选四录）

- **全门 rc=0**：vol1（G4 转绿实证：rounds=10316/agent=OK=2237）、vol2（复用盘
  + G4 产出相位 ✓）、vol3（拒绝词形零误判）、R03 掉电窗（points=60 保数据、
  rounds=2944）、vol5（**R6 ✓** 治理生效：拒绝 1 / forger 发送 3）、vol6
  （**零 ✗ 全 PASS**，R24 ✓ 写后读回生效）、vol7 10/10。构建缓存戳生效
  （prod 组/vol5-A 命中共享构建，全扫描仅 prod+synth 两次重链接）。
- **候选链终态（四录，候选 `f2227b10…`）**：
  **PASS ×8**：R01/R02/R03/R04/R06/R07/R08/R12；
  **FAIL ×1**：R05（vol4 的 AI 提案链激活前置未满足——真机制面已定位并登记
  于 `REG-70.15-R09R10-GATE-DESIGN`，非网络面/非回归）；
  **MISSING ×2**：R09/R10（专属门待建，设计已定稿 + DEC-70-13 三则裁决落定）；
  **BLOCKED ×1**：R11（EXT-70-HW 外部，蓝图 [10] §7）。
  `candidate-check` = **3 violations NOT_READY**（1 FAIL envelope + 2 MISSING，
  机器可读）。剩余缺口收敛为"R09/R10 专属门建设"单一工程项 + 外部硬件。

### 9. R09 专属门首跑（2026-09-15 深夜，工具面建成）

`tools/run70-vol89-gate.sh` 建成并完成首个完整窗（qemu-test-synth 装配 + 明文
broker + peer 六段轮换 healthy/expired/crossgen/mutated/unsigned/recover）：
**R1 ✓**（11 条结构化 v2 请求到达 peer、HiGHS optimal 求解）、**R2 ✓**（expired
ttl 拒绝 ×1）、**R3 ✓**（crossgen 绑定代次拒绝 ×1）、**R5 部分 ✓**（unsigned
伪造关联 Correlation→Late 丢弃路径存在）、**R7 ✓**（零 PANIC）；
**未过**：R4（mutated 拒绝 0/2——投递有损）、R1b/R6（used=true=0——代次竞争
丢弃，见 `REG-70.15-R09R10-GATE-DESIGN` 两个剩余集成缺口）。
**VOL70.89-GATE: FAIL**（如实——门正确咬住真实集成缺口；这恰是专属门的
价值：把"提案链不通"从推测变为可选址的词形证据）。

> **第三/四跑（`--negative-cycle` 单实例轮转 + AI_INBOX_CAP 64 + broker
> 死订阅清理 + 修剪预算化落地后）**：R1 ✓ v2 请求 ×35、**R1b ✓ used=true
> ×9**（path=Slow [backend=planner] 远程计划消费——代次竞争修复实弹生效）、
> R2 ✓ ×1、R3 ✓ ×1、R4 ✓ ×1、R5 ✓ Late ×2、R7 ✓ **零 PANIC**——
> **四类负例同窗全部 fail-closed 实证**（ttl/绑定代次/预算钳制/Late 丢弃
> 各有互异目标侧词形）。唯 R6 ✗：恢复段（窗 77% 后）零 used=true——
> 末窗 brain/request 发布未达 broker 而同连接 telemetry 持续 ✓
> （第 4 次复现；svc-mqtt ai-publish 面或 broker 订阅转发的确定性退化，
> AI-RELAY 面审计为下段工作）。

> **第二跑（代次竞争放宽落地后，7/8 ✓）**：`facts_match_binds` 收窄为
> **目录代次绑定**（快照推进同目录代次 ⇒ 消费时重锚当前代：候选身份/时域
> 全取当前输入，远程分配值保留；认证器全量物理复核 + dispatch 四字段复核
> 双层兜底；目录代次变化仍丢弃）。**实测：used=true ×2（path=Slow
> [backend=planner] 远程计划消费）+ 四类负例全部命中**（expired ttl ×3、
> crossgen ×1、mutated 预算钳制 ×1、unsigned Late ×1）、15 条 v2 请求、
> 零 PANIC。唯 R6 ✗：恢复段（peer 第 6 次重启后）目标侧 brain/request
> 发布未达 broker（transcript +1085s 后零记录而 worker 视角在发）——
> 明文证据 broker 的多轮重启订阅者转发退化（evidence 工具面）+
> QoS1 inflight 语义，登记于本行"剩余"：恢复段投递修复（broker 死订阅者
> 清理或 s6 独立 broker）后 R09 转 PASS。红测：rebased_on_same_catalog_
> generation（红→绿）+ dropped_on_catalog_generation_change（保持红）。

### 10. AI-RELAY 审计收口与 R09 转绿（2026-09-15 深夜终段）

- **AI-RELAY 审计结论**：broker 零丢失（21 请求/21 应答对账）；**R6 四次复现真根因 =
  影子收件箱（InMemorySink cap=1024）在无采集上传半环的装配无人排空 ⇒ 第 ~94 批起
  Overflow 全拒 ⇒ 快照停建 ⇒ EA 恒 snapshot-stale**（`SYNTH-OBS accepted=0 rejected=11`
  证据行）。红测 `obs_ingest_does_not_wedge_at_sink_capacity` 实锤 1024/76（旧代码必红）；
  修法 = `ObsShadow::ingest_frames` 批后排空收件箱环（该环仅承担批内去重；跨批回放
  保护由 WatermarkTracker 独立承载，永不排空——安全语义不变）。
- **vol89 门 9/9 全 PASS（R09 转 PASS）**：used=true ×9 全窗分布（含恢复段 ×4）、
  四类负例拒绝词形全命中（expired ×2/crossgen ×1/mutated ×1/unsigned Late ×1）、
  零 PANIC。门自身两处判据缺陷修正：R6 尾窗从 400KB 字节窗改末 1/4 行窗（巨型日志
  下字节窗只覆盖 0.4%——历史误判源）+ peer 日志逐轮清理（跨轮累积使计数虚高）。
- **vol4 门装配切换（R05 复跑）**：qemu-prod 强制 ReleaseTls:8883 而本门 peer 明文
  ⇒ AI 中继结构性不可建。R05 scope=INTEROP（broker/peer 故障恢复）不要求 prod TLS
  ⇒ 切 qemu-test + synth（compile→relink 顺序铁律 + broker 端口对齐）。
  **复跑结果：故障恢复面全判据首次通过**——R3 锚1 后 Committed=3/回读 ACQ=50 ✓、
  R5 LEASE-ARMED ✓（长断链武装）+ 降级 used=false ×60 + DEGRADE-OBS ×115 + 锚2 后
  重连/订阅恢复 ✓、R6 READY+尾部 ✓。唯 R2 段1 used=true 未落地（12 请求全发/peer
  全答/broker 转发 ✓，worker 接收面与故障注入窗竞态；used=true 语义已由 vol89 ×9
  实证）——段1 加长 240s 的最后一次复跑进行中。

### 11. R05 转绿与候选终态（2026-09-15 终段收官）

- **vol4 门段1 加长 240s 后 PASS（R05 转 PASS）**：段1 used=true ✓ + 全部恢复判据
  （R3 锚1 后 Committed=3/回读 ACQ=50、R5 LEASE-ARMED + 降级 used=false ×73 +
  DEGRADE-OBS ×105 + 锚2 后重连/订阅恢复、R6 READY+尾部）——**故障恢复面与
  AI 提案链在同一窗内同时成立**（qemu-test 装配 + AI 中继可达）。
- **候选终态（候选 `8355d2d4…`）**：**PASS ×10**（R01-R09 + R12）、**MISSING ×1**
  （R10）、**BLOCKED ×1**（R11 EXT-70-HW）⇒ `candidate-check` = **1 violation**。
  本系列会话从 8 violations 逐项收敛：G4 重标（×3 卷转绿）→ R03 掉电窗 →
  R06/R24 治理 → SYNTH-NO-OBS 收口 → 代次竞争修复 → 影子收件箱排空 →
  装配切换与门判据修正。
- **R10 剩余工程**：per-asset 控制绑定架构（`fetch_control_binding` 多绑定化 +
  dispatch 引擎多 profile 准入——写链/仲裁器/settle 路径的安全关键面，不宜在
  长会话尾段仓促实施）+ 多资产目录手术（asset 2 + PCS-2 点集 + 从站寄存器；
  级联面已映射）。DEC-70-13 批上限与 2×4 求解器测试已先行落地。

### 12. CORE-70-R10 开发与续验（2026-09-16 起，尚未收官）

- **per-asset 控制绑定架构（红测先行）**：`control_contract_bindings` 多绑定注册表
  （同资产歧义由仓库激活层 `asset-multi-contract` 拒绝——守卫在更深一层，注册表层保持
  纵深防御）+ `fetch_control_binding` 多绑定化（cfg 放宽 `any(test,sel4)`，host 假件
  3 例：双绑定产出/缺点 fail-closed/传输缺失）+ **引擎多路由**（`AssetRoute` +
  `ConfigSeed.extra_routes`→`DispatchConfig.extra_routes`；`execute_core` 路由化——
  信封签发/语义摘要/爬坡基线/SBO 选择/链复验全部按命令命中路由执行，每资产独立链实例；
  `arbitrate` 按设备键参数化，批量按涉备集逐一裁决+清账；`last_confirmed` 基线按键
  (device,point) 经 `ExecutionCtx.target` 单点传递；装配期 (device,point) 双绑 assert）
  + `PlannerView` 绑定注册表化（构造期逐条对目录合同校验；未注册资产 None fail-closed）
  + svc-device-agent 多收口点（`request_target` 与 process 同链解码只读分派；未注册
  目标归默认收口点=原拒绝语义）。红测：`intent_multi_route_routes_by_registered_target`
  （扩展路由资产 Committed + 默认资产 Committed + 未知资产 NotAuthorized 零投递）。
- **多资产目录手术（按新架构重做）**：catalog.toml +asset 2/合同 2/铭牌 2/点语义
  1005..1009；points.toml +PCS-2 五点（device 8，从站 2 寄存器 0x0110..0x0113+0x0201，
  scale 镜像 PCS-1）；来源 1/2 授权面 1004→1009；slave61.py +VALUES/WRITEABLE/COILS；
  synth 馈电 +1005（`[SynthPoint; 12]`——含 asset 2 分配的远程计划要过独立认证，
  认证器 `readback_ok` 要求 1005 在快照受理观测里，此为 vol89 窗承重点）。
  **容量**：生产目录编译载荷 1556/1664B——REG-70.8.3-CATALOG-CAPACITY 余量仅 108B，
  蓝图级扩容裁决更紧。coverage 完备集/parity 未动（1005 presence 判定不入完备集）。
- **撤回旧学习态归因（2026-09-17 独立复核）**：前轮把低提交率归因于
  学习收敛，并将 09-15 的 2237 次确认称为异常窗，证据不足。09-14 的
  20 OK 日志包含内存 PANIC，不能充作健康基线。后续逐阶段计时显示
  TSDB 检查点阻塞冻结快照发布，输入到达时已过 30 秒有效期；零死区日志
  不支持该窗由学习停止控制。见下方存储复核与 vol2-checkpoint3 实证。
- **可选回读点快照接入（R10 认证失败根因）**：builder 只遍历
  `policy.required`——`optional` 字段声明「缺席不影响完整性」却从未被消费；
  合同 2 的回读点 1005 不在安全覆盖集 ⇒ 不进快照 ⇒ 认证器对 asset 2 恒判
  `contract-incomplete`（串口 `TRUSTED-ERR Envelope(2,…contract-incomplete)`
  实证，首窗 R10 计划全拒）。修复两半：①builder 补 5.5 段——optional 点合格
  （在场/新鲜/合格/可裁决）则与 required 同通道入 accepted，缺席/不合格
  **静默省略**（不进 missing/stale/bad，完整性不受可选点影响）；②生产政策
  `build_policy` 从目录合同派生 optional = 合同回读点 − required（目录单源，
  不动 coverage 完备集——与 REG-70.15-CORE-CHAIN-SINGLE-GAP 的 1001 入覆盖
  集路径正交）。红测：optional 在场入快照+缺席不拒（snapshot-service）。
- **夹具级联**：semantic-catalog 生产形状夹具（assets 3/points 17/contracts 2）、
  catalog_gateway（注册表双绑定+极小目录歧义负例）、catalog_spec_source（双资产规格）、
  resource-registry（PlannerView 注册表形态 + 独立极小变体目录避容量墙）、obs_shadow
  （镜像锁 12 点/1200 事件/授权面 1009）、dispatch_assembly/dispatch_service
  ConfigSeed 补字段。全量回归 **9073/0**、fmt/clippy 0、aarch64 目标侧编译过。
- **vol89 门 R10 探针转硬判据**（assets=2 缺失=FAIL）；实弹窗装配词形逐行核验：
  `CATALOG-BINDING point=10 device=1 asset=1` + `point=1009 device=8 asset=2`、
  `CONTRACT-VIEW-ACTIVE contracts=2`、`REGISTRY-ACTIVE binding=(1,10)…asset=1 n=1` +
  `binding=(8,1009)…asset=2 n=2`、双 ingress lease_ok=true、零 PANIC。


#### R10 续验：R01 判据裁决（2026-09-17 更新，撤回 09-16 降标）

保留目标压力窗 `agent=OK > 512`，另保留 G1–G7 与生产 64KiB 审计预算
10000 次追加测试。G5 的 `floor(DL/45)` 是最低控制活性，512 是旧内存故障点
的压力覆盖；不能用前者替代后者。交接优先选项 (a) 的“结构性不可达”前提
已被存储修复后的目标实测推翻，最终 R01 须重新越过该故障点。

09-16 的中间窗（`r10-review/vol1-before-serial.log`）确有 96 次确认、
8562 次 snapshot-stale，G1–G7 当时通过，但它只证明存在节流，不能证明
架构上必须低于 512。09-14 的 20 OK / 17 Committed 基线还含 telemetry
alloc PANIC，不是健康验收基线。保留原始日志及 `vol1-calibration.json`
作为被撤回诊断的审计记录，不能作为放宽压力门槛的依据。

反证为 `evidence/70/70.15/batch/vol2-checkpoint3.log`：prod 镜像
`dd33b676ec5c2e73b12d388eaa07078fca25e2d9a757ce8c6f46d4a36dcc5003`，
420 秒首启 + 1200 秒复用，4230 次确认、4792 轮，各相位确认
[1005,1092,1098,1035]，G1–G7 PASS。首启/复用检查点 17/50 次全部成功，
数据丢弃 0，复用盘打开旧 points=15。这是存储修复的中间验收，尚不是新增
生产计划持有代码的最终树证据。10000 次审计测试仍独立验证容量585、
9415次 BufferFull 可观测、旧记录及链头保持；不冒充目标审计满载证明。


#### R10 独立续验（2026-09-17，录制尚未开始）

本节先前的学习收敛归因和“assets=2 即 R10”结论不能作为完成依据。
当前生产 `run_typed` 仍不持有未来步；`BatchSaga` 和 `PlanHolder` 尚无生产驱动，
故 R10 多时段、部分失败、掉电续接尚未获得目标证据。12 case 统一录制和提交仍待完成。

已复现并修复：空持有器 tick 越界；Saga 未注册路由 panic；批内 Unknown 被后继
Committed 覆盖；Saga 重启将 Executing 改回 Pending 导致重发；对账误入执行入口；
IntentRecord 写盘遗漏 plan_sha256；多电池共用一个 SOC；计划输入将观测网关 ID
误作控制设备 ID。回归日志位于 `evidence/70/70.15/r10-review/`，其中 red/green
文件保留失败与修复后的真实结果。对账现在使用无写载荷的独立 Modbus FC03 查询。

中间版本门结果：vol1（1300s）PASS；vol2（420+1200s）FAIL，仅 G5（16 < 26）；
vol5（560s）PASS。随后代码已变化，这些结果不能冒充最终树的统一候选证据。
vol2 时间诊断首次两窗：生成到 OBS 完成分别 38297ms/30995ms，超过 30000ms
控制有效期，推翻“只因学习收敛”的单因结论。快照读取成本红测：同一 blob 四次
读取产生 468 次块 RPC；正在验证有界读缓存，时间戳与 G5 门限均未放宽。


#### 2026-09-17 存储与控制链追加复核（中间树，不是最终候选）

- `vol2-tsdb.log` 的脚本输出 PASS（reuse 5061 Committed），但 37 次
  `TSDB-DISK-DROPPED`、pending_batches 最高 260；这个 PASS **撤销作为验收依据**。
  `vol2-tsdb-rejudge.json` 已按新增持久成功判据判负。R02 正常窗现在必须有
  `status=OK` 的 checkpoint，零 ERR/零丢批；故障注入窗 R03 仍单独核对失败与恢复。
- SnapshotFace 缓存 4×4 KiB、TSDB 缓存 16×4 KiB；总计最多 80 KiB，未扩大
  服务堆。TSDB 文件保持原 wire，将七次小写合为一次；七路 host 批窗从
  9063 降为 1176 块 RPC（仅成本回归，不能代替目标时长判据）。实弹中
  checkpoint 首阶段约 7–12s，原 26–38s 同步停顿的来源已定位。
- 实际 TSDB LFS 卷仍是 256 KiB（声明区段预留 8 MiB，不冒充已可用容量）。
  512 批×7 路红测在第 448 批失败；原子追加到有界尾 chunk 后重挂可查全量。
  没有扩大几何、删历史或放宽持久水位来通过。新索引也改为原子替换；
  启动按 CRC 校验过的文件重建实际计数，解决 chunk 已换而索引未换的断电边界。
- 故障注入覆盖第二次 checkpoint 每个块 RPC 写入截止点，原已确认点必须
  全保留；worktree 只还原旧索引写法即复现历史数据不可查。另有文件原子覆盖
  基线 A/B：旧实现先 remove 目标，故障后 NotFound；修复后仅完整旧值/新值。
- 写失败时保留已受理 TSDB 缓冲，修复原 `chunks.remove` 提前丢失可重试点。
  自动不可逆计划现在拒绝（无人工批准能力）；高优先级抢占未知结果必须先对账，
  不再以不同幂等键的新写冒充对账。两项控制链红测修复后通过。
- 原统一录制脚本已设前置阻断并删除以 assets=2/DEFERRED 伪证 R10 的配方。
  临时 DEBUG-r10 时间诊断已撤去；最终重建和全门复跑仍未完成。

生产批入口裁决见 ADR-0054：在 dispatch 内持有完整认证计划，Saga 事件先持久
后推进，冷启动旧未发步不按新单调时钟补发，剩余需求重新取可信输入并认证。
该决策仅冻结实施边界，**不代表生产入口已接入**。续验另复现冷启动后 journal
有未知记录但 arbiter 来源表为空时，新计划仍能写入；现已阻止这种新键投递，
同批重放仍返回原未知回执，相关红/绿测试在 `restart-arbiter-*` 日志。
本次全仓 host 回归 9094 passed / 0 failed，fmt/clippy 零告警；此统计先于最后
一项冷启动仲裁修复（后者已单独通过 control-dispatch 全套），不可当最终树验收。


#### 2026-09-17 生产持有入口接入中（尚未统一录制）

ADR-0054 已登记。新增 HeldPlans 数据区 `[25736,25992)`（512B块），双
64KiB 槽，不移动旧区段。dispatch 可信多命令入口现在持有整份候选、证书
验证材料、可信输入与编译意图，Saga 事件先持久后推进；“已持有”返回
Persisted，只有全批完成才可返回 Committed。周期 tick 先于 EA 冷却/worker
不可用/无动作分支；无可信输入或重启时，原未发步明确取消，已发未知效果
只读对账。新增逐命令状态、回读与未执行原因查询。每批最多8条、整计划
最多2批（16条）的内部存储界；peer wire 当前仍限8 assignments，扩展及
三资源目标验收尚未完成，不能据内部容量宣称目标完整支持。

已通过主机“只提交一次、四步独立 tick、重启保留已确认且取消未发步”链测。
丢失 ACK 的两次独立回读还必须持久更新原 intent journal 后才能放行下一步，
该问题已有红绿回归。Saga 事件存储确认失败会毒化实例，重新加载前禁止续写。
另修复 durable-state 忽略 CRC 完整未来格式而回退旧槽的问题：现在明确拒载；
基线 worktree 红测在 `held-schema-baseline-red.log`。生产目标编译、崩溃注入、
输入保留租约、完整 R10 多场景实弹仍待完成；不得用 assets=2 或 DEFERRED 放行。


#### 2026-09-18 R10 收官：控制窗违约修复 + 专属门建成 + 全门马拉松统一重录（终态）

**控制窗双胞胎缺陷（DEC-70-13 ① 违约，R10 门首跑实弹捕获）**：
`telemetry.rs::DEFAULT_CONTROL_HORIZON_MS=30s` 与
`resource-registry/provider.rs::base_env` 的 +30s 各写一份，而计划时域
= 4×15s=60s（DEC-70-13：包络生命周期 ≥ 计划时域）。多时段计划第 3 步
（+45s）起超包络窗，认证器 `Envelope { asset_id }` 拒（串口 TRUSTED-ERR
实证）——30s 是单命令时代遗留，DEC-70-13 裁决后未同步。修复：
`eneros-decision-contract::CONTROL_HORIZON_MS=60_000` 单源，两处引用；
红测 `base_env_validity_covers_control_horizon`（+30s 变异必红，已咬合）。
受影响测试两例按原意更新（载荷有效期显式更紧形态承载"min(策略窗,载荷)"
不变式；evalcfg 锁单源常量）。

**synth 批节奏与 R10 时域**：qemu-test-synth 每 3 round 一批 ⇒ 快照轮换
≈14–26s < 60s 时域；换代即取消未发步（DEC-70-13 ③ 设计语义）⇒ 单计划
最多执行步 0。`SYNTH_OBS_EVERY_ROUNDS` 3→16（轮换 ≈80–128s ≥ 时域；
仅测试装配 feature 门控，生产零影响）。**连带适配**：vol4 段5 长断链
100s→420s（请求按轮换分簇后 miss 攒满须覆盖 3 轮换最坏形；判据本身
不变——LEASE-ARMED 必须出现，×641 实证武装）；vol4 DL 650→1000。

**R10 专属门（tools/run70-r10-held.sh，三窗）终态全绿**：
- 段A 健康多时段：v2 请求 assets=2 steps=4 ×22、PLAN-HELD、
  HELD-BATCH Completed、Applied ×14、传输级写后同址读回 ×8、
  双资产写入（0x200×5 / 0x201×3）、写时刻跨度 34s（多时段分步发放）、
  零 PANIC；
- 段B 脏断电重启续接（同盘 kill -9 后二启）：HELD-STORE-OPEN OK、
  未发步取消 Blocked ×2、已确认前缀保留（2 条 intent 零重复执行）、
  重启窗 FC06 ×4 全部可归因到新投递（dispatch=Committed ×4；对账只读）——
  ADR-0054 持有链掉电续接合同首次目标侧完整验证；
- 段C 部分失败（slave --reject-write-addr 0x201 按址拒写注入）：
  0x200 写 ×5（资产1 健康）∧ 0x201 WRITE-FI ×1（资产2 被拒）、
  混合终态（Applied ×1 / Failed+Blocked ×3）、批 Failed——部分失败
  不吞健康资产。
- 判据器两处修正（如实）：map_receipt 对新鲜应用不上浮 readback（对账
  路径专属）⇒ A4b 改传输面取证（写后同址 FC03 读回同值）；A5b 跨度
  取整秒（bash 算术无浮点，浮点比较静默归 0 实测咬过）。slave61.py
  新增 --reject-write-addr 与 WRITE/WRITE-FI 的 ts 字段；门每段清
  transcript（跨运行残留旧条目会炸 ts 提取）。录制器 verdict_lines
  正则扩展 [An]/[Bn]/[Cn] 判据段与 R10-HELD-GATE 终判行家族。

**全门马拉松（冻结树统一重跑，全部 PASS）**：vol3（DL=1200）、
vol1（DL=1300；rounds=3972 / agent=OK=3728 **越过 512 压力线**——
交接期"512 与 G5 结构性冲突"判断被存储修复后的最终树推翻）、vol2
（SEED 420+REUSE 1200）、R03 掉电（300×3）、vol5（DL=560；R6 伪
principal 拒绝 BadSignature ×2——TCP recv 按墙钟计时修复的实弹验证，
此前轮询计数冒充毫秒使 TLS 记录停顿宽限实际 ~5ms，会话反复 ConnectionReset
重连掉订阅）、vol4（DL=1000）、vol6（DL=700）、vol7（DL=700+T2 420）、
vol89（DL=1440；UEFI 间歇冻结重跑一次后过）、R10 门。全量 host 回归
9102/0、fmt/clippy 0。**UEFI 冻结两次（vol89/R10 各一）与端口残留一次
（R10 门早退未清编队——已修 kill_window）如实登记，重跑即过。**

R10 的 12 case 统一录制与本登记段为同树最终态；peer wire 仍限 8
assignments（2 资产×4 步恰好满载），三资源扩展维持未验收登记。

#### 2026-09-19 R10 复查六缺口修复收官（30s 窗回退 + 三资源贯通 + 输入保留 + 实际回读 + 门判据重订 + 候选身份闭环）

前置：2026-09-18 手动复查（manual-review-handoff-20260918.md）判定"尚不能确认 R10 完整收官"，六缺口：
Standards 3 项（60s 控制窗违约 / 冻结输入保留缺失 / 实际回读不可查询）+ Spec 5 项（含三资源未验收、
读回判据误放行）+ 候选身份未闭环。本轮全部修复并实弹验证；提交 216a7e5（代码/配置/门）+ 后续文档提交。

1. **控制有效期合同（Standards-1/Spec-1）**：`CONTROL_HORIZON_MS` 60s→30s 回退——ADR-0054:34-35 的
   30s 是安全事实，上一轮为容纳 4×15s 配置放宽到 60s 属违约（DEC-70-13 ①"包络生命周期 ≥ 计划时域"
   按"时域收敛进窗"满足，非反向放宽窗）。演示时域 4×7s=28s≤30s（step_ns 15s→7s、ttl_secs 60→30、
   synth %16→%8：建距 38–64s > 28s 时域 + 首步窗错过时 ~40s 一代重试）；输入新鲜度策略窗 60→30s
   对齐载荷有效期。红绿测：`held_plan_horizon_beyond_control_window_rejected`（4×15s 提交即拒 /
   4×7.5s==valid_until 边界受理 / 过期 tick 未发步取消零新写）。**未放宽任何窗，未改 ADR。**

2. **冻结输入保留（Standards-2/Spec-3，ADR-0054:29-30）**：新增 `HeldInputRetainer` seam
   （`TEL_REQ_SNAP_HOLD/RELEASE`=7/8）——准入前钉住证书引用的冻结快照（钉面不可用/容量满 ⇒ 拒绝新
   计划准入，fail-closed）；计划终态且无未决后释放（幂等可重试）；重启按非终态计划重派生钉集（钉 =
   RAM 派生态，唯一持久事实 = 持有计划）。event_face 剪枝跳过 pinned（`MAX_SNAP_PINS=4`）。
   红绿测：`t8_pinned_snapshot_survives_prune_and_release_recycles`（30 窗压力钉存活 + 释放回收；
   变异"剪枝跳过删除"实证咬合）+ 引擎侧准入即钉/完成即释/拒收 fail-closed/断电重钉四测。

3. **实际回读持久化（Standards-3/Spec-4，ADR-0054:15-16）**：`post_apply_readback`——saga Applied 后
   经独立只读通道读**命令点**（设备真实持有值）写入 `CmdOutcome.readback` → Saga 事件持久化 →
   `DISP_HELD_QUERY`（HQR1）可查；串口 `HELD-COMMAND ... readback=Some(v)`。传输 ACK（设备结果码）/
   实际回读（命令点独立读）/ 物理反馈点（合同 readback_point_id，对账路径）三语义分离。红测
   `applied_commands_carry_independent_readback_not_setpoint`：设备读值漂 +0.05（容差内 ≠ 设定值），
   断言 Saga 记录 = power+0.05 非设定值回显。R10 门 A4c 判据 `readback=Some` ≥1（实弹 ×12 行）。

4. **三资源×四时段完整计划（Spec-2）**：计划级 inline 上限 8→64（蓝图 08 §9 容量表原文；65 拒绝测试
   `v2_plan_cap_boundary_64_accepted_65_rejected`）与单批上限 8 分离（`MAX_INTENTS_PER_BATCH` 单源，
   compiler_version 1→2；12 分配默认拆 2 批红锁 `default_policy_enforces_dec7013_batch_cap_of_eight`）。
   第三资产手术：catalog asset3/合同3/铭牌3 + points 1010/1011/1014（device 9）+ slave61
   0x0118/0x0119/0x0202 + synth 馈电 14 点（超 12 帧单请求上限 → 拆双 OBS 请求，sequence 单调守恒测）。
   **容量链六处连锁**（实测逐层咬合）：目录 PAYLOAD_MAX 1664→4096（1857B）；配置区 16→24 块
   （Stage 基址 +8 块落业务带声明的节距余量，Active 基址不动保旧盘 envelope 自描述可读）；
   MGT1 分片 16→24；svc-config 12KiB 区段栈缓冲出栈（12KiB 栈帧 + 块读深路径超 32KiB 栈 = 静默溢出
   冻结实弹根因）；device-agent 1MiB bin 专属堆（3×276,040B MetricsCollectorImpl ingress，768KiB
   共享堆实弹 OOM）；`AI_REQUEST_MAX` 1024→4096 + `APP_MSG_MAX_LEN` 1024→4096（3 资产请求 JSON 实测
   1079B 超 1024 双墙 = 实弹零请求/每 tick link_err 根因；尺寸回归锁
   `v2_three_asset_request_size_fits_relay_budget`）。
   实弹（run14）：`PEER-REQ-V2 assets=3 steps=4` ×3、`used=true assignments=12 assets=3`、
   `PLAN-COMPILED intents=6 skipped=6`（12 分配守恒：分辨率合并计入 skipped 不丢账）、
   `HELD-PLAN-DONE applied=6 state=Committed`、三地址 0x200/0x201/0x202 全写。

5. **R10 门判据重订（Spec-5）**：`tools/r10_readback_oracle.py`——序列敏感写后读（文件序 = 时间序；
   每读至多消费一次；`--selftest` 7 例负例集：写前读（旧门误放行形，保留红例 readback-before-write
   .jsonl：旧 oracle=2 / 新 oracle=0）/旧读值/错误顺序/错误设备/重复消费）。旧判据（地址+值集合匹配）
   误放行实证归档 `target/r10-review/readback-oracle-negatives-20260918.log`。
   `tools/r10_plan_accounting.py`：按 plan id 关联 PLAN-COMPILED(intents+skipped) ↔ HELD-PLAN-DONE
   (applied+blocked+failed)，通过线 = I+S==12 ∧ 全 Applied ∧ Committed——从原始工件重算守恒，不搜
   词形凑数。门重订：A1 assets=3 / A2 守恒 / A3 等待 HELD-PLAN-DONE state=Committed（非首个批
   Completed）/ A4a 唯一 intent id 计数（周期重复打印不计数）/ A4b 新 oracle ≥ UNIQ / A4c
   readback=Some / A5 三资产写 + 跨度 / A7 终态守恒；B 窗改**飞行中杀**（首个 Applied 后 +3s，计划先
   完成则等下一个——run12 实弹：合并计划可 +12s 前整体完成 ⇒ 重启窗无未发步）；数据盘 rm+truncate
   真新鲜（truncate 不清零旧内容 ⇒ 跨运行残计划泄漏实弹修复）。vol89 门 assets=2→3 同步。
   **R10 门 PASS：21/21 判据全绿**（run14；A1-A7/B0-B5/C1-C4）。

6. **候选证据身份闭环**：提交 216a7e5（代码/配置/门，预检 sign-keys/mock-leak/gng-version 过）后
   在干净树上重跑全门马拉松（vol1-vol6/vol7/vol89/r03-powercut/R10），录制 12 case 统一候选链
   （record_r10_chain.sh 在最终提交树上执行——source_commit=HEAD ∧ dirty=∅），
   `candidate-check` 0 violation、仅 CORE-70-R11 BLOCKED（EXT-70-HW 实板未到货）。

**验证总账**：全仓 9118 测试 0 失败（两轮：9097→马拉松咬出两缺陷修复后 9118）；clippy
--workspace --all-targets -D warnings 零告警；fmt 干净。**全门马拉松（提交 216a7e5/091a487）**：
vol1(vol1 3967 轮/3549 提交 >512 压力线保持)/vol2/vol3/vol4/vol5/vol6/vol7/vol89/r03-powercut/R10
全部 rc=0——其中 vol89 与 R10 首跑失败（1664B 应答悬崖/爬坡限拒级联，即上文次生缺陷①②），
修复提交 091a487 后在最终树重跑全 PASS；批日志与退出码 `evidence/70/70.15/batch/*.exit`。

**登记（Open/观察）**：
- run11/run13/马拉松首跑的资产 3 写失败**已根因关闭**（HELD-COMMAND reason= 实弹定位 =
  SafetyRejected ramp-limit |0-5|kW/0.9s=5.5kW/s 超合同 3.333kW/s——System1 与计划步交错写），
  修复 = 计划在途 System1 抑制（091a487）。
- MQTT-HS ReadFailed 自检计数全程递增（n=126+）为既有背景噪音（旧证据同样形态），不阻塞连接与
  发布；归因未做，登记不改。
- REG-70.11-HOLD-UNWIRED（hold_input_offset 零调用方）状态更新：生产保留面已由本轮钉/释放面
  （TEL_REQ_SNAP_HOLD/RELEASE）承担；offset 收紧面仍无调用方（维持登记）。
- 三资源下快照 pts=12（8 必收 + 4 可选含 1010/1011）；asset_soc(3) 依赖可选点在快照内（synth 馈电
  覆盖）。若未来站点停喂 1011，asset 3 自动退出规格（fail-closed 不发请求）——语义如设计。


## R13. 架构体检 R13 登记（2026-09-20）

来源：架构体检 R13（报告 /tmp/architecture-review-20260920-204126.html）七条登记项深挖；清偿 = 蓝图 72 两修复批（本段）。

1. **修复批A（锚定与治理批）**：A1 代次判据死文本订正+双策略命名锁测（计划绑定代次 vs 准入同代——CONTEXT.md §1 消歧行+两侧注释互引+三现状锁测）；A2 节奏上界锚定（judge `--cadence-max-s` 显式传参化+vol1/2/3 调用点补参+selftest 四例+service.rs 注释订正）；A3 ADR-0052 修订段（激活前置经 ADR-0054 满足的定向解冻回写）；A4 driver_probe.rs 文档头名实订正。状态：已完成（本段落盘时）。
2. **修复批B（计划投递上下文单源与构造收拢）**：B1 `"pcs"`/`5_000` 六生产位点常量单源（STATION_PLAN_TARGET_LABEL/HELD_DELIVERY_SETTLE_MS）+源文本守卫测试；B2 `EngineSagaExecutor::new` 11 参→5 参构造收拢（PlanBinding+DeliveryContext 命名字段）+ADR-0052 §4 allow 退役。状态：见本段 B 完成行。
3. **R13-C · driver_probe 重命名**（触发 = 71 开卷捎带）：`crates/runtime/root-task/src/driver_probe.rs` → platform 语义名（现名 probe 字样系历史沿革，A4 已在头注登记名实差异）。引用面 7 文件清单（2026-09-20 实测）：root-task 内 5 处 + handover-abi/durable-log/time 三处 doc 交叉引用。重命名时须同步三处 doc 引用。
4. **R13-D · 词表翻译集中**（触发 = 下次新增任一词态）：四站点清单——`receipt_from_journal`(engine.rs:2700，ExecutionStatus→DispatchState 9 臂)、`held_receipt`(held_engine.rs:126，BatchState 聚合→4 臂)、`map_receipt`(engine.rs:3068，DispatchState→CmdOutcome 5 臂)、`durable_intent.rs:339`(MessageState→2 臂)；集中为 translation 模块 + 穷尽表测；DispatchState 码 0-11 冻结不动。（行号 = a72ae68 基准，实施时重锚。）
5. **R13-E · 窗口账本（WindowLedger）**（触发 = 下次 event_face.rs 实质改动）：`window_tasks`(event_face.rs:501，11 参拆借) 收拢为计数器+holds+snap_pins 成束结构 + 读出面；事实 = 7 计数器现零外部读出（2026-09-20 实测），`retention_blocked()` 是唯一进 interface 者。
6. **R13-F · 节奏上界结构化导出**（触发 = C3「交错时间预算单源」立卷）：A2 的 `--cadence-max-s` 显式传参升级为产品侧单源导出（判据器运行时读取，消灭脚本手抄值）。
7. **R13-Top 参考**：C1（割切时刻在途计划快照）/C2（System1 写许可仲裁）/C3（交错时间预算）为 R13 主报告前三候选，未列入本两批——71 规划时优先评估。
8. **A3 前提纠正（R13-R7 复核）**：R13 原报「registry.toml 无 ADR-0052 条目」系 grep 词形伪影——条目实际在册（`id = "ADR-052"` 两位式，path 正确，`cargo xtask policy adr` PASS entries=48）。不重复补登；两位式 id 与后期四位式并存系 registry 历史沿革，统一化归 R13-C 同批的 71 开卷治理项。
9. **R13-G · 投递上下文持久化全档（登记不施工）**：触发条件 = 首个非 "pcs" 真实标签需求（多站点差异/逐资产计划标签）。触发前不得实施（蓝图 72 修复批B §0.2 裁决：当前唯一生产标签=常量，持久化当期收益 0、纯成本 = HPL1→HPL2 magic 升版 + 四格迁移矩阵 + MAX_BYTES 60_000 预算面）。冷启动可执行设计存档 = `蓝图/72-架构体检R13登记项清偿蓝图/02-修复批B-计划投递上下文单源与构造收拢.md` §B3（格式/四格矩阵/预算/自动覆盖防线/红测清单五件齐，实施时重锚行号）。
10. **修复批B 事实修正两则（相对蓝图 0.1 预判）**：① `EngineSagaExecutor::new` 生产构造点实际仅 held_engine.rs 一处（engine.rs run_typed 直调 execute_core 不经 executor；saga_e4e5 用 MockExecutor）——迁移面 = 生产 1 + settlement_lock 集成测试 3；② `PlanBinding`/`DeliveryContext` 可见性 = `pub`（蓝图原判 pub(crate)）——集成测试 tests/settlement_lock.rs 在 crate 外，D-3 红测需命名字段构造 input_digest/wire_sha 可区分对；pub 面向本仓集成测试这一现实消费组，非投机公开承诺。③ 收编位点实为 3+2 处：蓝图列的 3 处 5_000 之外，dispatch_service.rs:178 typed 路径 `let deadline_ms = 5000`（无下划线词形，蓝图 grep 漏计）与 :328 submit_intent 路径同值同语义手抄——一并收编（B1 验收「生产代码路径零裸 5_000」的完整执行）。
11. **R13-R6 节奏上界测量记录（A2 步骤1/4，2026-09-21 实弹）**：vol1 持续控制压力窗（DL=900s，镜像 fa30a5c8/workspace 0.338.0，宿主墙钟轮询器逐 SNAP-BUILT 到达打点）：SNAP-BUILT ×35，**平均建距 23.2s，最大间隔 32.1s**（首个间隔；稳态最大 29.2s），最小 19.1s。历史对拍：马拉松 v4 vol1 窗 35 次/900s = 平均 25.7s（同量级）。**数值裁决：实测最大 32.1s ≤ 45s ⇒ CONTROL_CADENCE_MAX_S=45 保持，仅文档订正**（不放宽⇒无反向红测义务；不收紧⇒保持既判）。vol1/2/3 脚本显式 `--cadence-max-s 45` 与本记录互证；本次窗 VOL70.1 GATE PASS（G5 下限=20=窗900s/45s 与改前复算逐字一致）。旧证据不回溯重判（历史 verdict 均以 45 推导，本批改动后复算 vol1/vol2/vol3 三日志 verdict 逐项一致——已验）。
12. **修复批B 完成行（2026-09-21，v0.338.0）**：B1 六生产位点（蓝图列 3+3，实收 3+3 另加 dispatch_service typed/intent 两处同值手抄——见第 10 条③）全部改引 STATION_PLAN_TARGET_LABEL/HELD_DELIVERY_SETTLE_MS 单源；源文本守卫测试两枚（engine.rs 守 held_engine、dispatch_service 自守卫——先红后绿各验一次，守卫针经字符数组构造防自咬）。B2 EngineSagaExecutor::new 11 参→5 参（PlanBinding+DeliveryContext 命名字段，平铺字段构造时解构——14 处方法访问点零改动），ADR-0052 §4 allow 删除，settlement_lock 三构造点机械迁移（D-3 红测断言零改动全程绿）。host：全仓 9161/0、fmt/clippy(-D warnings) 零告警、feature-matrix/audit/sign-keys/orphan-check/config-refs/mock-leak 六门绿。QEMU：r10-held 复跑 5 次，段 A/C 判据全绿（run1/run3/run4 段A 10/10+段C 4/4；run2 段A 计划饥饿=预存时序方差形态，其失败词形 batch or command expired/intent window below policy minimum 在旧代码证据 70.4/vol89/fix-review 均已存在），[svc-control-dispatch] 行形态 diff vs 基线=零词形漂移（仅串口交叠噪声），target_device 值零串口暴露（三窗 grep=0）；B2/B3 五跑同签名未中=预存 R10-B 割切竞速延续（a72ae68 候选链 R10 MISSING 既有开放项，B 窗首计划完成后计划流停止的确定性相位使盲切 ready+40s 恒落于完成间隙——非本批改动引入，未达全 PASS 如实登记）。vol1 节奏测量窗 PASS（第 11 条）。
13. **R13 收口会话复验行（2026-09-21，工作树=57458a9 无源改动）**：S6/S8 补齐——蓝图 72 两档状态头回写完成态 + 执行卡 `蓝图/72-架构体检R13登记项清偿蓝图/03-执行卡-72.md` 落盘（gitignored，交接面）。独立复验：代码工件六查（常量/六位点/双守卫/结构体/5 参签名/allow 退役）grep 实证；受影响双 crate（control-dispatch/control-services）全绿 + 全仓 `cargo test --workspace --locked` exit 0；fmt/clippy/feature-matrix/audit/sign-keys/orphan-check/config-refs/mock-leak 全绿。QEMU 独立复跑 r10-held 一次（强制重建镜像）：段A 10/10 全绿、段C 4/4 全绿、B0/B1/B4/B5 绿、B2/B3 同签名未中（R10-B 预存开放项延续，同第 12 条定性）、target_device 零串口暴露（三窗 grep=0）。**镜像哈希语义澄清**：image raw 逐次编译非确定（每轮 config compile 新鲜锚/钥材料，gate 戳并入束摘要即为此设计）且每次 QEMU 启动写 raw（--reset-bootctrl+guest UEFI 写）——image.sha256 锚语义=启动时所拓镜像，跨会话比哈希判源漂移是伪判据。

## R14. Deepening-v1 蓝图执行登记（2026-09-21，基线 `60b73a8`）

来源：架构评审报告 `/tmp/architecture-review-20260921-225312.html` 七候选 → 蓝图 `docs/architecture/deepening-blueprint-v1.md`（本卷立档）→ 本会话执行。逐候选终态：

1. **候选 A · HoldRegistry（Strong）——Done（`d39fee1` + `6c5ee33`）**：`DispatchEngine` 五钉住字段收拢为 `engine::hold_registry::HoldRegistry`（全私有），六处四种淘汰惯用法单点化，F70-04 代次安全从注释维系升级为结构维系，`held_pin_failures` 去 pub 并补 `pin_failures()` 只读面（原两写点零读者）。R-A1 由 7 枚表级测钉各容器 16/17 边界；R-A2 存活引用计数留 engine；R-A3 词形冻结经**字符串字面逐字节 diff** 静态证实（`60b73a8 → HEAD`：held_engine.rs 仅 `"nonempty"` panic 文案随代码迁走，engine.rs 零生产字面变更）。host：control-dispatch 89 单测 + 全集成（含 typed_chain F70-04/H06 六测回归网）+ 下游 control-services 216 测全绿、clippy `-D warnings`、`aarch64-unknown-none` 交叉编译 exit 0。次生处置：新模块并入 `single_production_arbiter_in_crate` 源码审计清单（8→9 文件），防新面游离审计。
2. **候选 C · run_ipc_loop 效应器 seam（Worth exploring）——Rejected（前提证伪，登记不施工）**：C.1「17 处 host/target cfg 孪生」实为 17 处**整项门零对手臂**（`not(target_os="none")` 在本文件仅出现于四处 `allow(dead_code)`），所谓孪生对是 `sel4-eipc` **特性轴**且两臂皆裸机；真实孪生 7 对全部落在 `all(none, …)` 形态内（另有 1 处跨行 cfg 使单行 grep 漏计，全文相关属性实计 39）。照文实施的两个硬雷：`service.rs:2430-2494` `include_str!` 源码文本锁（`ea_wire::EA_OK` / `Sel4IpcClient::new` 计数绑死本文件）+ `run_ipc_loop` 本体 host 不可编译 ⇒ C.3 第 2 步不成立。证据表与裁决见蓝图 §11.1。**再次提出本候选须先答 §11.1 两雷**。
3. **候选 C 相邻 feature 矩阵疑点——登记不修**：`ota_probe_body`（824）门不含 `sel4-eipc` 而调用体 `ota_download_probe`（1816）含——代码读判成立，实弹复验（`--target aarch64-unknown-none --features qemu-test-synth`）证伪其独立性：不带 `sel4-eipc` 的 none 构建失败面 ≥10 处无关错误（`ipc_sel4` 缺席/`ChunkedSession::bind_shared_const` 缺失/`[u8]` 尺寸），即整组合不受支持。单点补门 = 把矩阵一个洞补成另一个洞（伪修复），真修复须整面 feature 矩阵建模（另卷议题）。详见蓝图 §11.2。
4. **候选 E · HandoverSet（Worth exploring）——Rejected（前提证伪，试点已退回）**：按 E.2 字面试点落出 157 行纯增量后判读退回。三条实证：① 寄存器装配**已是单点**（`initial_registers` spawn.rs:1939–1991，restart 路径 service.rs:1628 同源重建），E 只把同一事实抄第二遍；② E 的动机"X7 截断结构封死"**已封死**——`INITIAL_REG_COUNT=11` + 编译期 `const _: assert!(INITIAL_REG_COUNT > REG_X7)`（1932–1937，注释即 D 卷历史事故），错则构建即炸，强于方法级封装；③ 三臂**非和类型**：X3/X4/X5 是 `storage → net → 通用` 优先链，X7 由 storage 写 `boot_dma_vaddr` 后**再由 uart 链外独立覆盖**（1986–1989 自述"与三路分支正交"）——收为枚举后 `from_parts` 只能"取先登记一臂、不再叠加 X7"，而 `UART_HANDOVER_TABLE` 今日为空恰是**真机 bring-up 的 owner 填空项**，收枚举=把正交性从结构降为注释纪律（该组合今日不可达，属自由度损失而非线上已发生故障——强弱分级见 §11.4）；决定性理由实为①②：E 会在同文件造出第二套寄存器归属真相，正是 A/C 要消灭的多点记账形态，与 §A"注释→结构"的深化方向相反。证据表见蓝图 §11.4；`spawn.rs` 已退回至与 `60b73a8` 逐字节一致（试点增量存 `/tmp/de-agent-partial-E.patch` 备查）。
5. **候选 D · ServiceSpawnSession——施工（E 已排除，D 单独做）**：D.1 前提复核成立（`configure_service` spawn.rs:2038、14 参、8 元组、`cfg(target_os="none")` host 不可直测）。实施口径按蓝图 §11.5 收紧：14 参必须**命名字段结构体**装配（五相邻 `u64` 位置错配是首要失败模式），805 行函数体逐字搬运零删改，终验以交叉编译 + QEMU 启动门为准。
6. **候选 B（xtask FlagParser + SignedReport）/ F（并入 A）/ G（搁置）**：B 落地见 `git log` 中 `Deepening-v1 §B` 提交，实施期订正与历史验收结果见 `docs/architecture/deepening-blueprint-v1.md` §11.7–11.8（本册不存在所谓「下一节 R14 收口行」）；F 按 A.2 口径仅收钉住观测、九 getter 快照化不做；G 依 §10 永不做（A 后导航摩擦未复现）。2026-09-22 续开发中 B 的签章钥长度安全修复见蓝图 §11.9，独立于 2026-09-21 旧验收记录。

## R15. 2026-09-22 后续安全修复与验证边界

`docs/architecture/deepening-blueprint-v1.md` §11.9–11.10 为本轮唯一详细对账位置：
QA/产线/烧录签钥严格 32B、`rustls` 安全补丁版本锁定到 0.23.45，
以及启用 federation `tls` 才暴露的 `svc-federation` host 编译缺口均经
复现、修复和对应回归。新锁的 `cargo deny` 四项、全仓 host 测试、
TLS 实握手和联邦 TLS 测试、ARM64 交叉编译通过；**旧证据不自动代表新候选身份**。
QEMU R10-B 割切相位、真实硬件与重签候选证据仍开放，不将本轮 host PASS
推断为发布 GO。`run70-r10-held.sh` 有全局杀 QEMU 进程及覆写现有 evidence
的副作用，本轮保护带原有未提交证据的共享工作区，未直接调用。

## R16. R10-B 割切见证实跑结案 + 时敏 flake 登记（2026-09-22，执行会话）

1. **R10-B 首次真 PASS（推翻 R15 的"仍开放"）**：`tools/run70-r10-held.sh`
   （owner 重写为 witness 触发版）在终树实跑 **20 判据全绿、
   `=== R10-HELD-GATE: PASS ===`**，镜像 `eneros-qemu-test.raw`
   sha256 前缀 `4466db0f8dc1d8bd`。关键行：
   `[✓][B0] plan=83,d6,01,… applied=2 pending=6`（**同一计划内** Applied 前缀与
   Pending 后缀共存且该计划无 DONE 行）→ `[✓][B2] 同计划未发步 Blocked` →
   `[✓][B3] … applied=2 cancelled=6`（重启窗零 Executing、attempts 不漂移）；
   段A 10/10（含 `HELD-PLAN-DONE state=Committed applied=3`、终态守恒、逐计划
   写后读回绑定、反倾泻）、段C 4/4、三窗零 PANIC。详表见
   `docs/architecture/deepening-blueprint-v1.md` §11.14；旧 B0 假绿的机制分析见 §11.11。
   **对 Deepening-v1 §A 的意义**：钉住/释放协议收拢后，实弹同时满足"完成即释"
   与"在途割切 → 前缀保留 + 未发步取消"，`[svc-control-dispatch]` 词形零漂移。
   **身份状态**：PASS 实跑时树含未提交改动，其后该批已按主题分枚入库且
   **入库未改任何字节**（只移 commit 边界；docs/evidence 不进镜像），提交后
   全门复跑 9198 passed / fmt 净 / clippy `-D warnings` 净 ⇒ 本 PASS 可归属
   当前候选树；换码换依赖仍需复跑（R15 的"旧证据不代表新候选"纪律仍有效）。
2. **执行会话过程偏差（如实登记）**：R15 已判定该脚本"有全局杀 QEMU 与覆写
   evidence 的副作用，本轮不直接调用"，而本会话在其之前实跑了两次
   （2026-09-21 23:57 旧盲切版、2026-09-22 12:0x witness 版）。缓解：运行前
   `evidence/70/70.15/r10-held/` 整目录快照至 `/tmp/r10-held-preserved-baseline-0000`
   （原 mtime 09-21 21:42-21:48，可复原）；本会话未 `git add` 任何 `evidence/` 路径；
   事后 `git status configs/protocol-mqtt.toml` 干净（脚本 trap 已复原）；
   实跑时段宿主无他人 QEMU 在飞。**后续规则**：该门属独占资源，跑前先与
   owner 对齐、先快照 evidence、并把 `A_DL/B_DL/C_DL` 与 `B_CUT_WAIT_S` 取值记进门记录。
3. **REG-IEC104-LOOPBACK-FLAKE（新增 Open）**：锚点
   `crates/protocols/iec104-master/tests/loopback.rs:229`
   （`loopback_interrogation_telemetry`）——总召后遥测到达判据用
   `8 × sleep(5ms)` = 40ms 墙钟预算，**空机单跑 3 次挂 1 次**，全仓
   `--no-fail-fast` 并发套件中必现。本轮全门复跑唯一失败即此测；
   R14/R15/R16 三批均未触碰 iec104（`git diff --name-only` 零文件）⇒ 判为
   预存时敏 flake。**不私改预算**（放宽会把真实晚到投递掩盖成通过）；
   修法待裁决：改事件驱动等待，或以 `process_rx` 返回可用性为条件而非固定次数。

## R17. 蓝图 73 · 架构深化 v2 核验收口批A 登记（2026-09-22，v0.339.0）

> 来源：架构深化评审 v2（`/tmp/architecture-review-20260922-151840.html`，八候选
> C1–C8）经独立核验 8/8 成立（`/tmp/architecture-review-20260922-204318.html`）→
> 顺位前二立卷（批A = C2 目标代码重构承载升版；批B = C4 门判据登记化 host-only
> 不升版）。基线 `0e4fd05` / v0.338.0 / 全仓 9198。

1. **批A 完成（boot 安装区间具名化与 LBA 单位契约，v0.338.0 → v0.339.0）**：
   - `InstallRanges` 具名 struct 落 `drv-svc::boot_guard`（4 pub u64 字段 +
     `pack_le`/`unpack_le` 40B wire 打包/解包单源）；trait 方法/守卫两入口/
     3 产出点/6 解构点全部迁移——裸四元组退役，位置换位/漏位不再可表示
     （窄域签名禁令 `install_ranges_bare_tuple_signature_regression_lock` 锁死）。
   - C9 单位契约：`BlockBackend` doc 写死字节偏移（形参 `offset_bytes` 纯改名）；
     `InstallBackend` 增 `read_sectors`/`write_sectors` 具名单位方法（无默认体，
     BootSlotBackend/MockFace/InstallFace 三实现齐落；supertrait 保留体纯委托）；
     boot 面调用点（`handle_boot_cmd` 2 处 + `boot_install.rs` 9 处）全切换，
     数据面调用点零改动——同名异单位分裂（REG-70.7-BOOT-TRIAL-IO 前科面）终结。
   - svc-ota 手工解包（0x0A 显示 + 0x0F 位置消费 `guard[..8]`）收编为
     `unpack_le` 单源；拒绝分支零改动。
   - wire 文档纠偏：`BLK_BOOT_GUARD` 响应 = 32B 有效（4×u64 LE）+ 8B 保留
     恒零——旧表 33B 含 `active_is_a(1)` 声称失实（无写入者）。
   - 验证（G4 先红后绿）：T-golden（`blk_boot_guard_response_bytes_golden`
     40B 逐字节金锁，期望手工展开，重构前后原样绿 = C1 wire 冻结执行证据）/
     T-unit（`dual_plane_unit_contract_boot_sectors_vs_data_bytes` 同介质同数值
     两平面读不同字节）/ T-signature-lock（P0 红：老签名在案 → P1 绿）。
     drv-svc 47/47；全仓 **9201/0**（基线 9198 + 3）；fmt 净；clippy
     `-D warnings` 净；六门绿（feature-matrix violations=[] / audit 三 profile
     PASS / sign-keys / orphan-check / config-refs / mock-leak 零违规）；
     **目标侧全链 PASS**（config compile qemu-test → sel4 重链接 → image raw
     134217728B sha256=34ffffd791d80f56…）；无新 QEMU 行为窗（规格即验收面：
     串口/wire 逐字节不变，唯一 println 源码变化为内联捕获改位置参数、输出
     字节不变）。commit 序列（G5）：P0 测试垫底 `be5a002` → P1–P3 实现 →
     收口+登记随尾。
2. **Open：`active_is_a` wire 字节是否上线**——owner 决策另立卷（上线 = wire
   行为变化；现行尾部 8B 保留恒零、全仓无写入者，block_cmd.rs wire 表已订正
   为真话并标注登记未上线）。
3. **批B 完成（门判据登记化，host-only 不升版，2026-09-22 同会话）**：
   - `tools/gate-criteria.toml` 首批 5 条判据登记（VOL70.5-R9 any_of=
     PUSH:OK∣synthetic + none_of=FAILED——定案点②语义重述；R9b；
     R10-B4-DELIVERY；PANIC-SCAN 采纳 judge.py 最严口径——vol5 `-i` 与
     r10-held 第三词表退役【73 批C 订正：此句虚述——当时仅 vol5 窗B 与
     r10-held B5 两点落地，残余 11 个活体判据点仍裸 grep，且所谓"最严
     口径"本身漏检三个真实 panic 出口；见 REG-70.FL-RETIRED-DIALECT /
     REG-70.FL-PANIC-TOKEN-GAP，12 点已全收编 + 守卫锁死】；
     ACQ-MODBUS）+ `tools/criterion.py` 判据器
     （check/--count-only/--selftest；全 token/锚定边界；缺输入 check=✗/
     count-only=exit 3；回声逐字节同形）。
   - 四脚本收编：vol5-gate R9/R9b/PANICS_B + 门头自举 selftest；
     r10-held fc06 `FileNotFoundError→sys.exit(3)`（缺输入=红）+ B4 显式
     input-missing 红分支 + NDELIV 走判据器计数 + B5→PANIC-SCAN；run61
     ACQ 特例；vol4 R3/R6 ACQ 两点。**R9 裸子串假绿（归档实况：4 行无关
     词形满足判据、目标 token 0 次）与 B4 缺输入=0 假绿关闭**——
     `repro-current.txt`（3/3 ✓）与 `--post-fix`（3/3 ✗）并排为修复生效
     物证。
   - py↔rust `gate_terminal_status` 共享 fixture 锁（24 行，
     `tools/testdata/gate-terminal-fixtures.txt`；xtask 229/0 +
     selftest 11/11 双绿——C5：共享 fixture 数据，两侧实现原样）。
   - 归档复判**零翻红**（`docs/architecture/fix73-gate-recheck.md`：
     70.5 两窗 R9 计数 4→1=虚胖现真值结论一致；r10-held B4 `0≤0` 经
     语义注记为重启后未发步取消的真实形态非缺输入假绿；B5 一致）——
     定案点①不触发，登记未为保绿放宽。
   - commit 序列（G5/C8 分枚可回退）：P0 fixtures+复现 → P1/P2 登记+
     判据器 → P3 四脚本收编 → P4 fixture 锁 → P5/P6 复判报告+文档。
4. **登记不修项入册**（执行卡 §5 原文在案）：C1 Applied 第二结算出口（含 B4
   词形契约矛盾）；C6 绑定五形状 + 两处 `unwrap_or(0)`（**核验后第三顺位，
   73 收口后优先评估立卷**）；线索D ADR-0054 readback P1 功能缺口；线索A
   在途归属 5+ 处；线索B System1 两套时钟（半证伪 P3）；线索C usize 路由
   下标（P3，engine.rs:1976 裸 expect 唯一引线）；C3/C5/C7/C8 顺位其后。

## B74-1. 蓝图 74 · 修复卷一登记（2026-09-25，v0.340.0，结论面 fail-closed 收口）

R15-Q/U/F 三 Strong 清偿（第十五轮体检 Top 面）。红测先行 ×3 组、
变异咬合 ×3、target 构建 EXIT=0；提交 d8b7195d（Q）→ 7c515e83 +
3fb1a32f（U）→ decc5192（F）。测试计数：开工基线 9227/0 → 收工
9231/0（+4：validator mutex ×3 + spawn fail-closed ×1）。

| 条目 | 位置 | 级别 | 修复与证据 |
|---|---|---|---|
| REG-74-Q | `crates/ai/safety-validator/src/validator.rs` validate | **P1**（结论面 fail-open） | `passed` 在 mutex 后置违规入列**前**定型：仅有 charge_discharge_mutex 违规时 `passed = 非空 \|\| !has_hard_reject = true`——31-review P1 AI-05 的后置 Fatal 纵深防御形同虚设，且该词形全仓零测试。修：mutex 块整体上移至 hard_rules/has_hard_reject 计算之前（最小 diff 无新代码；顺序正确性：检查只读 current_schedule（规则链+clamp 终态）与违规列表无交互；规则 Fatal 早退不受影响——早退时本块尚未执行）。红测 ×3（2 红 + 反向锁）：`simultaneous_charge_discharge_is_rejected` / `revalidate_clamped_propagates_mutex_reject`（:161 只看 passed 的传导面）/ `legal_schedules_still_pass_after_mutex_gate`（纯充/纯放/零条目防过收紧）。变异咬合①：块回移原位 → 2 红，cp 当轮备份恢复 → 51/0 绿。**消费面枚举**：fast-path engine.rs:109 与 dual-brain coordinator.rs:534 由放行转 SafetyFatal 拒绝（合法收紧，行号亲验）；dispatch engine.rs:2104-2105（提交树坐标；1481-1483 系工作树含 R14 在途层坐标）**结构免疫**——`charge=p.min(0.0).abs()` 与 `discharge=p.max(0.0)` 对同一 p 至多一个非零 ⇒ mutex 不可达，行为零变化（证据留档：未来引擎侧若加同置条目，此面由本登记与 :161 红测共同兜底）；control-gateway safety.rs 同名异物不受影响 |
| REG-74-U | `crates/security/ota-ab/src/upgrade.rs:139` + `kernel_ota.rs:194,253` + `business-services/src/ota_service.rs:506` | **P2**（幻影布尔臂） | 生产实现 WP-64.3.5 起恒 Ok(true)（错误全走 Err），Ok(false) 唯一来源是测试桩——与 CONTEXT「SignerKeyring Ok(false) 陷阱不外泄」同族，interface 面收尾：verify 收窄 `Result<()>`，布尔臂数据性不可表示（变异咬合②：复现 Ok(false) 臂 → E0308 编译红）。kernel_ota.rs 两处同型 `if !verified` 死臂一并退役（失败经 `?` 按具体 OtaError 传播，可诊断性不降级）；ota_service.rs:505 幻影臂删除（`verify-failed:{err}` 映射词形逐字保留；tools/ 零依赖已复核）；TestOta `verified` 字段退役（10 构造点清边），`negative_verify_failure_rejected` 改走 FaultOta Err 路径——补上 verify_err 此前从未置 Some 的无活体覆盖面（`ERR:ota:verify-failed:sm3-hash-mismatch`，与 t39 候选词形同族）；**blueprint 枚举外漏网点** vpp_e2e.rs:316 由全量套件编译网捕获（E0600）→ 3fb1a32f 补遗（教训：grep 完整性 sweep 被 head 截断=假完备，全量编译才是真网）。rollback.rs:264 `load_from_device` 的 Ok(bool) 为恢复/回退独立语义，不在本面 |
| REG-74-F | `crates/runtime/root-task/src/spawn.rs` UntypedAllocator | **P2**（release fail-open 边角 + 纪律复制 5 处） | blocked 逐位检查逐字复制 5 处，等长不变式仅 debug_assert：release 下 blocked 短于 descs 时越界 `get(i)=None → unwrap_or(false)` = **静默不拉黑**，与 BOOT-007 全链 fail-closed 纪律相悖。修：`is_blocked` 私有谓词单点化（越界缺省 true——失配时宁可拒分配不可放行），4 活体位点收编（第 5 位点随 alloc_at 删除）；with_blocked debug_assert 保留并升级注释（debug 尽早暴露构造方 bug，release 由谓词兜底）。删孤儿 `alloc_at`（全仓零调用方：init bundle 已由内核 boot.c 直映射、root-task 1:1 直读，注释所述用途不存在），alloc_device_at doc 的 `[alloc_at]` 链接同步改写（rustdoc -D warnings 防断链）。红测 `blocked_length_mismatch_fails_closed` **双面设计**：release 面断言四分配口（alloc/alloc_device_at/alloc_contiguous_pages/alloc_cap_table）一律 None + 零记账（修复前全红=分配成功）；debug 面 catch_unwind 锁构造期断言拦截（测试 profile 强制 unwind）。变异咬合③：缺省翻回 false → release 红测红。**有意设计不动摇**：平行数组+索引占位仍是 BOOT-007 承重决策（cap 槽号 = cap_base+索引 对齐 seL4 语义），不改数据结构，既有 reserved_window 两测试全绿；target 构建 EXIT=0（spawn.rs 为 cfg(none) 主战场，host 绿≠target 可编译） |
| REG-74-Q-R1 | `crates/ai/fast-path/src/engine.rs:117-123` + `crates/ai/dual-brain/src/coordinator.rs:576-582` 回落原解路径 | **P3**（纵深缺口，登记不修） | 74-01 复查轮（三路对抗审计）发现：mutex 后置门只查 clamp **终态** current_schedule，原解不经该门——若原解含同时充放且规则链恰好把一侧钳到 ≤eps、再叠加钳后重验 4 轮不收敛，`revalidate_clamped().unwrap_or(原解)` 会携带在终态上认证的 `passed=true` 放行未过 mutex 门的原始解。**生产不可达（74 三卷复查轮订正论证，旧论证失实）**：生产规则集（fast-path `new()` 与 coordinator `empty()`+同两规则同构）全 hard——钳制必伴 violation（Critical+hard）⇒ `has_hard_reject` ⇒ `passed=false` 于 engine 闸门拒出 ⇒「passed=true 且带 clamp 建议」形态生产不可表示，回落分支不可达；触发需未来接入**软**钳规则（is_hard=false 或无 violation 钳制）且 4 轮不收敛。旧句「电气/爬坡钳制只降超限值不改号不造零」**被证伪**：爬坡钳 safe_value=prev±50 绝对目标+先消同向，可把一侧钳到恰 0（钳没可达——数值例 prev_net=80/charge=40/discharge=60 → charge 钳 0，论据锁测试 `reg74_q_r1_argument_lock_production_rules_all_hard` 断言②实证钉住），但钳没使终态更干净，不构成放行路径。不修理由：修=对原解补过 mutex 门，属 fast-path/coordinator 认证语义设计变更，超出卷一边界（蓝图 §4）。两处「原解已通过全部规则」注释已补注指向本条 |
| REG-74-Q-R2 | `crates/ai/safety-validator/src/validator.rs:117` mutex 门 NaN 盲区 | **P3**（既有，非本卷引入） | `NaN > SIMUL_EPS` 恒 false，mutex 门对 NaN 失明。默认生产接线（电气规则 WP-26.6 finite 前置 Fatal，validator 循环早退先于 mutex 块）下不可达；`SafetyValidator::empty()` 或自定义无 finite 检查规则集下 NaN 双充放可带 `passed=true` 通过。74-01 前即存在；修属 validator 输入卫生面，与结论面顺序修不同族，登记不修 |

## B74-2. 蓝图 74 · 修复卷二登记（2026-09-25，v0.341.0，启动链事实钉版与构建面收敛）

R15-B/A/D/E/G 五候选清偿 + 施工中新发现 R15-B 次生面（A-R1）。红测先行
（对拍门 6 断言，含复查轮补锁）、变异咬合 ×4、两构建路径实弹 + QEMU 双路冒烟、bootstrap
钉版哈希实弹下载校验。提交 1326ec3e（74.2.1）→ 53a5b73c（74.2.4）→
dd1ff380（74.2.2）→ 2a34e9ec（74.2.3）→ 3406529e（74.2.5）→
198c31ff（74.2.6）→ 48cd8f63（74.2.7）→ d0f62617（收口）→ 5aa0d44e
（复查轮补锁）→ karpathy 全局审计轮（10 提交）。测试计数：开工基线
9231/0 → 收工 9238/0（+7 = 对拍门七断言组）。

| 条目 | 位置 | 级别 | 修复与证据 |
|---|---|---|---|
| REG-74-B | `third_party/sel4-top/overlay-init-bundle-reserved.dts` + `third_party/sel4-patches/0001-init-bundle-mapping.patch` + `crates/boot/boot-protocol/src/boot_info.rs` + 两构建脚本 + `xtask/src/image_factory.rs` | **P1**（启动链最重契约四事实源互不锁） | 窗口三元组（INIT_BUNDLE_PHYS/MAPPED_WINDOW_LEN/RESERVED_WINDOW_LEN）+ 内核加载址共 5 个事实源散布 dts/补丁 C 字面+隐函数/Rust 常量/两构建脚本/xtask，零机器对拍（boot_info.rs:207 注释宣称 dts 为单一事实源但全仓无对拍）。修：新增 `boot-protocol/tests/window_contract_lock.rs` 五断言组互锁（dts reg↔常量 / 补丁字面↔常量 / 隐函数公式 n+=MAPPED/2MiB+1 / 内核加载址 corpus 式三源一致 / 74.2.1 手抄退役回归锁），期望串全由常量 format! 拼接（R14 守卫自咬教训：测试文本零裸字面）。变异咬合 ×4：dts reg 漂移→断言1红；n+=5→4→断言3红；常量 8→4 MiB→断言 2/3 红（dts 断言不红=reg 锁 RESERVED 非 MAPPED，蓝图预言面吻合）；IMAGE_START_ADDR 漂移→断言4红。蓝图两笔误订正：include_str 路径 3 级→4 级（tests/ 至仓根实际深度）；「2<<19」（=1 MiB）→ 2 MiB 粒度（按「8MiB→5；16MiB→9」意图）。蓝图「施工时取简单者，登记选择」：断言 4 选 include_str 文本解析（xtask KERNEL_LOAD_ADDRESS 行剥下划线 parse），未建常量模块。复查轮补锁断言 6：boot-manager-uefi KERNEL_LOAD_START ↔ xtask KERNEL_LOAD_ADDRESS 两 Rust 字面互锁（复查发现同事实字面零对拍面）；dts 头注残留「INIT_LOAD_ADDRESS 见 image_factory.rs」死链一并清（1326ec3e 已退役该常量） |
| REG-74-A | `third_party/sel4-build.sh` + `sel4-build-rust.sh` + 新增 `sel4-build-common.sh` | **P2**（~38 行逐字姊妹拷贝 + 已发现不对称缺陷） | 两构建脚本共享块（工具链 8 路径+3 exports/libcpio 4 行/converge 调用/18 公共 cmake 参数）收编进 common（被 source 非入口），wrapper 只持变体（13 vs 14 / BUILD 目录 / 拷产物 / rust 4 参+3 导出）；**不对称缺陷顺带修复**：依赖自检+自动 bootstrap 环原 build.sh 独有，rust 路径冷缓存此前在 cmake 步骤才裸错——迁入 common 双路径同受益（stub-bash 沙箱实测触发链）。等价性：两路径实弹重建 exit 0（C 40 步/rust 41 步）；top-rust CMakeCache 逐字节一致、top 仅 CMAKE_TOOLCHAIN_FILE 类型标记差异（命令行 -D 对旧缓存条目的既有 cmake 行为，值相同）；converge T1/T2/T5 沙箱咬合（篡改 boot.c 收敛回期望 sha256）。**KernelRootCNodeSizeBits 13/14 不统一**（裁决项见下） |
| REG-74-A-R1 | canonical 启动线 gic-version=3 全族（CI g0 job / `sel4-powercut.sh` / `sel4-build.sh` echo 线 / docs/sel4 三文档） | **P1**（预存启动断裂，本卷施工冒烟挖出） | seL4 14.0.0 qemu-arm-virt 平台唯一内置 GICv2 驱动（config.cmake 硬编码 `INTERRUPT_CONTROLLER gic_v2.h` + `add_sources gic_v2.c`），而全仓 canonical 启动命令用 `gic-version=3`——GICv3 机器无 GICC 帧，gic_v2 cpu_iface_init 首写 GICC_CTLR 即**同步外部 abort**（ESR 0x96000050=WnR+ext abort；FAR 0xffff_ffff_ffe0_2000 与反汇编 `str wzr,[x0,#4096]` 逐位吻合；qemu 9.2.4 与 10.2.1 同现）。三层对照排除本卷嫌疑：重构前后脚本冷构建同 abort（CMakeCache 等值）；CMakeCache 快照证明 KernelArmGicV3=OFF 系预存（任何脚本版本从未传过该旗标）；生产证据线 xtask qemu_harness **恒 gic-version=2**（三处）从无此病。**失效无人观测的窗口（karpathy 审计轮订正 + 深化）**：CI 全历史**零成功运行**（gh run --status success 空集）；8-31 末两次实控死于 setup 层——`dtolnay/rust-toolchain@nightly-2026-04-04` 标签在 dtolnay 仓库不存在（全仓零 dated tag，ls-remote 实证），工作流 6-11s 即死、构建步从未到达；其后提交未推送（本地领先 origin 182）。karpathy 轮已修：6 处调用点改钉 master SHA（02cb101e）+ toolchain 入参（ci.yml ×5 + nightly-fuzz.yml ×1——后者为审计新发现的第 6 处，系唯一仍调度的 workflow 连续 26 次日跑全死于同因）。修：CI g0/powercut/build.sh echo 三活面订正 gicv2（g0 冒烟实弹：kernel-init/bootinfo-valid/cap-create/ipc-loopback PASS + virtio 三 init/tx/storage 全 PASS）；docs/sel4 三文档为历史交付记录不改、以本条为勘误。**次生面登记不修**：C root task 失败路径空指针崩溃（net-rx FAIL/设备缺失 → vm fault at 0 转储，root-heartbeat/evidence-complete 不可达）——root_task.c 健壮性缺口，属 g0 证据链复活工程（连 CI 恢复），超出本卷 |
| REG-74-A-SIZEBITS | `KernelRootCNodeSizeBits` 13（C 路径）vs 14（rust 路径） | **P3**（裁决项，登记不修） | build.sh C 路径停 13（g0 证据介质，`eneros.sel4.v1` 5 项证据树绑定 13 位根 CNode 内核），rust 路径 70.15 实弹升 14（槽位耗尽，spawn.rs ROOT_CNODE_SLOTS=1<<14 只对齐 rust 侧）。统一到 14 会改 g0 内核二进制 → 需 g0 证据重跑 + C 路径槽位面重验。本卷参数化显式化（common 化后两 wrapper 各持显式值 + 注释互指），统一与否留 owner 裁决 |
| REG-74-E | `third_party/sel4-bootstrap.sh` + `sel4-powercut.sh` + `docs/security/supply-chain.md` | **P2**（QEMU 三源三版本，钉版清单缺项） | 两构建脚本 `-DQEMU_BINARY` 指 xpack 9.2.4-1 但 bootstrap 清单不含 qemu（冷缓存死路径）；powercut.sh 默认 8.2.6-1（同死路径）；CI/xtask 用系统 qemu（第三面）。修：bootstrap 第 7 项钉版 qemu-arm 9.2.4-1——哈希取官方 .sha 边车（203dc2c7…）+ 与缓存解压树二进制逐字节比对，**实弹端到端**（重跑现场下载 35.75 MB → fail-closed 校验 OK）；powercut 默认收链 9.2.4-1 + gicv2（REG-74-A-R1）；supply-chain.md 增补钉版分面声明（构建面=钉版 / CI 开发机系统 qemu=登记例外）。**蓝图事实订正**：「r03 断电门证据介质 8.2.6→9.2.4 须重跑重录」不适用——r03 实走 xtask qemu-prod UEFI 链 + harness gicv2，从不调用 sel4-powercut.sh（全仓零调用方），无介质变更。复查轮观察项：QEMU 版本串现存三处字面（bootstrap QEMU_URL / common QEMU_BIN / powercut 默认值）=约定同步非单源（升版须三处齐改）；powercut 不读 SEL4_CACHE 用裸 $HOME/.cache；与 bootstrap fetch 侧目录名单源化同为登记观察项，本卷不动 |
| REG-74-G | `third_party/sel4-top/CMakeLists.txt:31-37` + `third_party/sel4-root/root_task.c.bak` | **P3**（双轨定位颠倒 + 未跟踪残留） | 注释称 Rust ELF「不替代 C root task 的产品地位」与 ADR-0023 §4（Rust 载明主线）及 CI 现实（gkernel 全链依赖 Rust root-task）相反；「eneros-hello」陈旧名。修：注释订正如实图景（Rust=产品主线 / C=G2/G3 证据位=g0 job 5 项，**与「删 C 路径」误读划界**：root_task.c 不删）；删 root_task.c.bak（未跟踪旧备份 63KB，本体在库） |

### B74-2 验证账

- 对拍门 5/5 绿 + 变异 ×4 咬合（当轮 cp 备份恢复，禁 git checkout 纪律遵守）。
- 两构建路径全日志构建 exit 0；CMakeCache 等价性 diff（rust 逐字节/top 值级）。
- bootstrap 冷缓存自检触发链（stub bash）+ 真下载哈希校验双实弹。
- QEMU 冒烟：C 路径 g0 词形（gicv2）+ rust 路径 root-task 横幅/BOOT-007 probe。
- `bash -n` ×6（common/两 wrapper/bootstrap/powercut/commit 前）；YAML
  safe_load 过；fmt --check 过；clippy -D warnings 过（收尾全量复跑）。
- **CI push 三 job 绿**：登记为 CI 面实弹验证项（本地不可全验；主 CI 自
  2026-08-31 未运行，SEL4_CACHE_KEY bump 后首跑即冷重建——恢复 CI 即自动验证）。

### B74-2-R. karpathy 全局审计轮（2026-09-25，v0.341.0 复查）

三路对抗子代理（shell 面/Rust 测试面/CI 钉版面，全新视角）+ 主审亲验；
每项真 bug 均多维验证（≥2 独立复现）后才修；actionlint/shellcheck 双
工具首次引入。测试计数 9237→9238（对拍门第 7 断言）。

| 条目 | 位置 | 级别 | 修复与证据 |
|---|---|---|---|
| REG-74-A-R2 | `third_party/sel4-build.sh` + `sel4-build-rust.sh` git 模式位 | **P1**（预存：644 与 CI 直接执行矛盾） | 两脚本全部历史提交均 100644（×4 历史点核验），ci.yml 112/376/595/596 直接执行（644 execve=Permission denied，本地 644 副本实弹复现「权限不够」）。CI 零成功运行故从未暴露。修：`git update-index --chmod=+x`（内容零变化）。bootstrap/powercut 恒经 bash 调用保持 644（外科边界） |
| REG-74-A-R3 | ci.yml 三处 actions/cache 键 | **P1**（预存：变体假命中） | 三 job 同键共享，C 路径与 gkernel rust 链向 target/sel4 写**同名** elfloader（sizebits13 vs RUST_ROOT_TASK=ON/14），「存在即跳过」门下 actions/cache 同键先存者独占 → 间歇性变体假命中（g0 拿到 rust elfloader→root image 缺失硬败）。修：键按产物变体隔离（g0/image job=`-c` 共享、gkernel=`-rust`）；actionlint+YAML 双验 |
| REG-74-A-R4 | ci.yml ×5 + nightly-fuzz.yml ×1 `dtolnay/rust-toolchain@nightly-2026-04-04` | **P0**（预存：CI 门禁性死因） | 标签不存在（dtolnay 仓库零 dated tag；gh run 33351052897 实证 8-31 setup 层死；CI 全史零成功）。修：6 处改钉 master SHA 02cb101e + `toolchain: nightly-2026-04-04` 入参（与 rust-toolchain.toml 同源钉版；moving-branch 反模式规避，同 bootstrap 钉 commit 纪律）。CI 实弹绿仍属恢复运行后的验证项 |
| REG-74-E-R1 | `sel4-powercut.sh` run_wait_marker kill 语义 | **P0**（预存：断电矩阵语义失效；74.2.6 gicv2 修复后首次变为可触发） | `timeout N qemu & pid=$!` 的 `$!` 是 timeout 包装进程——`kill -9 $pid` 只杀包装、QEMU 孤儿化跑过 ~3s kill 窗口（root_task.c:874-879 busy-wait）自行 COMMIT：rollback 路径永不可达（RUN2 判据必败）+ 孤儿与下一 Run 并发同盘 O_DIRECT。子代理实锤 `timeout 30 sleep 30 & kill $!` → 子进程存活。修：kill_qemu_tree（`pkill -9 -P` 先杀 QEMU 子进程、再杀包装）。**端到端实弹**：断电矩阵三 Run 全过——RUN1 prepare-powercut OK → RUN2 `tx-rollback-oldvalue PASS`（旧 kill 语义下结构性不可达）+ COMMIT 断电 → RUN3 newvalue+recover PASS → POWERCUT-MATRIX: ALL PASS；ps 清点零孤儿 |
| REG-74-B-R1 | `window_contract_lock.rs` 断言 3/4/5 收紧 + 新增断言 7 | **P2**（假绿洞与解析脆弱面） | 断言 5 原 `contains("ENEROS_INIT_BUNDLE_PHYS")` 被 use 行满足=假绿（import 在场+赋值硬编码可绕过）→ 收紧锁消费点 `load_address: ENEROS_INIT_BUNDLE_PHYS`；断言 4 收紧只认 `-DIMAGE_START_ADDR=` 旗标形态（注释提法不再满足）；断言 3 锚定 vspace.c hunk（`---`/`+++` 各含锚点，取 last 段——首版 nth(1) 抓中空段被测试网当场咬回=收紧逻辑自身经红测校准）；断言 7 新增：xtask 跨度手抄 `KERNEL_LOAD_ADDRESS + 8*1024*1024` ↔ boot-manager KERNEL_LOAD_END−START 互锁。变异咬合 ×4 全红（import 硬编码/旗标漂移/END 漂移/n+=4）恢复 7/7 绿 |
| REG-74-C-R1 | `sel4-build-common.sh` sel4_run_converge | **P1**（本卷引入：自定义 CACHE 收敛错树） | common 支持预置 CACHE 传入且 bootstrap 调用显式传 `SEL4_CACHE="$CACHE"`，converge 调用漏传——`CACHE=/custom` 时构建消费 /custom 而 converge 收敛 ~/.cache（两树皆有完整 tarball 时 exit 0 = 静默未收敛）。修：`SEL4_CACHE="$CACHE" bash sel4-converge-patches.sh`；stub 实测 SEL4_CACHE 传导正确 |
| REG-74-G-R1 | boot_info.rs:20-23 + build.sh:3 + supply-chain.md「单点一致」 | **P2**（注释腐化漏网三面） | ENEROS_INIT_BUNDLE_MAX_LEN doc 残留「2 MiB」×2（同文件 74.2.3 漏订第五/六面）→ 订正 8 MiB；build.sh 头注「GICv3」→ GICv2（REG-74-A-R1 同族）；supply-chain「单点一致」→「约定同步」（版本同串、路径管道各自解析，升版须三处齐改=观察项）；common.sh `QEMU_GIC_VERSION=3` 补惰性变量注记（seL4 14.0.0 平台零消费者，全树 grep 实证） |

**登记不修（karpathy 轮，外科边界/设计裁决）**：
- bootstrap 下载/解压非原子（中断残留半包/半树重跑见路径即跳，无法自愈）——pre-existing；蓝图 §4「不动 bootstrap fetch/sha256 结构」边界内，登记为韧性缺口；
- build-rust.sh 冷机首跑死锁：cargo 前导出 `SEL4_BUILD_DIR=top-rust` 而 libsel4.a 由本脚本 step 2 才产出，build.rs fail-closed panic（错误信息指引的 sel4-build.sh 产出 top/ 不满足前置）——pre-existing，正确修法需设计裁决（cargo 前预建 top-rust 内核库 vs sel4-sys 回退语义），超最小面；
- bootstrap `--verify-only` 文档与实现不符（无 $1 分支）——pre-existing 文档债；
- 0001 补丁 boot.c hunk 注释仍引「xtask INIT_LOAD_ADDRESS」死链——补丁字节变更=内核 sha256 终态失效=g0/gkernel 证据链重录，成本远超死注释收益；
- 0001 补丁 vspace.c 注释「8 MiB = 2048 个 4 KiB 页 = 4 PT + 1 PD」的「4KiB 页」措辞与 2 MiB PT 粒度混用——同上不动补丁；
- g0 job 经济性：rx 硬门 + root_task 崩溃面未修（g0 证据链复活工程），CI 复活后 g0 首轮预期诚实红——建议启动证据独立成 job，属 CI 结构设计项。

## B74-3. 蓝图 74 · 修复卷三登记（2026-09-25，v0.342.0，安装器验签唯一入口与 JSONL 线码单源）

R15-C/L/M 三 Strong 候选清偿；4 提交（c64c3747 → 29368496 → a33e4684 → e12d235c）+ 收口提交。
测试计数（单面口径）workspace 9238→9266（+28：installer lib 0→20、jsonl-wire 叶 0→7、logging 换算锁 +1）；
`cargo test --workspace --locked` 9266/0 全绿；std 门控面另 +1（fields 收尾回归 fixture——
logging `--features std` 31 / metrics 23，ci.yml:83-84 同款命令）；boot 族搬迁面计数逐字节不变（boot-manager 8 / boot-protocol 47+4+7）；
两 UEFI bin 构建实弹复验（aarch64-unknown-uefi release `--locked`，efi 产物刷新）。

| 条目 | 位置 | 级别 | 修复与证据 |
|---|---|---|---|
| REG-74-C | `crates/boot/boot-protocol/src/signature.rs` + `boot-manager-uefi/src/lib.rs` + `installer-uefi/src/{lib,main}.rs`（R15-C） | **P1**（宣称唯一入口 vs installer 裸验 + 503 行单文件零测试） | `Ed25519SignatureProvider` 整体迁入 boot-protocol（trait 旁，byte-identical，boot 族测试计数不变=搬迁等价证据）；installer 验签重写为 `verify_install_manifest(encoded, anchor)`：前置检查原序（Platform/KeyId/Rollback 词形不变）→ `check_signature_domain`（算法/长度/域/版本下限/**dev-key 拒绝**）→ provider 密码学验证，错误一律映射 `Signature`（`eneros.install-rejection.v1` 十五词形冻结锁测试）。**红测实锤新拒绝面**：dev-key（key_id 全 0xDD）现签 manifest 修复前经裸 verify_strict 直接放行——16→32 位身份提升以 0xDD 填充保持 dev-key 谓词（x 全 0xDD ⇔ x‖0xDD 全 0xDD），`allow_dev_key` 变异咬合（翻 true → dev-key 红测 FAILED，恢复绿）。boot-manager 残留的 provider 外裸 `VerifyingKey::from_bytes` 直验死码（结果被 `let _` 丢弃）随之退役——grep 判据：boot 族唯一 `VerifyingKey::from_bytes` = provider 本体。**行为映射漂移登记**：该死码的 `?` 曾使非法策略公钥在验签前以 `LoaderError::PublicKey` 提前拒（现改由 provider 侧 `InvalidKey`→`LoaderError::Signature` 承接）；仅信任常量非法时可达（build.rs 十六进制解码门在先，生产不可达），拒绝词形同属失败面。installer 直验**结构性不可表示**：ed25519-dalek 降为 dev-dependency（生产 bin/lib 零直接依赖）。domain payload_digest 语义登记：installer 被签消息 = manifest 被签区自身（`INSTALL_MANIFEST_SIGNED_SIZE`=160B），provider 摘要一致性对 installer 平凡真；实质增益 = dev-key 面 + 算法/长度/域统一门 + 单点密码学路径 + 唯一入口宣称成真（蓝图 §1 诚实评估兑现）。installer host 可测 lib 诞生：ISO9660 解析泛型化下沉（`IsoMedia` trait 整扇区 seam + `parse_iso_record`/`find_iso_entry`/`iso_read_exact`/`find_iso_payload`），main.rs 留 `BlockIoMedia` adapter + 写盘/回读/空白判定（不 gold-plate）；测试 0→20（manifest 8 + ISO 12，合成字节无外部文件）；CI 测试清单 + `-p eneros-installer-uefi`。变异咬合：name 偏移 33→34 → ISO 4 测红；flags 位 25→24 → 正例定位红。次生发现（登记不修，见下）：boot-manager 同位零填充使其域门 dev-key 对 manifest 来源 key_id 结构不可达 |
| REG-74-L | 新 `crates/observability/jsonl-wire` + `logging/src/{logging,file_store}.rs` + `metrics/src/file_store.rs` + `tracing/src/span.rs`（R15-L） | **P2**（双胞胎逐行同构 ×2 + hstr 四抄） | 叶 crate（no_std+alloc+heapless 最小依赖，protocol-keys 先例）：`escape_json_into`（写侧唯一）+ `Scan` 骨架（skip_ws/peek/bump/expect/read_string/read_number_text/skip_value 读侧唯一；skip_value 取双胞胎超集=对象+数组配平）+ `hstr` 截断 + `JsonlError::Syntax` + `HString` 别名。logging/metrics 双 Scan 收编（领域数值与形状留各 crate：logging read_number→u64/read_fields 表、metrics read_u64/f64/labels），错误经 `From<JsonlError>` 映射且变体名冻结（`SerializeFailed`/`InvalidValue` 冻结于既有测试）；logging.rs escape+hstr 与 tracing span.rs hstr（第 4 处）同批收编。**历史 bug 回归 fixture**：fields 收尾 `}` 误判丢 trace_id/span_id/agent_id（按 file_store 收尾注释现场复原——非空 fields 且后随非零三元组条目逐字段往返保全；修复前既有 round-trip 测不含此形态=回归网假完备，本测补位）。**变异暴露次生缺口**：metrics round-trip 从无转义素材（引号/反斜杠/控制字符）——补齐后叶转义表漏 `\` → 叶3+logging3+metrics2 红、漏 `\u00` → 叶4+logging2+metrics2 红（变异矩阵三面全咬）。语义订正：logging read_number 由仅数字消费改经 read_number_text 解析（合法 wire 零变化；畸形尾随文本从部分消费+后续失配统一为解析失败）。异族登记不动：security/recovery `hstr_truncate`、pentest `HString` |
| REG-74-M | `logging/src/logging.rs` + `config-bundle/src/schema.rs:94`（R15-M Phase 1） | **P2**（同一数字三义 + min_level 死旋钮） | 唯一换算点 `LogLevel::from_config_level(u8) -> Option<LogLevel>`：0=ERROR→Error/1→Warn/2→Info/3→Debug/**4=TRACE→Debug（非双射 clamp——config TRACE 档在 logging 枚举无对位，取最详档）**/其他→None（fail-closed）；冻结测试含非双射断言。schema.rs `LEVEL_*` 注释互指唯一换算点、禁止手写比较（wire 编码零改动）。**死旋钮四事实核清（登记，代码不动）**：① agent-sdk logger 零外部消费方；② eneros-logging 生产 src 零消费（仅 tracing/vpp-scenario 测试面 use）；③ svc-observability/observability.rs 零 min_level·LoggingConsumer 引用（config_service.rs:227 解码后无人读）；④ 声明未用依赖两处 = business-services Cargo.toml:37 + vpp-scenario:21（均 [dependencies] 面向 src 零 use）。**Phase 2 裁决门 REG-74-M-APPLY**：svc-observability 接 apply 链 / FIELD_MIN_LEVEL 休眠登记 / schema 字段退役（wire 变更）三选一，前置 = R15-K observability 货架裁决——owner 未裁决前禁止接线/删字段 |

### B74-3 登记不修（卷三边界）
- boot-manager `verify_manifest` 的 16→32 key_id 零填充使其域门 dev-key 对 manifest 来源 key_id 永不可达（与 installer 0xDD 填充语义不一致）——改填充属 boot-manager 域门行为变更，超出卷三边界（R15-C 只收 installer 接线）；TrustPolicy 上游 key_id 门在先，生产暴露面为零。未来统一须与 REG-74-M-APPLY 同批裁决。
- `write_payload`/`hash_target`/`select_blank_target` 不迁 lib（UEFI IO 管道；blank 判定已由 image-format surface 深模块承载）——蓝图 §2 WP-74.3.2.1 显式边界。
- installer 词形面（十五 code）已冻结锁测试，但不新增 QEMU/串口门（evidence/ 零依赖 installer 词形，为未来 UEFI 窗保底）。
- durable-log 三连抄（R15-O）不在本卷——并入 ADR-0046 重建工单；R15-H/I/J/K/N/R/S/V 零消费货架族待 owner 裁决另立卷。

### B74-3-R. karpathy 全局审计轮（2026-09-25，v0.342.0 复查，零功能 bug）

三路对抗子代理（boot/ISO/crypto 面迁移等价性逐行 diff + 双 efi 构建实弹 + dev-key
反例构造；observability 面骨架与 35d43785 基线逐字节 diff + fuzz 10 项含 10 万层
嵌套/UTF-8 断边界/孤立代理 + panic 面 + 变异复咬；claims-vs-reality 十项对账），
全部结论以实际运行输出为证据。**功能 bug 零**；五项观察登记：

- **冻结闭包过期（预存，登记不修）**：`product/closure/` 冻结图 `workspace_package_count=132`/305 包（生成于 b3baef5b 2026-08-09，末次入库 eee525b5 蓝图60），HEAD~5 时 workspace 已 169 成员（现 170/365 包）——CI「Verify frozen Cargo graph and binary BOM」门（cargo-closure.ps1）在 74-03 之前即已翻红（v0.315 时代起 37+ 成员漂移）；本卷 jsonl-wire +1 成员（169→170）对已红灯门零边际影响。重生成 = 交付波工件全量刷新（需 pwsh + owner 裁决），非本卷面。
- **LoaderError::PublicKey 死变体保留**：零构造点（provider 侧 InvalidKey 承接，映射漂移已在 REG-74-C 登记）——变体属 boot-manager 拒绝词形词汇表，删除=词形面变更超本卷边界，保留并登记。
- **installer 词形冻结测为弱冻结边界**：冻结测迭代 15 已知词形，新增变体不被测试抓——但 `code()` match 无通配臂 = 编译期穷尽，新增变体必须改生产 match（结构性收口在编译层成立）；无反射下测试面冻结到此为止。
- **jsonl-wire skip_value 错位闭合宽松**（`[{]` 返回 Ok 残留垃圾）：与基线 metrics 逐行同构、非本卷引入；仅可达于未知键位（合法 to_json 零未知键）——登记不修。
- **管道实时计数伪值澄清**：`cargo test` 并行输出交错可使 grep/awk 误解析（本轮两代理独立复现同象，会话早前「暂态 FAILED=1」同源）——落盘全日志为权威口径（9266/0 共 4 趟；`error[` 零命中）。

claims 对账（第三路逐条实跑）：REG-74-C/L/M 与登记册 B74-3 全部数字/判据属实——
增量 20/7/1、boot 族 8/47/4/7、grep 唯一性、ed25519-dalek dev-dep 面与传递边
（经 boot-protocol 唯一入口，3 处为设计必然）、ci.yml 解析与清单、UEFI 构建
可复现、from_config_level 语义方向（0=ERROR→Error=最安静档）、CONTEXT 三词条
路径、死旋钮四事实、声明未用依赖恰两处。

### B74-FR. 蓝图 74 三卷全局复查轮（2026-09-25，HEAD b7271301 之上，零现行功能 bug）

用户点名的 karpathy 全局复查：三路对抗子代理（卷一 Q/U/F 维、卷二 boot/构建/CI/powercut
维、卷三 crypto/ISO/JSONL/词表维）+ 基线 `cargo test --workspace` 9266/0 复跑（与
74-03 收口声称精确吻合）。**现行功能 bug 零**；修复 10 项（注释/守卫/加固类）+ 登记
不修 5 项，全部经多维验证（代码逐行 + 实弹测试 + 变异咬合）后实施：

| # | 位置 | 级别 | 内容与证据 |
|---|---|---|---|
| FR-1 | `third_party/sel4-root/root_task.c:11` | P3 注释（确定漏网） | gic-version=3 启动命令头注——全仓唯一漏订正面（同病同修先例 build.sh:38 已在 35d43785 落实），`-kernel` 指向的恰是本卷镜像，照抄即复现 REG-74-A-R1 外部 abort。订正 gicv2 + 注记。验证：gic-version 全出现点 grep（ci.yml/build.sh/powercut/qemu_harness×3 均 gicv2，唯此漏网） |
| FR-2 | `third_party/sel4-build-common.sh:92` | P3 注释（主张失实） | 「QEMU_GIC_VERSION 复查轮全树 grep 实证零消费者」被证伪：seL4_tools `simulation.cmake:157-161`（`GenerateSimulateScript`）按 `=3` 拼 `,gic-version=3` 启动线——本构建形态零调用方=死代码，运行面惰性成立非现行 bug，但旧措辞是 REG-74-A-R1 病灶复活路径的误导源（未来 include(simulation) 即触）。订正措辞。验证：cache 树实读 + sel4-top CMakeLists include 面 grep |
| FR-3 | `boot-protocol/tests/window_contract_lock.rs` | P2 守卫假绿洞 | 四 contains/filter 断言不免疫注释形态（`// reg = <…>`、`# -DIMAGE_START_ADDR=…` 注释留旧串 = 门假绿，dts 调试 no-map 时常见操作）。修：`strip_comments`（CLike `//`+`/*…*/` / Shell `#`）前置四断言 + 剥除器自测（GHOST 占位防裸字面互数）。**变异咬合 ×4 全咬**：dts reg 行/patch 字面行/构建旗标行/消费点行注释化 → 各自红（旧代码全绿=洞实证）；未变异 corpus 8/8 绿=无假红引入。已知近似登记：字符串字面内注释 token 误剥（当前 corpus 无此形态，fail-closed 方向） |
| FR-4 | `window_contract_lock.rs` `patch_n_plus_value` | P3 守卫脆弱（假红方向） | find-first `n += ` 不分 +/- 行：补丁将来 `-  n += 4;`+`+  n += 5;` 演化会抓删除行恒红（门失效为常红掩盖真回归）。收紧：只认 `+` 前缀新增行。当前 corpus 单一字面，行为不变 |
| FR-5 | `docs/candidates-registry.md` REG-74-Q-R1 + `fast-path/engine.rs:117` 注释 | P3 论据证伪 | 不可达论证「电气/爬坡钳制只降超限值不改号不造零」被证伪——protection.rs 爬坡钳（safe_value=prev±50 绝对目标+先消同向）可把一侧钳到恰 0，触发条件「把一侧钳到 ≤eps」与论证「不造零」自相矛盾。**真屏障 = 生产规则集全 hard**（fast-path `new()` 与 coordinator `empty()`+同两规则同构）：钳制必伴 hard Critical ⇒ passed=false 于闸门拒出 ⇒「passed=true 且带建议」不可表示 ⇒ 回落分支生产不可达。结论（P3 不修）不变、论据重锚 + 论据锁测试 `reg74_q_r1_argument_lock_production_rules_all_hard`（断言②=钳没实证：prev_net=80/charge=40/discharge=60 → charge 钳恰 0.0；未来接入软钳规则即红） |
| FR-6 | `boot-protocol/src/signature.rs` `Ed25519SignatureProvider::verify` | P3 加固 | provider 不自证算法域——「调用方先跑 check_signature_domain」仅 doc 契约，pub API 直调时 Sm2 域配 Ed25519 签名 bytes 可静默通过。修：verify 首行算法自证（`AlgorithmMismatch`，域检查先于消息检查与 check_signature_domain 顺序对齐）+ 测试。两生产调用方均域门在先，行为零变化 |
| FR-7 | `boot-manager-uefi/Cargo.toml` | P3 死重 | ed25519-dalek 在 [dependencies] 而生产 src 零直接使用（唯一使用 = lib.rs:291 `#[cfg(test)]` 测试桩现签）——与 installer 同款降 dev-dep（REG-74-C 遗漏面）。验证：host 测试 8/8 绿 + aarch64-unknown-uefi release `cargo check` 两 bin 过（CI 三 env 键注入） |
| FR-8 | `.github/workflows/ci.yml:114` | P3 注释失准 | 「g0/image job = C 路径共享 -c」——C 变体实为 g0-fail-closed 与 g1-fuzz-gates 两 job（"image" 来自 g1 内 step 名）。订正。验证：job 边界行号实读 |
| FR-9 | `installer-uefi/src/lib.rs:581` | P3 注释错位 | 测试夹具 `record[28] = 1; // file version`——ISO9660 偏移 28-31 是 volume sequence number（file version 是名称 `;1` 后缀）；解析器不读该字节，fixture 无实际影响。订正注释 |
| FR-10 | `jsonl-wire/src/lib.rs:17` 异族清单 + REG-74-L | P3 登记失实 | 「security/recovery 两处 hstr_truncate」失实：实为**三处**（diag.rs/lib.rs 两处 + factory_reset.rs `truncate_heapless` 换名同语义，名字 grep 天然漏网）；且 **vpp-scenario exit_report 两处**（`hstr`/`write_bounded`）整族漏登。doc + 登记册双面订正（清单逻辑：异族逐处登记不动的承诺须按语义 grep `push(ch).is_err()` 而非按名） |

**登记不修（本轮裁决）**：
- **R15-F release 行为的 CI 覆盖缺口（观察）**：`blocked_length_mismatch_fails_closed` 双面设计的 release 断言面（is_blocked 越界缺省 true ⇒ 四分配口 None）只在 `cargo test --release` 可达，CI 全部面只跑 debug——缺省翻回 false 时 CI 全绿（提交信息的变异咬合是一次性手动演示）。本地 `cargo test --release -p eneros-root-task blocked` 2/2 绿（测试本身正确，未接线）。修=CI 恢复运行时在测试 job 补一行 release 面 root-task 测试，与 B74-2「CI push 实弹验证项」同批。
- **dashboard ws.rs `esc` 第二转义方言（H1）**：`\n\r\t` 短形 vs 叶 `\u00xx`——同族（observability）双胞胎不同步的活标本，但 dashboard 为零依赖 no_std 叶（收编需引入 heapless 闭包）且 WS 帧不经 JSONL 线路/Scan 读回；jsonl-wire doc 措辞已把「写侧唯一实现」范围订正到 JSONL 线码面，dashboard 异族登记。未来 dashboard 若需 Scan 兼容输出再收编。
- **jsonl-wire `HString` 别名导出零消费方**：logging/metrics/span 均保留本地 `use heapless::String as HString`；蓝图 WP-74.3.3 spec 载明形态，保留。
- **logging read_number 语义订正只登记了单向**：实际双向增量——`+123` 由拒变收（`read_number_text`+`parse::<u64>` 接受前导 `+`）、`"12 "`（数字后空白）由收变拒；to_json 产物无 `+`/空白 ⇒ 「合法 wire 零变化」成立，REG-74-L 语义订正记录补全。
- **tools/run70-r03-powercut.sh:20 用法注释 `CUT_WAIT=300` 死参数**（脚本消费 `CUT1_DELAY`/`CUT2_DELAY`）——70 卷遗留非 74 面，且 tools/ 有批C 在途层不动。

测试计数：9266 → **9269**（+3：validator 论据锁 1、window_contract 剥除器自测 1、provider 算法自证 1）。

## B75-1. 蓝图 75 · 修复卷一登记（2026-09-25，v0.343.0，结算单源与持有尸骸收口）

> 来源：第十六轮架构体检（2026-09-25）候选 R16-A/B/G/N/O/P/Q/Z（Y 登记不修）；蓝图 `蓝图/75-架构体检R16登记项清偿蓝图/01-修复卷一-control-dispatch结算单源与持有尸骸收口.md`。基线：R0 收编工作树在途层（R13/R14 C1 拆分层，84 文件，host 9269/0 实证）后施工；每 WP 一提交。

| ID | 面向 | 状态/描述 | 源 |
|---|---|---|---|
| REG-75-A | `crates/protocols/control-dispatch/src/plan_hold.rs`（删除）+ `lib.rs` + `held_engine.rs` doc | **Done（v0.343.0 WP-75.1.0，提交 313d8948）**：ADR-0054 取代后的 1179 行死模块永久删除（消费面全枚举零命中；BatchSink 假想 seam 唯一实现者=自身测试；经孤儿门同判据——删文件+剥 mod 后仍编译）。「持有」概念回到 held 单族；held_engine 模块 doc 补取代说明。**REG-70.11-HOLD-UNWIRED 销项**（Closed/superseded-by-ADR-0054）：telemetry 删除界未来复用「持有半边」读点改道 `HeldInputRetainer`（生产已接），不预埋。计数 9269→9257（−12 精确）。**观察**：`tools/gate-exemptions.toml` 的 plan_hold 豁免条目现指向已删文件（孤儿门按存在文件匹配，无害；按卷册 §0.7 不动 tools/，留待 owner 清理） | 蓝图 75 卷一 §2 |
| REG-75-B | `settle.rs`（RecoveredApplied 臂）+ `engine.rs`（应用分支）+ `held_engine.rs`（收缩）+ `tests/typed_chain.rs` | **Done（v0.343.0 WP-75.1.1，提交 03c6a5c7）**：恢复对账终局写序收表——`SettleTrigger::RecoveredApplied` 表行（durable=[Intent(Committed), Journal(Reconciling→AppliedConfirmed)]，写序同 Applied 两相律；audit=None、receipt 无摘要）+ settle_command 应用分支（按表行 durable 序迭代驱动、失败传播、错误词形 recovery persist/journal 逐字保留）+ settle_saga_applied 收缩为「身份预检+归一化」（条件转移→Reconciling 保留——表行表达不了）。**有意差异显式化**：恢复不重复记账（无 gate.record/committed_count/states.push；无回执摘要——丢失的 ACK 保持丢失）。SettleDispatch 提 pub(crate)。红→绿：守卫施工前实红（AppliedConfirmed×2/直写×4）；journal_crash_matrix 全矩阵零改动通过（等价性判据）；串口零变化。**变异咬合×3**：①表行倒序→表级红（成功路径终态等价，红落表锁——卷册「差异锁/矩阵必红」预测的落点订正：行为面对成功路径序不敏感，写序纪律由表级单测执法）②偷加记账→差异锁红 ③回填直写→守卫红。**变异③挖出守卫盲区实锤**：`last_confirmed.insert` 单行针被多行形式（rustfmt 拆行）静默逃逸——针改裸标识符（抗格式化），doc 去标识符字面（自咬教训再现两处）。计数 +3（表级测试/差异锁/守卫） | 蓝图 75 卷一 §3 |
| REG-75-G | `evidence.rs`（14 族）+ `engine.rs` 五调用点 + `held_engine.rs` 四调用点 | **Done（v0.343.0 WP-75.1.2，提交 c923ca37）**：九族手写 println（CERTIFIED/PLAN-COMPILED/PLAN-COMMAND-DEFERRED/PLAN-COMMAND-SKIP-EXPIRED/PLAN-COMMANDS-DEFERRED/PLAN-HELD/HELD-BATCH/HELD-COMMAND/HELD-PLAN-DONE）收进 evidence 表 formatter；调用点零字面（grep 实证）；输出逐字节不变。词形冻结从「QEMU 门 grep」升级为「host golden 逐字节」（`evidence_extended_families_golden`——期望值=迁移前现行字面逐字节手转录，防新实现自证；constraints=u32 类型为编译器钉正）。变异咬合：certified 改一字符→golden 红。`tools/r10_b_witness.py:68` 同步检查（读不改，§0.7）：Python 侧 HELD-COMMAND 行模板与表逐字吻合，零变化零登记。**QEMU 门义务交集（grep 实证）**：PLAN-COMPILED 族 = run62-vol1/vol3、run63-vol1/vol2/vol3、run70-r10-held、run61-phase1-e4；HELD 族 = run70-r10-held + r10_b_witness.py。卷册义务清单（run63-vol3/run70-r10-held/run62-vol1/vol3）复跑：run62-vol1 **PASS**（EXIT=0，VOL62.1-GATE: PASS）、run62-vol3 **PASS**（EXIT=0，VOL62.3-GATE: PASS）；run63-vol3 门总体 FAIL 但**本卷义务判据全绿**（PRODUCT-READY ✓ / 零 PANIC ✓ / CATALOG-ACTIVE ✓ / REGISTRY-ACTIVE ✓ / CONTRACT-VIEW-ACTIVE ✓ / **PLAN-COMPILED ✓** / **EXEC-JOURNAL applied-confirmed ✓** / **dispatch=Committed ✓**），三红（OBS-REJECT 缺失 / SNAPSHOT rejected 缺失 / edges 基线 35 漂移）为在途层 C9 mqtt_assembly + telemetry.rs +81 行行为面预存漂移——其 svc-mqtt E0618 使目标镜像在本会话附带修复前根本无法构建，即 R0 状态此门不可能跑过（非本卷回归）；run70-r10-held 两轮复跑一致：**17 组判据绿**（段A A1-A7 全绿含 HELD 词形全族 / 段B B1 重启 STORE-OPEN ✓ + B4 恢复对账写归属 ✓——**WP-75.1.1 恢复收表的 target 实弹面** / 段C C1-C4 全绿），仅 B-cut/B0/B2/B3 三项同根红 =「80s 内无同计划 Applied+Pending 在途见证」kill 窗时序与当前计划节奏的**可复现预存漂移**（脚本自 a00af693 未改；计划执行节奏在 71–74 卷间变更，B-cut 判据未随之重订——登记 Open，归 R10 领域重订窗，非本卷回归）**。计数 +1（表驱动 golden）。**code-review 裁决补记**：①卷册 §4「r10_b_witness.py:68 加注释锚」字面义务与 §0.7「不动 tools/」冲突——按 §0.7 裁决读不改（判据逐字吻合零登记），本条即卷面追认；②宽交集三门（run63-vol1/vol2、run61-phase1-e4，grep 实证同 PLAN-COMPILED 族）未复跑——免跑理由：同词形族已由 run62-vol1/vol3 + run63-vol3 三门同词形绿面覆盖，且旧卷门可能携本卷外预存红，避免超卷扩大化 | 蓝图 75 卷一 §4 |
| REG-75-N | `held_engine.rs::held_horizon_violation`（新增纯判定）+ `engine.rs`（调用点+stale allow 注释） | **Done（v0.343.0 WP-75.1.6，提交 800d24e0）**：run_typed held 分叉三重时域校验（计划时域终点 vs 遥测/证书有效期+逐意图有效期）下沉纯函数——判据与拒绝词形「held horizon exceeds trusted telemetry」逐字保持；accept_held_plan 调用点位置不动；不触 typed 链类型合同与 correlation 派生（ADR-0048）。run_typed `#[allow(too_many_arguments)]` stale 注释更新为现行事实（held 模块已落地；horizon 已下沉；**参数成束重构继续挂起，锚点=本条**）。红测：既有锁 typed_chain `held_plan_horizon_beyond_control_window_rejected` 覆盖在案（卷册：有则复跑即可）。变异咬合：> 翻 >= 边界→既有锁红。计数不变 | 蓝图 75 卷一 §5 |
| REG-75-O | `tests/support.rs`（夹具单源）+ 七套件 | **Done（v0.343.0 WP-75.1.7，提交 e692d545）**：七份 ~70 行 trusted_telemetry 夹具收编 `trusted_telemetry_with_clock(units, now)` 为核 + 常量时钟/默认全 Ratio 两派生。**两个漂移维度显式化，禁止统一**：①单位集——settlement_lock 点 5/6=Other（RequiredPointCoverage 其他校验路径）经显式包装保留；②时钟——**施工中实弹咬出**：setpoint_ramp_contract 的 now_ms 是可变 CLOCK（其余六套件常量），夹具收编成常量后 3 测 TelemetryExpired 红——归一化抹平有意差异的活例；经 with_clock 现取 now_ms() 保留原语义。六套件 use 接线零调用点变化；残留 import 清理（clippy 0）；support 跨二进制 dead_code 按 legacy_candidate 先例 allow 注记。变异咬合：support 点 1 max 1000→300→typed_chain 8 红（单点变异七套件齐感）。计数不变。**code-review 复核订正（首版登记失实，H1 硬违规）**：R0 旧版实为**六套 Other/Other（点 4=Ratio、5/6=Other）、仅 typed_chain 全 Ratio**——首版收编把方向做反（默认=全 Ratio 并翻转 audit/setpoint/journal/intent/dispatch 五套件），恰犯卷册所禁「顺手统一」；因 evaluator 不消费 unit（纯元数据）断言未翻、全绿为真，属低后果高登记失真。订正（随卷补刀）：support 默认归位 Other/Other、typed_chain 显式全 Ratio 包装、setpoint 包装单位集归位、doc 同步 | 蓝图 75 卷一 §5 |
| REG-75-P | `engine.rs IdemNote` + `saga_exec.rs` | **Done（v0.343.0 WP-75.1.4，提交 c77756f0）**：CoreCommand.idem_note: String 三处构造字面（run_intent「同 (protocol,seq) 重放」/run_typed「原结果」/saga「saga 原结果」——进 durable 回执 note，journal 持久化冻结词形）收为三变体枚举 as_str() 逐字节保持；receipt_from_journal 仍收 &str（「batch replay: reconciliation pending」另一调用方不受扰）。journal 既有记录读回不反解 note（纯展示）——无兼容面。golden idem_note_word_forms；变异咬合：PlanReplay 全角逗号改半角→红。计数 +1 | 蓝图 75 卷一 §5 |
| REG-75-Q | `adapter.rs proposal_to_candidate_unbound` | **Done（v0.343.0 WP-75.1.3，提交 aae744da）**：unbound 形态内联重写 solve-status match 收编入 solve_status_kind 单源（plan_id_of 同型收编先例漏掉的那份）；错误词形自动同源（改动前两处文案逐字相同——行为锁咬不住单源性，按卷册改源扫描守卫）。守卫 solve_status_gate_is_single_sourced（Optimal/Suboptimal 放行臂 ≤1，针运行期拼接；doc 不得复现完整针形——自咬教训）。红证据：施工前守卫红（臂计数 2）；变异（回填内联）→红。计数 lib +1 | 蓝图 75 卷一 §5 |
| REG-75-Z | `settle.rs settle()` | **Done（v0.343.0 WP-75.1.5，提交 14d942e9）**：states 死参数删除（let _ 悬置——interface 承诺不存在的行为）；22 处表级测试 + 3 处编排调用实参同步；若未来某臂真需要 states 判据届时再加回并消费。CONTEXT.md SettleTrigger 词条签名同步 settle(trigger)。DoD：grep settle( 调用点零 states 残留。计数不变 | 蓝图 75 卷一 §5 |
| REG-75-Y | `held_plan.rs terminal()/unresolved()` 全量重放 | **Open（P3，Speculative，登记不修）**：两谓词对每 batch 调 saga()（空事件 new/非空 BatchSaga::restore 全量重放），held_device_busy 在每次仲裁对全部 plan 调用且 plans() 深克隆——O(事件数×plan数) 每命令。不修理由：正确性无虞；无实测规模伤害；restore 缓存终态位会碰 HeldBatch wire 解码路径（frozen wire 邻接面）。**施工门**：出现「恢复大计划后仲裁延迟」实测证据或 held plan 数量上双位数时，在 Decode 后构造期派生 terminal/unresolved 位（不加 wire 字段） | 蓝图 75 卷一 §6 |
| REG-75-T | `energy_agent_service.rs:231` + `dispatch_service.rs` 守卫 | **Done（v0.344.0 WP-75.2.0，提交 c3a8e7ea）**：EA tick 构帧 `disp_req.push(2); // ActorKind::Ai` 匿名字面收编 `eneros_telemetry_model::command::ActorKind::Ai as u8`（判别值 Ai=2 亲验，wire 字节恒等）；守卫扩面 `ea_tick_actor_kind_single_source_guard`（扫描 energy_agent_service.rs 全文件，针运行期拼接、不按 `//` 豁免——收编后字面任何形态不合法）+ 字节恒等断言同锁。红→绿；变异（改回 push(2)）→红。计数 +1 | 蓝图 75 卷二 §2 |
| REG-75-S | `ota_service.rs` 0x01 CHECK 臂 | **Done（v0.344.0 WP-75.2.1，提交 0287a8e8）**：0x01 臂 `admit_description(&desc.encode().expect(...))` 改直取请求段 `&req[5..5+DESC_SIZE]`（与 0x02 同法）——re-encode expect panic 面消除；parse 保留（即 canonical 校验门：decode 强制重编码一致，非 canonical 字节 parse 处即拒）。**差分前提锁** `check_desc_reencode_identity_differential`（典型/满宽/空串/极值计数/未登记 usage 五样例域，re-encode==原始段逐字节）——结构性根由=decode 内 canonical 强制，证伪即中止条件未触发。ERR_LEN 负例（短段不 panic）。**变异咬不住差分**（直取↔re-encode 等价）→ 生产区源扫描守卫（首个 `mod tests` 之前禁 `.encode().expect` 针，分段拼接防自咬；测试域合法用例不误伤）：变异回填→红。计数 +3 | 蓝图 75 卷二 §3 |
| REG-75-J | `ota_service.rs` AbCommandOutcome + `svc-ota.rs` 武装门 | **Done（v0.344.0 WP-75.2.2，提交 c66b1c36）**：`handle_ota_ab_command` 返回 bool→`pub struct AbCommandOutcome { handled, check_pass }`（NOT_HANDLED/HANDLED 常量=原 bool 逐语义等——含 CHECK Deny 仍 handled，判定本身在 resp[0]）；`check_pass` 仅 0x01 臂真（verdict==Pass）。svc-ota 武装门 `req[4]==0x01 && resp.first()==Some(&1)`（bin 跨 seam 解读回执首字节）改 `outcome.check_pass`——武装判据 host 直测。36 处 `.handled` 出参适配（numstat 复核：block_io 12 + ota_service 23 + svc-ota 1；karpathy 复查轮订正——首版登记「49（17+32）」误用了替换器试跑输出）。红测 Pass/Deny/信封违例三态；变异 check_pass 恒 true→Deny 臂红。wire 零变化。计数 +1 | 蓝图 75 卷二 §4 |
| REG-75-D | `business-services::svc_panic!` 宏 + 八 bin | **Done（v0.344.0 WP-75.2.3，提交 dceef165）**：七静默 spin panic bin（market/ledger/agent-bus/cloud-sync/config/mgmt/observability）+ svc-ota（词形恒等切换）共 8 处 panic handler 收编 `svc_panic!("svc-x")` 单源宏（`[<svc>] PANIC: {info}` 与 svc-ota REG-70.7 先例逐字一致；cfg 门 host 展开为零；PanicInfo use 随删）。守卫 `bins_panic_handler_vocalized_single_source`（八 bin 含自身名调用 + `#[panic_handler]` 自有定义零残留——handler 定义只存在于宏内）。tools/ `PANIC` 依赖核查：run68-vol5-gate 串口「PANIC 净值=0」为负向判据、健康运行零 PANIC 行、该门 kill 的全是宿主侧进程——零碰撞（记录在案）。target 构建 EXIT=0。变异（单 bin 回静默）→守卫红。计数 +1。**全局复查轮注记（2026-09-26）**：PANIC-SCAN（tools/gate-criteria.toml none_of 族）词形不含 `[svc-x] PANIC:` 展开形——no_std 自定义 handler 的 PanicInfo Display 无 "panicked at" 前缀，真 panic 时该门计数恒 0；全 workspace 优势形先存盲区（七 bin 原静默 spin 对门同样不可见，声化对人读仍是严格增益），扩 none_of 属 tools/ owner 裁决门 | 蓝图 75 卷二 §5 |
| REG-75-E | `energy_agent.rs` MIGRATION_CATALOG_VERSION + `agent_test_support.rs` 记录面 | **Done（v0.344.0 WP-75.2.4，提交 810a88d1）**：①裸 `proposal_to_candidate_unbound(&plan, 1)` 具名化 `pub const MIGRATION_CATALOG_VERSION: u64 = 1` + 防漂移锁测 `catalog_contract_version_locked`（include_str 读 configs/catalog.toml 解析 `contract_version` 行恒等——锁未来 toml 升版而常量未动，变异改 toml→红）；②RecordingDispatch 记录面真化：`submitted` 从陈旧 `(ProposedControlPlan, TrustedTelemetry, AuthenticatedActor, u64)` 元组（typed 臂整体丢弃、intent 臂伪造占位 ProposedControlPlan）改 `SubmissionTrace` 枚举最小事实（Typed{plan_id,actor,telemetry_epoch,telemetry_generated_at_ms}/Intent{protocol,sequence,actor}）——doc「记录轨迹」宣称变真，submit_count 有牙（直测 typed/intent 逐次留痕+字段内容；变异清空 push→红）。dev-dependency upa-model（测试构造 PointWriteIntent 值域单源）。计数 +2 | 蓝图 75 卷二 §6 |
| REG-75-R | `svc-market.rs:44-52` + `svc-agent-bus.rs:89` | **Done（v0.344.0 WP-75.2.5，提交 54a80280）**：降级词形两态分立——svc-market `open_fact_face().ok()` 改 match（Err 打印 `[svc-market] FACT-FACE-OPEN-FAILED {e:?}`，授予缺失/绑定失败/区段打开失败三态分立，行为不变仍降级 None；错误类型 `()` 故词形即信号）；svc-agent-bus `restore().ok()` 全吞改 if-let（`[svc-agent-bus] BUS-RESTORE-FAILED {e:?}`）。守卫双测（词形存在 + `.ok()`/`restore().ok()` 旁路零残留，针分段拼接）。tools/ 新词形 grep 零命中（QEMU 正向判据门不受影响，核查义务在案）。target EXIT=0。变异（回 `.ok()`）→红。计数 +2。**karpathy 复查轮注记**：svc-market 守卫 bypass 针（`).ok()` 相邻形）较窄——`x.ok()` 标识符紧邻形可绕（现 grep 零命中，覆盖面注记非缺陷）。**全局复查轮注记（2026-09-26）**：run68-vol4-gate.sh:199「FACT-FACE-UNAVAILABLE=0」降级计数器覆盖面被本 WP 分立收窄——OPEN-FAILED 不再入该计数；无假绿（:188 正向判据 `FACT-FACE-OPEN base_block=…` 在降级时必缺失仍兜底红）；下次该门重涂时并入新词形（tools/ owner 裁决门） | 蓝图 75 卷二 §7 |
| REG-75-C | `mkt_wire.rs`/`ldg_wire.rs`/`cld_wire.rs`（新）+ 三 bin | **Done（v0.344.0 WP-75.2.6，提交 31d40535）**：market 93B/ledger 64B/cloud-sync 56B+变长载荷手解自 bin 下沉 lib（svc_wire 同层位——信封层归 svc_wire、本模块裁载荷层、bin 只剩 match 臂；svc_wire 模块 doc 层位句随迁三模块实名）。**照抄不重写**：偏移/枚举映射/错误词形逐字搬；错误序保形（先总长门 ERR_LEN 后枚举域 ERR:market——序锁测试钉死）；cld 变长钳制照搬（`5+55+plen>req.len()`→ERR_LEN；**实弹发现 bin 现状 plen=0 即拒**（定长门 5+56 比 55B 头多 1B）——照抄保形并测试记录该边界）。10 测=golden 正向+枚举 1-6 逐项+负例族（92/94、market 0/7/255、63/65、短段/plen 溢出）+序锁。**双变异咬合**：偏移 p[33..37]→p[32..36]→golden 红；错误序倒置→序锁红。bin 净减 44 行。target EXIT=0。计数 +10 | 蓝图 75 卷二 §8 |
| REG-75-H | `dispatch_service.rs handle_request` + `dispatch_assembly.rs` 收缩 | **Done（v0.344.0 WP-75.2.7，提交 b40f8cba）**：opcode 单表分派——service 新单一入口 `handle_request(engine, req, telemetry: Option)`（SUBMIT/TRUSTED/HELD_TICK/HELD_QUERY/INTENT/unknown 六臂 + 空请求臂；「unknown opcode 走 not-submit 拒绝」惯例原只有 assembly 知道（`_` 兜底臂落 handle_submit 内层双 owner），现单点化）；handle_submit 原文三分臂（held_tick/held_query/submit_typed）逐行抽出为私有臂函数，自身收缩为无遥测 shim（既有调用点/测试零改动，拒绝词形逐字保形）；assembly 收缩为「engine None 早路由 + INTENT 特化（计数+遥测快照+审计观测）+ 其余全量直通」，`_` 兜底臂删除。路由全臂测试（六臂+unknown+INTENT 无遥测 fail-closed 词形）；变异（单表删 HELD_QUERY 臂）→路由测试红。**实施勘误**：卷册设计 INTENT 臂「telemetry 在场时」——实际恒传 Option（None=fail-closed 在 handle_submit_intent 臂内，语义同）；engine 形参 dyn ControlDispatch 非卷册所写 DispatchEngine（与 handle_* 家族现行对齐）。计数 +1 | 蓝图 75 卷二 §9 |
| REG-75-I | `mgmt_relay.rs`（新，397 行）+ `svc-mgmt.rs`（1406→1333） | **Done（v0.344.0 WP-75.2.8，提交 29a4835f）**：TargetExecutor 纯判据分相下沉 `business-services::mgmt_relay`（**EIPC 拨号/root_call/persist CAS 留 bin**；bin 行数 1406→1332 于 WP 提交，收口整备+review 补刀后终态 1333）。相A：ConfigSet S/C/A 响应归类（classify_stage/commit/activate_resp——**「先判 ERR」判据 2026-09-24 已修 bug 的第一道 host 锁**；真 bug 复活变异「非空即成功」→红，注：首试变异 `!is_empty && !starts_with(ERR:)` 为等价变换咬不住，已订正变异形态）；相B：重启对账 detail 解析+终态三态（parse_restart_detail/classify_restart_observation——超时边界 `>` 变异→红）；相C：快照分节格式化（format_snapshot_section sm3 分节行 golden——hashlib SM3 预算真值锁 + 五分节 body 缺项词形锁；摘要宽度 8B→4B 变异→红）。目标镜像全量 host 可测面 +8 测。**QEMU MGT 门义务**：run70-vol6（管理动作实效与配置生命周期）PASS 27/0（config-stage-part/config-staged:digest/config-activated/MGMT-OPS 词形全经下沉后归类函数实弹；首跑 FAIL=三门 qemu 进程宿主 OOM SIGKILL（串口 0 字节），复跑同码 PASS——非代码回归，证据归档 e018a9d7）。target EXIT=0。bin 净减 73 行。计数 +8。**code-review 补记（standards 轴 1 硬违规修复）**：format_identity_body 首版下沉后漏接线（bin identity 分节仍手抄 env! 字面=死代码，违 00 契约 §8.1 新模块零消费者禁令）——复查抓出后接线（bin/lib 同 package，CARGO_PKG_VERSION 词形逐字节恒等） | 蓝图 75 卷二 §10 |
| REG-75-U | 八 bin `_start` 外壳（speculative） | **Open（P3，Speculative，登记不修）**：每 bin ~30 行同构进程外壳（cfg_attr/static ALLOC/`_start` 5 参（svc-mgmt 7 参）/set_ipc_buffer+ALLOC.init/control_service_main/panic handler/host main 桩）×8+control-services 5 bin+drv-svc 族。不修理由：每份编译期可查、零行为风险、稳定；宏化波及三 crate 族（含 7 参变体）爆炸半径大；WP-75.2.3 已把唯一行为面（panic 声化）单点化——剩余是纯样板。**施工门**：新增第 9/10 个 business-services bin 时宏 `control_service_entry!{name, |mask| handler}` 一并收三族；owner 裁决门 | 蓝图 75 卷二 §11 |
| REG-75-F | `root-task/service.rs` DirectIndexes + run_ipc_loop 三惯用法 | **Done（v0.345.0 WP-75.3.0，提交 bd00ac0f）**：直连索引束 8→10 槽位（observability/hello 入名单）——restart_done 闭包内联 position（每探针步 O(n)）收编 `self.direct.observability`（unwrap_or(false)=不在场未完成语义保持）；run_ipc_loop 入口一次构建束、by_name 闭包退役（hello/fault 探针缺席分支经直连束投影）；setup 五处 `Orchestration::direct_handle` 改束投影后函数删除（grep 第五面核：仅注释与测试等值 oracle 残留=合法）。守卫 `direct_indexes_is_the_single_by_name_scan`（按名 position 扫描全文件恰 1 处=build idx 闭包本体；token 运行期拼接——doc 复现完整针形自咬一次已避开）。投影测试扩 10 列。EXPLICIT_UNWIRED svc-observability 散文改引 DirectIndexes 名单。变异（restart_done 回内联）→守卫红。target EXIT=0 两变体（默认+synth，mgt_probe_step 覆盖）；VOL70.6 MGT 门 PASS（restart_done 经探针链实弹）。计数 +1 | 蓝图 75 卷三 §2 |
| REG-75-K | `root-task/wiring.rs` ServiceWiring + heartbeat_miss_limit | **Done（v0.345.0 WP-75.3.1，提交 2a4d8f30）**：容忍窗入表——`ServiceWiring` 增 `pub miss_limit: Option<i64>`（行尾追加，13 行表序零变化 diff 审）；svc-telemetry 行 Some(3_000)（2026-09-14 实弹校准值搬家+校准史注释随行）；`heartbeat_miss_limit` 改 `wiring_of(name).and_then(|w| w.miss_limit).unwrap_or(DEFAULT)` 纯表投影（签名与两消费点零改动）。红测 `miss_limit_lives_in_the_wiring_table`：表行锁+函数=表投影逐行等值+无心跳行配窗一致性别面+源扫描守卫（`"svc-telemetry" =>` 词形清零——函数改回按名手抄 match 即红）。**双变异咬合**：表行删→双测试红（wiring 新测+supervisor 既有 per_service_miss_limit 锁）；函数回 match→源扫描守卫红。CONTEXT.md SERVICE_WIRING 词条加 miss_limit。target EXIT=0 两变体；VOL70.6 门 PASS（账本序列判据结构性依赖心跳不误杀=心跳面实弹）。计数 +1 | 蓝图 75 卷三 §3 |
| REG-75-L | `drv-svc/blk_backend.rs` + `drv-svc/block_cmd.rs` wire + `agent-sdk/block_io.rs` + 五生产装配点 | **Done（v0.345.0 WP-75.3.2，提交 2569bb90/证据 9e4a27e2）**：BLK_CMD_HEALTH 观测通道双端通电——生产侧 `fold_health_code` 折码（bad_blocks>0→2 / wear>80→4 / life<20→3 / 否则 0；多条件取最严重，阈值 doc 标注初版工程判据可调非冻结 spec）；消费侧 `EipcBlockDevice` 增 health 缓存+`new_probed` attach 期探针构造（BLK_CMD_HEALTH 13B wire 形状冻结，+1 EIPC 往返；失败→缺省完美兜底不拖垮构造）+`unfold_health_code` 反卷语义化近似（**诚实取舍登记**：wire 不传连续量，二值健康/劣化语义；未来连续量须扩 wire 另立卷；temperature 恒 0 无域）。health_status 返回缓存；既有 new()（mock/测试装配）保持缺省完美。五生产装配点切 new_probed（event_face ×2/telemetry_history/fact_face/svc-ota；FakeStorage 夹具留 new）+接线守卫（fold 恰一处——75-02 漏接线教训预防面）。红测 ×2（折码规则+边界+优先级；探针快照+反卷+失败兜底+new 缺省）。durable-log/event-store 委托链开始见真值（零裁决消费方预核；**登记**：未来 durable-log 按 health 降级写策略需连续量扩 wire）。证据面 health 判据扫描零命中。target EXIT=0；VOL70.7 块面门 PASS 16 判据（OTA 安装事务/BootControl/重启回滚经 svc-ota new_probed 实弹）。计数 +3 | 蓝图 75 卷三 §4 |
| REG-75-M | `drivers/fs/src/lfs/geometry.rs`（新）+ `control-services/event_face.rs` + `telemetry_history.rs` + `business-services/fact_face.rs`（复查轮补漏） | **Done（v0.345.0 WP-75.3.3，提交 6fc6daf2/复查轮补漏见卷三核算段）**：512B→4K 几何折算单源——`RegionGeometry::from_blocks_512`（8 倍率+整除对齐律；不满足→None）+`LFS_BLOCK_SIZE` 单源常量 re-export。替换面全量：event_face ensure_open/open_snap_volume/face_dev 夹具/t3b/t4 测试位点+telemetry_history open_disk_history；两文件本地 `FS_BLOCK_SIZE` const 删、`% 8`/`/ 8` assert→geometry `.expect`（**选 (a) 保持 panic 力度，行为逐字节等价**）；`.min(64)` 钳制留调用方（策略非几何）。**复查轮补漏（F1）**：fact_face.rs 三位点（盘块数+区段基址/长度）原卷漏迁移（蓝图 §1 枚举即漏 fact_face，首版登记却称「4 处一并归单源」= 失实）——补 `RegionGeometry` 收编（business-services 增 eneros-fs 显式依赖，向低层无环）；DATA_FACT 区段常量 8-对齐亲验（256 倍数链）⇒ expect 不可达。DoD grep 清零：三文件 `/ 8`、`% 8`、本地 FS_BLOCK_SIZE 算术惯用法零残留（残留仅单源引用与注释）。红测（换算/4096 边界/零长/对齐拒绝三态）+变异（倍率 8→16）→红+恢复。target EXIT=0；VOL70.2 mount 面门 PASS（seed+reuse 双窗，复用盘 TSDB mount 保数据实弹）。计数 +1。**全局复查轮注记（2026-09-26）**：①ensure_open 旧形态为裸 `/ 8`（错位输入=静默截断除法），改 `.expect` 实为**新增** panic 路径而非「保持」——fail-closed 改善、常量链 8-对齐亲验不可达，如实注记；②fact_face 补迁移（ce2127be）的 target 实弹闭环由 run68-vol4 双窗复跑补齐（ce2127be 当轮仅复跑 host/clippy/target）——fact-face 判据**实质全绿**（窗 B `FACT-FACE-OPEN base_block=24968 …consistent=true wm=true` 完整行 PASS；窗 A 同值行遭串口突发丢字仅存尾巴，UNAVAILABLE/OPEN-FAILED/PANIC 全 0=open 成功无疑），门整体 verdict 因串口丢字敏感+agent=OK 旧基线两项 tools/ 观察为 FAIL（详见卷三核算段·全局复查轮） | 蓝图 75 卷三 §5 |
| REG-75-V | `fs/lfs/mount.rs` mount_or_format 三因一果（speculative） | **Open（P3，Speculative，登记不修）**：scope 构造失败/非 BadSuperBlock mount 错/format 失败全折 None；doc 自认「与收口前两调用点吞错口径一致；需区分错误因的调用方直接用 `Lfs::mount`」。FS 层本身分类细心（filesystem.rs 只 CORRUPTION→BadSuperBlock）。不修理由：**有意保留**的收口前口径（doc 在案）+行为锁在案（mount.rs 保数据锁）；两调用方当前只消费 Option。**施工门**：运维取证需要区分「介质瞬态 IO 错 / 空白盘格式化失败 / 区段越界」时——届时 `MountOutcome { Opened{fresh}, ScopeInvalid, Mount(FsError), Format(FsError) }` + `fn ok(self) -> Option<Lfs>` 兼容壳，两调用方语义零变化 | 蓝图 75 卷三 §7 |
| REG-75-W | `blk_backend.rs` 数据面「逻辑块 ≤512B」焊死栈缓冲（speculative） | **Open（P3，Speculative，登记不修）**：read/write 两处 `let mut tmp = [0u8; 512]; if bs > tmp.len() { return Err(()) }`——4K 逻辑块介质首次 I/O 才静默失败（无词形）；block_size 运行期探出。不修理由：QEMU virtio-blk 恒 512（当前唯一介质）；改法廉价但零现收益。**施工门**：接入任何非 512 逻辑块介质前**必须**先做 `try_new` 里 `block_size != 512 → Err("unsupported-block-size")`（fail-closed 前置到 attach 期+词形可观测）或缓冲升 4K；届时与 REG-75-X 同卷 | 蓝图 75 卷三 §7 |
| REG-75-X | `block_cmd.rs` 数据面 ST_UNAVAILABLE 一词两因（speculative） | **Open（P3，Speculative，登记不修）**：backend None（防御性兜底）与 read/write `Err(())` 同落 ST_UNAVAILABLE——调用方（agent-sdk block_io → StorageError::HardwareFault）无法区分装配缺失 vs 介质故障；boot 事务臂已有单义词面先例（boot_word_reject）。不修理由：改 `BlockBackend::read/write` 签名 `Err(()) → Err(&'static str)` 波及 3 实现+全部 mock；收益纯取证（wire status 不动，仅 target 打印分因）。**施工门**：REG-75-W 施工门触发时同卷做（4K 介质接入排查需要分因词形） | 蓝图 75 卷三 §7 |


**收口核算**：全量 host 9269（基线）→ 9257（WP-75.1.0 −12）→ **9263**（+6 = +3 表/差异锁/守卫 +1 evidence golden +1 adapter 守卫 +1 idem golden；karpathy 复查轮勘误——收口提交 1335c1d9 信息中「+4」为笔误，总数 9263 与分解 +6 自洽）；clippy --workspace --all-targets -D warnings 绿；cargo fmt --check 绿。

**附带发现（本卷门义务执行挖出的三项预存断裂，均已最小修复；修正面均在本卷卷册边界外的遗留层）**：
1. `orphan-crate-unregistered:eneros-installer-uefi`（qemu-prod image 门，74-03 c64c3747 加 crate 漏登记 orphans_allowlist）→ 按 schema v1.1 约定（条目仅 package+reason，deny_unknown_fields）补登记 `product/policy/frozen-crates.json`；
2. `svc-mqtt` E0618（target-only，host 空 bin 桩不可见——R0 在途层 C9 mqtt_assembly 收口 bin 侧遗留）：`MqttGateVerdict::Proceed { inject_tls_material }` 解构绑定与同模块自由函数同名遮蔽，调用点编译红 → 绑定改名 `inject`（行为零变化）；
3. `installer-uefi/build.rs` panic!/expect/assert（disallowed_macros，host clippy --workspace -D warnings 在案即红，74-03 遗留）→ build script 改 Result 传播（消息保留，cargo 失败上屏等价）。

**R0 说明**：本卷施工前先收编工作树在途层为 R0 提交（R13/R14 修复层，C1 拆分 saga_exec/settle/evidence、held_engine/hold_registry 转正等；host 9269/0 实证在案），保证每 WP 提交为纯净 diff。

**收口核算（卷二）**：全量 host 9263（卷一终态）→ **9292**（+29 = +1 ActorKind 守卫 +3 ota 差分/负例/源扫描 +1 outcome 三态 +1 panic 守卫 +2 catalog 锁/记录面 +2 降级词形守卫 +10 wire golden/负例/序锁 +1 路由全臂 +8 mgmt_relay 判据）；clippy --workspace --all-targets -D warnings 0（收口整备：too-many-args 参数束 RestartFacts/field_reassign 夹具 allow/disallowed_macros 测试豁免/wire 模块 items-after-tests 重排/needless borrows——收口提交）；cargo fmt --check 绿；target 构建 EXIT=0 ×4 次（WP-75.2.3/2.5/2.6/2.8 义务）；QEMU VOL70.6 MGT 门 PASS 27/0（run70-vol6-gate，证据 e018a9d7）；CONTEXT.md 三词条（svc_wire 层位句收窄 + svc_panic!/mgmt_relay 新增）。

**karpathy 对抗复查轮（同日，HEAD 7d098af1+复查修复）**：三轴并行（保形等价×2 / 测试判据有效性 / 次生bug与外科手术）+ 主审逐条多维验证（改前镜像 diff/算术核验/变异实证/恢复 diff 校验）。**代码功能 bug：零**——保形等价轴对 wire 下沉（逐偏移逐枚举逐错误字节）、ota 直取/AbCommandOutcome（逐 return 点 9 类）、mgmt_relay（词形逐字节+缺项分派）、dispatch 单表（逐臂+词形）全部判定严格等价；唯一语义面变化=handle_submit shim 对 opcode 2 的死路径行为（无调用方触及，doc 注记补全）。**修复 2 项测试判据缺陷（变异实证）**：①cld 钳制门拒绝侧零覆盖——原负例 60B 先被定长门拒，变异（放松钳制门）全绿实证假绿，负例改构造 61B 恰过定长门+plen 溢出（修后变异独红）；②epoch 留痕断言恒真——stub source_epoch 恒 0，测试覆写非零 epoch 给牙（硬编码 0 变异→红）。**登记精度订正 2 处**：REG-75-J 调用点 49→36（numstat 复核 12+23+1）；REG-75-I 行数归属 1406→1332（WP 提交）/终态 1333。**证据链补全**：REG-75-I「证据归档」首版措辞过宽——仓库证据约定核实：EVROOT 内 t1/t2/t3 qemu-serial.log 被 `.gitignore:103 *.log` 刻意排除（串口转录不入库是既定约定，非漏归档）；跟踪面=image/station 指纹 + ota-anchor 门输出 + t1.mgt-transcript.jsonl（**v0.336 时代遗物，现行门已不产出该文件，登记为陈旧档案不动**，karpathy 外科手术纪律：非本卷产物不删）。MGT 门于 HEAD（d029ee4f）复跑 PASS 27/27（第 4 次尝试；第 1/3 次被宿主 OOM 截断、第 2 次 T-3 截断——四次全部「已杀死」签名一致，与代码无关：T-1/T-2 的 25 项判据在每次可用转录上全绿），跟踪面证据三文件随 HEAD 刷新提交。外科手术轴：xtask/tools/configs 零触碰、注释判据知识保形（「先判 ERR」全文迁至 lib+测试 doc）、mgmt_relay 12 pub 项全有消费者、词形 17 项只增不减、fmt/clippy 混合提交无藏匿行为面改动（-w 全量比对）。

**收口核算（卷三）**：全量 host 9292（卷二终态）→ **9298**（+6 = +1 直连唯一性守卫（F）+1 miss_limit 表锁（K）+3 health 折码/接线守卫/探针快照（L）+1 geometry 换算（M））；clippy --workspace --all-targets 0（收口整备：doc 空行/nonminimal_bool 两处——收口提交）；cargo fmt 绿；target 构建 EXIT=0 ×7 次（75.3.0/75.3.1 各默认+synth 双变体、75.3.2、75.3.3、收口终验）；QEMU 门义务全执行：VOL70.6 MGT 门 PASS 27 判据 ×2 轮（75.3.0 编排面/75.3.1 心跳面——账本序列判据结构性依赖看门狗不误杀）、VOL70.7 块面门 PASS 16 判据（75.3.2 health 通电后 OTA 安装事务实弹；证据 9e4a27e2；vol7 门脚本工作区版本副作用 0.337.0→还原 0.344.0 两度）、VOL70.2 mount 面门 PASS（75.3.3 seed+reuse 双窗）；CONTEXT.md SERVICE_WIRING 词条加 miss_limit+DirectIndexes 别名注记。**独立对抗复查轮（同日，HEAD d3900e62）**：六轴（保形等价/漏接线死代码/测试判据/wire 冻结/登记对账/全量复跑）。**代码功能 bug：零**——含两处重点怀疑项证伪：①probe 首响应吸收 epoch 与改前「首个数据命令吸收」同律（cmd() epoch 吸收在 status 检查前）；②13B 应答经 4B/word MR padding 回传后 decode_response_padded 的 payload_len 读 wire header 声明域（13 非物理 16），`<13` 检查恒真值通过。**修复 1 项单源残差（F1/P2）**：fact_face.rs 三位点 `/ 8`（盘块数+区段基址/长度）首卷漏迁移且登记失实（「4 处一并归单源」实为 3/4）——补 RegionGeometry 收编+business-services 增 eneros-fs 依赖+DATA_FACT 8-对齐亲验；**登记订正 3 处（F1 措辞/F2 卷三核算段补记+REG-75-M 计数不变→+1/F3 2569bb90 提交信息 9298 系终值时点笔误不改史）**。复查后全量 9298/0 + clippy 0 + target EXIT=0 复跑证实。

**全局复查轮（同日，HEAD 768fab7c 三卷全毕后；四路子代理对抗复查+主审逐条多维验证）**：范围 4db80711..HEAD 全部 38 提交，四轴（卷一 control-dispatch 10 项/卷二 business-services 11 项/卷二 dispatch+卷三 10 项/跨卷全局一致性 10 项）。**代码功能 bug：零**——卷一：RecoveredApplied 写序逐段、evidence 14 族旧字面三方逐字节（golden 无自证）、plan_hold 零残渣、IdemNote/adapter/horizon 逐字、夹具 H1 终态逐文件归位；卷二：wire 三模块逐偏移+错误序保形、svc_panic! 展开恒等、AbCommandOutcome 逐 return 等价、直取差分结构性前提（decode 强制 canonical）、catalog/记录面/降级词形/ActorKind/identity 接线；卷三：dispatch 单表六臂逐字（intent 臂 IDENTICAL）、mgmt_relay 三相逐行、mqtt_assembly 词形冻结+双取等值、DirectIndexes 10 槽+守卫、miss_limit 表投影、health 折码优先级（bad>wear>life 实现与 doc 一致）、RegionGeometry 全位点；跨卷：tools/ 14 文件全归 R0（WP 零触碰）、登记册 26 条抽查、CONTEXT.md 四词条为真、词形×门判据全对拍、版本 0.345.0/lock 干净、38 提交新增 10 守卫自咬扫描全 PASS、计数算术 −12+6/+29/+6 全吻合。基线全量 **9298/0 EXIT=0** 精确复跑。**锁面补强 4 处（P3，均锁本蓝图内已发生过的 bug 类；变异咬合×4 实证红+cp 恢复核验逐字节）**：①identity 分节 body golden 补锁+接线守卫（75-02 code-review 漏接线死代码类——format_identity_body 系唯一无 golden 的分节 body，bin/lib 同 package 词形恒等故唯源扫描有牙）；②夹具默认单位集方向守卫（卷一 H1 类——unit 为评估器纯元数据方向翻转零断言红；support.rs 完整针恰 2=默认数组尾位本体）；③DirectIndexes 守卫扩 find 惯用法封口（R16-F 退役三惯用法中两支原为 find 形，只封 position 则 find 型回潮不红；在册恰 2=表驱动执行循环（蓝图 62 接线表语义非直连）+投影等值测试）；④health total_blocks 单位口径 doc 注（wire=驱动 512B 扇区口径 vs 本地几何 4K 块口径，两 getter 勿混算——零裁决消费方纯未来陷阱面）。**QEMU 闭环（F1 后置义务，ce2127be 当轮未复跑门）**：run68-vol4 双窗复跑——fact-face 判据实质全绿（窗 B `FACT-FACE-OPEN base_block=24968 region_blocks_512=256 recovered=0 consistent=true wm=true` 完整行 PASS；窗 A 同值行遭串口突发丢字仅存尾巴 `base_block=24968 …wm=true`，UNAVAILABLE=0/OPEN-FAILED=0/PANIC=0 ⇒ open 成功无疑；RPO=0 recovered=362/快照 blob=16/学习态 restored=1/CONTROL-CHAIN Committed 全绿）——**RegionGeometry 迁移 target 首开+remount 双路实弹闭环成立**；门整体 FAIL（3 红判据/exit 7）源于两项 tools/ 基础设施观察（不动 tools/，owner 裁决门）：①窗 A FACT-FACE-OPEN 判据对串口交叠丢字敏感（busy 区多行互踩实证：同段 GoosePlugin 尾巴行/self-check 熔接线；69.5「串口交叠 re.match 漏计」同族）；②agent=OK ±25% 带 [347,578] 系 v0.324.0（9-06）旧基线——69/70 时代转录已见 1618/2237/2931/2959（决策管线合法扩容+窗内计数宿主速度依赖），今日 1428/1488 尚在该时代低端，非 75 回归。**收口**：全量 **9298→9300**（+2=identity 接线守卫+夹具方向守卫；golden/find 守卫为既有测试扩臂不增计数）；clippy --workspace --all-targets -D warnings 0；cargo fmt --check 绿；target 构建 EXIT=0。

## B76-1. 蓝图 76 · 修复卷一登记（2026-09-26，v0.346.0，EL0 panic 声化收口与守卫扩族）

> 来源：第十七轮架构体检（2026-09-26）候选 2（Strong）；蓝图 `蓝图/76-架构体检R17候选清偿蓝图/01-修复卷一-EL0-panic声化收口与守卫扩族.md`。基线 HEAD `99243508`（v0.345.0）；每 WP 一提交。宏宿主 = 守卫宿主 = `eneros-runtime`（五族公共依赖，逐 Cargo.toml 亲验零新依赖零环）。

| ID | 面向 | 状态/描述 | 源 |
|---|---|---|---|
| REG-76-A | protocol-services 11 bin（modbus-rtu/modbus-tcp/iec104/iec101/iec103/goose/sv/mms/mqtt/dlt645/gbt27930）+ drv-svc 5 bin（drv-blk/drv-net/drv-rng/drv-console/drv-fault） | **Done（v0.346.0 WP-76.1.3，提交 4b755f30）**：16 个在役 bin 的 `fn panic(_info) loop { spin_loop() }` 静默黑洞整段替换为 `eneros_runtime::svc_panic!("<名>")`。ADR-026 §1「EL0 服务 panic → 审计 + 打印 + 挂起」的**打印半边**补齐，挂起半边由宏 spin 原样保留（drv 族「fail-closed：由 root/watchdog 复位」语义不变）。词形 `[<svc>] PANIC: {info}` 与 business/C 族先例逐字节恒等 ⇒ 门窗口零新增行、PANIC-SCAN 无新命中面 | 蓝图 76 卷一 §4 |
| REG-76-B | 三互不相识 adapter → 一宏（A=审计+打印+挂起 / B=business 宏 / C=逐 bin 手抄） | **Done（v0.346.0 WP-76.1.1+1.2，提交 8d2e4964 / d62e552d）**：`svc_panic!` 自 `eneros-business-services` 下沉 `eneros-runtime`（business 八 bin 换路径为 `eneros_runtime::svc_panic!`，调用形不变）；C 族 control 五 bin + svc-storage 手抄打印收编（现词形与宏产出恒等亲验）。A 族五 bin（hello/fault/hog/pi/pie）保持 `svc_panic_hook` 审计链不动（ADR-026 全文语义已有存量实现）。宏体两处收紧：println 路径 `eneros_runtime::` → `$crate::`（消除蓝图自flag 的本 crate 误用自引用陷阱，展开字节恒等）、doc 补 ADR-026 立法与词形冻结依据。**注释迁档**（bin 只留一行调用）：SPEC-E「静默自旋曾把 target 侧栈溢出（win1-t2 首写挂死）藏了整个排障周期」、REG-70.7「静默黑洞有声化——无声 spin 掩盖死因」、drv 族「fail-closed：panic 挂起，由 root/watchdog 复位（不伪装继续服务）」三处原文案由宏 doc + 本行承载。`#[alloc_error_handler]`（svc-device-agent）不动 | 蓝图 76 卷一 §2/§3 |
| REG-76-C | `crates/runtime/runtime/src/panic_guard.rs`（新，cfg(test)）+ `business-services/src/lib.rs`（旧守卫退役）+ `protocol-services/src/protocol_ladder.rs`（t-ladder-3 针形订正） | **Done（v0.346.0 WP-76.1.4，提交 256d362e + 次生修复 153dfb9d）**：全族守卫单点 5 判据——① 30 bin 逐一「宏调用针在位 + `#[panic_handler]` 零残留 + 手抄 `fn panic(` 零残留」；② 宏本体打印/挂起两半边 + handler 签名上锁（判据首次覆盖宏自身，旧守卫只覆盖调用方）；③ 四族 bin 目录**完全划分**（清点闭合 31 文件 = 表 30 + svc-hang 豁免；新 bin 不入表即红）；④ 豁免族形态分立（A 族保审计 hook 且不用宏、svc-hang 保静默）；⑤ 自咬锁（本文件禁连续针形）。表 30 = 卷一点名 21 目标 bin（16 静默 + 5 control 手抄）+ svc-storage 1 + business 8——business 旧守卫退役条件「判据超集」成立（八 bin 两判据全保留并扩至 30 bin 三判据 + 宏本体 + 目录划分 + 豁免分立）。**次生修复（本轮全量首个红）**：pre-existing t-ladder-3 锁断言协议 bin 源含 handler 属性字面（「handler 留 bin——no_std bin 根约束」），宏收编后字面只在宏体 ⇒ 锁红；不变量（定义点在 bin crate 根、不下沉 lib）语义仍成立（宏在 bin 根展开），红的是针形——订正为「须有单源宏调用针 + 自有属性零残留」双断言（还原手抄即②红，删调用行即①红）。蓝图 §1 曾引 protocol_ladder.rs:251-258 作宏方案安全性实证，却未清点同族 host 结构锁，漏判一处 | 蓝图 76 卷一 §5 |
| REG-76-D | panic 路径进审计链扩展到非 A 族（`svc_panic_audited!` 可选宏） | **Open（P3，登记不修）**：protocol/drv 族不依赖 `eneros-services` crate；drv 类 bin 内存预算铁律（12/16KiB 栈帧教训）不欢迎审计栈入 panic 路径；A 族已完整合规 = ADR-026 全文语义有存量实现。施工门：某非 A 族 bin 需要 panic 审计存证（合规/取证）时，按 bin 内存预算逐个选用，不强制 | 蓝图 76 卷一 §7 |
| REG-76-E | svc-hang 静默 handler | **Open（登记不修）**：有意挂起靶（心跳/监督面测试依赖无声），守卫表显式排除并由判据④反向锁死「不得被声化」。施工门：svc-hang 转生产角色时一并收编 | 蓝图 76 卷一 §7 |
| REG-76-F | 非 A 族 16+ bin 的 OOM 声化（`#[alloc_error_handler]`） | **Open（P3，登记不修）**：独立面（多数 bin 无该 handler；svc-device-agent 有 OOM 词形先例）。施工门：下一轮体检或 OOM 排障需求驱动；本卷不夹带 | 蓝图 76 卷一 §7 |

**前提复验订正（对码亲验，三处）**：① 卷册基线记「全量 host 9298」过期——HEAD 99243508 实为 **9300**（75 全局复查轮 +2：identity 接线守卫 + 夹具方向守卫，登记册 1514 行在案），本卷收工基线按 9300 起算；② business 八 bin 调用形为全路径 `eneros_business_services::svc_panic!("svc-x")` 而非 §2 所述 `use` + 裸名（守卫针系子串匹配，两种形态同命中，迁移按全路径同形改写）；③ §4/§5「21 目标 bin」与 §5 步 4「退役 business 守卫（超集）」内在张力——21 = 16 静默 + 5 control 手抄，而超集条件要求表内含 business 8，故守卫表定稿 30（目录完全划分判据③实证 31 文件闭合，无漏项）。

**次生面自查（动手即改，未流入 HEAD）**：删 orphan `use core::panic::PanicInfo;` 时若不同步删其 `#[cfg(target_os = "none")]` 属性行，上一行会留悬空 cfg 静默挂到后继 `extern crate` 项（20 bin 命中此形，已在同一变更内成对删除）；`include_str!` 相对 `src/` 而 `env!("CARGO_MANIFEST_DIR")` 是 crate 根（A 族豁免路径差一级）；自咬锁断言方向（正确分段后连续针形应恰 0 次命中，非 1 次）——三处均为自查发现。

**收口核算（卷一）**：全量 host **9300 → 9304**（+4 = +5 全族守卫判据 −1 business 旧守卫退役；t-ladder-3 系既有测试改针不增计数）；target 构建 `third_party/sel4-build-rust.sh` EXIT=0（本卷全部改动在 `cfg(target_os = "none")` 区，host 绿不证明 target 可编译——这是唯一实弹面）；变异三连实证咬合（① svc-mqtt 还原静默 handler → 判据①③红；② 宏体删打印半边 → 判据②红；③ bin 名错拼隔离变异 → 判据①红），恢复一律 `cp` 逐字节 `cmp` 核验（禁 `git checkout`）。

**附带发现（卷一门义务执行挖出的预存断裂一项，已最小修复；修正面在本卷卷册边界外的 tools/ 遗留层）**：`tools/run70-vol5-gate.sh` 在 `set -uo pipefail` 下于 `ROOT` 赋值**之前**引用 `"$ROOT/tools/criterion.py"`（73 批B 判据器自举行插错位置）⇒ 门一启动即「ROOT: 未绑定的变量」退出，vol5 门在本卷之前已长期不可跑。修复 = 把 `ROOT=` 上移到首次使用之前（与 vol6/vol7 同序），`bash -n` 语法核验；不动判据、不动豁免。与蓝图 §0.7「不动 tools/」相冲处按先例处置（75 卷同类预存断裂最小修复留档），门义务优先于目录禁令。

**QEMU 门义务（卷一收口，三门全绿 2026-09-26）**：VOL70.5 **PASS**（exit 0，19 ✓/0 ✗——含 R7 窗A + 窗B「零 PANIC」双判据）；VOL70.6 **PASS**（27 ✓/0 ✗——含 R1/R30 零 PANIC）；VOL70.7 **PASS**（16 ✓/0 ✗——含 R1/R13 零 PANIC）。本卷声化的 protocol/drv/business/control 四族 bin 在真实弹窗串口全程零 panic 命中 ⇒ §0.5 预判「门窗口内无 panic ⇒ 声化零新增行 ⇒ 门不因本卷变红」实证成立（未挖出被静默吞掉的真 panic，无需分诊加豁免）。target 可编译性由三门各自的 `sel4 重链接 + image raw`（qemu-prod 与 qemu-test 双装配 release 全 bin 构建）兑现，覆盖面强于 `third_party/sel4-build-rust.sh` 单验。**首 Attempt 三门假失败分诊留档（环境非代码）**：一次工具中断只杀外层 `bash -c`，`run70-vol5-gate.sh` 子链存活并占住 IEC104 端口 2404 与 target 构建锁 ⇒ 并行第二轮 vol5 exit 3 / vol6 exit 4「PORT 2404 仍被占——清理失败」/ vol7 `QEMU QMP shutdown: readiness marker not found (NO_RESULT)` 大面积 ✗；关键旁证：该污染轮 vol7 的 R13「零 PANIC」仍 ✓ ⇒ 非 panic 回归亦非本卷代码。按 PID 清孤儿 + 端口核验后单链重跑即三门全绿。vol7 版本副作用（`Cargo.toml` 留 0.337.0 = B 镜像语义）已还原 **0.346.0** 并同步 Cargo.lock；`configs/protocol-mqtt.toml` 门改写已还原基线束。跟踪面证据八文件（image-a/b/t、combined、station-pubkey、tls-material、ota-anchor-gate-a、wa.broker-transcript）随 PASS 轮刷新。

**变异面补验（收口后追加实跑，两形皆红即咬合）**：① §2 依赖面咬合——svc-market 调用行回指已删旧路径 `eneros_business_services::svc_panic!` → host `cargo check --bin svc-market` 报 **E0433 cannot find `svc_panic` in `eneros_business_services`**（宏删除的编译期咬合，恢复 `cp`+`cmp` 逐字节核验）。同变异首次误用 `--target aarch64-unknown-none-softfloat` 直跑，红因是 `can't find crate for core`（裸金属 target 需 `third_party/sel4-build-rust.sh` 的 sysroot/feature 装配），属**红因不对症**、不作证据——裸 target cargo 直调不是本仓 target 验证面，登记以免后人照抄。② §5 三连（静默还原/宏删打印/名错拼）已在本卷正文登记。收工 target 构建复跑 `sel4-build-rust.sh` EXIT=0；fmt `--check` 净；全量 host 9304/0 EXIT=0 终态复证。**提交面 tidiness 观察**：收口 docs 变更因一次工具中断（`&` 把整条 `&&` 链后台化、链后半在父 shell 退出后仍续跑）落成同信息两笔 1b042dae（registry/CONTEXT/Cargo.toml/tools）+ 0ca94134（Cargo.lock/panic_guard 豁免），内容互补无冲突、终态正确——不改史（本仓纪律：宁新增不 amend），此处留痕。

**独立对抗复查轮（同日，HEAD 26e63b0e 之后；子代理八轴只读复查 + 主审逐条复验）**：**修复 4 项（P2×2 + P3×2，全在本卷新增面，代码功能 bug 零）**——① **假绿锁实锤并修**：判据②「挂起半边」针 `spin_loop` 同时现身宏 doc（lib.rs:24）⇒ 把宏体自旋换成裸 `loop {{}}` 测试仍绿（判据无牙）；针改为宏体独有全路径形 `core::hint::spin_loop`（分段拼接），变异 A 实证 `macro_body_locks_print_and_suspend_halves` 红 ✓。② lib.rs doc 两处与实际对不上：计数「21 bin 单点」订正 30（与判据③同源），「handler 定义权留在 bin crate 根」订正为「定义权在宏体、调用点在 bin 根」——本卷改的正是这条却把旧宣称留在 doc 里。③ 判据①正向针系子串匹配，**注释化调用行即可伪造命中**（假绿通道）→ 增「命中所在行前缀须为空或 `::` 且不含 `//`」语句性断言，变异 B（注释化 svc-mqtt 调用行）实证红且报错直书「假绿通道」✓。④ 判据③删一条被后一条完全支配的死断言。
**登记不修 2 项（观察，词形冻结优先）**：⑤ drv 五 bin 声化标签 `[svc-drv-blk]`（bin 名）≠ 其服务名注册值 `drv-blk`——静默先人无标签故零回归，且 `[<bin 名>]` 是全族 30 bin 与 WP-75.2.3 先例一致的冻结词形，改标签即破冻结；按服务名 grep 串口者需知此分裂。⑥ 16 bin 残留 `#![allow(clippy::empty_loop)]`（loop 已入宏体、宏宿主自带同款 allow）——惰性属性，删它需在 target 侧逐一复核其余 loop 面，收益零风险非零。
**对称性教训**：本卷给守卫文件自身上了「禁连续针形」自咬锁，却没给被扫对象（宏 doc）上「针须独出于代码」的同款纪律——①即此漏的产物；判据⑤/变异 A 补齐。**终态复证**：host 9304/0 EXIT=0、clippy `--workspace --all-targets -- -D warnings` 0、`fmt --check` 净、`sel4-build-rust.sh` EXIT=0；变异 A/B `cp`+`cmp` 逐字节还原；计数 +0（改判据不增测试数）。**范围外观察不改**：REG-75-D 旧注（1483 行段）对门词覆盖的表述与 `evidence/69/vol69.4` 转录实证相反，系卷七十五遗留、不在本卷面，留待 owner。

**二次独立复查轮（2026-09-27，HEAD `f0fae6a2` 之后；主审逐形**实跑变异**而非只读码）**：范围仍 99243508..HEAD 卷一全部改动（宏下沉 + 21 bin 声化 + 6 bin 收编 + panic_guard 五判据 + t-ladder-3 改针 + vol5 门脚本 ROOT 修复）。**代码功能 bug 零**——30 bin 调用形/宿主路径/标签逐字、宏体展开字节（`$crate::println!` ≡ `eneros_runtime::println!`）、手抄→宏词形恒等（`{info}` 内插 vs `{}, info` 两形产出同串）、orphan `PanicInfo` import 与其 cfg 行成对删除（全仓残余仅 svc-hang 一处=豁免族应存）、四族 Cargo.toml 零新依赖、串口门词覆盖由历史转录实证（`[svc-control-dispatch] PANIC: panicked at …` 11 次、`[svc-telemetry] …` 2 次命中 PANIC-SCAN `none_of`）、`tools/gate-criteria.toml` 五词表与 A 族/宏/probe 各出口对拍、`run70-vol5-gate.sh` ROOT 修复次序正确且同族脚本 `run{68,69,70}*.sh` 全扫零同类 premature-use。**判据锁面修 2 项（P2×2，两条假绿通道各自实跑到绿才算确证）**：⑦ **判据②整文件计数可被宏 doc 顶掉**——「doc 复述针形 + 宏体删半边」一增一减后计数不变 ⇒ 全绿；变异 ME（lib.rs 顶部 doc 提挂起针全路径形 + 宏体自旋换裸自旋）在 HEAD `f0fae6a2` 上实证 **19/19 全绿**（上轮只把针形换成「宏体独有全路径」= 点修，未上纪律，且「恰一次」单独不够：doc 顶掉宏体一处计数仍为 1）；修 = 扫描域锚定到宏定义之后（`macro_rules!` + 宏名分段针）再逐针恰一次，ME 复跑红 ✓，报错直书命中次数与两义（0=半边被删，≥2=宏体复述/双定义）。⑧ **判据①「首处命中 + 行前缀宽松（空或任意 `::`）」留两道假绿**——(a) 调用点回指**同名局部宏**：给 business-services 复刻一份静默 `svc_panic!`（正是本卷收编前的原病灶形）并把 svc-market 指回去 ⇒ 五判据全绿、host 编译无碍、target 照常链接，「单源」失守而无声（变异 MD 实证绿）；(b) 跨行块注释伪造行首语句（变异 MC 实证绿）。修 = 针形含宏宿主路径（`CALL_OPEN` 前缀入针）+ 冻结「独占一行」调用形（把 §3 步 1「bin 只留一行调用」从散文升为可执行），MD 复跑 **判据①③双红**（③同时红 = 目录内宿主限定宏调用计数 29≠30，双锁咬合）✓；(b) 经此修收窄为 ⑨ 的残余形（须整行逐字节等同语句才伪造得掉；该残余形 MC 二次复跑**仍绿**，实证与不修理由见 ⑨）。**新锁前置零误红核验**（动手前先跑，防次生）：30 调用点全为独占一行全路径形、四族目录内非宿主限定调用零命中、A 族五豁免与 svc-hang 静默断言不受影响、目录清点仍 31 文件。**登记不修 1 项**：⑨ 逐字节等同语句的一行被跨行块注释包裹时正向命中仍可伪造——但该形下 bin **无任何 handler**（target 链接期 lang item 缺失硬错，不可静默出货），真要无声回流须再配合 `#[cfg_attr(_, panic_handler)]` 别名形 = 双重刻意伪造而非漂移形态；补注释剥离器的收益不抵其字符串字面量误剥风险。**范围外观察不改 2 项**：⑩ `protocol_ladder.rs` t-ladder-3 的宏调用针仍是弱形（不含宿主路径），但同 11 bin 已被 panic_guard 判据① 强锁覆盖，冗余面不再叠一份强锁；⑪ `gate-criteria.toml` PANIC-SCAN 注释宣称词表为「本仓 panic 出口全集」（73 批C），而 `crates/runtime/oom-probe/src/main.rs:147` 的 `[oom-probe] panic — suspending` 五词皆不命中 ⇒ 73 遗留宣称有缝，属 tools/ 判据面留 owner。**QEMU 三门义务不复跑理由**：本轮改动通篇 `#[cfg(test)]`（panic_guard.rs），不进 target 镜像也不改 bin 二进制，37013bdf 轮 VOL70.5/6/7 三门 PASS 与跟踪面证据八文件对终态仍成立；target 面复跑 `sel4-build-rust.sh` 作 EXIT 复证。**终态**：host **9304/0 EXIT=0**（计数 +0，改判据不增测试数）、`fmt --check` 净、clippy `--workspace --all-targets -- -D warnings` 0、target EXIT=0；变异 ME/MD/MA/MB 施加与还原一律 `cp`+`cmp` 逐字节核验（禁 `git checkout`），每轮变异后 `git status` 留档确认树净。

## B76-2. 蓝图 76 · 修复卷二登记（2026-09-27，块命令 wire 与 MGT1 帧文法单源）

> 来源：第十七轮架构体检（2026-09-26）候选 1（Strong）+ 候选 6（Worth exploring）+ 17B 快照同族小口径；蓝图 `蓝图/76-架构体检R17候选清偿蓝图/02-修复卷二-块命令wire与MGT1帧文法单源.md`。基线 HEAD `09cd59b2`（卷一二次复查轮终态），v0.346.0，全量 host 9304。主题：两份「帧文法无主」各收敛为单一 codec module，host 获得判别力；**零 wire 字节移动**（移动的是布局知识的存放地）。

| ID | 面向 | 状态/描述 | 源 |
|---|---|---|---|
| REG-76-G | 块命令帧：1 服务端（`block_cmd.rs` 57 处逐偏移手解手装）+ 1 客户端（agent-sdk `block_io.rs`）+ 4 host 替身（business `SimStorage`、control-services `test_support.rs`/`telemetry_history.rs` 两 `FakeStorage`、agent-sdk `SimStorage`/`HealthMock`）→ 单源 | **Done（WP-76.2.1~4，提交 982347db / ac124712 / 9a74a95b / 6ac0c0b4）**：`drv-svc::blk_wire` 承载 body 布局（数据面字节偏移 / boot 面扇区两单位并存写进模块 doc 首行 + 禁第三单位）+ `body_of`（信封→body 唯一桥，服务端私有助手退役）+ `LenErr::Short` 窄错误面。golden 先行硬序达成：WP-76.2.1 在**零生产改动**下补 6 枚逐字节锁（全子码请求+响应，值由现服务端 `--nocapture` 跑出固化而非手算；此前仅 GUARD 40B 一枚锁，数据面构帧与全部解包方向零锁）。**替身弱门修复**：#1/#4 收编后自带此前缺失的长度门 + count 域界（旧形短载荷直切 `payload[9..13]` 是 panic 面、超 461B 会越 `resp.payload` 界），拒绝形态各替身惯例保形（status 语义 vs transport Err 不统一，见 REG-76-J）。§2 步 3 短帧负例族补齐（2cb5cdf2）：四道长度门各喂「差一字节」帧（12B READ / 8B WRITE / 24B TRIAL / 25B CONFIRM）+ 四子码空 body 退化形，一律 ST_INVALID 且零半解载荷；写该测时实测得出一条此前未登记的门次序事实并顺手钉桩——**boot 子码的能力门（无安装面 → ST_UNAVAILABLE）先于长度门**，故 boot 面长度负例必须挂安装面才落在 ST_INVALID（首轮断言即因此左 4 右 5 而红，是本测夹具漏装配、非产品缺陷；次序收编前后未变，现由独立断言固化）。DoD 复证：生产代码手抄偏移全仓零命中（残余仅测试夹具独立字节断言与注释内史形）；删除的 `println!` 行 = 0（串口词形零变化，`BOOT-TRIAL: RX req_len=` 打印仍在长度门之前）。变异双向咬合：MA（`parse_read` count 段移一位）→ drv-svc 7 红含金锁，替身收编**前** agent-sdk 全绿（假独立）、收编**后**转红 2 测；MB（`pack_write_prefix` 改大端）→ 客户端构帧金锁红——两变异均 `cp`+`cmp` 逐字节还原 | 蓝图 76 卷二 §2~§5 |
| REG-76-H | MGT1 管理帧文法：消费 bin（svc-mgmt v1/v2 两臂）与 root 探针（`mgt_probe::build_frame`）各拼一遍偏移，导出常量面板无人消费（外部仅 1 处局部别名） | **Done（WP-76.2.5~6，提交 af7a1d12）**：文法入 no_std 叶 `eneros-mgmt-api::mgt1_frame`（常量面板 + session/token/签名字段偏移 const + `classify_session`/`SessionShape` 三臂互斥完备 + 借用访问器 `v2_tail`/`request_id`/`action_len`/`action_and_arg`/`v1_session`/`token_view` + 构帧器 `build_token`/`build_sig_field`/`build_v1_frame`/`build_v2_frame`/`build_truncated_v2_frame`），验签 `verify_v2_tail` 与 `AuthenticatedSession`（ECDSA+时钟+alloc）**刻意留** business-services，`mgmt_principal` 以 `pub use` 保公共名零破坏。root-task 加 `eneros-mgmt-api` 依赖（零环理由写进 Cargo.toml 注）。DoD 偏移字面清零复证：`[16..20]`/`[24..40]`/`[40..48]`/`[48..56]`/`5 + SESSION_LEN`/`5 + 48` 在 svc-mgmt 与 mgt_probe **全零命中**；状态字节两份逐字重复收编为 `RpcStatus::wire_byte`（映射值保形）。**门安全**：五处串口 `println!` 与 `ERR:principal-unverified`/`ERR:time-unavailable`/`ERR:principal` 字面在 diff 里**增删行各 0**（vol6 R3/R9/R16 整行 grep、R19/R20、vol5 R6 依赖）。探针构帧**先行金锁 4 枚**（GET1 223B / CONFLICT 213B / FORGED / TRUNCATED 76B）在重构途中**实际咬红一次**（token channel 段与 expiry 段边界挪 1B），修的是代码不是金锁——门外 `tools/run70-vol6-gate.sh:97` 以 Python 重签 `SHA-256(token)`，字段挪位只会以门红暴露、不可能以编译错暴露；本轮另对该金锁做了逐段独立解码复验（与门外签名器 `magic‖principal16‖roles4‖tenant4‖channel16‖expiry8‖issued8` 同构）。互锁测（构帧 → 形状判定 → 访问器往返，v1/v2/短尾三态）把「探针-解码器镜像分叉」从不可测转为 host 可测（§7 步 5 的牙齿）。`mgt1_frame::TOKEN_LEN`(64) 与 `auth.rs::TOKEN_LEN`(113，SM2) 同名不同物 ⇒ 禁 crate 根 glob 再导出 | 蓝图 76 卷二 §6/§7 |
| REG-76-I | 17B 快照钉请求帧文法（服务端三臂各内联一遍 `req.len() != 17` + `req[1..17]`；客户端半边 C3 已收编于 dispatch_wire） | **Done（WP-76.2.7，提交 b465be4e）**：`tel_wire::parse_snap_request` + 帧长单一事实 `SNAP_REQ_LEN`，构帧侧与解帧侧同源；三臂**应答形状刻意不并入**（BLOB=`SNAPSHOT_OK‖bytes`/`REJECTED`、HOLD=OK/ERR、RELEASE 恒 OK 幂等），`TEL_REQ_SNAPSHOT_INPUT` 的 2B 拒绝文法另族不并；解析器不校验 op 首字节（三码共用 body，op 分派留调用方 match）并立负例锁防「好心过滤」。蓝图 §1C 未点名的第 4 处手抄本轮查出并收编（`certifier_service.rs:195` `with_capacity(17)`，cfg(none)+sel4 区，host 不可编、随 target 构建复证）。+4 测（往返 / 16·18·0 长度负例 / op 无关性 / 逐字节布局冻结 / client∘parse 两侧互锁）。DoD：telemetry.rs `!= 17` 与 `[1..17]` 零命中；agent-sdk 199→202、control-services 223→224 全绿，fmt 净、clippy `-D warnings` 0。变异三发：帧长改 16 被冻结字面在编译期咬死、id 偏移移位 → **新锁红而旧「双面锁」仍绿**（本 WP 补的就是这颗牙）、长度门 `!=`→`<` → 18B 负例红 | 蓝图 76 卷二 §8 |
| REG-76-J | 替身错误形态统一（#1/#4 status 语义 vs #2/#3 transport Err） | **Open（登记不修）**：各替身错误形态是其测试族的既有契约（越界=Err 的夹具被 `expect_err` 消费），统一=大面积改测试断言、收益零。本轮只收编**布局**（同 blk_wire 单源），形态各自保形并写进替身注释。施工门：新增替身时直接走 blk_wire + 选定一种形态并 doc 说明 | 蓝图 76 卷二 §10 |
| REG-76-K | `BLK_BOOT_STATUS` 响应文本全字节 golden | **Open（登记不修）**：词形已被 `update_status_format_is_deterministic` 锁（期望行 `seq=… floor=… A=… B=…`），且本轮生成实跑取到 51B 全字节形态（`seq=5 floor=1 A=15/1/0/1 B=0/0/0/796025588171149586`）——文本载荷随 bootctrl 事实变（B 域 trial 后 secver 极大），全字节锁会把某一合法事务形态钉死。施工门：STATUS 载荷格式变更（破坏性版本）时补 | 蓝图 76 卷二 §10 |
| REG-76-L | `RpcStatus` 归属整理（蓝图 §7 步 2 假设「若在 mgmt-api 则就地」） | **Open（登记不修，前提已订正）**：对码亲验 `RpcStatus` 唯一定义在 `business-services/src/mgmt_rpc.rs:155`、mgmt-api 内**零命中**——蓝图首选路径（映射入 mgmt-api）前提不成立，故状态字节单源落在 enum 宿主（`wire_byte`），两臂共用、映射值 Completed=1/Accepted=2/Failed=0 逐字保形；不为省一个函数把 enum 跨 crate 搬家（波及面 >> 收益）。施工门：svc-mgmt `TargetExecutor` 下沉（R16-I 后续）时一并 | 蓝图 76 卷二 §7/§10 |

**前提复验订正（动手前对码亲验，六处）**：① **登记号撞车**——蓝图 §9 指定本卷用 `REG-76-E/F/G`，而卷一收口已把 D/E/F 用尽（A/B/C Done + D/E/F 登记不修，见 B76-1 表）⇒ 本卷整体右移三位：Done 三项 = **G**（blk_wire 单源 + 替身弱门）/**H**（MGT1 文法归位 + 探针互锁）/**I**（17B 快照单源），登记不修三项 = **J/K/L**。② **路径漂移 4/6**——`svc-mgmt.rs` 实在 `business-services/src/bin/`（非 `bin/`）；root-task 实在 `crates/runtime/root-task/`（`crates/kernel/` **不存在**）；`telemetry.rs` 与 `dispatch_wire.rs` 实在 **control-services**（蓝图分别记作 business-services / agent-sdk）。行号窗口（svc-mgmt 1048-1230、telemetry 1059-1097）仍对得上，错的只是文件归属。③ **§2 步 1「BLK_BOOT_CONFIRM req（slot=1）+ resp seq」在 block_cmd 夹具面不可达**——`InstallFace` 只建模 A 运行身份，slot=1 卡 `confirm-secver-mismatch`、slot=0 卡 `confirm-slot-not-running`，改 secver/id 对齐后过身份门又卡记录重编码（`encode`）；seq 成功臂属 boot_install 事务域（API 级锁 `confirm_marks_successful_and_sequence_advances` 已存）。故本卷锁 CONFIRM **请求 26B 字节形** + seq **打包字节形**（TRIAL/CANCEL 两臂成功 seq 已锁，同一处代码），偏差如实登记不硬凑夹具。④ **§7 步 2 的 `RpcStatus` 归属假设不成立**（详见 L 行）——enum 在 business-services 而非 mgmt-api，映射单源落宿主。⑤ **§6 常量面板与 mgmt-api 既有 `TOKEN_LEN` 撞名**——`auth.rs:16 TOKEN_LEN = 113`（SM2 token）≠ `mgt1_frame::TOKEN_LEN = 64`（MGT1 token），故 `mgt1_frame` **不得在 crate 根 glob 再导出**，消费方走模块路径（否则 E0252 同名歧义）。⑥ **手抄点实为七处而非「五处」**——§1A 表列 5 处 + 括注 agent-sdk 测试内 `SimStorage`/`HealthMock` 2 处未计入编号；另 WP-76.2.7 施工中新查出**第 8 处**（`certifier_service.rs:195` 快照 blob 请求 `with_capacity(17)`，cfg(none)+sel4 区、蓝图 §1C 未点名）。全部随本轮收编。

**次生面自查（动手即查，五处）**：① **golden 夹具的响应复用假象**——首次生成 hex 时 `FLUSH_RESP` 显示 `len=16`（沿用上一请求 READ 的残留 `payload_len`），对码 `runtime.rs:486` 确认**生产侧每请求新建 `DriverResponse`**（`let mut resp = DriverResponse::new(0, ST_INVALID, epoch)` 在服务循环体内）⇒ 非产品缺陷，是夹具形；本卷 golden 一律每断言新建响应，并把「FLUSH/WRITE 成功响应零载荷」写成显式断言（将来若引入响应对象复用，此断言即红）。② **收工判据的一处覆盖缺口（判据补强，后续卷次同用）**——`third_party/sel4-build-rust.sh:53` 只 `cargo build -p eneros-root-task`，蓝图 §0.5「svc-mgmt bin / mgt_probe 改动在 target 区 ⇒ 收工跑该脚本」对**服务 bin 的 cfg(none) 面并不覆盖**（svc-mgmt/svc-ota/certifier 全不在其射程）。本轮补真编：`cargo build -p eneros-business-services --bin svc-mgmt --bin svc-ota --target aarch64-unknown-none --release --features sel4` 与 `-p eneros-control-services --bins` 均 EXIT=0；且需先 `source tools/gate-env.sh`（否则 `eneros-sel4-sys` 构建脚本找不到 `aarch64-none-elf-gcc`），另外**手工 export RUSTFLAGS 会破坏该构建脚本**（host 侧 cc 探测也被 RUSTFLAGS 波及）——只走 gate-env 一条路。③ **替身「假独立」实锤**：变异 MA（`parse_read` count 段移一位）在 WP-76.2.4 之前对 agent-sdk 族**零命中**（其 SimStorage 自解一遍布局，漂移自洽），收编后转红 2 测——「替身与生产同布局源」的可测含义由此确立，也是本卷把四替身列为施工面而非附属品的理由。④ **MGT1 构帧金锁咬红一次**（token channel/expiry 段边界挪 1B），修代码不改金锁，详见 H 行。⑤ **WP-76.2.7 附带查出蓝图未点名的第 8 处手抄**（`certifier_service.rs:195`），已随收编；同族检查另以全仓 grep `payload[1..9]`/`payload[9..13]`/`!= 17`/`[1..17]`/`OFF_BYTES` 复扫，生产侧残余为零。

**收口核算（卷二）**：全量 host 9304（卷一终态）→ **9330**（+26，实数）。分族现状（收工实测）：drv-svc 61、agent-sdk 202、control-services 224、business-services 177、mgmt-api 52、root-task 148（三门 PASS 后追加的一枚 **test-only** 短帧负例族使 drv-svc 60→61、总数 9329→9330，提交 2cb5cdf2：零生产代码改动 ⇒ 已录门证据仍归因有效）——注意 `classify_session_covers_truncated_v2_gap`（70.5 复查 P0 短尾回归锁）随文法**换主**（business-services → mgmt-api），故分族增量之和不必等于总增量。`cargo fmt --all --check` 净、`clippy --workspace --all-targets -- -D warnings` 0、target 侧真编 root-task + `svc-mgmt`/`svc-ota`/control-services `--bins`（aarch64-unknown-none + sel4）EXIT=0。**QEMU 三门义务全 PASS、判据零红**：`VOL70.7-GATE: PASS`（16 判据，BLK_BOOT_* wire 功能面：GUARD 查询 / trial 事务提交 / 确认后不回退 / 身份页贯通 / 同盘再启载荷身份一致）、`VOL70.6-GATE: PASS`（27 判据，MGT1 整行 grep 族 `rid=10 config-get -> Completed gen=1`、`rid=260 … gen=2`、`rid=216 config-set -> Failed idempotency-conflict`、`MGT-PRINCIPAL rejected (truncated v2 shape` 全部命中且由文法收编后的代码打印）、`VOL70.5-GATE: PASS`（19 判据，含 R6 `MGT-PRINCIPAL rejected`）。三门合计 **62 判据 0 红**，各含「零 PANIC / 零分配失败」判据。

**首轮 vol7 假失败分诊（复跑定性，非代码回归）**：首链 vol7 窗 T-2 串口日志仅 150B（`UEFI firmware …` 之后零输出）⇒ `xtask: FAIL: QEMU QMP shutdown: readiness marker not found (NO_RESULT)`，R6-R12 连片红，而 T-1 窗 R1-R4 全绿（**恰好是本卷改的 GUARD/TRIAL wire**）、后续窗 R13-R15 亦绿（确认成功才会有「确认后不误回退」）。定性手段 = 静默树单链复跑 vol7：**16 判据全绿 PASS**。同轮六文件 md5 指纹在 vol7/vol6/vol5 三轮之间**逐字节一致**（`/tmp/gatechain-*.fingerprint`），故首轮红与源码状态无关（`71ec2dbf` 提交说明里那条「两行落在门链窗口内」的归因顾虑**据此作废**——两行在链条启动前已在树内）。附带复证一处判据器缺陷（与本项目既有登记同族，属 owner 定案面不改）：**「零 PANIC」型判据对空/截断串口日志恒绿**——首轮 T-2 的 R5 正是在 150B 日志上判绿的假绿；同窗 R6+ 因需要内容而红，才让污染暴露。

**独立对抗复查轮（2026-09-27，HEAD `004b5290`；子代理双轴只读复查 + 主审逐条复验并实跑）**：范围 09cd59b2..HEAD 卷二全部 12 提交。**代码功能 bug 零**——复查逐臂证明 wire 等价（`body_of` 的门与旧 `payload_len<13/9/25/26` 双向等价、WRITE 借用段 ≡ 旧 `payload[9..payload_len]`、`slot_is_a = p[0]==1` 保形、`probe_health` 的 `<13→None` ≡ `parse_health`、超长 `payload_len` 由 `runtime.rs:173/226` 单点拒绝到不了任何切片）、`println!` 字面增删 0、`mgt1_frame` 的 panic→Option 新臂全部可证不可达（`classify_session` 下界 62/200 覆盖 53/191 需求）、`mgmt_principal` 十个公共名同值可达。**修判据面 4 项**：① **两枚"请求构帧形"金锁实为无牙**（只把手摆的帧与手写的字面对拍，既不驱动服务端也不碰 `blk_wire`——族头"任何 wire 布局移动都在这里变红"的宣称对它们为**假**）→ 每枚末尾补 `pack_* == 本帧 body 段` **配对断言**，并以变异实证（`pack_read` count 段移位 ⇒ `blk_request_bytes_golden_all_subcodes` 由绿转红 @:925），族头宣称同步订正为「两类成员牙齿不同，别混称」。② **两处字面各抄一份可静默分叉**（`blk_wire` 测试族的 `GOLDEN_*` 与其 doc 里"分叉则 block_cmd 族先红"的宣称不成立，二者互不引用）→ 配对断言即闭合该通道，宣称改写。③ **`payload_len == 0` 覆盖宣称失真**（`mk_req` 恒置 1，dispatcher 先拦 0，`body_of` 永不见 0 长请求）→ 注释改写为「覆盖的是只带子码形」，并**补一条 dispatcher 级真断言**（零长请求 → ST_INVALID）把该前提本身钉住。④ **无判别力断言**（agent-sdk 金锁里 Recorder 恒填 `0x5A`，`assert_eq!(buf, [0x5A;16])` 被自身满足）→ 改为应答字节取决于**请求偏移**（`off as u8 + i`），读回段解错偏移即红。**登记不修 2 项**：⑤ control 两替身收编时被我顺手加上的**域界门**（`n>0 && n<=BLK_READ_MAX`）严格说超出蓝图 §3/§5 口径（域规则本应留在生产调用方），今日**零测试经过其拒绝臂**（其夹具帧全部经 `BlockIo::cmd` 构造，长度恒合法）——保留理由 = 消除 `resp.payload[..n]` 越界 panic 面，登记为「防御性、暂不可测」，新增替身时按 §10-J 的口径统一。⑥ root 探针三个 builder 现 `.expect(action ≤ 255)`，旧形是 `action.len() as u8` 静默截断——全部调用点用 ≤11B 字面量，不可达；两者皆非正解，选"响"不选"静默错帧"，登记不修（若将来探针要发长 action，须在文法层正解）。**订正本轮自己的登记**：⑦ 前述「路径漂移 4/6」**夸大**——蓝图原文只把 `svc-ota/svc-mgmt` 少写了 `src/`（真错 1 处），`root-task/src/mgt_probe.rs` 原文写法本身没错，`telemetry/dispatch_wire` 系未限定归属而非写错；⑧ §10 行内引用 `update_status_format_is_deterministic` 的位置应为 **`boot_install.rs:877`**（原文记作 block_cmd.rs:886），不修理由不受影响；⑨ 前文「服务端私有助手退役」措辞失真——该助手是本卷 76.2.3 自己引入、76.2.4 才并入 `body_of`，基线上从未存在，改写为「本卷内自收敛」。**DoD 复核定性**：WP-76.2.4 的 DoD 原文（`payload[1..9]` 四文件零命中）按字面**未达成**（agent-sdk 金锁族内 5 处），但那些是「独立参照」字节断言、改为引用单源即失金锁意义——登记为口径改写而非达标；`boot_install.rs` 的 wire 表系纯叙述（无切片），已加单源指针防后来者当定义处用。**终态**：drv-svc 61 / agent-sdk 202 / mgmt-api 52 / business 177 / control 224 / root-task 148，host **9330/0**（+配对/零长请求均为既有测内新增断言，计数不变）、`fmt --all --check` 净、clippy 0；三门 PASS 证据不受影响（本轮改动通篇 `#[cfg(test)]`，不进镜像）。

## B76-3. 蓝图 76 · 修复卷三登记（2026-09-27，bootctrl 提交单源、untyped 记账单源与控制链不变量收口）

> 来源：第十七轮架构体检（2026-09-26）候选 3（Strong，含掉电写序裁决）+ 候选 7（Worth exploring，守卫缺失坐实）+ 候选 4（Strong，62.1-A 存量缺口）+ 候选 5（Worth exploring）；蓝图 `蓝图/76-架构体检R17候选清偿蓝图/03-修复卷三-bootctrl提交单源与不变量收口.md`。基线 HEAD `7da193ec`（v0.347.0，卷二终态，全量 host 9330/0，工作树净）。**四病同源**：同一不变量/同一协议手写多份，纪律活在注释与约定里、不在代码结构里。

| ID | 面向 | 状态/描述 | 源 |
|---|---|---|---|
| REG-76-M | BOOTCTRL 双副本提交协议三份手写（trial/confirm/cancel）+ 写序两派 + 4 段重复栈注释 + `lba_range_of(BootCtrl)` 生产 4 处 + 字节/扇区单位双源 | **Done（WP-76.3.1+3.2，提交 39a8cc1e / 0f443c65 / c35aa9d2）**：新增 `BootInstall::commit(high_copy, next, tag)` 私有单源——encode → **高序列副本先写** → 低副本 → flush，三事务尾部全量收编（字段变更/槽裁决原地不动，各自是事务语义不是协议）。`read_current` 的 `high_copy` 在 confirm/cancel 从丢弃 `_` 改为消费；生产侧 `lba_range_of(BootCtrl)` 4→1（新 `bootctrl_lba()`，读面与 commit 各引用）；4 段逐字栈注释收拢为 read/commit 各一份；单位双源上编译期锁 `const _: () = assert!(BOOTCTRL_COPY_STRIDE_LBAS * 512 == BOOT_CONTROL_COPY_OFFSETS[1])`。**写序判据三条（复查轮收紧，非蓝图字面那句）**：`read_current` 第二值是「应先写的副本」而非「较高序列副本」——① 双可解 → 先写较高序列侧（较低侧通常正是锚匹配副本；平局取 copy0）；② **仅一侧可解 → 先写损败侧**（幸存侧是当前唯一可引导事实，先覆它 = 任一中断点全盲）；③ 双不可解 → 读面即拒。蓝图 §3 步 2 与固件注释那句「高序列副本先写」只覆盖①，按字面收编会在②形态下把**收编前原本安全**的 confirm/cancel 改危险（且②的镜像形态（copy1 损）在 trial 里是**存量缺陷**，本轮一并闭合）——蓝图 §1A.4 自己判定该形态「首写落无效副本、锚匹配幸存，安全」，故以蓝图该条判定为准偏离其收编指令。锁：`commit_writes_undecodable_copy_first_when_one_side_is_sole_survivor`（两好侧 × 三事务 = 6 臂，首写落点 + 掉电后真实 `resolve_boot_control` 双重断言；把②改回字面高序列 ⇒ 12 条违规逐臂显形，其余 16 测仍绿 = 本测覆盖的是独立一轴而非重复断言）。**写序分歧的性质判定**（立卷执行实证 + 本轮可达性推演复述，勿再重复上报）：`resolve_boot_control` 注释明文断言「运行时写序 = 高序列副本 → 低序列副本 → flush」，trial 满足、confirm/cancel 违反；构造 {copy0=锚匹配 R1, copy1=更高序列 R2} 后 confirm 首写覆掉唯一锚匹配副本，两写间掉电 → 真实固件侧解析 `AnchorMismatch` = 引导失败（trial 同状态安全回退）。**当前系统不可达该形态 ⇒ 潜伏防御缺口，非现行故障**。红测永久化（WP-76.3.1）：`confirm_and_cancel_write_high_seq_copy_first` + `power_loss_between_writes_preserves_anchor_match`（掉电重放 helper 把一次性探针变常驻锁）——合入轮 confirm 臂现状红（`left: 18432 / right: 18440`、`AnchorMismatch`）如实登记，WP-76.3.2 转绿，未注释掉任何红臂。插桩对称化：trial 四行词形逐字节保形（tag 参数化），confirm/cancel 得新增 `BOOT-CONFIRM`/`BOOT-CANCEL` 同族四行——vol7 R9/R10 grep `BOOT-CONFIRM: slot=B secver=`（svc-ota 侧）与新行零冲突，门 PASS 实证。**偏离报告原案登记**：体检报告建议 `BootctrlMedia` adapter 类型，立卷裁决落为私有方法（无第二消费者 / 介质访问已由 `InstallBackend` face 承载 / 新类型零增益）；本轮再偏离一处——蓝图标名 `commit(current, next, tag)`，实作为 `commit(high_copy, next, tag)`（`current` 在本函数内无用途，写序输入是 `high_copy` 而非旧记录本体）。次生：`read_current` 公共签名未变 ⇒ `block_cmd.rs:240` 消费面零改动；blk_backend 影子层「整条 4096B 记录落副本基址」形状契约不变（commit 产出仍是整条记录写）。**追加判别力修复（c35aa9d2，自查所得）**：两测原形 `for name in [trial,confirm,cancel] { assert }` 在第一臂红时 panic，cancel 臂从未被观察（合入轮实测即如此）——改单臂 helper 返回违规描述 + 汇总断言，变异复跑（commit 强制 copy0-first）证三臂逐条独立显形。**门义务达成**：vol7 R1~R16 全绿 + 四行新插桩在窗内实际显形（`evidence/70/70.7/vol76-3-gate-dbg-zero-diff.txt` §一） | 蓝图 76 卷三 §1A/§2/§3 |
| REG-76-N | untyped freeIndex 记账四份手写，其中 `alloc_contiguous_pages` **缺红线 6 守卫**（`1u64 << desc.size_bits` 无护栏） | **Done（WP-76.3.3，提交 c3c823ab）**：新增两私有单源——`align_up(i, obj_size)`（alignUp 算术 + 红线 6 守卫）与 `fit(i, obj_size, need)`（capacity → alignUp → `aligned+need ≤ capacity` → `used` 递增）；三 bump 口（`alloc`/`alloc_contiguous_pages`/`alloc_cap_table`）记账段全换 fit，各自 skip 链（is_blocked/is_device/size_bits 充分性/RAM-top/CNode 槽位 bits）原地保留（策略判据非记账模型）。守卫缺口就此闭合；`:1458`「与 alloc 完全一致的 alignUp 记账」注释删除（其声称由结构兑现——立卷订正：该注释声称的是**模式**不是粒度，模式本就一致，分歧只在 obj_size 推导）。`alloc` 请求侧 `1u64 << size_bits` 加循环不变量前置守卫（收编前由 desc 侧守卫顺带兜住，删之则成新的移位 panic 面）；`alloc_cap_table` 的 `size_bits+5 ≥ 64` 判据提到循环外（净效果恒等）。**`alloc_device_at` 刻意不并入**（`used = max(used, off+size)` 是「标记已用」的物理定位分配，与 bump 切分不同模；doc 互链，另立 REG-76-R）。新 3 测（root-task bin 148→151）：守卫负例 bits=64/65/128 × 三口一律 None 零记账（兼「守卫吸收」回归锁）/ 守卫必须跳过而非中止整轮 / 对齐矩阵（used=4097 跨粒度边界 × 三口 × capacity 恰好与差一）。变异双向实跑：删 `align_up` 守卫 → 两测红（`attempt to shift left with overflow` = 缺口可观察形态）；alignUp 退化直接累加 → 矩阵红 + cap_table 断言红。定性同立卷：**防御性收口**（bootinfo 给不到 2^64 量级），非现行故障。**实弹零变化证明达成**：vol7 三窗 `DBG dma-alloc pick …` 8 行与卷二 PASS 轮基线 **sha256 全等**（`709dbe6e…1418`，四份抽取同哈希）——重排零结果由串口直证（留档 §二）。次生：`used_bytes`/`allocated()` 统计语义不变；fit 私有无 pub 面。**全局复查轮补充（2026-09-27）**：contiguous 内 `align_up` 失败语义由旧「`?` 中止整轮遍历」变为「跳过该候选 continue」——分歧域仅 `used+4095` 溢出 u64（capacity ≤ 2^63 时不可达），与守卫「跳过不中止」同向，有意为之 | 蓝图 76 卷三 §1B/§4 |
| REG-76-O | `SettleTrigger::Applied` 表行是装饰——engine 手写两相不读 `settlement.durable`，「改表不改行为、改行为要记得改两处」（62.1-A，自蓝图 62 复查登记） | **Done（WP-76.3.4，提交 b9e0d6d3）**：Applied 臂改为**遍历 `settlement.durable` 驱动写序**，`sm3(回执原文)` 作为 Journal 臂的运行期供给（表是模板、摘要不可静态入表；耦合边界写进 `SettleOp::Journal` doc——Applied 触发下 Journal 臂统一带 digest，若将来出现第二个不该带的 op 须在 op 上标注）。观测面六步逐字节保形：durable 两写 → `push(ReadbackVerified)` → 证据行 `EXEC_JOURNAL_APPLIED_CONFIRMED` → `last_confirmed` → `gate.record` → `push(Committed)` → `count`（audit_chain 冻结面 + 门 grep 依赖此序）；`AppliedDurableFailed`/`AppliedAdvanceFailed` 嵌套机制与 err 词形不动。新锁 `applied_write_order_follows_table_row`（engine.rs 内单测——表函数 pub(crate)，集成测试取不到）：① 表序断言（首臂 Intent(Committed)、次臂 Journal(TransportAcked→AppliedConfirmed)）；② 源结构针（Applied 臂须含 durable 遍历、须不含手抄状态对；针形运行期拼接免自匹配）。修前形态以文本比对实证（旧臂无遍历 + 有手抄状态对 ⇒ 该锁修前红）。**「表获得牙齿」最终实证 = 表行两臂换位变异**：端到端因果门 `applied_write_order_durable_ack_failure_never_confirms_journal` 转红（settlement_lock 6 passed/1 failed）+ lib 两锁红；收编前同一变异零行为变化（平行宇宙实证）。settlement_lock 的 62.1-A 自首注释更新为「缺口已闭合，本测转端到端双保险」。次生（如实登记）：`ctx.response.expect` 取值点从 intent 写之后提到遍历之前——None 时唯一差别是 panic 早于 durable 写（本就 panic 的路径，零半提交）。control-dispatch lib 计数并入 P 行口径（87→88 本项 →96 累计） | 蓝图 76 卷三 §1C/§5 |
| REG-76-P | `run_typed` 一批五扫、幂等键重算（SHA-256 摘要实算 2 遍、correlation 三处派生）、判定与计数分离 | **Done（WP-76.3.5，提交 68246772）**：新 `pub(crate) mod prepared`——`classify(batches, plan_id, legacy_single, journal, now)` 纯函数一遍扫描产出 `PreparedPlan{steps, involved, conflict, unresolved}`，run_typed 只消费不再判。`intent_journal_identity` 每 intent 一次（engine.rs 内引用 5→0）；时态判据收进 `kind_at`（原投递体内两行 `if`）、replay_only 聚合收进 `PreparedPlan::replay_only`（原扫③手写 `all(...)`）；次序保形：冲突拒绝 → 未决回执早返回 → 逐设备仲裁。**红线④裁决**：分类一次 = 时点前移，故投递点对非 Expired 步按 fresh now 复检一次（`execute_core` 走真实 IPC，窗口可在分类与投递之间关闭）——省掉即把 Deferred 步永久判死；`intent_window_estimate` 仍吃投递时刻窗长。**幂等红线（§1D）兑现**：`PreparedStep` 不携带任何执行功率字段，摘要只吃 `intent.power_kw()`，与执行面 `power_override_kw` 的分叉保持并上锁（`identity_binds_planned_power_in_digest_not_key`：键不含功率、义含）。新 8 测全部 host 表测（不搭引擎夹具 = 本 WP 收益点）：窗口边界 / 复检三态 / 涉备集首现序 / Current 命中+未决+聚合 / 冲突与无摘要哨兵 / 键义红线 / legacy 键口径 / correlation 单源负证（源结构针）。三处偏离蓝图表列，如实登记：① `StepKind` 三态非四态（`Replayed` 与时间轴正交，并枚举 = 再造「表行装饰」，命中半边由 `replay_current` 承载）；② 不单列 `correlation` 字段（它恒等于 `identity.0`，单列 = 同一事实表两遍）；③ 不携带 `classified_at` 字段（lib 构建零消费点 = 死面，删除）。行数如实登记：**run_typed 362→353（-9，非蓝图预期 -80~120）**——搬出的是判据不是篇幅，扫①②③ 62 行→8 行消费、投递段因三态 match + 裁决注释变长把净减抵掉；prepared.rs 生产面 145 行（含 doc）。变异三发实跑：复检退化 → 复检测红；`all→any` → 聚合测红；identity 摘要丢语义 → 命中/legacy 两测红。控制链无本卷门窗（vol7 是 OTA/引导面）⇒ 按 §0.4 host 收口 + 登记（留档 §三）。**红线⑥字面偏离（全局复查轮 2026-09-27 补登记）**：蓝图要求「deferred/expired 计数从分类聚合」，实现为**投递点** fresh now 的 `kind_at` 判定臂内计数——与分类共用同一判据函数（同源达成，REG-70.FL「看不出来」病灶同样闭合），且计数≡投递实决（分类 Due、投递前已过期的步不会被误记 delivered；按字面从分类时点聚合反而引入计数漂移）——**勿按蓝图字面改回分类聚合** | 蓝图 76 卷三 §1D/§6 |
| REG-76-Q | Applied 全 effects 化（PushState/Evidence/GateRecord 枚举进表） | **Open（登记不修）**：观测面顺序已被端到端锁 + audit_chain 冻结面锁死；枚举化使表行膨胀、每加一观测面要动表类型——收益/成本倒挂。施工门：settle 表新增复杂触发族（新结算观测面）时重估 | 蓝图 76 卷三 §8 |
| REG-76-R | `alloc_device_at` 并入 `fit` | **Open（登记不修）**：物理定位分配（max 标记已用）与 bump 切分不同模，强行同槽 = 错抽象；本轮已在 doc 面互链两义。施工门：出现第二个「定位+标记」分配点时抽 `mark_used_at` | 蓝图 76 卷三 §8 |
| REG-76-S | `run_typed` 路由预检与扫④的 `route_index_for` 二查合并 | **Open（登记不修，理由与蓝图相反）**：蓝图判「PreparedStep 携带 route_idx 后二查自然消除」——实测**不可消除**：预检在 held-plan 分支**之前**（多步计划在那里换道，分类面若前置携带 route_idx 则为永不投递的步做无用查表，且 fail-closed 拒绝面次序被改）。本轮只在二查处注记两查各司其职。施工门：预检与投递合并为单趟且 held-plan 换道点后移时一并 | 蓝图 76 卷三 §8/§6 |
| REG-76-T | blk_backend 三处 `lba_range_of(BootCtrl)` 收编 | **Open（登记不修）**：影子区段几何是其独立关注点（部分写回读重建影子），与事务协议无关；换算已单源（`sector_byte_offset`）。施工门：blk_backend 增第二区段族时一并 | 蓝图 76 卷三 §8 |
| REG-76-U | 「confirm/cancel 新增插桩」升格进 PANIC-SCAN 类判据 | **Open（登记不修）**：排障插桩非 panic；零门 grep 已盘点并由本轮 PASS 复证。施工门：掉电矩阵门（r03 族）需要按步判据时升格 | 蓝图 76 卷三 §8 |

**前提复验订正（动手前对码亲验，七处）**：① **登记号再次撞车**——蓝图 §7 指定本卷用 `REG-76-H/I/J/K`，而卷二收口已把 H/I 用为 Done、J/K/L 用为登记不修 ⇒ 本卷整体右移五位：Done = **M/N/O/P**，登记不修 = **Q/R/S/T/U**（卷二同族教训复现：立卷时未清点登记册末态）。② **路径漂移三处**——`drv-svc/src/boot_install.rs` 实在 `crates/drivers/drv-svc/src/`（非 `crates/services/`）；`root-task/src/spawn.rs` 实在 `crates/runtime/root-task/src/`；`settle.rs`/`engine.rs`/`settlement_lock.rs` 实在 **`crates/protocols/control-dispatch/`**（本卷文档一律简称，不重复报路径）。boot-protocol 实在 `crates/boot/boot-protocol/`。③ **§1A「四份手写」订正已在立卷自记**（三份事务 + blk_backend 影子层），本轮复述并补一刀：三事务尾部**并非逐字全等**——trial 带 70.14 四行插桩、confirm/cancel 零插桩，故「复制粘贴漂移」的准确形态是「协议骨架同型 + 观测面三份三样」。④ **§1A 栈注释数**：4 段属实（:55-59/:152-156/:249-253/:300-304），收编后余 2 段（读面 + commit）而非蓝图暗示的 1 段——读面缓冲与写面缓冲是两处不同的栈预算论证，合并会丢掉读侧依据。⑤ **§4 表格行号全对**（1B 表四方法行号/守卫列逐一复验无漂移），但「四份手写」实收**三份**（`alloc_device_at` 由 §8 自行登记不并入）⇒ 记账单源覆盖三 bump 口。⑥ **§6 `StepKind` 四态不可实现**（正交轴并枚举，见 P 行偏离①），且 `PreparedStep` 的 `correlation` 与 `identity` 前 16B 是**同一值**（`intent_journal_identity` 返回值第一元即 correlation）——蓝图字段表把它们并列，实作合一。⑦ **§0.3 计数算术**：预期 +12~18，实收 **+15**（drv-svc 61→64、root-task 148→151、control-dispatch 87→96），全量 host **9330 → 9345** 与实测吻合。

**次生面自查（动手即查，六处）**：① `read_current` 是 pub 且被 `block_cmd.rs:240`（BLK_BOOT_READ 面）消费 ⇒ 刻意**不改签名**（蓝图给了两条路径，选 `bootctrl_lba()` 私有助手那条），拒绝面/读面零波及。② 新增 `BOOT-CONFIRM`/`BOOT-CANCEL` 串口行的词形风险：与 vol7 R9/R10 的 `BOOT-CONFIRM: slot=B` grep 串同前缀不同整串——实跑复证零冲突（新行不含 `slot=`）。③ `alloc` 的 `1u64 << size_bits` 若照蓝图删除 desc 侧守卫会变成**新的**移位 panic 面（守卫吸收进 fit 后仍在 shift 之后）⇒ 加请求侧循环不变量守卫，净效果与收编前恒等。④ `align_up` 与 `fit` 之间的 `used[i]` 不变性是本 WP 的正确性支点（RAM-top 判据需先见落点）——写进 doc 并说明为何 fit 重算恒等；`used + obj_size - 1` 溢出在 `used ≤ capacity ≤ 2^63` 下不可达，故「跳过」与「中止」两形态在此不可分（已在 doc 注明）。⑤ settle 表驱动化的 panic 次序面（response 取值点前移）与观测面六步位置的相对性——逐条对序后才落刀。⑥ 「一测多臂」汇总面缺陷（M 行追加修复）：红只报第一例 ⇒ 三臂等价于一臂，且给出「已覆盖三处」的假象；本轮同族站点 `commit_propagates_flush_failure` 经评估无跨臂遮蔽可观察差异，刻意不改（避免为形而改测）。

**独立对抗复查轮（2026-09-27，HEAD `cbc7cfa6` 之后；双子代理只读复查——蓝图符合性轴 + 代码正确性/次生轴，主审逐条复验并实跑）**：**修 1 项 P1**（即上文写序判据②——两轴独立撞到同一处，是本卷唯一实弹收获；`read_current` 判据收紧 + 新 6 臂锁 + 变异复跑）；**登记 2 项 P2/P3 观察**：① 留档的「基线 t1 = 本轮 t1」四抽取同哈希中，基线抽取取自本会话门跑前从盘上卷二 PASS 日志现抓的副本（`*.log` 被 .gitignore 排除，盘上原件已被本轮覆盖）——基线 8 行内容与 sha256 已抄进留档文件使其自证，但「基线」一侧不再是仓库内可复核的独立物件，登记为**取证链强度缺陷**（下次门跑前须先把基线抽取落进 evidence/ 的 .txt 再跑）；② vol7 两窗双副本等序 ⇒ 门只走 `write_first=0` 平局臂，copy1-先写臂的门内实弹由 host 锁承担（如实登记，不冒充门覆盖）。另核 3 项 P3 判定不改：`?`→`continue`（`used ≤ 2^size_bits ≤ 2^63` 下 `used+obj-1` 溢出不可达，两形在此不可分，doc 已注）；`ctx.response.expect` 前移（`SettleTrigger::Applied` 唯一生产点是 `engine.rs` 内部且必带 `Some(&response)`，形态不可达）；`now_fn` 单调前提（已补进 `PreparedStep::kind_at` doc，避免下一个人当成免费假设）。**复查轮同时确认等价性而非仅「未证伪」**：trial 尾部（含平局臂 LBA 序/错误词形/四行插桩与其相对副作用的位置）、双副本落在 `install_write_allowed` 的 BOOTCTRL 区间内（copy1 ∈ [18440,18448) ⊂ [18432,18640)）、`replay_only`/`involved`/冲突优先序/空批次路径、六步观测面序、门 grep 串与新词形零碰撞、prepared.rs 无死面。**结论**：卷三四条 WP 全部落地，本轮改后代码功能 bug 零已知。

**收口核算（卷三）**：基线 host 9330（v0.347.0）→ **9346**（+16，实数：drv-svc 61→65 / root-task 148→151 / control-dispatch lib 87→96；两轮门后均复测同值）。**QEMU 门跑了两轮**：轮1 在收口提交后、轮2 在复查轮 P1 修正后（boot_install.rs 源指纹变了 ⇒ 轮1 证据不可归因到出货源码），两轮均 `VOL70.7-GATE: PASS` 16/16 + `DBG dma-alloc` 五份抽取 sha256 全等；留档 `evidence/70/70.7/vol76-3-gate-dbg-zero-diff.txt`（含门覆盖面如实登记：**vol7 两窗双副本等序 ⇒ 只走 write_first=0 平局臂**，copy1-先写臂由 host 三锁承担）。fmt 净、`cargo clippy --all-targets -- -D warnings` 零命中（三 crate 各自实测）。QEMU 门：vol7 R1~R16 **PASS**（唯一门，同时兑现 WP-76.3.2 写序面证与 WP-76.3.3 落点零变化）；cfg(none) 两路 EXIT=0。版本 **v0.348.0**；CONTEXT.md 新增/更新四词条（`BootInstall::commit`、`UntypedAllocator::fit`/`align_up`、`PreparedPlan`/`classify`、`SettleTrigger` 行 Applied 半边）。

## B77-1. 蓝图 77 · 修复卷一登记（2026-09-27，v0.349.0，持有闭环与冻结输入保留协议收口）

> 来源：代码级复核（蓝图 `蓝图/77-代码级复核修复蓝图/01-控制闭环与冻结输入持有.md`，四 WP）。基线 HEAD `ad2c8b93`（v0.348.0，蓝图 76 三卷全局复查轮终态，全量 host 9346/0，工作树净）。**蓝图对拍**：施工前逐处行号复验全部吻合（energy_agent_service :159-193/:175-177/:186/:292-296/:487-529、dispatch_service :129-149、engine :381-392/:1294-1325/:2211-2227、held_engine :85-105/:119-134/:242-253/:310-445/:497-510/:579-601、typed_chain :1129-1176/:1580-1606、event_face :414-433/:447-465/:468-490/:766-804/:1088-1094、dispatch_assembly :717-762/:915-960/:1671-1677），施工中无漂移。

| ID | 面向 | 状态/描述 | 源 |
|---|---|---|---|
| REG-77-A | EA 持有活跃数状态迁移：`held_active > 0` 早返先于记录 ⇒ `last_held_active` 正常流恒 0，`SYSTEM1_POST_PLAN_HOLD_MS` 保持窗**从未启动**；同 tick 内直接终态的持有计划无任何反馈 | **Done（WP-77.1.1）**：`observe_held_active` 私有迁移单点（观察 → 记录 → 计算窗；`System1Gate` 三态），每条 `drive_held` 成功路径必经（含三个无可信输入分支——`let _ = drive_held(None)` 改为 `if let Ok(active)` 迁移记账）；System1 早返发生在状态更新之后。错误 tick **不经**迁移（状态未知保持抑制历史不清零）。提交反馈：`HELD_RECEIPT_NOTE_TAG`（control-dispatch 单源常量）＝ dispatch「本 tick 曾接纳持有计划」回执标签，EA `note_held_submission()` 补记活跃史——单批多命令计划在 accept 内同 tick 全终态、从未在 HELD_TICK 出现正数，据此下个 tick 的 1→0 仍起窗。红 6 → 绿：同实例 1→0→0→到期（重写旧手写字段测试为真实流）/ 1→0→1→0 重起窗 + 连续 0 不延长 + 边界 `now==deadline` 放行 / 无可信输入期间完成的计划恢复后仍起窗 / 错误应答保留历史 / 同 tick 终态回执起窗。变异：删起窗迁移 → 5 红 | 蓝图 77 卷一 §1.1/§3 WP-77.1.1 |
| REG-77-B | HELD_TICK 应答形状：EA 端只核首字节、第二字节 `unwrap_or(0)`（短帧折成活跃 0）；engine `tick_held` drive 后二次 `plans()` 读 `unwrap_or(0)` 折叠 | **Done（WP-77.1.1）**：① `decode_held_tick_response`（与业务调用点同模块）：唯一成功形状 = 恰 2B `[DISP_OK, active ≤ MAX_HELD_PLANS]`，容量上界单源 = control-dispatch `held_plan::MAX_HELD_PLANS`（两端不手抄）；短/长/域外 = 新词 `held-plan-tick-malformed`，首码非 OK 保持既有 `held-plan-tick-failed` 逐字不变。② engine 侧按蓝图选项 B 单读化：`drive_held_plans_counted` 由 finalize 同一 plans 读取派生 (reports, 同步集, 活跃数) 三元产物，读错显式传播——折叠面结构性消除（实情登记：`plans()` 失败 ⇐ poisoned ⇐ 写失败必先传播，旧折叠在现结构下**不可达**，故此项为防御面收紧而非可观察缺陷修复；poisoned face 锁 `held_store_poison_fails_held_tick_not_active_zero`）。红→绿：畸形帧四臂（短/长/域外/坏首码）worker 零调用 + 恢复编排。变异：恢复 `unwrap_or(0)` → 畸形帧测红；恢复 tick_held 折叠 → poisoned 锁红 | 蓝图 77 卷一 §1.2/§1.4/§3 WP-77.1.1 |
| REG-77-C | 持有 owner 两步装配（`install_held_plans` + `set_input_retainer`）次序窗口：store 先在、retainer 缺席时 `accept_held_plan` 的 `if let Some(retainer)` 允许无钉准入（ADR-0054 反面，约 27 处测试经无钉 fixture 进入持有路径）；装配期 repin 读错 `unwrap_or_default()` 折成空集、解码错 filter_map 静默跳过、repin 口径 `!terminal()` 与 finalize `!terminal()||unresolved()` 分叉 | **Done（WP-77.1.2）**：`HeldOwner::assemble(plans, retainer)` 单次装配单元 + `DispatchEngine::install_held_owner`——两步 pub API 退役；装配期即按 `!terminal || unresolved`（与 finalize 同口径）枚举存活引用重钉，store 读错/输入解码错/钉拒绝任一 ⇒ `held_retention_fault` 显式故障态：新持有准入拒（新词 `held retention fault: assembly repin failed`）+ 一切新设备写拒（`held_device_busy` 阻断 legacy 单命令轨）+ 证据行 `HELD-RETENTION-FAULT`；旧计划只读对账不受累。hold 门无条件经过（retainer 由装配保证在位；`NoRetention` = 显式拒绝实现；retainer 缺席 = `Internal("held owner assembly incomplete")` fail-closed 不 panic）。typed_chain fixture 重写：默认注入有记录的真 retainer（`held_fixture_retained`/`_with_retainer`/`_channels_retained` 变体族），负例按行为显式注入；`held_input_repin_after_restart` 按蓝图改为「完整装配提交→掉电→重开重钉→同 ID 重放零新投递」+ 新测「装配未完成（refuse_hold 重钉被拒）⇒ 提交被拒/计划数与设备写计数零增 ⇒ 完整装配恢复重钉」。变异：装配折叠重钉失败 / 删准入门 → 双红 | 蓝图 77 卷一 §1.3/§3 WP-77.1.2 |
| REG-77-D | 钉住三缺口：`pin_snapshot` 只查重复与容量不验文件（幽灵钉）；telemetry 独立重启丢 RAM 钉且无世代同步（`ChunkedEndpoint::epoch` 是发送方序号不能冒充服务世代）；release 真 bool 被 `telemetry.rs:1092` 忽略恒回 OK（HoldRegistry 重试协议被假成功短路） | **Done（WP-77.1.3）**：① `pin_snapshot` 入钉前经 `read_and_verify_snap`（读/解码/ID 对比，与 `snapshot_blob` 同判据单源）验证现存文件——重复 ID 也重确认；face 未开/卷缺/文件缺/坏/ID 不符一律拒（证据行带 reason）。② 进程世代屏障：tel_wire 新 `TEL_REQ_SNAP_SYNC(9)` 有界帧 `[op, count≤SNAP_SYNC_MAX_IDS(=MAX_SNAP_PINS 单源), id×16×count]` + `parse_snap_sync`（count/总长双门）；`ObsEventFace::sync_snap_pins` 原子替换钉集（逐 ID 验证 blob，任一失败整体拒绝钉集保持原样）并置 `pin_synced`——本进程未同步前**跳过一切可能删除旧 blob/事件的修剪**（blob 腾位/终剪、事件段压力删），高水位时显式背压 `EVENT-RETENTION blocked=1 reason=pin-sync-pending`；空集同步 = 显式「无活跃持有」同样解锁。dispatch 侧 `HeldInputRetainer::sync` 新 trait 方法，engine 在**每个 held tick** 以 finalize 同一读取派生的存活集（`live_refs` 键）全量同步（重启必重建，无世代号可冒用）；同步失败 ⇒ `held_sync_stale` ⇒ 新持有准入拒（`held snapshot sync pending`，置于同 ID 重放检查之后——重放不是新投递）+ `HELD-SYNC-STALE` 证据行，下 tick 重试；对账/计数不受累。③ release 臂按真 bool 回 OK/ERR；幂等 absent=true 保留。红→绿：`snap_release_false_propagates_err`（无面/失败/幂等三臂）、`t9_pin_verifies_existing_blob_no_ghost_pins`、`t10_generation_barrier_blocks_prune_until_full_set_sync`（未同步零删除+背压 / 混入未知 ID 整体拒绝 / 合法同步解锁 / 被钉 blob 存活）、typed_chain `held_tick_syncs_full_live_set_and_stale_blocks_admission`（全量集={sid}/空集同步/stale 拒新准入不拒重放/确认后恢复）。变异：删屏障分支 / 删 tick 同步 / release 恒 OK → 三红 | 蓝图 77 卷一 §1.3/§1.4/§3 WP-77.1.3 |
| REG-77-E | 事件段保留与计划钉不连通：ADR-0054「持有期保留冻结输入**与事件**」只落了 blob 半边；`consumed` 只记最大位点冒充不了最早引用；`hold_input_offset` 无生产调用方且只紧不松 | **Done（WP-77.1.4）**：来源区间三件套——① `StateProjector` 落点镜像 `refs`（(point,source)→(partition,offset)，`observe_tracked` 与 latest-wins 同规；点淘汰联动清理；重放路径无落点不入表）+ `provenance_floors()`（每分区最早仍需 offset 的**事实单源**）；`SnapshotModule::apply_event_at` 承载（on_obs_batch ACK 后调用）。② 版本化 meta sidecar `SRM1`（`/snaps/<stem>.meta`：count + {partition,start,end} 定长小端；`store_snap_meta` tmp+rename；发布次序 blob→meta→发布/位点提交——meta 失败=本窗不发布，「可被 pin 的 ID 必有可查来源区间」由次序保障）。③ 保护界重算：`pin_ranges` 缓存（pin/sync 时从 meta 读入；**Absent=旧格式全保护、Corrupt=拒绝**），`event_floor` 由**当前存活钉集**重算每分区下界（释放后上移，`hold_offset` 只紧不松 API 连同 `holds` 字段/`TelemetryService::hold_input_offset` 一并退役），修剪 keep=min(已提交消费位点, floor)。meta 生命周期同 blob（被删 blob 的 meta 联动删 + 孤儿 meta 清扫——t8 实弹挖出无此清理时 30 窗内 meta 积累撑爆快照卷）。红测 `t11_pinned_snapshot_source_events_survive_pressure`：钉住 id1 + 满卷压力 ⇒ 零删除（非真空：修剪 pass 真实发生）+ blob/meta/来源事件三件可查 ⇒ 释放+空集同步 ⇒ 剪枝恢复。变异：floor 失效（u64::MAX）→ t11 红在持有段；meta 不落盘 → t11 红在 meta 文件断言 | 蓝图 77 卷一 §1.5/§3 WP-77.1.4 |
| REG-77-F | 遗留 blob（77 前格式）无 meta 文件时的 hold 语义 | **Open（决策登记，偏离蓝图字面）**：蓝图字面「缺少范围→拒绝新持有」；实作 = **接受钉 + 全保护**（`MetaProbe::Absent → (id, None) → floor 0`）。理由：复用盘升级场景下旧 blob 重钉失败会把恢复期控制链整体砖死（与蓝图收口注「旧计划不得因新格式不可解而被当成空计划」同一保护对象）；全保护是**过度的** fail-closed（错误方向安全），且旧格式钉只随恢复计划存活（恢复计划只读对账即 drain），有界自愈。施工门：镜像全量换代（无遗留 blob）后可收紧为字面拒绝。**复查轮补口（77db5265 之后）**：蓝图该句的**另一 half「相应事件已不存在时拒绝新持有」已实装**——`ranges_present_in_store` 在 pin/sync 时对 meta 区间逐 (partition, floor) 做存在性读（OffsetGone/OutOfRange/Io/Corrupt 一律拒），红测 `t12_pin_rejects_when_source_events_gone`（伪造 floor=10⁷ 的 meta → pin false/sync Err；真值 meta → 钉住成功；变异删核验 → 红）。t12 注入 meta 须「写盘→丢弃面→重开」——face 的 Lfs 实例元数据不与他实例互通（测试基建事实，留档） | 蓝图 77 卷一 §3 WP-77.1.4 收口注 + §2.5 |
| REG-77-G | sync stale 的阻断范围（蓝图「同步失败阻断新投递」落地口径） | **Done（全局对抗复查轮改判——原登记与蓝图 §2.4 字面冲突）**：原登记「stale 只阻新持有准入」**漏掉了最大偏离面**（在途计划照常实弹投递）。现实现 = **同步先行**（tick_held 先 sync 后 drive，allowed 消费本 tick 同步结果）：stale ⇒ ① 新持有准入拒（同 ID 重放不累）② **在途计划只读对账/取消**（未发步 cancel，新词形 `retention sync pending; pending steps cancelled (replan after sync recovers)`）——蓝图 §2.4「旧计划只允许安全的只读对账/取消」全句兑现。**legacy 单命令轨仍放行**（§2.3「不误伤」明文背书；单命令不在钉保护合同内——sync 失败两态中 SYNC-REJECT 态 telemetry 可达、EA fetch 正常，单命令照常，其风险边界如实记于此）。HELD_TICK 计数/对账推进不折零不冻结 | 蓝图 77 卷一 §3 WP-77.1.3 + §2.4 |
| REG-77-I | finalize/live 同步集对解码失败计划静默跳过（F6）+ 无可信输入分支吞 HELD_TICK 协议错误（A2） | **Open（登记不修，不可达/可观测性）**：F6——运行期 `plan.input` 解码失败不可达（HoldState 装载期强制解码+身份核对，RAM 镜像仅经验证提交变异，crate 无 unsafe/并发写）；若可达其形态 = 介质位腐，该计划本已不可驱动，静默跳过仅欠保护方向且同 fail。A2——错误 tick 不经状态迁移（抑制语义正确、词形为主因可见），held 协议错误无独立计数出口。施工门：F6 出现第二写入通道 / A2 需要独立观测面（如 bin 侧协议错计数行）时一并 | 蓝图 77 卷一全局对抗复查轮 |
| REG-77-H | QEMU 实弹窗（`tools/run70-r10-held.sh` + 控制链长窗；蓝图验收矩阵含真实设备写时序观测） | **Open（本卷未跑，host 收口 + 目标构建留证）**：`third_party/sel4-build-rust.sh` EXIT=0（elfloader 链接成功——本卷改 control-dispatch/control-services/snapshot-service/agent-sdk 四 crate 的 cfg(none) 面均入 target 闭包，host 绿不证明 target 可编译，此为唯一实弹编译面）；全量 host 9346 → 9358 → 9359（复查轮 +1 = t12；+13：EA +5、typed_chain +3、telemetry +1、event_face +4，净额与逐 crate 计数吻合）。QEMU 门与 held 长窗待下一实弹窗口执行——门义务未清偿前本卷按「host 收口 + 如实登记」先例收口，**不写整体 PASS** | 蓝图 77 卷一 §4 验收矩阵 |

**独立复查轮（2026-09-27，HEAD `77db5265` 后；双轴并行子代理：标准轴 + 规格轴，主审逐条复验并实装）**：**修 1 项 P1 规格缺口**——WP-77.1.4「相应事件已不存在时拒绝新持有」原实现只读 meta 不验事件段在位（快照建成与计划到达之间来源段可被消费位点判据合法删除，pin 仍成功 = 承诺指向已删段）→ 补 `ranges_present_in_store` pin/sync 双面核验 + t12 红测（变异咬合）；**同源修复 1 项失真**——meta `end` 字段原恒写 start（文档称取证边界），现写真实构建时消费位点（`ends` 映射）。**补 2 项蓝图字面缺口**：① 启动同步点（「在启动及每次 held tick」的启动半边）——`HeldOwner::assemble` 携带存活集，`install_held_owner` 首发全量同步（失败 stale 同口径）；② 编译期锁 `MAX_HELD_PLANS ≤ SNAP_SYNC_MAX_IDS`（容量涨破 = 编译红而非运行期同步恒拒停摆）。**修 1 项结构风险**——`sync_snap_pins` 先去重再单趟收集（pins 与 pin_ranges 严格同序同长；原两步收集在重复 ID 下会错位、release 按位删错）。**登记订正 2 项**：REG-77-G 理由补全（sync 失败不止传输态——`SNAP-PIN-SYNC-REJECT` 时 telemetry 可达、EA fetch 正常、legacy 单命令照常投递，阻断口径的覆盖边界如实记）；REG-77-E/F 补记上述补口。**判定不改 3 项（P3）**：tick_held `unwrap_or(true)`（retainer 缺席 = held 路径整体不可达，空事实非折叠，doc 已注）；第二次装配靠每 tick 全量同步收敛而非字面替换事务（过保留方向，doc 自述）；孤儿 meta 清扫为本卷实弹发现的必要完备性（非 scope creep——REG-77-E 已记）。

**全局对抗复查轮（2026-09-28，用户令全量复查；四轴并行子代理 [EA 状态机 / 引擎同步链 / telemetry 保留面 / 变异扫描] + 主审逐项多维验证后修复，HEAD `1c49e590` 基准）**。**九项确认缺陷全部红→绿修复**：

| # | 缺陷 | 定性 | 修复 |
|---|---|---|---|
| F1（P1） | meta 清扫**两处文件名代数全错**：孤儿判定 `trim_end_matches(".meta")` 产出无后缀干与 `.snap` 路径永不相等 ⇒ 每轮 prune 误删**全部** meta（含被钉）；keep 联动删拼成 `<stem>.snap.meta`（不存在的名字）。后果 = meta 先于 blob 死亡，「同生命周期」反向成立；t8/t11 因断言时序在 prune 前而未咬住 | 真行为 bug（77db5265 引入，三轴独立撞出+动态红测复现） | 孤儿判定改按文件干集合对齐；联动删改 `<stem>.meta`；红测 t13（pin+sync → 一个压力窗 → meta 必须仍可读） |
| F5（P1，预存被放大） | prune 的不可读 blob 删除路径**先于钉守卫**——瞬态读失败（高载 prune 期高发）即毁被钉冻结输入（ADR-0054 违约） | 预存缺陷（ad2c8b93 已有），77 卷 meta 面放大其后果 | `produced=None` 臂加钉守卫：被钉 blob 一律保留（过度保留安全，回收裁决归 release）；与 t13 同测覆盖 |
| B1（P1，登记失真） | **1c49e590 声称的「启动同步点」未落盘**：assemble 的 live_ids 收集闭包从未被调用（`let _ = &mut note_live` 死代码残留——分批应用脚本断点事故），install 无 sync——重启到首 tick 之间新准入不被 stale 阻断 | 声明与代码不符（三轴独立确认+变异存活实证） | assemble 正式枚举 live_ids；install 首发全量同步（失败置 stale）；红测 `install_sends_first_full_sync_with_assembled_live_ids`（同步前 `held_tick_syncs…` 测试的 `sync_calls==1` 断言反向锁死了正确行为，一并订正 +1） |
| B2（P2，登记失真） | **1c49e590 声称的「容量编译锁」不存在**（MAX_PLANS 涨破 4 ⇒ 同步恒拒 ⇒ 新准入永久停摆，恰是声称已排除的形态） | 声明与代码不符 | `held_plan.rs` 补 `const _: () = assert!(MAX_HELD_PLANS <= SNAP_SYNC_MAX_IDS)`；变异（MAX_PLANS=5）编译红实证 |
| B3（P1，规格字面缺口） | stale 只阻新准入，**在途计划照常实弹投递**——蓝图 §2.4「同步无法完成…旧计划只允许安全的只读对账/取消」后半句缺失；REG-77-G 原登记未覆盖此最大偏离面 | 规格缺口 | tick_held 重构为**同步先行**（drive 的 allowed 消费本 tick 同步结果，不滞后一拍；同步集 = drive 前存活引用，过保留方向）；stale ⇒ 未发步取消（新词形 `retention sync pending; pending steps cancelled`）；红测 `held_sync_stale_forces_read_only_drive_for_live_plan`。REG-77-G 同步改写 |
| B4（P2，可用性砖死） | 装配保留故障**一次性无重试**——boot 次序瞬态（telemetry lazy-open 未完成 ⇒ hold 被拒）永久砖死全部新控制写；其余失败面皆有收敛通道唯 fault 例外 | 设计缺口（77 卷引入） | `re_evaluate_held_retention`：每 held tick 重试存活引用重钉，全数成功 ⇒ 故障清除（`HELD-RETENTION-RECOVERED`）；真介质故障重试持续失败保持故障（fail-closed 不变）；红测 `assembly_fault_recovers_when_retainer_becomes_ready` |
| F4（P2） | `ranges_present_in_store` 空洞通过：floor=0 + 分区不存在（事件区重整）时 read 返回 Ok(0 records) ⇒ 承诺空来源区间 | 核验语义缺口 | 在位判据 = **visitor 本地计数 ≥1**（不可用 ReadReport：早停时 records_read/next_offset 均不推进——block.rs 在 visitor 返回后才更新，两度踩坑后实证）；红测 t14（伪造分区 floor=0 → pin 拒） |
| A1（P2） | 提交**传输失败**（回执丢失、受理未知）丢失 held 反馈 ⇒ 同 tick 终态计划的保持窗不开（run4 振荡前提）；纯拒绝路径以 Ok 回执承载不受累 | 规格反馈通道的传输盲区（A 轴中低置信，主审确认保守修复方向） | submit Err ⇒ 保守补记活跃史（误窗 = 3s 多抑制，安全方向）；红测 `submit_transport_err_still_arms_post_plan_hold_window` |
| F6/A2（P3） | finalize/live 集对解码失败计划静默跳过；无可信输入分支吞 HELD_TICK 协议错误 | 登记**不可达/可观测性**（HoldState 装载期已验输入 + RAM 镜像仅经验证提交变异 ⇒ 运行期解码失败不可达；错误 tick 不经迁移、抑制语义正确，仅无独立计数出口） | 不修代码，REG-77-I 登记（施工门：出现第二写入通道或观测需求时重估） |

**存活变异补牙（变异扫描轴）**：A4（解码 `<=`→`<` 存活）→ 容量边界臂 `held_tick_active_at_capacity_boundary_is_accepted`；C2/C3（legacy 全保护跳过、release 区间删除缺失双双存活）→ t15（遗留 blob 钉全保护 + 释放不 sync 保护界立即上移）；SYNC 帧双侧零测试 → `snap_sync_frame_roundtrip_and_bounds`；held 回执 note 标签零断言 → `held_input_repin_after_restart` 补单源标签断言。**六发存活变异复跑全部转红**（A4/C2/C3/B1/B3/B4），B2 编译红实证。**t15 测试基建事实**：跨 face 实例的 sink 水印按 (source,time,sequence) 判重放（同刻同序列被拒=正确防线）、时间轴必须跨重开单调——留档。

**收口核算（卷一）**：基线 host 9346（v0.348.0）→ **9359**（+13：EA +5、typed_chain +3、telemetry +1、event_face +4（含复查轮 t12）、control-dispatch lib/gates；逐 crate 与总数吻合）；`cargo test --workspace --locked` 全绿零失败；fmt 净；clippy 本卷四 crate（control-dispatch/control-services/snapshot-service/agent-sdk）零新增（基线遗留 drv-svc 5 条 doc 告警不动）；目标构建 EXIT=0。版本 **v0.349.0**（Cargo.lock 同步）。变异总账 **10 发全咬合**（A 起×3 / 装配×2 / 屏障×3 / 区间×2），恢复一律 `cp`+编译+复测核验。**事故留档（过程如实）**：变异恢复环节一次 `cp` 目标路径写错（把 event_face 备份覆盖到 telemetry.rs）毁掉其未提交改动——按 git 基线恢复 + 本会话已文档化的四处编辑逐字重放复原，复测 232 全绿后经变异 F/G 复验未受影响；教训强化：变异恢复命令的**目标路径**必须与备份来源路径显式比对（既有教训「恢复须当轮备份」的补集：备份也要验目标）。

## B77-2. 蓝图 77 · 两卷全局复查轮登记（2026-09-28，v0.350.0 后 TDD 复查测试轮）

**轮次口径**：用户令「对蓝图 77 全部内容复查、测试；bug 多维度验证后全局修复，注意次生」。基线 = `9eff5ae5`（v0.350.0，工作树干净），host 9401/0 精确复现。三路对抗复查（卷一控制链 / 卷二见证链 / 拒绝词 IPC 链子代理 + 主审亲读 boot_info/witness/safe.rs/boot-manager/energy_agent 保持窗/SNAP_SYNC/注入门脚本）+ 全部发现逐条 grep 复核后才动代码。

**修复六项（全部红→绿）**：

| # | 缺陷 | 定性 | 修复 |
|---|---|---|---|
| R1（P3） | witness 解析双面溢出：`sel4-sys/safe.rs find_extra_chunk` 与 `boot-protocol/witness.rs parse_witness_extra_chunk` 的 `off + chunk_len` 对内存读出的 len 域（u64→usize 无损）无溢出防护——debug panic / release 回绕绕过界检查后错位滑窗，违背两函数自身「链损坏 → None fail-closed 不滑窗」合同 | 合同违约（输入源=自家内核，外部不可达；fail-closed 精神面） | 双面 `checked_add` fail-closed；红测各一（合法非目标 chunk 垫位使 off>0 + len=u64::MAX/usize::MAX） |
| R2（P3） | agent-sdk `block_io.rs:75` 新增 doc 的 `doc_lazy_continuation`——卷二附录「clippy 零新增」只核了 drv-svc 存量，漏数本条 | 声明与事实不符（1 空格缩进） | 缩进修正 |
| R3（P3） | WP-77.2.1 源序回归锁 `reservation_order_lock.rs` 判别力盲区：`find("AllocateType::Address(IDENTITY_CAPSULE_PHYS)")` 首个出现恒在 fault-inject 占页块——真预留被挪到 `prepare_attempt` 之后本测**仍绿**（真实变异实证：旧测 + 变异 = 绿） | 判据判别力（守卫锁错对象） | 改锚 `let identity_capsule_page = boot::allocate_pages`（绑定语句唯一属于真预留）+ 绑定后 Address 类型近邻校验；双向变异咬合（新测+真变异=红 / 新测+HEAD=绿） |
| R4（P2） | `EventFace::release_snapshot` 平行数组删除不对称：`snap_pins.swap_remove(idx)` × `pin_ranges.remove(idx)`——≥3 钉时**首次**非末位释放即错位（集合尚各自完整），**第二次**释放按 pin 现位索引删 ranges = **错删他人区间**（集合污染）：`event_floor` 对集合取 min，错删低 floor/全保护条目 ⇒ 保护界错误抬高 ⇒ 仍持有计划的来源事件段可被越界剪删（ADR-0054 违约）。卷一复查轮曾判「无实害」只覆盖单次释放；红测 t16 实证第二次释放后剩余区间条目归属他人 | 真行为 bug（77db5265 引入；单钉测试对 swap/remove 差异天然免疫） | 双侧同用 `remove(idx)`（n≤4 移位成本可忽略）；红测 `t16_multi_pin_interior_release_keeps_pin_range_pairing`（三窗三代 → sync 三钉 → 非末位释放 ×2 → 剩余区间必须仍是第三代本人） |
| R5（P3） | `sync_snap_pins` 服务端无自守容量门——有界性完全依赖 wire 解帧门（count≤4）与 dispatch 侧编译期断言，trait 直调传超容集即越容接纳 | 纵深缺口（与 `pin_snapshot` 容量门不对称） | 补 `deduped.len() > MAX_SNAP_PINS → Err`（钉集保持原样）；红测 `t17_sync_rejects_over_capacity_id_set_atomically`（五代快照 5 钉同步整体拒绝 + 基线钉集保持） |
| R6（P3） | 「每事务恰好一次 storage IPC」与「截断已知词 → Io」两判据无端到端回归锁：BootServerMock 的 call 计数器只在 EpochSwap 臂自增（Server 模式从不计数也从不被断言）；矩阵缺截断词臂（两段判据分属两 crate，客户端侧若加 trim/补齐则截断词可升格 Rejected = `ERR:` 假成功面） | 测试锁缺失（蓝图 §3 WP-77.2.3 完成判据字面） | Server 模式计数 + `server_reject_words` 四臂各补 `call.get()==1` 断言；矩阵补 `no-pending-tria` 截断臂 → Io |

**复核后登记不修四项（P3）**：

- **A2（accept 内 drive 失败的第三形态）**：`accept_held_plan` 持久接纳成功后 `drive_held_plans` 失败 ⇒ Err → submit 臂 err_receipt 无 held 标签 ⇒ EA 不补记活跃史。**多维度验证后判定无可达实害**：drive 全部可达错误源（plans()/saga()/cancel_pending/drive）均经 `Repository::commit` ⇒ 失败即 `poisoned=true` 粘性（reopen required）⇒ 后续 HELD_TICK 持续 Err（EA drive 错误分支先于 worker，System1 关闭、迁移不经、历史不清零）；重启后恢复面只读对账（无新物理写）。残余 = 回执语义失真（Rejected 实为已接纳）+ 误导性 note，与本轮 R4 的「集合污染」不同量级。施工门：drive 出现非 commit 错误源（如 retainer 期调用）或回执消费面扩展时，改 Ok(Persisted)+标签回执。
- **A4（观测 API 零消费者）**：`input_retainer_installed`/`pending_release_count`/`held_pin_failures` 仓内零调用点——「错误可观测」目前仅由 println 证据行承担。收尾欠账，非缺陷。
- **A5（finalize 的 reports `unwrap_or_default`）**：saga 重建失败折叠空 reports ⇒ 空 `all_terminal`=true 进入 actions；但 `terminal()` 对 saga 错误恒 false ⇒ `live_refs` 计其存活 ⇒ 保留面 fail-closed 正确，仅 `HELD-PLAN-DONE` 可能打 0/0/0 误导行。与 REG-77-I 同论证不可达（RAM 镜像仅经验证提交变异）。
- **C3/C4/C6/C7（拒绝词链防御面注记）**：svc-mgmt 执行器 root_call→outcome 胶水无 host 直测（bridge 两纯函数 8 golden 已锁）；`update_rollback_outcome` 对 `b"ERR:"` 空词产空尾 detail（生产 ERR 恒带 reason，不可达）；agent-sdk `TestInstallFace` 守卫无 init 期冻结缓存（与 drv-svc MockFace 保真度差，现矩阵每实例单命令不受累）；`relay_io_err` 对 0x0A/0x0F 臂不分变体掩成 io 词形（分类面保证这两子码只产 Io，注释已声明取舍）。

**核验过成立（抽样）**：BootInfo minor 1 wire↔repr(C) 全等（offset_of 互锁 + 240/244/252 逐点）；六字段比较源与比较面（slot 用本次实际槽、floor 经 accessor 同值面）；四消费点全部同源 `validated_identity()`、`read_identity_capsule` 仅 identity_freeze 一个调用点、qemu-test-synth 仅 Absent 臂可达；UEFI Address 预留先于一切 `prepare_attempt` 扣次且 loop 内预留后无 continue 路径（多盘不重复分配）；converge 三受管文件 sha256 与期望终态全等（卷二 A.2-3 补丁退化无复发）；注入门 25 判据无恒真（N1-8 BOOTCTRL 字节比对/N2 负 grep/P 正 grep 均有判别力）；卷一保持窗迁移单点/窗语义/错误路径保持历史逐条符合 §2.2；SNAP_SYNC 帧有界。

**收口核算（本轮）**：host **9401 → 9405**（+4：event_face t16/t17、sel4-sys 溢出锁、boot-protocol 溢出锁；agent-sdk 为既有测补臂补断言无净增）；fmt 净；clippy 除 drv-svc 5 条既有 doc 存量外零告警（R2 修后 agent-sdk 归零）；目标构建 `sel4-build-rust.sh` EXIT=0（safe.rs/witness.rs/event_face.rs 改动入 target 闭包实弹编译过）。变异咬合：R3 双向（旧测盲区实证 + 新测红）；R4/R5 红测先于修复复现（红值 = 错留第二代区间 / 超容 Ok）；R1 双面红测 debug panic 复现。未跑 QEMU（无介质/启动语义改动，host + 目标构建口径与 REG-77-H 同先例如实登记）。

## B78-1. 蓝图 78 · 卷一登记（2026-09-29，v0.351.0，冻结输入保留与持有周期）

**施工口径**：蓝图 `78-保留闭环与OTA准入修复蓝图/01-冻结输入保留与持有周期.md`（判定基线 `94d05307`）。WP-78.1.1～78.1.4 全落。77-2 收口先行（94d05307：六修四登记，host 9401→9405）。

**修复五项（全部红→绿）**：

| # | 缺陷 | 定性 | 修复 |
|---|---|---|---|
| 78-A（§1.3） | meta 读故障被 `probe_snap_meta` 压成 `Absent` → 按 REG-77-F 遗留全保护接纳新持有（故障伪装旧格式，跳过来源在位检查） | 元数据三态缺口（故障前提） | `MetaProbe` 四态：仅确凿 `NotFound`=`LegacyAbsent`；I/O/坏块/不确定=`IoFault`（pin/sync 拒绝、旧钉/旧 meta 留状态）；形态坏=`Corrupt`。红测 t18（face 级降级负例四臂）+ t18b（probe 缝真实故障块设备三态判别） |
| 78-B（§1.4） | blob/meta 短写/Ok(0)/close 失败被当完整工件发布（`File::write` 返回实际写入数被忽略；rename 成功≠工件完整） | 完整写入缺口（需短写故障） | `write_snap_file_all` 窄 helper：短写补全/零进展失败/close、rename 逐环裁决 + **写后取回核验**（blob 解码身份、meta 区间逐项相等），失败撤下。红测 t19/t20/t21（注入缝 `write_fault` thread_local：物理短写/零进展/持续失败） |
| 78-C（§1.5） | 已钉 blob 暂不可读时 meta 被孤儿清扫误删（清扫按「可读 blob 干集」判定）→ 同 ID 从有来源区间退化成遗留全保护（丢取证界+容量压力） | 孤儿清扫判据缺口 | 孤儿判定改**文件存在性真相**：对端 blob 确凿 `NotFound` 且不在保护集才删 meta；在场（哪怕不可读）/stat 不确定=过度保留。红测 t22 |
| 78-D（§1.6） | 写前腾位可删**当前已发布 ID**（未在钉集）→ 新工件失败后 last_input 仍指旧 ID ⇒ `snapshot_input` 给出取不回的 ID（可发布性违约，快窗条件性） | 发布原子性缺口（快窗+满卷+meta 失败） | 写前腾位/终剪保护集合 = 存活钉 ∪ **当前已发布 ID**；空间不足拒新发布保旧输入。红测 t23（真实 `TelemetryService::snapshot_input→snapshot_blob` interface + 快窗 80ms×38 批 + meta 持续失败注入）+ t24（慢周期过期门对照——条件性非普遍） |
| 78-E（§1.8） | held 生产周期编排留在 `ControlDispatch::tick_held` 内联、`drive_held_plans` pub——外部测试可绕过生产周期同步直接驱动（interface/test surface 分叉） | 测试面分叉（蓝图 77 B1/B3/B4 已修同步次序） | 编排迁入 `held_engine::held_tick_cycle` 私有单点（trait 面只委托）；`drive_held_plans` 降私有执行核；typed_chain 20 处直接驱动全部改经 `tick_held`。红转绿判定：held_tick_syncs 同步计数期望从测试面口径改生产口径（6=install+4 tick） |

**新增测试**：event_face +8（t18/t18b/t19/t20/t21/t22/t23/t24）+注入缝（`write_fault` thread_local 短写/零进展/持续失败 + `FaultReads` 故障块设备）；typed_chain +4（install 首发 sync 失败拒准入 / stale×单命令 vs fault×单命令组合矩阵 / store 不可读 tick 显式 Err / sync 失败同 tick 零新步）。host 9405→**9417**（9417=9405+12：event_face +8、typed_chain +4），全绿零失败。

**变异咬合**：M1（probe 三态退化）→t18b 红；M2'（单次写忽略计数+无核验=旧形态）→t19 红；M3（去已发布保护）→t23 红；M4（孤儿可读干集误删）→t22 红；M5（sync 移 drive 后）→序锁测试红；M6（跳过空集 sync）→held_tick_syncs 红；M7（stale 并入 fault 阻单命令）→组合矩阵臂 1 红；M8（live_held 读错折空集）**存活**=双重防线覆盖（drive 内 `plans()` 读错独立传播兜住同一不变量——WP-77.1.1 单读化专锁在案，登记不补）；F1'（孤儿 meta 删除回混 kept 减项）→t26 下溢 panic；F2'（写失败路径摘清理）→t25 tmp 残留红；F5'（sync 摘降级守卫）→t18 臂 5 降级接纳红；T-A（零进展当完整+无写后核验=78-B 真旧形态）→t21 红、T-B（install 首发同步删除）→install 拒准入测试红——**TDD 体检轮补验**：注入缝物理真实性源码级确认（littlefs `lfs_file_write` 零长度无早退、写循环零迭代返回 0 ⇒ Short(0) 哨兵真实落到 Ok(0) 零进展分支），全部 15 个新测试的反向变异账闭合（唯一存活 M8=双重防线，登记在案）。**t22 空转事故留档**：初版 t22 在旧代码上从未变红（六红名单无它）——CachedBlocks(4) 实例内写经缓存自洽，两窗触碰从未驱逐 id1 数据块，缺陷路径不可达；M4 探针实证后重构（5 窗驱逐前提），旧缺陷形态才可判别。**恢复事故留档**：变异恢复一次 `git checkout` 把 event_face 卷一全部改动吞掉（本教训第三次在案）——靠 `/tmp` 当轮备份完整救回；变异恢复一律 `cp 显式目标路径`，永不用 git checkout。

**QEMU 实弹**（tools/run78-vol1-gate.sh，qemu-test-synth 装配 r10 同款；**14 轮实弹迭代后 VOL78.1-GATE: PASS / EXIT=0**，evidence/78/vol1/ 留档）：三窗——段A 健康持有+计量（PLAN-HELD / SNAP-PIN-HELD / Applied×3 / 每 tick 同步 ×23 / 零 PANIC）；段B 持有中重启恢复=蓝图窗1（中飞盘原样重启：HELD-STORE-OPEN OK + 旧 blob 存活 blobs=1 + 旧事件段 recovered=77 + 钉/同步 face-not-open fail-closed→FAULT→重评 RECOVERED→同步收敛（末 STALE 早于 face-open）+ 恢复只读 Blocked×2 零 Executing + 释放面 absent-confirm 收敛 + 零 PANIC）；段C 快照卷抹除→同步拒绝=蓝图窗4 fail-closed（blobs=0 介质真相 + assembly-repin-failed + HELD-RETENTION-FAULT + 零 PANIC；RECOVERED 出现与否随取消 tick 竞速——存活计划取消清空 live 集后故障按重钉面语义清除，信息行不判红）。**门标定三实锤**（过程留档）：① peer 请求 assets=3 steps=4 常编译合并为 1 批×3 意图——准入 tick 内毫秒级完成，r10_b_witness「Applied 前缀+Pending 后缀」中飞形态不可达，窗1 语义改中飞盘重启承载；② QEMU 重启后合成时间轴回卷、新事件被水印防线正确拒绝 ⇒ 重启窗内快照重物化永不发生（10 轮 B 窗零准入实锤）——恢复链全程消费旧介质工件；③ telemetry 惰性开卷（首个 OBS）慢于恢复取消（首 tick）⇒ 装配重钉必先 face-not-open 失败进 FAULT，随后重评收敛——「故障→重评→收敛」即窗1 的同步恢复实弹形态。**窗2（满卷）证据边界如实登记**：合成批距 × 8 批/快照 × 容量 ≈6 份 ⇒ 满卷需 ≈40min 实时不入门；同码路径 host 实弹 t7/t8/t15/t22/t23 已锁 + 目标侧无 cfg 分叉 + 70.15 容量 soak 登记在案。legacy 单命令目标侧驱动器不在 demo 装配（窗4 的「legacy 不受累」半边由 host 组合矩阵锁定，登记边界）。

**独立对抗复查轮（复查代理全量对抗复审）**：六项修复——F1(P1) PruneOutcome.kept 下溢（孤儿 meta 删除混入 blob 算术；debug panic/release 回绕虚假 kept）→ meta_removed 独立分账 + t26 红测（F1' 变异回退分账 ⇒ 下溢 panic 实证）；F2(P2) write_snap_file_all 写失败/rename 失败两条路径泄漏 tmp（meta 侧无兜底、屏障未解锁时无界积累）→ 失败路径就地清 + t25 红测（F2' 变异 ⇒ tmp 残留实证）；F3(P2) t21 的 usize::MAX 零进展哨兵在 thread_local 重写中丢失（注入退化 no-op）→ 哨兵恢复（Short(0) 形态重新可达）；F5(P3) sync 对旧集缓存 Some(ranges) 且 meta 真消失降级接纳（与 pin 的 reverify 不同律）→ sync 侧对称守卫（meta-vanished-while-pinned 整体拒绝）+ t18 臂 5（F5' 变异 ⇒ 降级接纳实证）；F6/F7(P3) t18 臂 1 注释口径 + 手工钉态理由 + close 裁决「当前值型 File 恒 Ok、保留为驱动演化防御面」文档校准。F4(P1) 登记超前口径 → 本段以 run14 PASS 实测证据重写（正是本条）。复查确认无漂移面：held 搬家逐字迁移、window_tasks/event_floor 零残留、t23 时序窗真实（age≈18s<30s）、write_fault thread_local 并行可靠、保护集 release 后语义正确。host 9417→**9419**（+t25/t26）。

**CONTEXT.md 校准**：`TEL_REQ_SNAP_SYNC` 行 stale 语义补「在途计划本 tick 只读（未发步取消）」半边（蓝图 §1.1 指认的文档失准）；新增 `SnapshotRetention` 深 module 所有权行。

**收口核算**：版本 v0.350.0→**v0.351.0**（Cargo.lock 同步，gng-version PASS）；目标构建 `sel4-build-rust.sh` EXIT=0 ×2（初版+复查修复后复验，event_face/held_engine/engine 全入 target 闭包）；fmt 净；clippy 除 drv-svc 5 条既有存量外零新增。host 终态 **9419/0**。过程事故追加：门脚本一次 python 重写吞掉段A 的 `kill_window` 调用（A 段进程存活占 5577/1883 ⇒ 后续段启动失败）——跨段补 `wait_ports_free` 轮询；段序（B 先于 A2 产盘）一轮次实错重排。
