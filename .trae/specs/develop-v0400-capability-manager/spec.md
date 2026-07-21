# Capability Manager (v0.40.0) Spec

## Why

能力管理器（CapabilityManager）是 Agent 访问控制的安全边界。v0.39.0 实现了单令牌结构与签名，v0.40.0 实现集中式管理器：签发、校验、冻结、撤销、过期清理。崩溃 Agent 的能力被冻结以防止僵尸 Agent 发命令，确保 Agent 越权操作被拒绝。

## What Changes

- 新增 `crates/agents/agent/src/capability/store.rs` — TokenStore（令牌存储 + 双索引）
- 新增 `crates/agents/agent/src/capability/manager.rs` — CapabilityManager（能力管理器）
- 扩展 `AgentError`：+3 个错误变体（TokenFrozen / TokenRevoked / NoCapability）
- 更新 `capability/mod.rs` — 声明新模块 + re-exports
- 更新 `lib.rs` — re-exports + VERSION 0.40.0
- 新增 `tests/capability_manager_test.rs` — 集成测试
- 版本文件同步 0.39.0 → 0.40.0

## Impact

- Affected specs: v0.41.0（System Agent）、v0.42.0（故障恢复编排）
- Affected code:
  - `crates/agents/agent/src/capability/store.rs` — 全新文件
  - `crates/agents/agent/src/capability/manager.rs` — 全新文件
  - `crates/agents/agent/src/capability/mod.rs` — 模块声明
  - `crates/agents/agent/src/error.rs` — +3 错误变体
  - `crates/agents/agent/src/lib.rs` — re-exports + VERSION
  - 版本文件：`Cargo.toml` / `Makefile` / `ci.yml` / `gate.rs`

## ADDED Requirements

### Requirement: TokenStore 令牌存储

系统 SHALL 提供 `TokenStore` 结构，作为 `CapabilityManager` 的内部存储层，维护令牌主表和按 owner 索引。

```rust
pub struct TokenStore {
    tokens: BTreeMap<u64, CapabilityToken>,
    by_owner: BTreeMap<AgentId, Vec<u64>>,
}
```

支持操作：`new` / `insert` / `remove` / `get` / `list_by_owner` / `token_ids_by_owner` / `list_expired_ids` / `len` / `is_empty`。

#### Scenario: 插入令牌后可按 owner 查询
- **WHEN** 插入 owner=AgentId(1) 的令牌后调用 `list_by_owner(AgentId(1))`
- **THEN** 返回包含该令牌的 Vec

#### Scenario: 移除令牌后索引同步更新
- **WHEN** 移除一个令牌后调用 `list_by_owner(owner)`
- **THEN** 返回的 Vec 不再包含该 token_id

### Requirement: CapabilityManager 能力管理器

系统 SHALL 提供 `CapabilityManager` 结构，封装令牌签发、校验、冻结、撤销和过期清理。

```rust
pub struct CapabilityManager {
    store: TokenStore,
    frozen: BTreeSet<u64>,
    revoked: BTreeSet<u64>,
    issuer_keypair: Sm2KeyPair,
    issuer_id: AgentId,
    rng: CsRng,
}
```

#### Scenario: 签发令牌
- **WHEN** 调用 `manager.issue(builder, now)` 传入配置好的 builder
- **THEN** 返回 `Ok(CapabilityToken)`，令牌已签名并存储

#### Scenario: 校验访问权限（有权限）
- **WHEN** Agent 持有匹配 target + permission 的有效令牌，调用 `check_access`
- **THEN** 返回 `Ok(&CapabilityToken)`

#### Scenario: 校验访问权限（越权拒绝）
- **WHEN** Agent 没有匹配 target 或 permission 的令牌
- **THEN** 返回 `Err(NoCapability)`

#### Scenario: 冻结 Agent 所有令牌
- **WHEN** Agent 崩溃后调用 `freeze(agent_id)`
- **THEN** 该 Agent 所有令牌被标记为冻结，返回冻结数量

#### Scenario: 冻结令牌后 check_access 被拒绝
- **WHEN** 令牌被冻结后调用 `check_access`
- **THEN** 跳过冻结令牌，若无其他有效令牌则返回 `Err(NoCapability)`

#### Scenario: 撤销令牌
- **WHEN** 调用 `revoke(token_id)`
- **THEN** 令牌从存储中移除并加入 revoked 集合

#### Scenario: 过期清理
- **WHEN** 调用 `cleanup_expired(now)`
- **THEN** 所有过期令牌被移除，返回清理数量

### Requirement: check_access 目标匹配检查

系统 SHALL 在 `check_access` 中验证令牌的 `target` 字段与请求的 `target` 匹配。蓝图原始代码缺少此检查（安全漏洞），本版本修复。

#### Scenario: 不同 target 的令牌不匹配
- **WHEN** Agent 持有 target=Device(1) 的令牌，请求访问 target=Device(2)
- **THEN** 跳过该令牌，返回 `Err(NoCapability)`

## MODIFIED Requirements

### Requirement: AgentError 错误类型

在现有 `AgentError` 枚举中新增 3 个变体：

```rust
/// 能力 Token 已冻结
TokenFrozen,
/// 能力 Token 已撤销
TokenRevoked,
/// 无匹配能力
NoCapability { agent: AgentId, target: String },
```

`NoCapability.target` 使用 `String`（`format!("{:?}", target)`）而非 `ResourceTarget`，避免 `error` 模块依赖 `capability` 模块的循环引用。

## Deviation Declarations (D1~D10)

| 偏差 | 蓝图描述 | 实际实现 | 原因 |
|------|---------|---------|------|
| D1 | `HashMap`/`HashSet` | `BTreeMap`/`BTreeSet` | no_std 无 `std::collections::HashMap`/`HashSet` |
| D2 | `issuer_key: Sm2PrivateKey` + `issuer_pk: Sm2PublicKey` | `issuer_keypair: Sm2KeyPair` | `build_and_sign` 需要完整 keypair（sk+pk） |
| D3 | `issue(builder, owner: AgentId)` | `issue(builder, now: u64)` | owner 已在 builder 中设置；需要 `now` 用于 `issued_at` |
| D4 | `check_access(agent_id, target, perm)` | `check_access(agent_id, target, perm, now: u64)` | no_std 无 `crate::time::now_ms()` |
| D5 | `next_token_id: u64` 递增 | 移除 | token_id 由 `build_and_sign` 内部 CSRNG 随机生成 |
| D6 | `TokenStore.by_target: HashMap<String, Vec<u64>>` | 跳过 `by_target` 索引 | `ResourceTarget` 非 `String`；`check_access` 仅用 `by_owner` 索引 |
| D7 | `new(keypair: Sm2KeyPair)` | `new(keypair: Sm2KeyPair, issuer_id: AgentId)` | 签发者 ID 需要配置，不可硬编码 `AgentId(0)` |
| D8 | `builder.build_and_sign(&self.issuer_key, AgentId(0))?` | `builder.build_and_sign(&self.issuer_keypair, self.issuer_id, now, &mut self.rng)?` | 真实 API 需 4 参数（keypair + issuer_id + now + rng） |
| D9 | `check_access` 不检查 target | 添加 `token.target == *target` 匹配检查 | 修复蓝图安全漏洞：无 target 检查会导致越权 |
| D10 | `manager.rs` 内联存储 | 拆分为 `store.rs` + `manager.rs` | 蓝图交付物清单要求 `store.rs`；分离存储与业务逻辑 |
