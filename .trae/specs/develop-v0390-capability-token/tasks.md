# Tasks

- [x] **Task 1: 扩展 AgentError 五个能力 Token 错误变体** ✅ (含修正：移除 Eq derive 以支持 f32 字段)
  - 修改 `crates/agents/agent/src/error.rs`：
    - 在 `RestartFailed` 之后新增 5 个变体：
      - `TokenExpired`
      - `TokenSignatureInvalid`
      - `PermissionDenied { required: u32, actual: u32 }`
      - `ConstraintViolated { value: f32, limit: f32 }`
      - `TokenNotSigned`
    - 为 5 个新变体添加 Display impl
    - 添加测试：`test_capability_error_variants_display` + `test_capability_error_variants_eq`
  - 验证：`cargo build -p eneros-agent`

- [x] **Task 2: 添加 eneros-crypto 依赖到 agent crate** ✅
  - 修改 `crates/agents/agent/Cargo.toml`：
    - `[dependencies]` 新增 `eneros-crypto = { path = "../../security/crypto" }`
    - 更新 description 字段
  - 验证：`cargo build -p eneros-agent`（确认依赖解析成功）

- [x] **Task 3: 创建 capability/token.rs — 数据结构与 Token 实现** ✅ (419 lines)
  - 创建 `crates/agents/agent/src/capability/token.rs`：
    - 模块文档注释（偏差声明 D1~D13）
    - 导入：`alloc::string::String` / `alloc::vec::Vec` / `eneros_crypto::{sm2_verify, Sm2PublicKey, Sm2Signature, CsRng}`
    - 支持类型：
      - `DeviceId(pub u64)` — derive Clone/Copy/Debug/PartialEq/Eq/Hash
      - `SocketAddr { pub ipv4: u32, pub port: u16 }` — derive Clone/Copy/Debug/PartialEq/Eq/Hash
      - `SystemResource` 枚举（Cpu/Memory/Storage/Network/Gpio/Timer/SystemBus）— derive Clone/Copy/Debug/PartialEq/Eq/Hash
    - `ResourceTarget` 枚举（Device(DeviceId)/Agent(AgentId)/File(String)/Network(SocketAddr)/SystemResource(SystemResource)）— derive Clone/Debug/PartialEq/Eq
    - `PermissionSet(pub u32)` — 手动 bitflags 实现（D6）：
      - 常量：READ=0x01/WRITE=0x02/EXECUTE=0x04/CONTROL=0x08/CONFIG=0x10/ADMIN=0x20/NONE=0x00/ALL=0x3F
      - 方法：bits()/from_bits()/contains()/insert()/is_empty()/is_all()
      - impl BitOr/BitOrAssign
      - derive Clone/Copy/Debug/PartialEq/Eq/PartialOrd/Ord/Hash
    - `ConstraintType` 枚举（MaxPower/MinPower/SocMin/SocMax/VoltageMin/VoltageMax/FreqMin/FreqMax）— derive Clone/Copy/Debug/PartialEq/Eq/Hash
    - `ConstraintPack` 结构（max_power/min_power/soc_limit/voltage_limit/frequency_limit）— derive Clone/Debug
      - `check_constraint(&self, value: f32, ctype: ConstraintType) -> bool`
      - `clamp(&self, value: f32, ctype: ConstraintType) -> f32`
      - `default()` → 全零约束（拒绝所有）
    - `CapabilityToken` 结构（9 字段，signature: [u8; 64]）— derive Clone/Debug
      - `is_expired(&self, now: u64) -> bool`
      - `check_permission(&self, perm: PermissionSet) -> bool`
      - `check_constraint(&self, value: f32, ctype: ConstraintType) -> bool`
      - `verify(&self, issuer_pk: &Sm2PublicKey) -> Result<(), AgentError>` (D10: 返回 Result<(), AgentError>)
      - `serialize_unsigned(&self) -> Vec<u8>` — 序列化除 signature 外的所有字段
  - 注意：此文件暂时无法编译（需要 lib.rs 声明模块，Task 6）

- [x] **Task 4: 创建 capability/builder.rs — Token 构建器** ✅ (171 lines)
  - 创建 `crates/agents/agent/src/capability/builder.rs`：
    - 导入：`eneros_crypto::{sm2_sign, Sm2KeyPair, CsRng}` + crate 类型
    - `CapabilityTokenBuilder` 结构（owner/target/permissions/constraints/ttl_ms）
    - `CapabilityTokenBuilder::new() -> Self` — 默认值（owner=AgentId::ZERO, permissions=NONE, constraints=default, ttl=0）
    - Builder 方法：`owner(mut self, id)/target(mut self, t)/permission(mut self, p)/constraints(mut self, c)/ttl(mut self, ms)`
    - `build_and_sign(self, issuer_keypair: &Sm2KeyPair, issuer_id: AgentId, now: u64, rng: &mut CsRng) -> Result<CapabilityToken, AgentError>` (D1~D3)
      - 1. 生成随机 token_id（rng.fill_bytes → u64）
      - 2. 构造 CapabilityToken（signature 全零）
      - 3. serialize_unsigned → data
      - 4. sm2_sign(&data, &keypair.private_key, &keypair.public_key, rng)
      - 5. 填入 signature = sig.to_bytes()
      - 6. 返回 Ok(token)

- [x] **Task 5: 创建 capability/verifier.rs — Token 验证器** ✅ (52 lines)
  - 创建 `crates/agents/agent/src/capability/verifier.rs`：
    - `TokenVerifier` 结构（issuer_pk: Sm2PublicKey）— derive Debug
    - `TokenVerifier::new(issuer_pk: Sm2PublicKey) -> Self`
    - `TokenVerifier::verify(&self, token: &CapabilityToken) -> Result<(), AgentError>` — 委托 token.verify(&self.issuer_pk)

