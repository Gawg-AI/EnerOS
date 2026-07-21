# Tasks — v0.99.0 联邦共识协议（PBFT 变体）

> Spec：`spec.md`（develop-v0990-pbft-consensus）。蓝图：`蓝图/phase2.md` v0.99.0（无刚性子版本）。
> 全部 no_std + alloc 合规；eneros-federation 追加 consensus.rs / pbft.rs / view_change.rs 3 模块；既有 4 模块（membership/discovery/channel/tunnel）零改动；零新增第三方依赖。

- [ ] Task 1: eneros-federation 骨架扩展
  - [ ] SubTask 1.1: `Cargo.toml` description 升级三版本（v0.97.0 + v0.98.0/v0.98.1 + v0.99.0）；依赖不变（eneros-crypto 已在 v0.98.0 引入）
  - [ ] SubTask 1.2: `src/lib.rs` 追加 `pub mod consensus; pub mod pbft; pub mod view_change;` + 新增类型全量重导出（NodeId/ConsensusState/MsgType/PbftMessage/LogEntry/ConsensusResult/ConsensusError/ConsensusBus/MockConsensusBus/ConsensusEngine/f/quorum/primary_of/sign_message/verify_message）+ crate 文档追加 v0.99.0 说明与 D1~D12 偏差表（既有文档与重导出保留）
  - 验证：`cargo metadata --format-version 1` 成功

- [ ] Task 2: 实现 `src/consensus.rs` — 数据结构与总线抽象 + 12 测试
  - [ ] SubTask 2.1: `pub type NodeId = u64`；`ConsensusState { Idle, PrePrepare, Prepare, Commit, Done }`；`MsgType { PrePrepare, Prepare, Commit, Reply, ViewChange }` + `to_u8/from_u8`；`ConsensusError` 7 变体（均 Debug/Clone/Copy/PartialEq/Eq，D6/D10）
  - [ ] SubTask 2.2: `PbftMessage`（7 字段全 pub：msg_type/view/sequence/digest/payload/sender/signature，Debug/Clone/PartialEq，D4）；`LogEntry`（voter BTreeSet + prepared/committed/executed + `prepare_count()/commit_count()` 访问器，D5）；`ConsensusResult { sequence, digest, view }`
  - [ ] SubTask 2.3: sync trait `ConsensusBus`（broadcast/receive，D3）+ `MockConsensusBus { queues, isolated, fail_times }`（isolated 节点不投不收模拟离线；fail_times → Err(BusError)；字段全 pub）
  - [ ] SubTask 2.4: `ConsensusEngine` 字段全 pub（nodes/local_id/view/sequence/state/log/kp/peers/timeout_ms/last_progress_ms/consecutive_vc/4 计数器/last_latency_ms；**禁 derive Debug** 因含 Sm2KeyPair）+ `new()` 构造校验（空 nodes/重复/不含 local_id/peers 缺节点 → NotEnoughNodes/UnknownNode）+ `is_primary()` / `is_committed(seq)`
  - [ ] SubTask 2.5: `submit()` 主节点路径骨架（调 pbft.rs 逻辑）+ `poll()` 排空邮箱驱动 + `handle_message()` 前置校验（view 过期/未知 sender/验签失败 → rejected_count+=1）与按 msg_type 分发
  - [ ] SubTask 2.6: 内嵌测试 TC1~TC12（派生/to_u8 往返/MsgType 含 ViewChange TC1~TC4；LogEntry 访问器与 voter 去重 TC5~TC6；Mock 投递/隔离/故障注入 TC7~TC9；new 校验三错误路径 + is_primary 视图轮换 TC10~TC12）
  - 验证：`cargo test -p eneros-federation consensus` 12 通过

- [ ] Task 3: 实现 `src/pbft.rs` — 三阶段 + 16 测试
  - [ ] SubTask 3.1: 自由函数 `f(n)=(n-1)/3` / `quorum(n)=2f+1` / `primary_of(nodes,view)=nodes[view%len]`（D7）；`sign_message/verify_message`（D9 域分离 msg_body = `type:u8‖view‖seq‖digest‖sender(‖payload)`）
  - [ ] SubTask 3.2: `submit` 主路径：非主 → NotPrimary；digest=SM3(request)；PrePrepare（payload=request + 签名）broadcast；LogEntry（prepare_voters={local}，D7 主票）；state=PrePrepare；submit_count+=1
  - [ ] SubTask 3.3: `on_pre_prepare`：非主 sender → ViewMismatch；digest≠SM3(payload) → StaleMessage；重复 seq 忽略；备份建条目（prepare_voters={primary}）+ 广播 Prepare + state=Prepare
  - [ ] SubTask 3.4: `on_prepare`：voter 去重插入；`>= quorum && !prepared` → prepared → commit_voters.insert(local) → 广播 Commit → state=Commit
  - [ ] SubTask 3.5: `on_commit`：voter 去重；`>= quorum && !committed` → committed/executed → sequence=seq → state=Done → committed_count+=1 → last_latency_ms → Ok(Some(ConsensusResult))
  - [ ] SubTask 3.6: 内嵌测试 TP13~TP28（f/quorum/primary_of 数学 TP13~TP15；签名验签正/反 TP16~TP17；非主 submit TP18；4 节点全链路共识 digest 一致 TP19~TP21；1 备份 isolated 仍达成（quorum=3 含主票）TP22；拜占庭：伪签/假 digest/重复票/错 view TP23~TP26；equivocation 双 digest 不安全态不提交 TP27；7 节点 f=2 共识 TP28）
  - 验证：`cargo test -p eneros-federation pbft` 16 通过

