# Checklist — v0.99.0 联邦共识协议（PBFT 变体）

> Spec：`spec.md`（develop-v0990-pbft-consensus）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: eneros-federation 扩展 `src/consensus.rs` / `src/pbft.rs` / `src/view_change.rs`（既有 crate `crates/agents/federation/` 内），未新增根目录 crate
- [x] C2: 根 `Cargo.toml` workspace 成员无新增（eneros-federation 已为成员），workspace 仍可解析
- [x] C3: eneros-federation `Cargo.toml` 依赖不变（eneros-crypto path 引用既有），无新增第三方依赖
- [x] C4: 新文档 `pbft-consensus-design.md` 位于 `docs/agents/`，未平面化放 `docs/` 根
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-federation`（既有 120 + 新增 40 = 160）全部通过；`cargo test -p eneros-crypto`（417）零回归
- [x] C8: `cargo build -p eneros-federation --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy -p eneros-federation --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/agents/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe` / `async`（no_std 合规；子模块不重复加 no_std attr；测试模块内 `std::` 位于 `#[cfg(test)]` 下允许）

## D. consensus.rs 数据结构与总线（C16~C35）

- [x] C16: `pub type NodeId = u64`（D2）
- [x] C17: `ConsensusState { Idle, PrePrepare, Prepare, Commit, Done }` 5 变体（D6 增 Idle），派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C18: `MsgType { PrePrepare, Prepare, Commit, Reply, ViewChange }` 5 变体（D4 增 ViewChange），`to_u8()/from_u8()` 往返一致
- [x] C19: `PbftMessage { msg_type, view: u64, sequence: u64, digest: [u8;32], payload: Vec<u8>, sender: NodeId, signature: [u8;64] }` 字段全 pub，派生 Debug/Clone/PartialEq（D4 含 sender/payload）
- [x] C20: `LogEntry` voter 为 `BTreeSet<NodeId>`（D5），`prepare_count()/commit_count()` 访问器返回集合长度
- [x] C21: `LogEntry` 字段全 pub（sequence/request/digest/prepare_voters/commit_voters/prepared/committed/executed）
- [x] C22: `ConsensusResult { sequence, digest, view }` 派生 Debug/Clone/PartialEq
- [x] C23: `ConsensusError` 7 变体 `{ NotPrimary, UnknownNode, InvalidSignature, ViewMismatch, StaleMessage, NotEnoughNodes, BusError }`（D10），派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C24: `ConsensusBus` 为 sync trait（broadcast/receive，无 async，无 Send+Sync，D3）
- [x] C25: `MockConsensusBus` 字段全 pub（queues/isolated/fail_times）
- [x] C26: `MockConsensusBus::broadcast` 向除 isolated 外所有节点邮箱投递；isolated 节点不投出也不收信
- [x] C27: `MockConsensusBus::broadcast` fail_times>0 → 递减 + `Err(BusError)`
- [x] C28: `MockConsensusBus::receive` 弹出该节点队首，空 → None
- [x] C29: `ConsensusEngine` **不 derive Debug**（含 Sm2KeyPair，私钥保护硬约束），字段全 pub
- [x] C30: `ConsensusEngine::new` 合法输入 → view==0/sequence==0/state==Idle/4 计数器全零/last_latency_ms==0/consecutive_vc==0
- [x] C31: `new`：nodes 为空或含重复 → `Err(NotEnoughNodes)`
- [x] C32: `new`：nodes 不含 local_id 或 peers 缺节点公钥 → `Err(UnknownNode)`
- [x] C33: `is_primary()` ==（`primary_of(nodes, view) == local_id`）
- [x] C34: `is_committed(seq)` 仅在日志条目标记 committed 后返回 true
- [x] C35: `handle_message` 前置校验：msg.view < self.view → rejected_count+=1 + `Err(StaleMessage)`；未知 sender → `Err(UnknownNode)`；验签失败 → rejected_count+=1 + `Err(InvalidSignature)`

