# Tasks — v0.40.0 Capability Manager

## Wave 1: 基础数据结构（可并行）

- [x] Task 1: 扩展 `AgentError` 新增 3 个错误变体
  - 在 `crates/agents/agent/src/error.rs` 的 `AgentError` 枚举中新增以下变体（紧跟现有 `TokenNotSigned` 之后）：
    - `TokenFrozen` — 能力 Token 已冻结
    - `TokenRevoked` — 能力 Token 已撤销
    - `NoCapability { agent: AgentId, target: String }` — 无匹配能力（target 使用 `format!("{:?}", target)` 的 String，避免 error 模块依赖 capability 模块造成循环引用）
  - 为 3 个新变体实现 `Display` trait（使用 `core::fmt::Write`，no_std 合规）
  - 在 `error.rs` 中新增单元测试 `test_manager_error_variants_display`（验证 3 个新变体的 Display 输出）
  - 在 `error.rs` 中新增单元测试 `test_manager_error_variants_eq`（验证 PartialEq 行为）
  - 注意：保留现有 `#[derive(Debug, Clone, PartialEq)]`（不 derive `Eq`，因 v0.39.0 的 `ConstraintViolated` 含 f32）
  - 验证：`cargo build -p eneros-agent` 通过

- [x] Task 2: 创建 `capability/store.rs` — TokenStore 令牌存储
  - 新建文件 `crates/agents/agent/src/capability/store.rs`
  - 模块文档注释：说明 TokenStore 作为 CapabilityManager 的内部存储层，包含 D1（BTreeMap 替代 HashMap）+ D6（跳过 by_target 索引）偏差声明
  - 导入：`alloc::vec::Vec`、`alloc::collections::BTreeMap`、`crate::capability::token::CapabilityToken`、`crate::id::AgentId`
  - 定义 `TokenStore` 结构：
    ```rust
    pub struct TokenStore {
        tokens: BTreeMap<u64, CapabilityToken>,
        by_owner: BTreeMap<AgentId, Vec<u64>>,
    }
    ```
  - 实现方法：
    - `pub fn new() -> Self` — 创建空存储
    - `pub fn insert(&mut self, token: CapabilityToken)` — 插入令牌并更新 by_owner 索引
    - `pub fn remove(&mut self, token_id: u64) -> Option<CapabilityToken>` — 移除令牌并同步更新 by_owner 索引
    - `pub fn get(&self, token_id: u64) -> Option<&CapabilityToken>` — 按 ID 查询令牌
    - `pub fn get_mut(&mut self, token_id: u64) -> Option<&mut CapabilityToken>` — 按 ID 可变查询
    - `pub fn list_by_owner(&self, owner: AgentId) -> Vec<&CapabilityToken>` — 列出 owner 的所有令牌
    - `pub fn token_ids_by_owner(&self, owner: AgentId) -> Vec<u64>` — 列出 owner 的所有令牌 ID
    - `pub fn list_expired_ids(&self, now: u64) -> Vec<u64>` — 列出所有已过期令牌的 ID
    - `pub fn len(&self) -> usize` — 令牌总数
    - `pub fn is_empty(&self) -> bool` — 是否为空
    - `pub fn iter(&self) -> impl Iterator<Item = (&u64, &CapabilityToken)>` — 迭代器
  - 实现 `Default` trait
  - 编写单元测试（至少 8 个）：
    - `test_store_new_empty`
    - `test_store_insert_and_get`
    - `test_store_insert_updates_by_owner`
    - `test_store_remove`
    - `test_store_remove_updates_index`
    - `test_store_list_by_owner`
    - `test_store_list_expired_ids`
    - `test_store_len_and_is_empty`
  - 验证：`cargo build -p eneros-agent` 通过（注意：此时 mod.rs 尚未声明 `pub mod store;`，可临时在文件内 `#[cfg(test)] mod tests` 内部测试）

## Wave 2: 能力管理器（依赖 Task 1 + Task 2）

