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
| REG-70.5-CAPSULE-STALENESS | `crates/boot/boot-manager-uefi/src/main.rs`（capsule 页分配失败 = soft degrade，失败页**非本程序所有不得写零**——70.5 复查裁决定性）+ `crates/runtime/root-task/src/service.rs::read_identity_capsule`（decode 通过即信任，无新鲜性/绑定核验） | **Open（70.5 复查，候选 70.6）**：暖重启（RAM 保持）+ 本次 capsule 页分配失败 → 上一次 boot 的 capsule 复活且被 root 照常推送（旧 slot/gen/secver/floor）——旧 floor 低于本次锚水位时准入窗口悄然放宽（撤销绕过面）。修法候选 = root 侧 `capsule.product_version ≡ 运行镜像 CARGO_PKG_VERSION` 单点核验（image_factory ProductVersion 单源保证同 build 同值；异槽/异版残留 capsule 必被拒），**须 QEMU 实弹验证后落**（引导主路径新增 fail-closed 拒绝，误伤代价 = OTA 全 not-ready）；根治 = RootBootInfo 通道（F3-14）交叉核验四元组 | 本表（70.5 复查） |
| REG-70.5-OTA-FETCH-TOCTOU | `crates/agents/business-services/src/bin/svc-ota.rs::fetch_config`（get_entry("ota") 与 get_generation 两步非原子） | **Open（70.5 复查，登记不修）**：两步取数间配置束轮换可装配"旧 entry 几何 + 新 generation"杂合 policy——方向上大半 fail-closed（root_key 取自 entry，旧签才可过），且轮换=停机换包（§9 冻结表）下窗口不可达；在线轮换立项前必须先单事务化取数 | 本表（70.5 复查） |
| REG-70.5-DESC-NONCANON-WORD | `crates/agents/agent-sdk/src/ota_wire.rs`（magic 错/非 canonical 填充均 `DecodeError::Short`）+ `ota_service.rs`（parse 错误统一 `ERR:len`） | **Open（70.5 复查，诊断缺口登记）**：镜像摘要篡改（desc-signature-invalid）与填充区篡改（ERR:len）失败环节不可分；功能失败关闭无洞，纯可诊断性——改词形须与 gate 判据/负例同频评估（词形退役惯犯类），归后续卷 | 本表（70.5 复查） |
| REG-70.5-MGMT-EXPIRES-UNIT | `crates/agents/business-services/src/mgmt_rpc.rs::is_valid_at`（now_ms 单调毫秒）vs `AuthenticatedSession::expires_at_ms`（v2 = unix 毫秒） | **Open（70.5 复查，登记不修）**：单位错配使 is_valid_at 对 v2 恒真——真过期判据在 `verify_v2_tail`（无活洞），但 `AuthenticatedSession` 作为公共合同藏着单位陷阱（v1 面同样经它）；`tenant_scope`/`channel_binding` 全链无授权消费点（签名绑定但未强制——70.5 卡面自认，授权消费设计归 70.6+ 管理面卷） | 本表（70.5 复查） |
| REG-70.5-TLS-DROP-SOCKET | `crates/security/tls/src/target_factory.rs::TlsTransportHandle::drop`（只还缓冲守卫，内层 socket 无显式 abort/归还） | **Open（70.5 复查，登记不修）**：Drop 不经 close() = smoltcp socket 槽位无显式归还/对端无 FIN——当前唯一析构路径 `plugin.stop() → client.disconnect() → transport.close()` 显式（Drop 不可达）；若未来出现 handle 被丢弃的恢复路径须先补此语义 | 本表（70.5 复查） |
| REG-70.5-SVC-MGMT-HEADER-MAGIC | `crates/agents/business-services/src/bin/svc-mgmt.rs::handle`（req[0..5] "MGT1" 魔数从不校验） | **Open（70.5 复查，观察项）**：分发器（control-services）按服务路由已隔离异协议流量；本面零当前攻击者，登记防误接通（若有 bin 直连 svc-mgmt 端点的新装配，先补头部校验） | 本表（70.5 复查） |

## F-70-6. 蓝图 70.6 登记（2026-09-12，v0.336.0 会话）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.6-MGMT-DOWNLINK | `crates/protocols/mqtt/src/client.rs::poll`（recv 单次消费）+ `crates/protocols/protocol-services/src/mqtt_io.rs::forward_mgmt_requests`（驱动轮触发）+ evidence/70/tls_mqtt_broker.py（路由单测正确） | **Open（70.6 实弹）**：外部 station→MQTT→设备 下行投递失效——TLS/明文同症；上行 176 帧 TELFRAME 正常；broker 路由宿主复现正确；设备自检 `errs=126 last_err=ReadFailed`（每驱动轮 recv 失败）。70.6 门禁以 root 探针直驱 MGT1 替代取证；修复后可切回外注式判据 | 本表（70.6） |
| REG-70.6-OPS-RESTORE-N0 | `crates/agents/business-services/src/mgmt_ops.rs`（Encode/Decode 129B 行）+ `bin/svc-mgmt.rs::persist_ops/restore` | **Open（P2）**：账本持久化写成功（persisted gen=2 n=15 串口词形）而跨重启恢复 n=0——编码/加载/介质三者之一的记录内容丢失点待查；恢复机制本身已通（CONFIG-DISK 同径恢复正常） | 本表（70.6） |
| REG-70.6-DISPATCH-HEAP-870R | `crates/agents/control-services/src/dispatch_assembly.rs`（堆顶 2656B EnvelopeIngress 分配） | **Open（P2 预存）**：qemu-prod 长窗 >900s dispatch 堆顶分配失败 PANIC（~870 Committed 轮；70.1 预算顶格 fail-fast 语义按设计触发）——70.1 门窗内不触顶，70.6 长窗实弹暴露；归预算复核（DEC-70-11）非缺陷 | 本表（70.6） |
| REG-MGT1-V2-INTEROP | `crates/agents/business-services/src/mgmt_principal.rs::verify_signature`（PrehashVerifier） | **Done（70.6 修复）**：p256 Verifier 对入参再哈希 → openssl 形态（dgst -sha256 -sign 摘要直签）全部误拒；70.5 v2 验签从未与 openssl 签发正例互操作。改 PrehashVerifier（已哈希语义）+ 测试夹具 sign_prehash 同步 + 探针正例 QEMU 实弹（R21） | 本表（70.6） |
| REG-MGT1-V2-ROLE-CHAIN | `crates/agents/business-services/src/mgmt_principal.rs`（role_chain 展开） | **Done（70.6 修复）**：v2 角色解出单名（admin）过不了 operator 判据（has_role 精确匹配 × 50 卷全继承链约定）；展开 viewer⊂operator⊂admin 全链 + 测试同步 | 本表（70.6） |
| REG-70.5-MGT-GATE-NO-POSITIVE | `tools/run70-vol6-gate.sh`（R19/R20/R21） | **Done（70.6 收口）**：合法 v2 签名正例（探针 maintenance-enter rid=100 → Completed 维护生效）+ 短尾 v2 负例（truncated v2 shape 词形）+ 伪 principal 回归三项 QEMU 实弹 | 本表（70.6） |
| REG-70.6-READINESS-WINDOW | `crates/runtime/root-task/src/service.rs::READINESS_DEADLINE_MS`（3_000→15_000） | **Done（70.6 裁决）**：dispatch 装配（catalog/binding/chain-key + durable 打开块 IO）ready 信号 TCG 抖动下晚至 8-10s，3s 窗两轮实弹误降级 required 服务；15s 保底永不发信号有界判定 | 本表（70.6） |
