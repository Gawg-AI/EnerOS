# Checklist — v0.40.0 Capability Manager

## C1: 错误变体扩展（Task 1）

- [x] C1: `AgentError` 新增 `TokenFrozen` 变体
- [x] C2: `AgentError` 新增 `TokenRevoked` 变体
- [x] C3: `AgentError` 新增 `NoCapability { agent: AgentId, target: String }` 变体
- [x] C4: 3 个新变体均有 `Display` impl
- [x] C5: `TokenFrozen` 的 Display 输出包含 "frozen" 字样
- [x] C6: `TokenRevoked` 的 Display 输出包含 "revoked" 字样
- [x] C7: `NoCapability` 的 Display 输出包含 agent 和 target 信息
- [x] C8: 新变体 derive Clone/PartialEq（保留 v0.39.0 的 derive 配置，不含 Eq）
- [x] C9: `NoCapability` 使用 `String` 而非 `ResourceTarget`（避免 error 模块循环依赖 capability 模块）
- [x] C10: `test_manager_error_variants_display` 测试存在且通过
- [x] C11: `test_manager_error_variants_eq` 测试存在且通过
- [x] C12: `cargo build -p eneros-agent` 通过（Task 1 完成后）

## C2: TokenStore 数据结构（Task 2）

- [x] C13: `crates/agents/agent/src/capability/store.rs` 文件存在
- [x] C14: 模块文档注释包含 D1（BTreeMap 替代 HashMap）偏差声明
- [x] C15: 模块文档注释包含 D6（跳过 by_target 索引）偏差声明
- [x] C16: 模块文档注释包含 no_std 合规声明
- [x] C17: 导入 `alloc::vec::Vec`
- [x] C18: 导入 `alloc::collections::BTreeMap`
- [x] C19: 导入 `crate::capability::token::CapabilityToken`
- [x] C20: 导入 `crate::id::AgentId`
- [x] C21: `TokenStore` 结构定义（2 字段：tokens + by_owner）
- [x] C22: `tokens: BTreeMap<u64, CapabilityToken>` 字段
- [x] C23: `by_owner: BTreeMap<AgentId, Vec<u64>>` 字段
- [x] C24: `TokenStore` derive Debug

## C3: TokenStore 方法实现（Task 2）

- [x] C25: `pub fn new() -> Self` 方法实现
- [x] C26: `pub fn insert(&mut self, token: CapabilityToken)` 方法实现
- [x] C27: `insert` 更新 `tokens` 主表
- [x] C28: `insert` 更新 `by_owner` 索引（若 owner 不存在则创建新 Vec）
- [x] C29: `pub fn remove(&mut self, token_id: u64) -> Option<CapabilityToken>` 方法实现
- [x] C30: `remove` 从 `tokens` 主表移除
- [x] C31: `remove` 同步更新 `by_owner` 索引（移除 token_id，若 Vec 为空则移除 owner 键）
- [x] C32: `pub fn get(&self, token_id: u64) -> Option<&CapabilityToken>` 方法实现
- [x] C33: `pub fn get_mut(&mut self, token_id: u64) -> Option<&mut CapabilityToken>` 方法实现
- [x] C34: `pub fn list_by_owner(&self, owner: AgentId) -> Vec<&CapabilityToken>` 方法实现
- [x] C35: `pub fn token_ids_by_owner(&self, owner: AgentId) -> Vec<u64>` 方法实现
- [x] C36: `pub fn list_expired_ids(&self, now: u64) -> Vec<u64>` 方法实现
- [x] C37: `list_expired_ids` 调用 `token.is_expired(now)` 检查过期
- [x] C38: `pub fn len(&self) -> usize` 方法实现
- [x] C39: `pub fn is_empty(&self) -> bool` 方法实现
- [x] C40: `pub fn iter(&self) -> impl Iterator<Item = (&u64, &CapabilityToken)>` 方法实现
- [x] C41: `impl Default for TokenStore` 实现

## C4: TokenStore 单元测试（Task 2）

- [x] C42: `test_store_new_empty` 测试存在且通过
- [x] C43: `test_store_insert_and_get` 测试存在且通过
- [x] C44: `test_store_insert_updates_by_owner` 测试存在且通过
- [x] C45: `test_store_remove` 测试存在且通过
- [x] C46: `test_store_remove_updates_index` 测试存在且通过
- [x] C47: `test_store_list_by_owner` 测试存在且通过
- [x] C48: `test_store_list_expired_ids` 测试存在且通过
- [x] C49: `test_store_len_and_is_empty` 测试存在且通过
- [x] C50: `cargo build -p eneros-agent` 通过（Task 2 完成后）