- [x] Task 3: 创建 `capability/manager.rs` — CapabilityManager 能力管理器
  - 新建文件 `crates/agents/agent/src/capability/manager.rs`
  - 模块文档注释：说明 CapabilityManager 封装签发/校验/冻结/撤销/过期清理，包含 D2~D5、D7~D9 偏差声明
  - 导入：
    - `alloc::collections::BTreeSet`（D1 偏差：替代 HashSet）
    - `alloc::string::ToString` / `alloc::format`
    - `eneros_crypto::{CsRng, Sm2KeyPair}`
    - `crate::capability::builder::CapabilityTokenBuilder`
    - `crate::capability::store::TokenStore`
    - `crate::capability::token::{CapabilityToken, PermissionSet, ResourceTarget}`
    - `crate::error::AgentError`
    - `crate::id::AgentId`
  - 定义 `CapabilityManager` 结构：
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
  - 实现方法：
    - `pub fn new(keypair: Sm2KeyPair, issuer_id: AgentId) -> Self` — 构造管理器（D7 偏差：issuer_id 可配置）
    - `pub fn issue(&mut self, builder: CapabilityTokenBuilder, now: u64) -> Result<CapabilityToken, AgentError>` — 签发令牌（D3 + D8 偏差）
      - 调用 `builder.build_and_sign(&self.issuer_keypair, self.issuer_id, now, &mut self.rng)`
      - 将返回的令牌克隆一份存入 store（原令牌返回给调用者）
    - `pub fn verify_token(&self, token: &CapabilityToken) -> Result<(), AgentError>` — 验证令牌签名（委托 `token.verify(&self.issuer_keypair.public_key)`）
    - `pub fn check_access(&self, agent_id: AgentId, target: &ResourceTarget, perm: PermissionSet, now: u64) -> Result<&CapabilityToken, AgentError>` — 检查访问权限（D4 + D9 偏差）
      - 遍历 `store.list_by_owner(agent_id)`
      - 跳过 `frozen.contains(&token.token_id)` 的令牌
      - 跳过 `revoked.contains(&token.token_id)` 的令牌
      - 跳过 `token.is_expired(now)` 的令牌
      - 跳过 `token.target != *target` 的令牌（D9 修复：target 必须匹配）
      - 跳过 `!token.check_permission(perm)` 的令牌
      - 找到匹配令牌返回 `Ok(token)`
      - 全部不匹配返回 `Err(NoCapability { agent: agent_id, target: format!("{:?}", target) })`
    - `pub fn freeze(&mut self, agent_id: AgentId) -> usize` — 冻结 Agent 所有令牌，返回冻结数量
      - 遍历 `store.token_ids_by_owner(agent_id)`
      - 将每个 token_id 加入 `frozen` 集合
      - 返回加入的数量
    - `pub fn unfreeze(&mut self, token_id: u64) -> bool` — 解冻单个令牌，返回是否成功
    - `pub fn revoke(&mut self, token_id: u64) -> bool` — 撤销令牌
      - 调用 `store.remove(token_id)`
      - 加入 `revoked` 集合
      - 返回是否成功移除
    - `pub fn list_tokens(&self) -> Vec<&CapabilityToken>` — 列出所有令牌
    - `pub fn cleanup_expired(&mut self, now: u64) -> usize` — 清理过期令牌，返回清理数量
      - 调用 `store.list_expired_ids(now)`
      - 逐个 `store.remove(id)`
      - 返回清理数量
    - `pub fn is_frozen(&self, token_id: u64) -> bool` — 检查令牌是否冻结
    - `pub fn is_revoked(&self, token_id: u64) -> bool` — 检查令牌是否撤销
    - `pub fn store(&self) -> &TokenStore` — 获取存储引用
    - `pub fn issuer_id(&self) -> AgentId` — 获取签发者 ID
  - 编写单元测试（至少 12 个）：
    - `test_manager_new`
    - `test_manager_issue_success`
    - `test_manager_issue_and_verify`
    - `test_manager_check_access_allowed`
    - `test_manager_check_access_denied_no_token`
    - `test_manager_check_access_denied_wrong_target`（D9 验证）
    - `test_manager_check_access_denied_wrong_permission`
    - `test_manager_check_access_denied_expired`
    - `test_manager_check_access_denied_frozen`
    - `test_manager_freeze_agent`
    - `test_manager_revoke_token`
    - `test_manager_cleanup_expired`
    - `test_manager_unfreeze`
  - 验证：`cargo build -p eneros-agent` 通过

## Wave 3: 模块集成（依赖 Task 2 + Task 3）

- [x] Task 4: 更新 `capability/mod.rs` + `lib.rs`
  - 修改 `crates/agents/agent/src/capability/mod.rs`：
    - 新增 `pub mod store;`（字母序在 `pub mod token;` 之前）
    - 新增 `pub mod manager;`（字母序在 `pub mod store;` 之前，`pub mod token;` 之后；实际顺序：builder / manager / store / token / verifier）
    - 新增 re-exports：`pub use store::TokenStore;` 和 `pub use manager::CapabilityManager;`
    - 更新模块文档注释（v0.40.0，包含 TokenStore + CapabilityManager 描述）
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 在 re-exports 中新增 `CapabilityManager` 和 `TokenStore`
    - 更新 `pub const VERSION: &str = "0.40.0";`
    - 更新模块文档注释（包含 capability manager 描述）
  - 验证：`cargo build -p eneros-agent` 通过

