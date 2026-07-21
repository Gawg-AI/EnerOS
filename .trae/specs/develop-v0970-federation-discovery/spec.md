# v0.97.0 联邦发现协议 Spec

## Why

v0.96.0 完成 P2-D 收官（云边协同），蓝图 phase2 v0.97.0（P2-E 起点）要求实现**联邦发现协议**：新 Edge Box 持有效证书自动加入联邦（JoinRequest → 证书验证 → 注册 → 广播）+ 心跳保活 + 超时剔除，支持联邦即插即用动态扩展，为 v0.98.0 跨域通信通道与 VPP 多机协同提供成员基础。

## What Changes

- **新建 crate `eneros-federation`**（`crates/agents/federation/`，D1），**零依赖**（仅用 core/alloc，CertVerifier/PresenceBus 为本地 trait + Mock）：
  - `src/membership.rs` — `MemberInfo` / `NodeRole` / `JoinRequest` / `CertRef` / `MemberRegistry`（add / remove_stale / list / heartbeat）
  - `src/discovery.rs` — `CertVerifier` trait / `PresenceBus` trait / `FederationDiscovery`（handle_join + broadcast_presence + sweep_stale + 4 计数器）/ `FedError` + `MockCertVerifier` / `MockPresenceBus` 故障注入
  - `src/lib.rs` — no_std crate 文档（D1~D12 偏差表）+ 重导出
- `Cargo.toml`（新 crate）：**无任何依赖**（core::net::IpAddr 来自 core，证书/总线以 trait 抽象注入，D5/D8）
- 根 `Cargo.toml` members 追加 `"crates/agents/federation"`（既有成员零改动）
- 新增 `configs/federation.toml`（心跳间隔/超时倍数/成员上限 + 中文注释 6 点）
- 新增 `docs/agents/federation-discovery-design.md`（12 章节 + 2 Mermaid + D1~D12 偏差表）
- 根目录 4 文件版本同步 0.96.0 → 0.97.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（T1~T40），含 join→broadcast→heartbeat→stale 全链路、重复 ID 拒绝、证书拒绝、Mock 故障注入
- **无 BREAKING**：既有全部 crate 零改动

## Impact

- Affected specs：无既有 spec 受影响（全新 crate）；关联 develop-v0940-vpp-aggregator（前序 VPP 聚合）、develop-v0960-cloud-aggregator（P2-D 收官）
- Affected code：新增 `crates/agents/federation/`、`configs/`、`docs/agents/`、根 4 文件
- 依赖：**零依赖**（无新第三方依赖，无 workspace path 依赖；PKI v0.32.0 适配器后续以 `Box<dyn CertVerifier>` 注入）
- 下游解锁：v0.98.0 跨域通信通道（gRPC + mTLS）、VPP 多机协同