## C5: CapabilityManager 数据结构（Task 3）

- [x] C51: `crates/agents/agent/src/capability/manager.rs` 文件存在
- [x] C52: 模块文档注释包含 D2（keypair 替代 sk+pk）偏差声明
- [x] C53: 模块文档注释包含 D3（issue 接受 now）偏差声明
- [x] C54: 模块文档注释包含 D4（check_access 接受 now）偏差声明
- [x] C55: 模块文档注释包含 D5（移除 next_token_id）偏差声明
- [x] C56: 模块文档注释包含 D7（new 接受 issuer_id）偏差声明
- [x] C57: 模块文档注释包含 D8（build_and_sign 4 参数）偏差声明
- [x] C58: 模块文档注释包含 D9（target 匹配检查）偏差声明
- [x] C59: 模块文档注释包含 no_std 合规声明
- [x] C60: 导入 `alloc::collections::BTreeSet`
- [x] C61: 导入 `alloc::format` / `alloc::string::ToString`
- [x] C62: 导入 `eneros_crypto::{CsRng, Sm2KeyPair}`
- [x] C63: 导入 `crate::capability::builder::CapabilityTokenBuilder`
- [x] C64: 导入 `crate::capability::store::TokenStore`
- [x] C65: 导入 `crate::capability::token::{CapabilityToken, PermissionSet, ResourceTarget}`
- [x] C66: 导入 `crate::error::AgentError`
- [x] C67: 导入 `crate::id::AgentId`
- [x] C68: `CapabilityManager` 结构定义（6 字段）
- [x] C69: `store: TokenStore` 字段
- [x] C70: `frozen: BTreeSet<u64>` 字段
- [x] C71: `revoked: BTreeSet<u64>` 字段
- [x] C72: `issuer_keypair: Sm2KeyPair` 字段（D2 偏差）
- [x] C73: `issuer_id: AgentId` 字段
- [x] C74: `rng: CsRng` 字段
- [x] C75: `CapabilityManager` derive Debug

## C6: CapabilityManager 方法实现（Task 3）

- [x] C76: `pub fn new(keypair: Sm2KeyPair, issuer_id: AgentId) -> Self` 方法实现（D7 偏差）
- [x] C77: `pub fn issue(&mut self, builder: CapabilityTokenBuilder, now: u64) -> Result<CapabilityToken, AgentError>` 方法实现（D3 + D8 偏差）
- [x] C78: `issue` 调用 `builder.build_and_sign(&self.issuer_keypair, self.issuer_id, now, &mut self.rng)`
- [x] C79: `issue` 将令牌克隆存入 store
- [x] C80: `issue` 返回原令牌给调用者
- [x] C81: `pub fn verify_token(&self, token: &CapabilityToken) -> Result<(), AgentError>` 方法实现
- [x] C82: `verify_token` 委托 `token.verify(&self.issuer_keypair.public_key)`
- [x] C83: `pub fn check_access(&self, agent_id: AgentId, target: &ResourceTarget, perm: PermissionSet, now: u64) -> Result<&CapabilityToken, AgentError>` 方法实现（D4 + D9 偏差）
- [x] C84: `check_access` 遍历 `store.list_by_owner(agent_id)`
- [x] C85: `check_access` 跳过 `frozen.contains(&token.token_id)` 的令牌
- [x] C86: `check_access` 跳过 `revoked.contains(&token.token_id)` 的令牌
- [x] C87: `check_access` 跳过 `token.is_expired(now)` 的令牌
- [x] C88: `check_access` 跳过 `token.target != *target` 的令牌（D9 修复）
- [x] C89: `check_access` 跳过 `!token.check_permission(perm)` 的令牌
- [x] C90: `check_access` 找到匹配令牌返回 `Ok(token)`
- [x] C91: `check_access` 全部不匹配返回 `Err(NoCapability { agent, target: format!("{:?}", target) })`
- [x] C92: `pub fn freeze(&mut self, agent_id: AgentId) -> usize` 方法实现
- [x] C93: `freeze` 遍历 `store.token_ids_by_owner(agent_id)`
- [x] C94: `freeze` 将每个 token_id 加入 `frozen` 集合
- [x] C95: `freeze` 返回冻结数量
- [x] C96: `pub fn unfreeze(&mut self, token_id: u64) -> bool` 方法实现
- [x] C97: `pub fn revoke(&mut self, token_id: u64) -> bool` 方法实现
- [x] C98: `revoke` 调用 `store.remove(token_id)`
- [x] C99: `revoke` 将 token_id 加入 `revoked` 集合
- [x] C100: `revoke` 返回是否成功移除
- [x] C101: `pub fn list_tokens(&self) -> Vec<&CapabilityToken>` 方法实现
- [x] C102: `pub fn cleanup_expired(&mut self, now: u64) -> usize` 方法实现
- [x] C103: `cleanup_expired` 调用 `store.list_expired_ids(now)`
- [x] C104: `cleanup_expired` 逐个 `store.remove(id)`
- [x] C105: `cleanup_expired` 返回清理数量
- [x] C106: `pub fn is_frozen(&self, token_id: u64) -> bool` 方法实现
- [x] C107: `pub fn is_revoked(&self, token_id: u64) -> bool` 方法实现
- [x] C108: `pub fn store(&self) -> &TokenStore` 方法实现
- [x] C109: `pub fn issuer_id(&self) -> AgentId` 方法实现