## E. pbft.rs 三阶段（C36~C58）

- [x] C36: `f(n) = (n-1)/3`（n=4→1，n=7→2，n=1→0）
- [x] C37: `quorum(n) = 2*f(n)+1`（n=4→3，n=7→5，n=1→1）
- [x] C38: `primary_of(nodes, view) = nodes[(view % len) as usize]`，视图轮换正确
- [x] C39: `sign_message/verify_message`：msg_body = `msg_type:u8‖view:u64be‖sequence:u64be‖digest‖sender:u64be（‖payload）` 域分离（D9）；正签可验、错签/错公钥拒验
- [x] C40: `submit` 非主节点 → `Err(NotPrimary)`，无广播无日志
- [x] C41: `submit` 主节点：seq=sequence+1、digest=SM3(request)、PrePrepare 含 payload 与有效签名
- [x] C42: `submit` 后 LogEntry 建立（prepare_voters={local_id}，D7 主票）、state=PrePrepare、submit_count+=1、返回 seq
- [x] C43: 备份 `on_pre_prepare`：sender 非该 view 主节点 → rejected_count+=1 + `Err(ViewMismatch)`
- [x] C44: 备份 `on_pre_prepare`：digest ≠ SM3(payload) → rejected_count+=1 + `Err(StaleMessage)`
- [x] C45: 备份 `on_pre_prepare` 合法 → 建 LogEntry（prepare_voters={primary}）+ 广播 Prepare + state=Prepare
- [x] C46: 重复 PrePrepare（同 seq）→ 忽略 Ok(None)，不重复建条目
- [x] C47: `on_prepare`：voter BTreeSet 插入（同 sender 重复投票去重，C 拜占庭防护）
- [x] C48: `on_prepare` 达 quorum 且未 prepared → prepared=true → commit_voters.insert(local_id) → 广播 Commit → state=Commit
- [x] C49: `on_commit` 达 quorum 且未 committed → committed/executed=true → sequence=seq → state=Done → committed_count+=1 → `Ok(Some(ConsensusResult))`
- [x] C50: `last_latency_ms` = 提交时刻 − 请求受理时刻（注入时钟，D12）
- [x] C51: 4 节点全链路：submit → 循环 poll → 全部 4 节点 `is_committed(1)==true`、digest 一致、state==Done（蓝图 §6.2）
- [x] C52: n=4 且 1 备份 isolated（拜占庭静默）→ 其余 3 节点仍 committed（主票+2 备份票=3=quorum，蓝图 §7.3 容错）
- [x] C53: 拜占庭伪造签名 Prepare → 各诚实节点 rejected_count+=1，日志无该票
- [x] C54: 拜占庭 equivocation（向不同节点发不同 digest PrePrepare）→ 任一 digest 均无法收齐 quorum，无节点 committed（安全性）
- [x] C55: 错误 view 的 Prepare/Commit → `Err(StaleMessage)` 丢弃
- [x] C56: 7 节点（f=2，quorum=5）共识达成
- [x] C57: n=1 单节点（f=0，quorum=1）自共识达成
- [x] C58: 连续多次 submit（seq 1→2→3）逐序提交，sequence 单调递增

## F. view_change.rs 视图切换（C59~C72）

