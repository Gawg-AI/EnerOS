# Checklist — v0.97.0 联邦发现协议

> Spec：`spec.md`（develop-v0970-federation-discovery）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: 新 crate `eneros-federation` 位于 `crates/agents/federation/` 下，未直接放根目录
- [x] C2: 根 `Cargo.toml` workspace members 已追加 `"crates/agents/federation"`，workspace 仍可解析
- [x] C3: `Cargo.toml` 中无任何 path/外部依赖（零依赖 crate，D5/D8）
- [x] C4: 新文档 `federation-discovery-design.md` 位于 `docs/agents/`，未平面化放 `docs/` 根
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-federation` 40 通过
- [x] C8: `cargo build -p eneros-federation --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy -p eneros-federation --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/agents/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe` / `async`（no_std 合规；子模块不重复加 no_std attr；测试模块内 std::cell/std::rc 位于 `#[cfg(test)]` 下，lib.rs `#![cfg_attr(not(test), no_std)]` 惯例允许）

## D. 数据结构（C16~C25）

- [x] C16: `NodeRole { EdgeBox, EdgeCoordinator, CloudCoordinator }` 派生 Debug/Clone/Copy/PartialEq/Eq/Default，默认 EdgeBox（Default 语义）
- [x] C17: `CertRef { fingerprint: u64 }` 派生 Debug/Clone/Copy/PartialEq/Eq/Default，`from_bytes` 为确定性 FNV-1a 64 折叠（f = f ^ b; f = f * 1099511628211，offset basis = 14695981039346656037）
- [x] C18: `MemberInfo { node_id: u64, addr: core::net::IpAddr, role: NodeRole, capabilities: Vec<u64>, last_seen: u64, cert: CertRef }` 派生 Debug/Clone/PartialEq
- [x] C19: `JoinRequest { node_id: u64, addr: IpAddr, role: NodeRole, cert: Vec<u8>, capabilities: Vec<u64> }` 派生 Debug/Clone/PartialEq
- [x] C20: `MemberRegistry { members: BTreeMap<u64, MemberInfo>, self_id: u64 }` 字段全 pub
- [x] C21: `MemberRegistry::add` 同 id 覆盖（后入覆盖先入，BTreeMap insert 语义）
- [x] C22: `MemberRegistry::heartbeat(node_id, now_ms)` 存在 → 刷新 last_seen = now_ms 返 true；不存在 → false
- [x] C23: `MemberRegistry::remove_stale(timeout_ms, now_ms)` 判定条件 `now_ms - last_seen > timeout_ms`（严格大于，D12 边界存活），返回被剔 id 升序
- [x] C24: `MemberRegistry::list()` 按 node_id 升序返回克隆 Vec
- [x] C25: `core::net::IpAddr` 使用（no_std 原生，D4 无偏差代价）

## E. 证书与总线 trait（C26~C31）

- [x] C26: `FedError { InvalidCert, DuplicateNode, UnknownNode, BroadcastFailed }` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C27: `CertVerifier` 为 sync trait（`verify(&mut self, cert: &[u8]) -> Result<(), FedError>`），无 Send+Sync 要求，无 async
- [x] C28: `PresenceBus` 为 sync trait（`broadcast(&mut self, member: &MemberInfo) -> Result<(), FedError>`），无 Send+Sync 要求，无 async
- [x] C29: `MockCertVerifier { accept: bool, verify_count: u64 }`：accept=false → `Err(InvalidCert)`；accept=true → `Ok(())`；每次 verify verify_count += 1
- [x] C30: `MockPresenceBus { broadcasts: Vec<MemberInfo>, fail_times: u32 }`：fail_times > 0 时 Err(BroadcastFailed) 且 fail_times -= 1；fail_times == 0 时 Ok 且 member 入 broadcasts
- [x] C31: Mock 均实现对应 trait，可作 `Box<dyn CertVerifier>` / `Box<dyn PresenceBus>` 注入

## F. FederationDiscovery 核心逻辑（C32~C45）