## 偏差声明（D1~D12，Karpathy Think Before Coding：显式取舍）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `crates/federation/`；文档 `docs/phase2/federation_discovery.md` | `crates/agents/federation/` + `docs/agents/federation-discovery-design.md`（项目 §2.3.1/§2.3.3 硬规则；联邦为 Agent 级协调归 agents 子系统） |
| **D2** | `node_id: String` / `self_id: String` / `capabilities: Vec<String>` / `members: HashMap<String, _>` | 全部 `u64` / `Vec<u64>`（能力码 u64，配置定义语义）/ `BTreeMap<u64, MemberInfo>`（无堆字符串 + 确定性，v0.95.0 D2 惯例） |
| **D3** | `pub async fn start/handle_join/broadcast_presence/run`；`Duration` / `interval().tick().await` | sync 方法（no_std 无 async runtime，v0.95.0 D3 惯例）；全部时间以 `now_ms: u64` / `*_ms: u64` 参数注入；`start`/`run` 不实现（无 ticker，集成阶段调用方循环驱动） |
| **D4** | `addr: IpAddr`（std::net） | `core::net::IpAddr`（Rust core 原生支持 no_std，无偏差代价）；`parse_addr(&node_id)` 蓝图未定义 → 改为 `JoinRequest.addr: IpAddr` 显式携带（不臆造 ID→地址映射） |
| **D5** | `verify_cert(&req.cert)?` 全局函数；`CertRef::from(&req.cert)` | `CertVerifier` sync trait（`verify(&mut self, cert: &[u8]) -> Result<(), FedError>`）+ `MockCertVerifier` 故障注入（接口先行；PKI v0.32.0 适配器后续注入 `Box<dyn CertVerifier>`，§5.5 防重复造轮子）；`CertRef { fingerprint: u64 }`（cert 字节确定性折叠，无密码学语义，仅标识） |
| **D6** | `broadcast_presence` 为内部网络广播 | `PresenceBus` sync trait（`broadcast(&mut self, member: &MemberInfo) -> Result<(), FedError>`）+ `MockPresenceBus`（记录广播 + 故障注入；v0.95.0 D8 CloudChannel 模式；DDS v0.78.0 适配器后续注入） |
| **D7** | `JoinRequest` 无 role；`handle_join` 硬编码 `role: NodeRole::EdgeBox` | `JoinRequest` 增加 `role: NodeRole` 字段（EdgeCoordinator/CloudCoordinator 节点亦需加入联邦；默认 EdgeBox） |
| **D8** | §4.4"心跳超时 → 标记离线"；§4.5 代码 `remove_stale` 删除 | 采用删除语义（蓝图代码权威）：`MemberRegistry::remove_stale(timeout_ms, now_ms) -> Vec<u64>` 返回被剔除 id；`FederationDiscovery::sweep_stale` 累加 `stale_count` 计数器实现"标记"可观测（不引入 online 标志位，Karpathy 最简） |
| **D9** | §8.5 坑点"节点 ID 冲突"未定义行为 | 确定性拒绝：`handle_join` 时 node_id 已存在 → `Err(FedError::DuplicateNode)`（不覆盖不更新，冲突显式暴露） |
| **D10** | 蓝图未定义 `FedError` | `FedError { InvalidCert, DuplicateNode, UnknownNode, BroadcastFailed }`（4 变体最小完备：证书/冲突/心跳未知节点/广播失败） |
| **D11** | 测试 `tests/discovery.rs` | crate 内嵌 `#[cfg(test)]` 40 测试（v0.87.0~v0.96.0 项目惯例；集成场景以 Mock 故障注入覆盖） |
| **D12** | 蓝图未覆盖时间语义细节 | `last_seen = now_ms`（加入/心跳刷新）；stale 判定 `now_ms - last_seen > timeout_ms`（严格大于，边界存活）；`remove_stale` 默认 timeout = `heartbeat_interval_ms * 3`（蓝图 §4.5 `* 3`）；无 NaN 风险（全 u64 时间，无 f32 参与判定） |

## ADDED Requirements

### Requirement: 成员数据模型与注册表

系统 SHALL 提供（全部 no_std + alloc 兼容）：`MemberInfo { node_id: u64, addr: core::net::IpAddr, role: NodeRole, capabilities: Vec<u64>, last_seen: u64, cert: CertRef }`（Debug/Clone/PartialEq）、`NodeRole { EdgeBox, EdgeCoordinator, CloudCoordinator }`（Debug/Clone/Copy/PartialEq/Eq/Default，默认 EdgeBox）、`JoinRequest { node_id: u64, addr: IpAddr, role: NodeRole, cert: Vec<u8>, capabilities: Vec<u64> }`（Debug/Clone/PartialEq）、`CertRef { fingerprint: u64 }`（Debug/Clone/Copy/PartialEq/Eq/Default）+ `CertRef::from_bytes(cert: &[u8]) -> CertRef`（确定性折叠：逐字节 `f = f ^ b; f = f * 1099511628211`（FNV-1a 64 质数），无密码学语义仅标识）。

系统 SHALL 提供 `MemberRegistry { members: BTreeMap<u64, MemberInfo>, self_id: u64 }`（字段全 pub）+ `new(self_id)` / `add(&mut self, m: MemberInfo)`（同 id 覆盖）/ `remove(&mut self, node_id) -> bool` / `heartbeat(&mut self, node_id, now_ms) -> bool`（存在 → 刷新 last_seen 返 true；不存在 → false）/ `remove_stale(&mut self, timeout_ms: u64, now_ms: u64) -> Vec<u64>`（剔除 `now_ms - last_seen > timeout_ms` 成员，返回被剔 id 升序）/ `list(&self) -> Vec<MemberInfo>`（node_id 升序克隆）。

#### Scenario: 注册表增删与心跳
- **WHEN** add 2 成员（id 2、1），list()
- **THEN** 按 node_id 升序返回 [1, 2]
- **WHEN** heartbeat(1, 5000)（成员 1 存在）
- **THEN** 返回 true 且成员 1 last_seen == 5000；heartbeat(99, _) → false
- **WHEN** 成员 last_seen=1000，`remove_stale(9000, 10_001)`
- **THEN** `now - last = 9001 > 9000` → 剔除并返回 [id]；`remove_stale(9000, 10_000)` → `9000 > 9000` 不成立 → 保留（D12 边界存活）

### Requirement: 证书验证与在场总线抽象

系统 SHALL 提供 sync trait `CertVerifier { fn verify(&mut self, cert: &[u8]) -> Result<(), FedError>; }` 与 sync trait `PresenceBus { fn broadcast(&mut self, member: &MemberInfo) -> Result<(), FedError>; }`（无 async、无 Send+Sync，D5/D6）。