- [x] C59: `check_timeout`：state ∈ {Idle, Done} → `Ok(false)`，无 VC 广播
- [x] C60: `check_timeout`：now − last_progress ≤ 有效超时 → `Ok(false)`
- [x] C61: 有效超时 = `timeout_ms << min(consecutive_vc, 3)`（D8 指数退避）
- [x] C62: `check_timeout` 超时 → 广播 ViewChange（msg_type=ViewChange、msg.view=self.view+1、有效签名）→ view_change_count+=1 → consecutive_vc+=1 → `Ok(true)`
- [x] C63: `on_view_change`：msg.view ≤ self.view → `Ok(false)` 陈旧忽略
- [x] C64: `on_view_change`：伪造签名 → rejected_count+=1 + `Err(InvalidSignature)`
- [x] C65: VC 票集 BTreeSet 去重（同节点重复 VC 仅计 1 票）
- [x] C66: VC 达 quorum → `enter_view(new_view)`：view 更新、state=Idle、consecutive_vc=0、last_progress_ms=now
- [x] C67: 无独立 NewView 消息（D8：VC 广播法定人数后各节点自主收敛）
- [x] C68: `enter_view` 后新主节点对尾部未 committed 日志重发 PrePrepare（同 sequence/digest，恢复共识）
- [x] C69: 主节点 isolated → 备份超时 → 3 备份 VC 达 quorum → enter_view(1) → 新主重发 → 3 诚实节点 committed（蓝图 §6.5 故障注入）
- [x] C70: 共识正常推进中 `check_timeout` → `Ok(false)`，无 VC 消息（不误触发）
- [x] C71: 连续两轮主离线（view 1 主再 isolated）→ 二次 VC 成功且退避间隔增大（防 ViewChange 风暴，蓝图 §8.5）
- [x] C72: enter_view 后 `is_primary()` 反映新视图主节点

## G. 配置文件（C73~C78）

- [x] C73: `configs/federation-consensus.toml` 存在，`[consensus]` 段含 mode / timeout_ms / max_view_change_backoff / nodes
- [x] C74: mode 语义注释：P2 降级说明（默认部署 "primary-backup" 用 v0.92.0 主从仲裁，跨信任域 Byzantine 场景才启用 "pbft"）
- [x] C75: 中文注释 ≥6 点（3f+1 假设 / quorum 2f+1 / 共识 <1s §7.2 / ViewChange 退避 §8.5 / 签名验证 §7.3 / 计数器+延迟观测 D12）
- [x] C76: nodes 示例为 4 节点（满足 3f+1，f=1）
- [x] C77: timeout_ms 默认值与共识 <1s（§7.2）匹配（如 3000ms 内可完成 + 一次超时余量）
- [x] C78: 配置项命名与文档 §配置章节一致

## H. 设计文档（C79~C86）

- [x] C79: `docs/agents/pbft-consensus-design.md` 存在且 12 章节齐全（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
- [x] C80: Mermaid 图 ≥2（三阶段时序图（蓝图 §4.3 重绘）+ ViewChange 决策流程图）
- [x] C81: ViewChange 流程图含 StaleMessage / InvalidSignature / 退避 / 新主重发 PrePrepare 分支
- [x] C82: D1~D12 偏差表与 spec.md 一致
- [x] C83: 接口契约章节与实现签名一致（含 sync 化 D3 说明）
- [x] C84: P2 降级说明独立章节（默认主从仲裁 v0.92.0；pbft 经配置启用；跨信任域适用场景）
- [x] C85: 安全章节覆盖拜占庭防护 5 类（伪签/假 digest/重复票/错 view/equivocation）
- [x] C86: 性能章节声明共识 <1s（4 节点，蓝图 §7.2）与测量口径（注入时钟 last_latency_ms）

## I. 版本同步（C87~C91）

- [x] C87: 根 `Cargo.toml` `[workspace.package] version = "0.99.0"`
- [x] C88: `Makefile` 版本注释同步 0.99.0
- [x] C89: `.github/workflows/ci.yml` 版本注释同步 0.99.0
- [x] C90: `ci/src/gate.rs` 注释串尾追加 v0.99.0 类型清单（ConsensusEngine/PbftMessage/ConsensusResult/ConsensusError/ConsensusBus/MockConsensusBus/f/quorum/primary_of 等）
- [x] C91: eneros-federation `Cargo.toml` description 升级三版本

## J. 测试覆盖（C92~C104）