- [x] C32: `FederationDiscovery` 字段全 pub：`registry: MemberRegistry` / `verifier: Box<dyn CertVerifier>` / `bus: Box<dyn PresenceBus>` / `heartbeat_interval_ms: u64` / `join_count: u64` / `reject_count: u64` / `broadcast_count: u64` / `stale_count: u64`
- [x] C33: `new(self_id, verifier, bus, heartbeat_interval_ms)` 创建时 registry 空、4 计数器全零
- [x] C34: `handle_join` verifier reject → `reject_count += 1` + `Err(InvalidCert)`，registry 不变、bus 无广播
- [x] C35: `handle_join` node_id 已存在（含 == self_id）→ `reject_count += 1` + `Err(DuplicateNode)`，原成员不被覆盖（D9 确定性拒绝）
- [x] C36: `handle_join` 校验通过且 id 可用 → 构造 MemberInfo（last_seen = now_ms，cert = CertRef::from_bytes(&req.cert)）→ registry.add → bus.broadcast
- [x] C37: `handle_join` broadcast Ok → `broadcast_count += 1` + `join_count += 1` + `Ok(member)`
- [x] C38: `handle_join` broadcast Err → `reject_count += 1` + `Err(BroadcastFailed)`，成员已注册保留（调用方可重试 broadcast）
- [x] C39: `broadcast_presence(&mut self, member)` 委托 bus.broadcast，Ok → `broadcast_count += 1`
- [x] C40: `heartbeat(&mut self, node_id, now_ms)` registry.heartbeat false → `Err(UnknownNode)`
- [x] C41: `heartbeat` registry.heartbeat true → `Ok(())`
- [x] C42: `sweep_stale(&mut self, now_ms)` timeout = `heartbeat_interval_ms * 3`（D12 蓝图 §4.5 `* 3`）
- [x] C43: `sweep_stale` 被剔数累加到 `stale_count`，返回被剔 id 升序
- [x] C44: `sweep_stale` 后 heartbeat(被剔 node_id) → `Err(UnknownNode)`
- [x] C45: `join_count` / `reject_count` / `broadcast_count` / `stale_count` 跨多次调用累计正确

## G. 配置与文档（C46~C52）

- [x] C46: `configs/federation.toml` 存在，`[federation]` 段含 `heartbeat_interval_ms = 3000` / `stale_multiplier = 3` / `max_members = 64`
- [x] C47: 中文注释覆盖 6 点：发现延迟 <5s / 证书验证强制 / 心跳超时剔除 3 倍间隔 / 节点 ID 冲突确定性拒绝 / 多版本节点兼容 / 成员数可观测
- [x] C48: `docs/agents/federation-discovery-design.md` 存在，12 章节齐全
- [x] C49: 含 2 个 Mermaid 图（新 Edge Box 自动加入联邦数据流图 + handle_join/heartbeat/sweep_stale 决策流程图）
- [x] C50: 含 D1~D12 偏差表，与 spec 偏差声明一致
- [x] C51: 接口契约与实现一致（函数签名、字段、错误变体、计数器语义）
- [x] C52: 文档中 `start`/`run` 未实现说明（D3：无 ticker，集成阶段调用方循环驱动）

## H. 版本同步（C53~C56）

- [x] C53: 根 `Cargo.toml` `[workspace.package] version = "0.97.0"`
- [x] C54: `Makefile` 版本注释同步 0.97.0
- [x] C55: `.github/workflows/ci.yml` 版本注释同步 0.97.0
- [x] C56: `ci/src/gate.rs` 注释追加 v0.97.0 类型清单（11 项）

## I. 测试覆盖（C57~C65）

- [x] C57: 内嵌 40 个单元测试（T1~T40）全部实现并通过
- [x] C58: 测试分布：数据结构/派生 T1~T6 / CertRef 折叠 T5~T6 / registry add/list/heartbeat/remove_stale T7~T12 / FedError/Mock 故障注入 T13~T18 / handle_join 成功/拒绝/重复 T19~T27 / broadcast 失败成员保留 T28~T29 / heartbeat 未知节点 T30~T31 / sweep_stale 边界/剔除 T32~T35 / 全链路集成 T36~T40
- [x] C59: 含 join→broadcast→heartbeat→stale 全链路集成测试
- [x] C60: 含证书拒绝故障注入测试（MockCertVerifier accept=false）
- [x] C61: 含重复 node_id 拒绝测试（含 self_id 冲突）
- [x] C62: 含 sweep_stale 边界测试（`now - last == timeout` 保留，`> timeout` 剔除，D12）
- [x] C63: 含多角色混布测试（EdgeBox/EdgeCoordinator/CloudCoordinator role 回显）
- [x] C64: 所有测试无 `std::` 违规（no_std 合规）
- [x] C65: 回归零破坏：eneros-cloud-coordinator（80）/ eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过

## J. 蓝图达成（C66~C72）

- [x] C66: v0.97.0 交付物全覆盖：成员数据模型（MemberInfo/JoinRequest）/ 注册表（MemberRegistry）/ 证书验证（CertVerifier trait）/ 在场广播（PresenceBus trait）/ 联邦发现（FederationDiscovery.handle_join/broadcast_presence/heartbeat/sweep_stale）
- [x] C67: 新 Edge Box 自动加入联邦：JoinRequest → verify → add → broadcast 全链路闭环
- [x] C68: 心跳保活 + 超时剔除：heartbeat 刷新 last_seen，sweep_stale 按 3 倍间隔剔除（D8/D12）
- [x] C69: 节点 ID 冲突确定性拒绝：DuplicateNode 不覆盖不更新（D9）
- [x] C70: 4 个 pub 计数器（join/reject/broadcast/stale）全程可观测（D9）
- [x] C71: 无 BREAKING：既有全部 crate 零改动，既有公共 API 全保留
- [x] C72: 下游解锁：v0.98.0 跨域通信通道 / VPP 多机协同
