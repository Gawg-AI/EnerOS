# Tasks — v0.97.0 联邦发现协议

> Spec：`spec.md`（develop-v0970-federation-discovery）。蓝图：`蓝图/phase2.md` v0.97.0。
> 全部 no_std + alloc 合规；新 crate `eneros-federation`（零依赖）；既有 crate 零改动。

- [x] Task 1: 新建 crate 骨架 `crates/agents/federation/`
  - [ ] SubTask 1.1: `Cargo.toml` — package `eneros-federation`，`description` 含 v0.97.0；**零依赖**（仅 core/alloc，D5/D8）
  - [ ] SubTask 1.2: `src/lib.rs` — `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；crate 文档含 v0.97.0 说明 + D1~D12 偏差表；`pub mod membership; pub mod discovery;` + 全部重导出（11 项：MemberInfo / NodeRole / JoinRequest / CertRef / MemberRegistry / CertVerifier / PresenceBus / FederationDiscovery / FedError / MockCertVerifier / MockPresenceBus）
  - [ ] SubTask 1.3: 根 `Cargo.toml` `members` 追加 `"crates/agents/federation"`（既有成员零改动）
  - 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 实现 `src/membership.rs` — 成员数据模型与注册表
  - [ ] SubTask 2.1: `NodeRole { EdgeBox, EdgeCoordinator, CloudCoordinator }`（Debug/Clone/Copy/PartialEq/Eq/Default，默认 EdgeBox）、`CertRef { fingerprint: u64 }`（Debug/Clone/Copy/PartialEq/Eq/Default）+ `CertRef::from_bytes(cert: &[u8]) -> CertRef`（FNV-1a 64 确定性折叠：`f = f ^ b; f = f * 1099511628211`，offset basis `14695981039346656037`，无密码学语义仅标识）
  - [ ] SubTask 2.2: `MemberInfo { node_id: u64, addr: core::net::IpAddr, role: NodeRole, capabilities: Vec<u64>, last_seen: u64, cert: CertRef }`（Debug/Clone/PartialEq）、`JoinRequest { node_id: u64, addr: IpAddr, role: NodeRole, cert: Vec<u8>, capabilities: Vec<u64> }`（Debug/Clone/PartialEq）
  - [ ] SubTask 2.3: `MemberRegistry { members: BTreeMap<u64, MemberInfo>, self_id: u64 }`（字段全 pub）+ `new(self_id)` / `add(&mut self, m: MemberInfo)`（同 id 覆盖）/ `remove(&mut self, node_id) -> bool` / `heartbeat(&mut self, node_id, now_ms) -> bool`（存在 → 刷新 last_seen 返 true；不存在 → false）/ `remove_stale(&mut self, timeout_ms: u64, now_ms: u64) -> Vec<u64>`（剔除 `now_ms - last_seen > timeout_ms` 成员，返回被剔 id 升序）/ `list(&self) -> Vec<MemberInfo>`（node_id 升序克隆）
  - [ ] SubTask 2.4: 内嵌测试 T1~T12（数据结构派生/字段回显 T1~T4；CertRef 折叠确定性 T5~T6；registry add/list 升序 T7~T8；heartbeat 命中/未命中 T9~T10；remove_stale 边界存活/剔除 T11~T12）
  - 验证：`cargo test -p eneros-federation membership` 通过

- [x] Task 3: 实现 `src/discovery.rs` — 证书验证/在场总线/联邦发现
  - [ ] SubTask 3.1: `FedError { InvalidCert, DuplicateNode, UnknownNode, BroadcastFailed }`（Debug/Clone/Copy/PartialEq/Eq，D10）
  - [ ] SubTask 3.2: sync trait `CertVerifier { fn verify(&mut self, cert: &[u8]) -> Result<(), FedError>; }` 与 sync trait `PresenceBus { fn broadcast(&mut self, member: &MemberInfo) -> Result<(), FedError>; }`（无 async、无 Send+Sync，D5/D6）
  - [ ] SubTask 3.3: `MockCertVerifier { pub accept: bool, pub verify_count: u64 }`（accept=false → `Err(InvalidCert)`）与 `MockPresenceBus { pub broadcasts: Vec<MemberInfo>, pub fail_times: u32 }`（fail_times 递减故障注入，成功入 broadcasts）
  - [ ] SubTask 3.4: `FederationDiscovery { registry, verifier: Box<dyn CertVerifier>, bus: Box<dyn PresenceBus>, heartbeat_interval_ms: u64, join_count, reject_count, broadcast_count, stale_count }`（字段全 pub）+ `new(self_id, verifier, bus, heartbeat_interval_ms)`（registry 空、4 计数器全零）
  - [ ] SubTask 3.5: `handle_join(&mut self, req: JoinRequest, now_ms: u64) -> Result<MemberInfo, FedError>` — verifier.verify Err → reject_count+=1 + Err(InvalidCert)；node_id 已存在（含 == self_id）→ reject_count+=1 + Err(DuplicateNode)（D9）；构造 MemberInfo（last_seen=now_ms，cert=CertRef::from_bytes）→ registry.add → bus.broadcast：Ok → broadcast_count+=1 + join_count+=1 + Ok(member)；Err → reject_count+=1 + Err(BroadcastFailed)（成员已注册保留）
  - [ ] SubTask 3.6: `broadcast_presence(&mut self, member: &MemberInfo) -> Result<(), FedError>`（委托 bus，Ok → broadcast_count+=1）、`heartbeat(&mut self, node_id, now_ms) -> Result<(), FedError>`（registry.heartbeat false → Err(UnknownNode)）、`sweep_stale(&mut self, now_ms) -> Vec<u64>`（remove_stale(heartbeat_interval_ms * 3, now_ms)，stale_count += 剔除数，D8/D12）
  - [ ] SubTask 3.7: 内嵌测试 T13~T40（FedError 派生 T13；Mock 故障注入 T14~T18；handle_join 成功全流程 T19~T22；证书拒绝 T23~T24；重复 ID/self_id 拒绝 T25~T27；广播失败成员保留 T28~T29；heartbeat 命中/未知节点 T30~T31；sweep_stale 边界/剔除/计数 T32~T35；join→broadcast→heartbeat→stale 全链路 T36~T38；多节点混布 role 回显 T39~T40）
  - 验证：`cargo test -p eneros-federation` 40 通过

- [x] Task 4: 新增 `configs/federation.toml`
  - [ ] SubTask 4.1: `[federation]` 段：`heartbeat_interval_ms = 3000` / `stale_multiplier = 3` / `max_members = 64`
  - [ ] SubTask 4.2: 中文注释 6 点（发现延迟 <5s §7.2 集成阶段验收 / 证书验证强制 §7.3 D5 / 心跳超时剔除 3 倍间隔 §4.4/§4.5 D8/D12 / 节点 ID 冲突确定性拒绝 §8.5 D9 / 多版本节点兼容 §8.4 capabilities 能力码协商 / 成员数可观测 §9 join/stale 计数器）

- [x] Task 5: 新增 `docs/agents/federation-discovery-design.md`
  - [ ] SubTask 5.1: 12 章节齐全（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
  - [ ] SubTask 5.2: 2 个 Mermaid 图（新 Edge Box 自动加入联邦数据流图 + handle_join/heartbeat/sweep_stale 决策流程图含证书拒绝/重复 ID/广播失败/超时剔除分支）
  - [ ] SubTask 5.3: D1~D12 偏差表与 spec 一致；接口契约与实现签名一致

- [x] Task 6: 根目录版本同步 0.96.0 → 0.97.0
  - [ ] SubTask 6.1: 根 `Cargo.toml` `[workspace.package] version = "0.97.0"`
  - [ ] SubTask 6.2: `Makefile` 版本注释同步
  - [ ] SubTask 6.3: `.github/workflows/ci.yml` 版本注释同步
  - [ ] SubTask 6.4: `ci/src/gate.rs` 注释串尾追加 v0.97.0 类型清单（MemberInfo / NodeRole / JoinRequest / CertRef / MemberRegistry / CertVerifier / PresenceBus / FederationDiscovery / FedError / MockCertVerifier / MockPresenceBus）

- [x] Task 7: 构建验证（§2.4.2 全量）
  - [ ] SubTask 7.1: `cargo metadata --format-version 1` 成功
  - [ ] SubTask 7.2: `cargo test -p eneros-federation` 40 通过
  - [ ] SubTask 7.3: `cargo build -p eneros-federation --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [ ] SubTask 7.4: `cargo fmt --all -- --check` 通过
  - [ ] SubTask 7.5: `cargo clippy -p eneros-federation --all-targets -- -D warnings` 0 warning
  - [ ] SubTask 7.6: `cargo deny check advisories licenses bans sources`（零新增依赖）
  - [ ] SubTask 7.7: 回归零破坏：eneros-cloud-coordinator（80）/ eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过

- [x] Task 8: 按 `checklist.md` 逐项核验并勾选（未通过禁止收工）

# Task Dependencies

- Task 2 依赖 Task 1（crate 骨架）
- Task 3 依赖 Task 2（membership 类型）
- Task 4/5/6 与 Task 2~3 可并行（配置/文档/版本同步）
- Task 7 依赖 Task 1~6 全部完成
- Task 8 依赖 Task 7 通过