## Wave 4: 集成测试 + 单元测试补强（可并行，依赖 Task 4）

- [x] Task 5: 编写集成测试 `tests/capability_manager_test.rs`
  - 新建文件 `crates/agents/agent/tests/capability_manager_test.rs`
  - 编写集成测试（至少 10 个）：
    - `test_manager_issue_and_check_access_end_to_end` — 端到端签发 + 访问检查
    - `test_manager_check_access_wrong_target_rejected` — 错误 target 被拒绝（D9 验证）
    - `test_manager_freeze_blocks_check_access` — 冻结后 check_access 被拒绝
    - `test_manager_unfreeze_restores_access` — 解冻后恢复访问
    - `test_manager_revoke_removes_token` — 撤销移除令牌
    - `test_manager_cleanup_expired_removes_expired` — 过期清理
    - `test_manager_multiple_agents_isolation` — 多 Agent 隔离
    - `test_manager_multiple_tokens_same_agent` — 同一 Agent 多令牌
    - `test_manager_verify_token_signature` — 验证令牌签名
    - `test_manager_no_capability_error` — 无能力错误返回正确字段
  - 验证：`cargo test -p eneros-agent --test capability_manager_test` 通过

## Wave 5: 文档 + 版本同步（可并行，依赖 Task 4）

- [x] Task 6: 编写设计文档 `docs/agents/agent-capability-manager-design.md`
  - 新建文件 `docs/agents/agent-capability-manager-design.md`
  - 文档结构（14 章）：
    1. 概述（v0.40.0 目标）
    2. 背景与动机（v0.39.0 单令牌 → v0.40.0 管理器）
    3. 架构设计（TokenStore + CapabilityManager 双层架构图，mermaid）
    4. TokenStore 数据结构（BTreeMap 双索引）
    5. CapabilityManager 数据结构
    6. 签发流程（issue 方法流程图，mermaid）
    7. 校验流程（check_access 算法，含 D9 target 匹配修复）
    8. 冻结/解冻机制（崩溃 Agent 处理）
    9. 撤销机制
    10. 过期清理
    11. 错误处理（3 个新错误变体）
    12. no_std 合规性
    13. 偏差声明表（D1~D10）
    14. 测试覆盖
  - 验证：文档存在且包含 mermaid 图

- [x] Task 7: 同步版本标识符 0.39.0 → 0.40.0
  - 修改 `Cargo.toml`（workspace 根）：`version = "0.39.0"` → `version = "0.40.0"`
  - 修改 `crates/agents/agent/Cargo.toml`：`version = "0.39.0"` → `version = "0.40.0"`
  - 修改 `Makefile`：`VERSION ?= 0.39.0` → `VERSION ?= 0.40.0`
  - 修改 `.github/workflows/ci.yml`：版本字符串 0.39.0 → 0.40.0
  - 修改 `ci/src/gate.rs`：版本字符串 0.39.0 → 0.40.0（2 处）
  - 验证：`grep -r "0.39.0" --include="*.toml" --include="*.yml" --include="*.rs" --include="Makefile" .` 仅剩历史 spec 文档（不在版本同步范围）

## Wave 6: 完整构建验证（依赖所有任务）

- [x] Task 8: 完整构建验证
  - 执行 `cargo fmt --all -- --check` 验证格式
  - 执行 `cargo clippy -p eneros-agent --all-targets -- -D warnings` 验证 lint
  - 执行 `cargo test -p eneros-agent` 验证所有单元测试通过
  - 执行 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 验证 workspace 回归
  - 执行 WSL2 交叉编译：`wsl bash -c "cd /mnt/e/eneros && cargo build -p eneros-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"`
  - 执行 `cargo deny check licenses bans sources` 验证许可证
  - 执行 `cargo deny check advisories`（已知环境问题，记录但不阻塞）
  - 记录测试数量和构建时间

# Task Dependencies

- Task 1（error.rs）→ Task 3（manager.rs 依赖新错误变体）
- Task 2（store.rs）→ Task 3（manager.rs 依赖 TokenStore）
- Task 3（manager.rs）→ Task 4（mod.rs/lib.rs 集成）
- Task 4 → Task 5（集成测试依赖模块声明）
- Task 4 → Task 6（设计文档依赖最终 API）
- Task 4 → Task 7（版本同步独立但逻辑上在集成后）
- Task 5 + Task 6 + Task 7 → Task 8（构建验证依赖所有完成）
