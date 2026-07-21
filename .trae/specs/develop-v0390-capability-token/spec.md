# Capability Token (v0.39.0) Spec

## Why

能力令牌（Capability Token）是 Agent 访问控制的核心载体。每个 Agent 只能执行令牌授权的操作，防止越权。v0.39.0 实现 Token 结构、构建器与 SM2 签名验证，为 v0.40.0 能力管理器（签发/校验/冻结/撤销）提供基础。

## What Changes

- 新增 `crates/agents/agent/src/capability/` 模块（4 个文件：`mod.rs` / `token.rs` / `builder.rs` / `verifier.rs`）
- 新增 `CapabilityToken` 结构（9 字段，SM2 签名）
- 新增 `ResourceTarget` 枚举（5 变体：Device / Agent / File / Network / SystemResource）
- 新增 `PermissionSet`（手动 bitflags 实现，6 种权限）
- 新增 `ConstraintPack` + `ConstraintType`（电力约束包：功率/SOC/电压/频率）
- 新增 `CapabilityTokenBuilder`（Builder 模式 + `build_and_sign`）
- 新增 `TokenVerifier`（封装签发者公钥的批量验证器）
- 新增支持类型：`DeviceId` / `SocketAddr` / `SystemResource`
- 扩展 `AgentError`：+5 个错误变体（TokenExpired / TokenSignatureInvalid / PermissionDenied / ConstraintViolated / TokenNotSigned）
- `eneros-agent` Cargo.toml 新增 `eneros-crypto` 依赖（**首次引入外部依赖**）
- `lib.rs` 新增 `pub mod capability` + re-exports + VERSION 0.39.0

## Impact

- Affected specs: v0.40.0（能力管理器）、v0.78.0（消息签名）、v0.113.0（Secure Boot）
- Affected code:
  - `crates/agents/agent/Cargo.toml` — 新增 `eneros-crypto` 依赖
  - `crates/agents/agent/src/lib.rs` — 模块声明 + re-exports + VERSION
  - `crates/agents/agent/src/error.rs` — +5 错误变体
  - `crates/agents/agent/src/capability/` — 全新模块
  - `crates/agents/agent/tests/capability_test.rs` — 集成测试
  - 版本文件：`Cargo.toml` / `Makefile` / `ci.yml` / `gate.rs`

## ADDED Requirements

### Requirement: CapabilityToken 结构与签名

系统 SHALL 提供 `CapabilityToken` 结构，包含以下字段：
- `token_id: u64` — 令牌唯一 ID（CSRNG 随机生成）
- `owner: AgentId` — 令牌持有者
- `target: ResourceTarget` — 目标资源
- `permissions: PermissionSet` — 权限集（bitflags）
- `constraints: ConstraintPack` — 安全约束（功率/SOC/电压/频率）
- `issued_at: u64` — 签发时间戳
- `expires_at: Option<u64>` — 过期时间戳（None = 永不过期）
- `issuer: AgentId` — 签发者
- `signature: [u8; 64]` — SM2 签名（r‖s，64 字节）

#### Scenario: 构建并签名令牌
- **WHEN** 调用 `CapabilityTokenBuilder::new().owner(id).target(t).permission(p).constraints(c).ttl(3600000).build_and_sign(keypair, issuer_id, now, rng)`
- **THEN** 返回 `Ok(CapabilityToken)`，`signature` 非零，`issued_at == now`，`expires_at == Some(now + 3600000)`

#### Scenario: 验证有效令牌
- **WHEN** 调用 `token.verify(issuer_pk)` 对未篡改的令牌验证
- **THEN** 返回 `Ok(())`

#### Scenario: 验证篡改令牌
- **WHEN** 令牌的 `permissions` 字段被修改后调用 `verify`
- **THEN** 返回 `Err(TokenSignatureInvalid)`

#### Scenario: 过期检查
- **WHEN** `token.is_expired(now)` 且 `now > expires_at`
- **THEN** 返回 `true`

### Requirement: PermissionSet 权限位集

系统 SHALL 提供 `PermissionSet` 结构（手动 bitflags 实现，不依赖 `bitflags` crate），支持 6 种权限：
- `READ = 0x01` / `WRITE = 0x02` / `EXECUTE = 0x04` / `CONTROL = 0x08` / `CONFIG = 0x10` / `ADMIN = 0x20`

支持 `contains` / `insert` / `bits` / `is_empty` / `is_all` / `BitOr` / `BitOrAssign` 操作。

#### Scenario: 权限检查
- **WHEN** 令牌拥有 `READ | WRITE` 权限，调用 `token.check_permission(READ)`
- **THEN** 返回 `true`

#### Scenario: 权限不足
- **WHEN** 令牌仅拥有 `READ` 权限，调用 `token.check_permission(WRITE)`
- **THEN** 返回 `false`

### Requirement: ConstraintPack 电力约束

系统 SHALL 提供 `ConstraintPack` 结构，包含 6 个电力约束字段（均为 `f32`）：
- `max_power` / `min_power` / `soc_limit: (f32, f32)` / `voltage_limit: (f32, f32)` / `frequency_limit: (f32, f32)`

支持 `check_constraint(value, ctype) -> bool` 和 `clamp(value, ctype) -> f32` 方法。