## C7: CapabilityManager 单元测试（Task 3）

- [x] C110: `test_manager_new` 测试存在且通过
- [x] C111: `test_manager_issue_success` 测试存在且通过
- [x] C112: `test_manager_issue_and_verify` 测试存在且通过
- [x] C113: `test_manager_check_access_allowed` 测试存在且通过
- [x] C114: `test_manager_check_access_denied_no_token` 测试存在且通过
- [x] C115: `test_manager_check_access_denied_wrong_target` 测试存在且通过（D9 验证）
- [x] C116: `test_manager_check_access_denied_wrong_permission` 测试存在且通过
- [x] C117: `test_manager_check_access_denied_expired` 测试存在且通过
- [x] C118: `test_manager_check_access_denied_frozen` 测试存在且通过
- [x] C119: `test_manager_freeze_agent` 测试存在且通过
- [x] C120: `test_manager_revoke_token` 测试存在且通过
- [x] C121: `test_manager_cleanup_expired` 测试存在且通过
- [x] C122: `test_manager_unfreeze` 测试存在且通过
- [x] C123: `cargo build -p eneros-agent` 通过（Task 3 完成后）

## C8: 模块集成 — mod.rs（Task 4）

- [x] C124: `capability/mod.rs` 新增 `pub mod store;`
- [x] C125: `capability/mod.rs` 新增 `pub mod manager;`
- [x] C126: 模块声明字母序正确（builder / manager / store / token / verifier）
- [x] C127: `capability/mod.rs` 新增 `pub use store::TokenStore;`
- [x] C128: `capability/mod.rs` 新增 `pub use manager::CapabilityManager;`
- [x] C129: 模块文档注释更新（v0.40.0，包含 TokenStore + CapabilityManager）

## C9: 模块集成 — lib.rs（Task 4）

- [x] C130: `lib.rs` re-export 新增 `CapabilityManager`
- [x] C131: `lib.rs` re-export 新增 `TokenStore`
- [x] C132: `lib.rs` `pub const VERSION = "0.40.0"`
- [x] C133: `lib.rs` 模块文档注释更新（包含 capability manager 描述）
- [x] C134: `cargo build -p eneros-agent` 通过（Task 4 完成后）

## C10: 集成测试（Task 5）

- [x] C135: `crates/agents/agent/tests/capability_manager_test.rs` 文件存在
- [x] C136: `test_manager_issue_and_check_access_end_to_end` 测试存在且通过
- [x] C137: `test_manager_check_access_wrong_target_rejected` 测试存在且通过（D9 验证）
- [x] C138: `test_manager_freeze_blocks_check_access` 测试存在且通过
- [x] C139: `test_manager_unfreeze_restores_access` 测试存在且通过
- [x] C140: `test_manager_revoke_removes_token` 测试存在且通过
- [x] C141: `test_manager_cleanup_expired_removes_expired` 测试存在且通过
- [x] C142: `test_manager_multiple_agents_isolation` 测试存在且通过
- [x] C143: `test_manager_multiple_tokens_same_agent` 测试存在且通过
- [x] C144: `test_manager_verify_token_signature` 测试存在且通过
- [x] C145: `test_manager_no_capability_error` 测试存在且通过
- [x] C146: `cargo test -p eneros-agent --test capability_manager_test` 通过