- [x] **Task 6: 创建 capability/mod.rs + 更新 lib.rs** ✅ (含修正: eneros-crypto 补充 sm2_sign/sm2_verify re-export, 196 tests pass)
  - 创建 `crates/agents/agent/src/capability/mod.rs`：
    - `pub mod token;`
    - `pub mod builder;`
    - `pub mod verifier;`
    - Re-exports: CapabilityToken/ResourceTarget/PermissionSet/ConstraintPack/ConstraintType/CapabilityTokenBuilder/TokenVerifier/DeviceId/SocketAddr/SystemResource
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 新增 `pub mod capability;`
    - 新增 re-exports
    - 更新 VERSION = "0.39.0"
    - 更新模块文档注释
  - 验证：`cargo build -p eneros-agent`

- [x] **Task 7: 编写 capability 单元测试** ✅ (29 tests: 15 token + 11 builder + 3 verifier, total 225)
  - 在 `token.rs` 末尾追加 `#[cfg(test)] mod tests`（≥15 个测试）：
    - PermissionSet: bits/contains/insert/BitOr/ALL/NONE
    - ConstraintPack: check_constraint within/violated + clamp
    - CapabilityToken: is_expired/check_permission/check_constraint
    - serialize_unsigned 确定性（同一 token 两次序列化结果相同）
  - 在 `builder.rs` 末尾追加 `#[cfg(test)] mod tests`（≥10 个测试）：
    - build_and_sign 成功（生成有效签名）
    - build_and_sign 后 verify 成功
    - 篡改 permissions → verify 失败
    - 篡改 token_id → verify 失败
    - 篡改 owner → verify 失败
    - 篡改 issued_at → verify 失败
    - 篡改 signature → verify 失败
    - 不同 keypair 签名 → 用错误 pk verify 失败
    - ttl 正确设置 expires_at
    - token_id 随机性（两次 build 生成不同 token_id）
  - 在 `verifier.rs` 末尾追加 `#[cfg(test)] mod tests`（≥3 个测试）：
    - TokenVerifier verify 成功
    - TokenVerifier verify 篡改失败
    - TokenVerifier new + verify 链式调用
  - 验证：`cargo test -p eneros-agent`

- [x] **Task 8: 编写集成测试 tests/capability_test.rs** ✅ (10 integration tests, total 235)
  - 创建 `crates/agents/agent/tests/capability_test.rs`（≥8 个测试）：
    - 端到端：build_and_sign → verify → Ok
    - 端到端：build_and_sign → 篡改 → verify → Err
    - TokenVerifier 端到端验证
    - PermissionSet 组合权限检查
    - ConstraintPack 电力约束边界测试
    - 过期 token 检查
    - 不同 owner 的 token 独立验证
    - 多个 token 用同一 keypair 签名 + 批量验证
  - 验证：`cargo test -p eneros-agent`

- [x] **Task 9: 编写设计文档** ✅ (740 lines, 14 sections, 2 mermaid diagrams)
  - 创建 `docs/agents/agent-capability-token-design.md`（≥400 行）：
    - 14 节：版本目标/架构定位/前置依赖/handle 算法流程/数据结构设计/模块结构/偏差声明 D1~D13/错误处理/PermissionSet 设计/ConstraintPack 电力约束/序列化与签名/性能分析/后续解锁版本
    - 2 个 mermaid 图：build_and_sign 流程图 + verify 流程图

- [x] **Task 10: 同步版本标识** ✅ (Cargo.toml/Makefile/ci.yml/gate.rs 0.38.0→0.39.0)
  - `Cargo.toml` workspace version: 0.38.0 → 0.39.0
  - `Makefile` VERSION: 0.38.0 → 0.39.0
  - `.github/workflows/ci.yml` 版本头: v0.38.0 → v0.39.0
  - `ci/src/gate.rs` 版本注释: v0.38.0 → v0.39.0（2 处）
  - `crates/agents/agent/Cargo.toml` description 更新
  - 验证：无 0.38.0 残留（docs 历史引用除外）

- [x] **Task 11: 全量构建与质量验证** ✅ (6/7 PASS: fmt/clippy/test/workspace/cross-compile/deny-licenses pass; audit-advisories fail=已知环境问题)
  - `cargo fmt --all -- --check`
  - `cargo clippy -p eneros-agent --all-targets -- -D warnings`
  - `cargo test -p eneros-agent`
  - `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`
  - `cargo run -p eneros-ci`
  - WSL2 aarch64 交叉编译: `cargo build -p eneros-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - `cargo deny check licenses bans sources`

# Task Dependencies

- Task 1, Task 2 可并行（不同文件）
- Task 3, Task 4, Task 5 可并行（不同文件，但都依赖 Task 1 的错误变体 + Task 2 的 crypto 依赖）
- Task 6 依赖 Task 3 + Task 4 + Task 5（需要 3 个文件都创建好才能声明模块）
- Task 7 依赖 Task 6（需要模块可编译）
- Task 8 依赖 Task 7（需要单元测试通过）
- Task 9, Task 10 可并行（不同文件）
- Task 11 依赖所有前置任务

# Wave Plan

- **Wave 1**: Task 1 + Task 2（并行）
- **Wave 2**: Task 3 + Task 4 + Task 5（并行）
- **Wave 3**: Task 6
- **Wave 4**: Task 7
- **Wave 5**: Task 8
- **Wave 6**: Task 9 + Task 10（并行）
- **Wave 7**: Task 11