#### Scenario: 约束内
- **WHEN** `constraints.check_constraint(50.0, ConstraintType::MaxPower)` 且 `max_power = 100.0`
- **THEN** 返回 `true`

#### Scenario: 约束违反
- **WHEN** `constraints.check_constraint(150.0, ConstraintType::MaxPower)` 且 `max_power = 100.0`
- **THEN** 返回 `false`

#### Scenario: 截断到边界
- **WHEN** `constraints.clamp(150.0, ConstraintType::MaxPower)` 且 `max_power = 100.0`
- **THEN** 返回 `100.0`

### Requirement: CapabilityTokenBuilder 构建器

系统 SHALL 提供 `CapabilityTokenBuilder`，使用 Builder 模式构建并签名令牌。

`build_and_sign` 方法签名（**偏差 D1-D3**）：
```rust
pub fn build_and_sign(
    self,
    issuer_keypair: &Sm2KeyPair,
    issuer_id: AgentId,
    now: u64,
    rng: &mut CsRng,
) -> Result<CapabilityToken, AgentError>
```

#### Scenario: 完整构建流程
- **WHEN** 使用 builder 设置所有字段后调用 `build_and_sign`
- **THEN** 生成已签名令牌，`token_id` 随机，`signature` 为有效 SM2 签名

### Requirement: TokenVerifier 批量验证器

系统 SHALL 提供 `TokenVerifier` 结构，封装签发者公钥以避免每次验证都传参。

```rust
pub struct TokenVerifier {
    issuer_pk: Sm2PublicKey,
}
impl TokenVerifier {
    pub fn new(issuer_pk: Sm2PublicKey) -> Self;
    pub fn verify(&self, token: &CapabilityToken) -> Result<(), AgentError>;
}
```

## MODIFIED Requirements

### Requirement: AgentError 错误类型

在现有 `AgentError` 枚举中新增 5 个变体：

```rust
TokenExpired,
TokenSignatureInvalid,
PermissionDenied { required: u32, actual: u32 },
ConstraintViolated { value: f32, limit: f32 },
TokenNotSigned,
```

`PermissionDenied` 使用 `u32`（PermissionSet 的 bits）而非 `PermissionSet` 本身，因为 `PermissionSet` 在 `capability` 模块中定义，而 `AgentError` 在 `error` 模块中——避免循环依赖。

## Deviation Declarations (D1~D13)

| 偏差 | 蓝图描述 | 实际实现 | 原因 |
|------|---------|---------|------|
| D1 | `build_and_sign(self, issuer_key, issuer_id)` | `build_and_sign(self, issuer_keypair, issuer_id, now, rng)` | no_std 无系统时钟（`crate::time::now_ms()` 不存在），SM2 签名需要公钥（Z 值）+ CSRNG |
| D2 | `crate::rng::next_u64()` 生成 token_id | `rng.fill_bytes()` 生成随机 u64 | no_std 无系统 RNG |
| D3 | `sm2_sign(&data, issuer_key)?` | `sm2_sign(&data, &keypair.private_key, &keypair.public_key, rng)?` | 真实 SM2 API 需要公钥（Z 值）+ RNG |
| D4 | `sm2_verify_hash(&msg_hash, &sig, issuer_pk)?` | `sm2_verify(&data, &sig, issuer_pk)?` | `sm2_verify_hash` 不存在；`sm2_verify` 内部处理 Z 值 + SM3 |
| D5 | `signature: Vec<u8>` | `signature: [u8; 64]` | SM2 签名固定 64 字节（r‖s），数组更高效且类型安全 |
| D6 | `bitflags!` 宏 | 手动 `PermissionSet(u32)` 实现 | 避免 `bitflags` crate 依赖，保持最小依赖 |
| D7 | `ResourceTarget::Network(SocketAddr)` 使用 `std::net::SocketAddr` | 自定义 `SocketAddr { ipv4: u32, port: u16 }` | no_std 无 `std::net::SocketAddr` |
| D8 | `ResourceTarget::Device(DeviceId)` | 自定义 `DeviceId(pub u64)` | `DeviceId` 类型不存在于 agent crate |
| D9 | `ConstraintPack::check(&self, cmd: &ControlCommand)` | 跳过（仅实现 `check_constraint` + `clamp`） | `ControlCommand` 类型尚未定义；`check_constraint` 已提供逐值检查 |
| D10 | `verify(&self, issuer_pk) -> Result<bool, AgentError>` | `verify(&self, issuer_pk) -> Result<(), AgentError>` | Ok(()) = 有效，Err = 无效，更符合 Rust 惯例 |
| D11 | `Sm2Signature::from_bytes(&self.signature)` | `Sm2Signature::from_bytes(&self.signature)` | 实际 API 接受 `&[u8; 64]`，与 `signature: [u8; 64]` 字段兼容 |
| D12 | `agent/src/capability/` | `crates/agents/agent/src/capability/` | 工作区目录结构调整（§2.3.1） |
| D13 | `eneros-agent` 零外部依赖 | 新增 `eneros-crypto` 依赖 | v0.39.0 引入 SM2 签名，需要密码学库；依赖路径 `../../security/crypto` |