## C11: 设计文档（Task 6）

- [x] C147: `docs/agents/agent-capability-manager-design.md` 文件存在
- [x] C148: 文档包含 14 个章节
- [x] C149: 第 3 章包含 mermaid 架构图（TokenStore + CapabilityManager 双层）
- [x] C150: 第 6 章包含 mermaid 签发流程图
- [x] C151: 第 7 章描述 check_access 算法，包含 D9 target 匹配修复说明
- [x] C152: 第 8 章描述冻结/解冻机制（崩溃 Agent 处理）
- [x] C153: 第 11 章描述 3 个新错误变体
- [x] C154: 第 12 章描述 no_std 合规性
- [x] C155: 第 13 章包含 D1~D10 偏差声明表

## C12: 版本同步（Task 7）

- [x] C156: `Cargo.toml`（workspace 根）version = "0.40.0"
- [x] C157: `crates/agents/agent/Cargo.toml` version = "0.40.0"
- [x] C158: `Makefile` VERSION = 0.40.0
- [x] C159: `.github/workflows/ci.yml` 版本字符串 = 0.40.0
- [x] C160: `ci/src/gate.rs` 版本字符串 = 0.40.0（2 处）
- [x] C161: `crates/agents/agent/src/lib.rs` VERSION = "0.40.0"
- [x] C162: 无残留 0.39.0 版本字符串（除历史 spec 文档）

## C13: 构建验证（Task 8）

- [x] C163: `cargo fmt --all -- --check` 通过
- [x] C164: `cargo clippy -p eneros-agent --all-targets -- -D warnings` 无 warning
- [x] C165: `cargo test -p eneros-agent` 全部通过
- [x] C166: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（workspace 回归）
- [x] C167: WSL2 交叉编译 `cargo build -p eneros-agent --target aarch64-unknown-none` 通过
- [x] C168: `cargo deny check licenses bans sources` 通过
- [x] C169: `cargo deny check advisories`（已知环境问题，记录但不阻塞）
- [x] C170: 记录测试数量和构建时间

## C14: no_std 合规性

- [x] C171: `store.rs` 无 `use std::*`
- [x] C172: `manager.rs` 无 `use std::*`
- [x] C173: `store.rs` 无 `panic!` / `todo!` / `unimplemented!`
- [x] C174: `manager.rs` 无 `panic!` / `todo!` / `unimplemented!`
- [x] C175: `store.rs` 仅使用 `alloc::*` / `core::*`
- [x] C176: `manager.rs` 仅使用 `alloc::*` / `core::*`
- [x] C177: 子模块无 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）

## C15: 目录结构校验

- [x] C178: 新文件位于 `crates/agents/agent/src/capability/` 下（C1 校验）
- [x] C179: workspace `Cargo.toml` members 已包含 `crates/agents/agent`（C2 校验，已存在）
- [x] C180: 跨 crate path 引用正确（C3 校验，`eneros-crypto = { path = "../../security/crypto" }`）
- [x] C181: 设计文档位于 `docs/agents/` 下（C4 校验）
- [x] C182: 无根目录 crate（C5 校验）
- [x] C183: 无垃圾文件被追踪（C13 校验：无 target/、*.elf、*.bin）
- [x] C184: `.gitignore` 已覆盖新产生的文件类型（C14 校验）

## C16: 安全性验证

- [x] C185: D9 修复已验证：`check_access` 检查 `token.target == *target`
- [x] C186: 冻结令牌被 `check_access` 跳过（无法绕过冻结）
- [x] C187: 撤销令牌被 `check_access` 跳过（无法绕过撤销）
- [x] C188: 过期令牌被 `check_access` 跳过（无法绕过过期）
- [x] C189: `NoCapability` 错误包含 agent 和 target 信息（便于审计）
- [x] C190: `issue` 使用 CSRNG 生成 token_id（不可预测）
- [x] C191: `issue` 使用 SM2 签名（不可伪造）
