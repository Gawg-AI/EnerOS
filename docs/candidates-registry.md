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
| REG-70.11-HOLD-UNWIRED | `crates/protocols/control-dispatch/src/plan_hold.rs`、`crates/agents/control-services/src/{event_face,telemetry}.rs` | **Open（P1，如实登记——原卡面/登记册口径失实已订正）**：`PlanHolder` **生产零调用方**（全仓仅 `lib.rs` 的 `pub mod`），`has_outstanding_input` / svc-telemetry `hold_input_offset` 同样零调用方 ⇒ **70.8.2 删除界的"计划输入持有"半边在生产恒空**、70.11.2 的"装配方轮询接线"从未存在；且 `hold_offset` **只收紧无释放面**（首个接线方会把删除界永久钉住 → 保留背压常驻）。接线需先定释放面与 dispatch→telemetry 服务间通道（属 70.11.3/.11.4），本会话**不建空转基础设施**，只订正口径并登记 | 本表 / 执行卡 §9 |
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
