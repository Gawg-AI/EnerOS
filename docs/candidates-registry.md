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
| REG-70.8-SYNTH-NO-OBS | `crates/runtime/root-task`（qemu-test-synth 合成源）、`crates/agents/control-services/src/event_face.rs`（惰性打开）、`configs/catalog.toml`（source 1 注释） | **Open（P1 验证面，2026-09-13 首次 QEMU 实弹暴露）**：70.8 起决策输入 = **事件面冻结快照**；而 qemu-test 装配的遥测源是 root 合成轨（只走旧 `TelemetrySample` 兼容路径、**永不产生 `ObservationEvent`**）⇒ 事件面永不打开、`SNAPSHOT-INPUT-REJECT reason=no-frozen-snapshot` ×21、EA 恒 `status=NOACTION`（fail-closed，无安全风险但**主链不可证**）。本卷 QEMU 证据因此只能证到装配面（`CERTIFIER-ACTIVE` / `PLAN-SPEC-NONE` / 同代目录行）。修法二选一：合成源升级为发 `obs_batch_wire` 帧，或门内起南向从站模拟器使采集轮产出 ObservationEvent | 本表 / 执行卡 §9.5 |
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
**下一步定位（5 行内）**：把该行的 `println!` 换成"先写一个普通静态计数器
（`AtomicU32::fetch_add`）再打印"，然后由 root 心跳读该计数器——计数递增而串口
无输出 ⇒ 是 console 路径阻塞；计数不递增 ⇒ 是调用边界阻塞。**原始定位（同会话前两轮）**：trial 臂加"进入/退出"两行日志或超时，再看是 storage 未回还是 ota 未收。**追加（运行期 QMP 观测，run F，2026-09-13 同会话）**：停滞时刻**四个 CPU 的 PC 全部落在 `idle_thread`**（`0xffffff8040011118`，`wfi` + 回跳自旋；用 `aarch64-none-elf-nm -n target/sel4/kernel.elf` 逐地址判定），PSTATE=EL1h ⇒ seL4 内**没有任何可运行线程**：这不是互等自旋型死锁，而是"所有 EL0 线程都阻塞在 IPC 上、内核无事可做"。嫌疑因此收窄为**某个 `Call` 的发方在等一个永不到来的 reply**（收方被停/被重启/卡在别的 `Call` 上），而非两方互等 | 本表 / 执行卡 §5.1 |
| REG-70.12-DESC-DIGEST-VOLATILE | `crates/product/board-facts/src/digest.rs::description_digest`、`crates/runtime/root-task/src/board_select.rs`（`ENEROS_BOARD_DESCRIPTION_DIGEST`） | **Open（本会话实弹暴露的设计约束）**：同一镜像五次启动的运行时 DTB **字节非确定** ⇒ `desc_digest` 每次不同（实测 5 组：`0a8057b2…`/`9e077f5b…`/`ac882e03…`/`adb086f3…`/…）。因此**构建期常量绑定 `ENEROS_BOARD_DESCRIPTION_DIGEST` 会逐次误拒**（`platform-description-mismatch`）。可选修法：① 启动期算摘要、与已置备基线比对；② 对 DTB 做**规范化**（剔除 volatile 字段）后再摘要；③ 改为绑定 manifest/镜像身份而非描述字节。本会话仅登记（未选型） | 本表 / 执行卡 §5.1 |
| REG-70.12-S7-AUDIT-FIXES | `crates/product/board-facts/*`、`xtask/src/{board,soak}.rs`、`tests/soak/*.sh` | **Done（S7 独立复查：23 条可用发现全部复现→修复→红测）**：P1×4 = ①产品能力覆盖只查 `verified`（13 族用仓库占位事实即可拿资格，`is_hardware_evidence` 是死代码）②`schema=v2` + `entries: []` 绕过全部账本判据 ③浸泡 PANIC 扫描遇非 UTF-8 整文件跳过 ④资格记录里的 run 从不过 soak 判据（qemu scope 的 manifest 也能支撑 72h 资格）。P2/P3 = 摘要漏缺失登记/版本、键侧无长度前缀（摘要撞车）、`record_digest`/账本身份从不参与判定、只给描述摘要的绑定被静默忽略、注册项不校验摘要与修订、`parse_size` 高位溢出、`looks_numeric` 漏分隔形态、未知 kind 静默退化、v2 可省 revision、chosen 报错指错族、`phy` 让 USB PHY 冒充网卡、RFC3339 接受非法日历且极年 overflow panic、零字节原始证据/绝对路径/`..` 逃逸/缺必需产物角色、协议覆盖可 60 秒 1 样本、影响映射只哈希顶层、matrix 读失败静默、shell 零字节算已采集、campaign 取 `values()` 第一个、空执行器检测可绕过 | 本表 / 执行卡 §5.2 |
| REG-70.14-JUDGE-POLICY-OVERRIDE | `xtask/src/soak.rs::command_run_check`（`--min-hours` / `--max-heap-slope` / `--control-limit-ms` / `--max-heartbeat-gap-s`） | **Open（S7 审计 P2，登记不修）**：阈值可由调用方放宽且**不写入判据产物** ⇒ "放水后的 PASS"与"资格 PASS"字节不可区分。修法属判据策略签名设计面（谁有权放宽、如何留痕、是否双人签署），本会话只登记 | 本表 / 执行卡 §5.2 |
| REG-70.14-SHELL-CHECKER-LOOSE | `tests/soak/soak72.sh::check_boot_chain`/`check_power_cut` | **Open（S7 审计 P1/P2，登记不修——随实板执行器落地收紧）**：判据只查行数与首行形态，**不查编号唯一/连续**（100 行 `cycle 1` 或 1 行 + 99 空行均通过）。真实执行器（实板）落地时一并改为"逐行全匹配 + 编号去重计数 = 目标次数"，并把这两个篡改用例加进 `--self-test` | 本表 / 执行卡 §5.2 |
| REG-70.14-CAMPAIGN-SCHEMA | `tests/soak/soak72.sh`（写 `eneros.soak-campaign.v1`）vs `xtask/src/qa.rs::SoakCampaign` / `evidence_chain.rs` | **Open（S7 审计 P3，登记不修）**：shell 写出的 campaign JSON 字段集（`verdict`/`measured_hours`/`duration_days_measured`…）与仓内两个消费者期望的（`duration_days`/`min_duration_days`/`candidate_id`/`workload_coverage`/`fault_stats`）不一致 ⇒ 下游 `deny_unknown_fields` 直接拒 ⇒ shell 的 PASS 判定**机器不可复核**。收敛属发布面 schema 工作 | 本表 / 执行卡 §5.2 |
| REG-70.12-WORKTREE-EXTERNAL-REVERT | 全仓（`xtask/src/main.rs`、`Cargo.toml`、`product/skus/*.json`、`tests/soak/*.sh`、root-task 侧文件等 ~18 个跟踪文件） | **Open（过程事实，必须留痕）**：本会话期间跟踪文件多次被**外部进程回退到 HEAD 且不报冲突**（同一分钟内先后出现/消失；`plan_hold.rs` 反复被删）。全部改动经 `git stash`（含 20 文件差异）与 `/tmp/eneros-70.12-70.14-backup/` 完整恢复；恢复后 H-1/H-2/H-4 与新判据套件复跑全绿。交接前须 `git status --short` 核对执行卡 §3 清单 | 执行卡 §8 |

