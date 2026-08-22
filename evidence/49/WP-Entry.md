# 蓝图 49 卷 Entry 登记（锚点重验 + 前置检查）

> 日期：2026-08-22 · 基线版本：0.313.30 · 基线 commit：e6c8680（48 卷复验
> 收尾登记提交；复验实弹 HEAD=ba36fec）

## 锚点重验（44-00 §4.5.1，全部实跑）

| 锚点 | 结果 |
|---|---|
| 目标侧后端 UnavailableBackend 默认 | ✅ `control-services/src/ai_worker.rs:51-64`（UnavailableBackend→`DecisionReject::Unavailable`）；`:70-153` HeuristicBackend SOC 死区 ±5kW（discharge≥0.5/charge≤0.3/power_kw=5.0） |
| `svc-ai-worker.rs:38-44` 注释 | ✅ L40 `[svc-ai-worker] backend=heuristic (Llama/HiGHS not declared)`（在 38-44 区间内） |
| llm-engine = llama.cpp FFI（feature 门控）| ✅ `crates/ai/llm-engine/` 在库；runner=host bin（G-SW1） |
| `libllama.a` 为 x86-64 host 构建 | ✅ 复跑 readelf：`ar x libllama.a llama.cpp.o && readelf -h` → `系统架构: Advanced Micro Devices X86-64`（2026-08-22 实证） |
| 491MB 模型进不了 init bundle（ADR-0036） | ✅ `third_party/test-model.gguf` = 491400032 B（491MB，Qwen2.5-0.5B-Instruct-q4_k_m） |
| DualBrainCoordinator target 零调用 | ✅ 引用仅：energy-market-agent（不在 target 装配）、ai/runner（host 工具）、dual-brain 自身测试——target 服务零引用 |
| 仅类型+DecisionGate 进装配 | ✅ 实质成立但**行号漂移**：`svc-control-dispatch.rs` 中 dual-brain 引用实际在 L348（`gate::DecisionGate::new` rate gate）+ L358（`audit::DecisionAudit::new`），非锚点所写 ：111（该行现为 InMemoryIntentStore 兜底分支）。见锚点修订记录 |
| intent-parser/fast-path/infer-scheduler/prompt-template 被拖进闭包零调用 | ✅ 反向依赖 grep：四 crate 仅被 dual-brain/runner/energy-market-agent/彼此引用；target 经 control-services→dual-brain 类型边拖入，无目标侧调用 |
| intent-contract 可复用 | ✅ `crates/ai/intent-contract`（no_std+alloc，serde_json D6 合规）——49.2 契约层宿主 |
| safety-validator trusted 输入校验 | ✅ `crates/ai/safety-validator/src/trusted.rs`（838 行） |
| model-ota 3098 行/SM2 验签/不在装配 | ✅ `wc -l crates/agents/model-ota/src/*.rs` = 3098 total（精确吻合锚点） |
| MQTT 链（46 卷后可用） | ✅ svc-mqtt EL0 服务在装配（virtio 槽 10，slirp→host broker 10.0.2.2:1883）；47 卷下行写意图链 INTENT-FWD→dispatch 在库 |
| ProposedControlPlan/DecisionPath/rate gate 在 dispatch 侧 | ✅ `dual_brain::decision` 类型经 ai_worker/dispatch 双侧消费；rate gate=`svc-control-dispatch.rs:348` |

## 前置检查

1. **47 卷 Exit 已达成**：状态板 Accepted（2026-08-22 收口，0.313.24，
   f55bdd6/71a74a3）；`evidence/47/exit-gate.{md,json}` 在库。
2. **上卷回归（§4.5.2，≥2 项照抄实跑）**：
   - 104 回归家族精确对账：`cargo test -p eneros-iec104-master -p eneros-iec104-slave -p eneros-iec60870-asdu` → 非 doc 计数 72+1+1+105+1+51 = **231** ✅（= 48 卷登记 master 74+slave 106+asdu 51）
   - CF01：`cargo xtask qa gate config-refs` → `CONFIG-REFS-CF01 → OK（active 17…）` ✅
   - （46 抽查项）l2 json×6 VERIFIED 绑 4f7344c ✅；（47 抽查项）w40-prod-key grep=0 ✅

## 锚点修订记录（日期 + 差异 + 命令输出）

- 2026-08-22：锚点「bin/svc-control-dispatch.rs:111」行号漂移——`grep -n
  "dual_brain" svc-control-dispatch.rs` 实际命中 L348/L358（L111 现为
  storage EP 未授予分支的 InMemoryIntentStore 兜底）。内容实质不变（仅
  类型+Gate+Audit 进装配），按新行号修订，不带偏差开发。

## 执行模式确认（卷首覆盖条款）

ADR-0045 **不停等签字**：owner D1 裁决（保留 LLM）+ 本卷推荐方案 B 即 ADR
依据，撰写后直接按方案 B 连续实施 49.2；owner 签字为 Exit 登记项（未签
如实登记 BLOCKED（owner 动作），不阻塞实施）。spike（49.3）三关卡顺序执行，
任一失败即 NO-GO 结论后继续 49.4 与 Exit。

## 文件清单（§4.5.3，开工前登记；卷内未列文件不动）

- **49.1**：`docs/adr/ADR-0045-system2-supply-architecture.md`(新)、
  `docs/adr/registry.toml`(+1)、`evidence/49/WP-Entry.md`(新)、
  `evidence/49/WP-49.1.md`(新)、workspace 版本 bump。
- **49.2**：`crates/ai/intent-contract/src/peer.rs`(新)+lib.rs(mod 注册)、
  `crates/agents/control-services/src/remote_brain.rs`(新)+lib.rs(mod)、
  `bin/svc-ai-worker.rs`（PeerLink EIPC+配置消费+审计留痕）、
  `crates/protocols/mqtt/src/plugin.rs`+`lib.rs`（app 平面：第二订阅+发布
  outbox）、`protocol-services/src/bin/svc-mqtt.rs`（响应转发+发布入队）、
  `runtime/root-task/src/spawn.rs`（EIPC_GRANTS +2）、
  `configs/ai-peer.toml`(新)+`configs/protocol-mqtt.toml`(+键)+
  `configs/registry.toml`(+ACTIVE)+xtask manifest CF01 链接、
  `scripts/peer_brain.py`(新，host 参考实现)、`evidence/49/WP-49.2.md`、bump。
- **49.3**：`docs/ai/local-inference-spike.md`(新)、
  `evidence/49/WP-49.3.md`+spike 原始数据(新)、bump。**禁止向 target 闭包
  加入任何推理代码**（spike 原型不入仓 workspace/target 图）。
- **49.4**：`crates/ai/dual-brain/Cargo.toml`(+lib.rs 门控)+消费方
  Cargo.toml 对齐、closure 死重清单证据、二进制体积对比、
  `evidence/49/WP-49.4.md`、bump。
- **Exit**：`evidence/49/exit-gate.{json,md}`、状态板、47/48 抽查各 2 项。