- [ ] Task 4: 实现 `src/view_change.rs` — 视图切换 + 12 测试
  - [ ] SubTask 4.1: `check_timeout`：Idle/Done → false；有效超时 = `timeout_ms << min(consecutive_vc, 3)`（D8 退避）；超时 → 广播 ViewChange（msg.view=self.view+1）→ view_change_count+=1 → consecutive_vc+=1 → true
  - [ ] SubTask 4.2: `on_view_change`：msg.view ≤ self.view 忽略；VC 票集 BTreeSet 去重收集；达 quorum → `enter_view`
  - [ ] SubTask 4.3: `enter_view`：view 更新 → state=Idle → consecutive_vc=0 → last_progress_ms 重置；新主对尾部未 committed 日志重发 PrePrepare（D8 恢复）
  - [ ] SubTask 4.4: 内嵌测试 TV29~TV40（正常推进无 VC TV29；Idle/Done 无 VC TV30~TV31；退避倍增 TV32；主 isolated → 超时 → 3 备份 VC 达 quorum → enter_view(1) TV33~TV35；新主重发 PrePrepare 恢复共识至 committed TV36~TV37；陈旧 VC（msg.view ≤ view）忽略 TV38；伪造签名 VC 拒绝 TV39；VC 后连续多轮（二次主离线）指数退避生效 TV40）
  - 验证：`cargo test -p eneros-federation view_change` 12 通过

- [ ] Task 5: 新增配置文件 `configs/federation-consensus.toml`
  - [ ] SubTask 5.1: `[consensus]` 段：mode = "pbft"（P2 降级说明：默认 "primary-backup"）/ timeout_ms / max_vc_backoff = 3 / nodes 示例 4 节点；中文注释 ≥6 点（3f+1 假设 / quorum 2f+1 / 共识 <1s §7.2 / ViewChange 退避 §8.5 / 签名验证 §7.3 / 计数器+延迟观测 D12）

- [ ] Task 6: 新增文档 `docs/agents/pbft-consensus-design.md`
  - [ ] SubTask 6.1: 12 章节 + 2 Mermaid（蓝图 §4.3 三阶段时序图重绘 + ViewChange 决策流程图含 StaleMessage/InvalidSignature/退避/新主重发分支）+ D1~D12 偏差表与 spec 一致 + 接口契约与实现签名一致 + P2 降级说明章节（默认主从仲裁 v0.92.0，pbft 配置启用）

- [ ] Task 7: 根目录版本同步 0.98.0 → 0.99.0
  - [ ] SubTask 7.1: 根 `Cargo.toml` `[workspace.package] version = "0.99.0"`
  - [ ] SubTask 7.2: `Makefile` 版本注释同步
  - [ ] SubTask 7.3: `.github/workflows/ci.yml` 版本注释同步
  - [ ] SubTask 7.4: `ci/src/gate.rs` 注释串尾追加 v0.99.0 类型清单（consensus/pbft/view_change 新增类型）

- [ ] Task 8: 构建验证（§2.4.2 全量）
  - [ ] SubTask 8.1: `cargo metadata --format-version 1` 成功
  - [ ] SubTask 8.2: `cargo test -p eneros-federation`（既有 120 + 新增 40 = 160）与 `cargo test -p eneros-crypto`（417）全通过
  - [ ] SubTask 8.3: `cargo build -p eneros-federation --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [ ] SubTask 8.4: `cargo fmt --all -- --check` 通过
  - [ ] SubTask 8.5: `cargo clippy -p eneros-federation --all-targets -- -D warnings` 0 warning
  - [ ] SubTask 8.6: `cargo deny check advisories licenses bans sources`（零新增第三方依赖）
  - [ ] SubTask 8.7: 回归零破坏：eneros-cloud-coordinator（80）/ eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过

- [ ] Task 9: 按 `checklist.md` 逐项核验并勾选（未通过禁止收工）

# Task Dependencies

- Task 1 独立先行（模块声明）
- Task 2 依赖 Task 1；Task 3 依赖 Task 2（数据结构/总线）；Task 4 依赖 Task 3（enter_view 复用 PrePrepare 重发）
- Task 5/6 与 Task 2~4 可并行（配置/文档）
- Task 7 依赖 Task 2~4 完成（类型清单定稿）
- Task 8 依赖 Task 1~7 全部完成
- Task 9 依赖 Task 8 通过