## F-70-6. 蓝图 70.6 登记（2026-09-12，v0.336.0 会话）

| ID | 锚点 | 状态 | 登记处 |
|---|---|---|---|
| REG-70.6-MGMT-DOWNLINK | `crates/protocols/mqtt/src/client.rs::poll`（recv 单次消费）+ `crates/protocols/protocol-services/src/mqtt_io.rs::forward_mgmt_requests`（驱动轮触发）+ evidence/70/tls_mqtt_broker.py（路由单测正确） | **Open（70.6 实弹）**：外部 station→MQTT→设备 下行投递失效——TLS/明文同症；上行 176 帧 TELFRAME 正常；broker 路由宿主复现正确；设备自检 `errs=126 last_err=ReadFailed`（每驱动轮 recv 失败）。70.6 门禁以 root 探针直驱 MGT1 替代取证；修复后可切回外注式判据 | 本表（70.6） |
| REG-70.6-OPS-RESTORE-N0 | `crates/agents/business-services/src/mgmt_ops.rs::MgmtOpsState::decode` | **Done（独立复查 2026-09-13 定根因并修复）**：根因 = 解码**未消费 RegionStateStore 的 8 字节 generation 前缀**（同 crate 先例 `LedgerSnapshot`/`TrialState`/`SessionCursor`/`LearnSnapshot` 均按 `generation(8) ‖ state` 解），且返回硬编码 `Generation(1)`——状态解析整体右移 8B（count 读成 state[8] 低位）→ 恢复恒 n=0、代次恒 1，即跨重启幂等账本被静默清空（已受理的副作用操作可被重复执行）。双复现：①宿主 store 往返（真实 `RegionStateStore` CAS 写 15 条 → 重开 load = n=0）；②门禁现场字节直解（`target/evidence70-vol6/data-disk.raw` 的 MgmtOps 区段 DSTA 记录，slot0/slot1 实为 n=1/n=15）。修复 = 消费前缀并返回真实代次；回归锁 = `decode_consumes_store_generation_prefix`（前缀形态断言真实记录数/代次 + 无前缀形态不得解出记录），原 `persistence_roundtrip_preserves_ledger` 改为 store 形态喂入（旧形态对本次缺陷全盲——这是漏过的直接原因） | 本表（独立复查） |
| REG-70.6-DISPATCH-HEAP-870R | `crates/agents/control-services/src/dispatch_assembly.rs`（堆顶 2656B EnvelopeIngress 分配） | **Open（P2 预存）**：qemu-prod 长窗 >900s dispatch 堆顶分配失败 PANIC（~870 Committed 轮；70.1 预算顶格 fail-fast 语义按设计触发）——70.1 门窗内不触顶，70.6 长窗实弹暴露；归预算复核（DEC-70-11）非缺陷 | 本表（70.6） |
| REG-MGT1-V2-INTEROP | `crates/agents/business-services/src/mgmt_principal.rs::verify_signature`（PrehashVerifier） | **Done（70.6 修复）**：p256 Verifier 对入参再哈希 → openssl 形态（dgst -sha256 -sign 摘要直签）全部误拒；70.5 v2 验签从未与 openssl 签发正例互操作。改 PrehashVerifier（已哈希语义）+ 测试夹具 sign_prehash 同步 + 探针正例 QEMU 实弹（R21） | 本表（70.6） |
| REG-MGT1-V2-ROLE-CHAIN | `crates/agents/business-services/src/mgmt_principal.rs`（role_chain 展开） | **Done（70.6 修复）**：v2 角色解出单名（admin）过不了 operator 判据（has_role 精确匹配 × 50 卷全继承链约定）；展开 viewer⊂operator⊂admin 全链 + 测试同步 | 本表（70.6） |
| REG-70.5-MGT-GATE-NO-POSITIVE | `tools/run70-vol6-gate.sh`（R19/R20/R21） | **Done（70.6 收口）**：合法 v2 签名正例（探针 maintenance-enter rid=100 → Completed 维护生效）+ 短尾 v2 负例（truncated v2 shape 词形）+ 伪 principal 回归三项 QEMU 实弹 | 本表（70.6） |
| REG-70.6-READINESS-WINDOW | `crates/runtime/root-task/src/service.rs::READINESS_DEADLINE_MS`（3_000→15_000） | **Done（70.6 裁决）**：dispatch 装配（catalog/binding/chain-key + durable 打开块 IO）ready 信号 TCG 抖动下晚至 8-10s，3s 窗两轮实弹误降级 required 服务；15s 保底永不发信号有界判定 | 本表（70.6） |