- [x] C92: consensus.rs 内嵌 12 测试（TC1~TC12）通过
- [x] C93: pbft.rs 内嵌 16 测试（TP13~TP28）通过
- [x] C94: view_change.rs 内嵌 12 测试（TV29~TV40）通过
- [x] C95: 新增测试总计 40 个，`cargo test -p eneros-federation` 160 全过
- [x] C96: 派生与编解码测试覆盖（TC1~TC4）
- [x] C97: Mock 总线投递/隔离/故障注入测试覆盖（TC7~TC9）
- [x] C98: 引擎构造三错误路径测试覆盖（TC10~TC12）
- [x] C99: 拜占庭 5 类攻击测试覆盖（TP23~TP27 + C53/C54）
- [x] C100: ViewChange 全链路测试覆盖（TV33~TV37 + C69）
- [x] C101: 退避防风暴测试覆盖（TV32/TV40 + C71）
- [x] C102: 计数器累计测试覆盖（submit/committed/rejected/view_change 4 计数器 + last_latency_ms）
- [x] C103: 既有 120 测试零回归（membership/discovery/channel/tunnel）
- [x] C104: eneros-crypto 417 测试零回归（本版本未改动 crypto，SM2/SM3 复用）

## K. 蓝图对齐与验收（C105~C115）

- [x] C105: v0.99.0 交付物全覆盖：consensus/pbft/view_change 3 模块 / ConsensusEngine / PbftMessage / ConsensusResult（蓝图 §3）
- [x] C106: 三阶段 PrePrepare/Prepare/Commit 实现（蓝图 §5.2/§9 功能）
- [x] C107: ViewChange 容忍主节点故障（蓝图 §5.2/§9 可靠）
- [x] C108: 容忍 f 拜占庭节点（蓝图 §7.3：n=3f+1、quorum=2f+1、签名验证、投票去重）
- [x] C109: 共识延迟可观测（蓝图 §9：last_latency_ms）
- [x] C110: 节点配置化（蓝图 §9 可维护：nodes 注入 + toml 配置）
- [x] C111: 签名验证失败丢弃（蓝图 §4.4）
- [x] C112: 主节点超时 → ViewChange（蓝图 §4.4）
- [x] C113: 上游 v0.98.0 通道复用关系在文档声明（§5.5 交互；本版本总线为抽象 seam，生产经 channel/tunnel 适配注入）
- [x] C114: 下游 v0.100.0 竞价解锁声明（文档 §5.5 交互）
- [x] C115: 附录验收矩阵对齐：PBFT 2f+1 容错达成（蓝图 appendix：联邦共识 v0.99.0）

---

## 验收记录（2026-07-19 收工核验）

- **B 构建校验实测**：C6 `cargo metadata` 通过；C7 `cargo test -p eneros-federation` 160/160（既有 120 + 新增 40）通过，`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全量回归全绿；C8 aarch64-unknown-none 交叉编译通过；C9 fmt 通过；C10 clippy 0 warning；C11 `cargo deny check` 全项 ok（零新增第三方依赖，SBOM 不变）。
- **C7/C104 数字订正**：spec 编制时估记 eneros-crypto 417 测试，实测当前为 **358/358** 通过；v0.99.0 未改动 crypto crate 任何源码（纯复用 SM2/SM3），零回归成立，数字以实测为准。
- **实现期 clippy 修正 3 处**（收工前修复并复验）：`peers.insert(id, kp.public_key)` 去 clone（Sm2PublicKey 为 Copy）；`make_msg` 测试辅助加 `#[allow(clippy::too_many_arguments)]`（参数与 PbftMessage 字段一一对应）；`vc_votes.get(&2).is_none()` 改 `!contains_key(&2)`。
- **既有 flaky 声明**：全量回归首轮 `eneros-user-heap::test_user_heap_integration` 失败 1 次；该测试主动触发 OOM 验证自定义 handler，对并行负载敏感——单独连续 3 次复跑全过，次轮全量回归亦通过，确认为既有并行 flaky，与 v0.99.0 改动（纯增量 federation 3 模块）无因果。
- **EX1~EX9 偏差表**已双向同步 `src/lib.rs` crate 文档与 `docs/agents/pbft-consensus-design.md` §12.1。