系统 SHALL 提供 `MockCertVerifier { pub accept: bool, pub verify_count: u64 }`（accept=false → `Err(InvalidCert)`）与 `MockPresenceBus { pub broadcasts: Vec<MemberInfo>, pub fail_times: u32 }`（fail_times 递减故障注入，成功入 broadcasts）。

### Requirement: 联邦发现 FederationDiscovery

系统 SHALL 提供 `FedError { InvalidCert, DuplicateNode, UnknownNode, BroadcastFailed }`（Debug/Clone/Copy/PartialEq/Eq）与 `FederationDiscovery { registry: MemberRegistry, verifier: Box<dyn CertVerifier>, bus: Box<dyn PresenceBus>, heartbeat_interval_ms: u64, join_count: u64, reject_count: u64, broadcast_count: u64, stale_count: u64 }`（字段全 pub）：
- `new(self_id, verifier, bus, heartbeat_interval_ms)`（registry 空、4 计数器全零）
- `handle_join(&mut self, req: JoinRequest, now_ms: u64) -> Result<MemberInfo, FedError>`：verifier.verify → Err → `reject_count += 1` + `Err(InvalidCert)`；node_id 已存在（含 == self_id）→ `reject_count += 1` + `Err(DuplicateNode)`（D9）；构造 MemberInfo（last_seen=now_ms，cert=CertRef::from_bytes）→ registry.add → bus.broadcast：Ok → `broadcast_count += 1` + `join_count += 1` + `Ok(member)`；Err → `reject_count += 1` + `Err(BroadcastFailed)`（成员已注册保留，广播失败显式返回，调用方可重试 broadcast）
- `broadcast_presence(&mut self, member: &MemberInfo) -> Result<(), FedError>`：委托 bus，Ok → `broadcast_count += 1`
- `heartbeat(&mut self, node_id, now_ms) -> Result<(), FedError>`：registry.heartbeat false → `Err(UnknownNode)`
- `sweep_stale(&mut self, now_ms) -> Vec<u64>`：`remove_stale(heartbeat_interval_ms * 3, now_ms)`，`stale_count += 剔除数`，返回被剔 id（D8/D12）

#### Scenario: 加入联邦全流程（蓝图 §4.3）
- **WHEN** Mock verifier accept、bus 成功，handle_join 新节点（now_ms=1000）
- **THEN** `Ok(MemberInfo)`：last_seen==1000、role 与请求一致、cert.fingerprint 与 cert 字节一致；registry 含该成员；`join_count == 1`、`broadcast_count == 1`；bus.broadcasts 含该 member
- **WHEN** verifier reject
- **THEN** `Err(InvalidCert)`、`reject_count == 1`、registry 空、bus 无广播
- **WHEN** 同 node_id 再次 handle_join（或与 self_id 相同）
- **THEN** `Err(DuplicateNode)`、`reject_count += 1`、原成员不被覆盖（D9）

#### Scenario: 心跳与超时剔除（蓝图 §4.4/§6.5）
- **WHEN** 成员 last_seen=1000，heartbeat_interval=3000，sweep_stale(now_ms=10_000)
- **THEN** timeout=9000，`10_000-1000=9000 > 9000` 不成立 → 成员保留，stale_count 不变
- **WHEN** sweep_stale(now_ms=10_001)
- **THEN** 剔除返回 [node_id]，`stale_count == 1`；后续 heartbeat(node_id) → `Err(UnknownNode)`

### Requirement: 联邦配置

系统 SHALL 提供 `configs/federation.toml`：`[federation]` 段（`heartbeat_interval_ms = 3000` / `stale_multiplier = 3` / `max_members = 64`），中文注释含：发现延迟 <5s（§7.2，集成阶段验收）/ 证书验证强制（§7.3，D5）/ 心跳超时剔除 3 倍间隔（§4.4/§4.5，D8/D12）/ 节点 ID 冲突确定性拒绝（§8.5，D9）/ 多版本节点兼容（§8.4，capabilities 能力码协商）/ 成员数可观测（§9，join/stale 计数器）。

## MODIFIED Requirements

### Requirement: workspace 集成与版本

根 `Cargo.toml`：`members` 追加 `"crates/agents/federation"`（既有成员零改动），`[workspace.package] version = "0.97.0"`。`Makefile` / `ci.yml` 版本注释同步。`ci/src/gate.rs` clippy/test 注释串尾追加 v0.97.0 类型清单（MemberInfo / NodeRole / JoinRequest / CertRef / MemberRegistry / CertVerifier / PresenceBus / FederationDiscovery / FedError / MockCertVerifier / MockPresenceBus）。**既有 crate 全部零改动**。

## REMOVED Requirements

无。
