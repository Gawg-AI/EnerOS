# Checklist

## codec.rs — 新建
- [x] C1 `CodecKind` 枚举（`Cdr` / `Bincode` / `Json`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C2 `CodecKind::as_u8()` 返回 0/1/2
- [x] C3 `CodecKind::from_u8()` 返回 `Option<CodecKind>`（0→Cdr / 1→Bincode / 2→Json / 其他→None）
- [x] C4 `CodecError` 枚举（3 变体：`Unsupported(CodecKind)` / `InvalidData` / `BufferTooShort`），派生 `Debug, Clone, PartialEq, Eq`
- [x] C5 `CodecError` 实现 `Display` + `core::error::Error`
- [x] C6 **不实现** `MessageCodec` trait 与实际编解码（D6）
- [x] C7 无 `use std::*`（仅 `core::*`）

## signing.rs — 核心类型（默认编译）
- [x] C8 `KeyId(pub u64)` newtype，派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
- [x] C9 `MsgId(pub u64)` newtype，派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
- [x] C10 `EnvelopeHeader` 结构体（7 字段：msg_id / timestamp / source / topic / qos / codec / key_id），派生 `Debug, Clone, PartialEq, Eq`
- [x] C11 `EnvelopeHeader::source` 类型为 `crate::policy::AgentId`（D11，复用 v0.77.0）
- [x] C12 `EnvelopeHeader::codec` 类型为 `crate::codec::CodecKind`
- [x] C13 `SignedEnvelope` 结构体（3 字段：header / payload / signature），派生 `Debug, Clone, PartialEq, Eq`
- [x] C14 `SignedEnvelope::signature` 类型为 `[u8; 64]`（固定 64 字节）
- [x] C15 `SignError` 枚举（6 变体：`EncodeFailed` / `UnknownKey(KeyId)` / `StaleTimestamp` / `SigningFailed` / `VerifyFailed` / `MockError`），派生 `Debug, Clone, PartialEq, Eq`
- [x] C16 `SignError` 实现 `Display` + `core::error::Error`
- [x] C17 `MessageSigner` trait 定义（`sign` + `verify` 两方法），**无** `Send + Sync` bound（D7）
- [x] C18 `MessageSigner::verify()` 接受 `now: u64` 参数（D9，防重放时钟注入）

## signing.rs — MockSigner 默认实现
- [x] C19 `MockSigner` 单元结构体，派生 `Debug, Default`
- [x] C20 `MockSigner::sign()` 返回 `Ok([u8; 64])`（确定性：基于 header.timestamp + payload.len() 派生）
- [x] C21 `MockSigner::verify()` 时间戳窗口内匹配签名返回 `Ok(true)`
- [x] C22 `MockSigner::verify()` 时间戳窗口内不匹配签名返回 `Ok(false)`
- [x] C23 `MockSigner::verify()` 时间戳过期（`now.abs_diff > 5_000`）返回 `Err(StaleTimestamp)`

## signing.rs — pack_and_sign / unpack_and_verify 自由函数
- [x] C24 `pack_and_sign()` 签名：`(signer: &dyn MessageSigner, payload: &[u8], source: AgentId, topic: &str, qos: u8, codec: CodecKind, key_id: KeyId, msg_id: MsgId, now: u64) -> Result<SignedEnvelope, SignError>`
- [x] C25 `pack_and_sign()` 接受 `payload: &[u8]`（已序列化字节，非 `impl Serialize`，D10）
- [x] C26 `pack_and_sign()` 接受 `now: u64`（时钟注入，D9）
- [x] C27 `pack_and_sign()` 内部构造 `EnvelopeHeader` 并调 `signer.sign()`，返回 `SignedEnvelope`
- [x] C28 `unpack_and_verify()` 签名：`(signer: &dyn MessageSigner, envelope: &SignedEnvelope, now: u64) -> Result<bool, SignError>`
- [x] C29 `unpack_and_verify()` 委托 `signer.verify(&envelope.header, &envelope.payload, &envelope.signature, now)`

## signing.rs — sm2 feature-gated（仅 `--features sm2` 编译）
- [x] C30 `#[cfg(feature = "sm2")]` 标注 `KeyStore` / `Sm2Signer` 与其 impl
- [x] C31 `KeyStore` 结构体（`keys: BTreeMap<KeyId, Sm2PublicKey>`，D5，非 HashMap）
- [x] C32 `KeyStore::new()` / `insert(id, pk)` / `get(id) -> Option<&Sm2PublicKey>` / `remove(id) -> Option<Sm2PublicKey>`
- [x] C33 `Sm2Signer` 结构体（`private_key: Sm2PrivateKey` / `public_key: Sm2PublicKey` / `key_id: KeyId` / `keystore: KeyStore` / `rng: CsRng`）
- [x] C34 `Sm2Signer::new(private_key, public_key, key_id) -> Self`（初始化空 KeyStore + CsRng）
- [x] C35 `Sm2Signer::register_peer_key(&mut self, id, pk)` 委托 `keystore.insert`
- [x] C36 `Sm2Signer::sign()` 调 `eneros_crypto::sm2_sign(&buf, &self.private_key, &self.public_key, &mut self.rng)`（4 参数，D14）
- [x] C37 `Sm2Signer::sign()` **不预 SM3 哈希**（直接传 header+payload 拼接 buffer，D14）
- [x] C38 `Sm2Signer::sign()` 将 `Sm2Signature` 转 `[u8; 64]`（调 `Sm2Signature::to_bytes()`）
- [x] C39 `Sm2Signer::verify()` 先校验时间戳窗口（> 5s 返回 `Err(StaleTimestamp)`）
- [x] C40 `Sm2Signer::verify()` 查 `keystore` 未命中返回 `Err(UnknownKey)`，命中后调 `eneros_crypto::sm2_verify(&buf, &sig, pk)`
- [x] C41 `MessageSigner` trait impl for `Sm2Signer`

## lib.rs — 模块声明 + 导出 + 测试
- [x] C42 添加 `pub mod codec;` + `pub mod signing;`（alphabetical 顺序）
- [x] C43 添加 `pub use codec::{CodecError, CodecKind};`
- [x] C44 添加 `pub use signing::{pack_and_sign, unpack_and_verify, EnvelopeHeader, KeyId, MessageSigner, MockSigner, MsgId, SignError, SignedEnvelope};`
- [x] C45 添加 `#[cfg(feature = "sm2")] pub use signing::{KeyStore, Sm2Signer};`
- [x] C46 更新 `lib.rs` 顶部模块文档注释（描述 v0.78.0 签名层）
- [x] C47 更新偏差声明表（v0.78.0 D1~D14）
- [x] C48 T49 新增：`CodecKind` 变体 + `as_u8()` / `from_u8()` 往返
- [x] C49 T50 新增：`CodecError::Display` 输出非空（3 变体）
- [x] C50 T51 新增：`KeyId` newtype 基本访问
- [x] C51 T52 新增：`MsgId` newtype 基本访问
- [x] C52 T53 新增：`EnvelopeHeader` 构造与字段访问
- [x] C53 T54 新增：`SignedEnvelope` 字段访问
- [x] C54 T55 新增：`SignError::Display` 输出非空（6 变体）
- [x] C55 T56 新增：`MockSigner::sign()` 返回 Ok(64 字节)
- [x] C56 T57 新增：`MockSigner::verify()` 匹配签名返回 Ok(true)
- [x] C57 T58 新增：`MockSigner::verify()` 时间戳过期返回 Err(StaleTimestamp)
- [x] C58 T59 新增：`pack_and_sign()` + MockSigner 成功构造
- [x] C59 T60 新增：`unpack_and_verify()` + MockSigner 匹配返回 Ok(true)
- [x] C60 T61 新增：`unpack_and_verify()` 篡改 payload 返回 Ok(false)
- [x] C61 T62 新增：`unpack_and_verify()` 篡改 signature 返回 Ok(false)
- [x] C62 T63 新增：`unpack_and_verify()` 时间戳过期返回 Err(StaleTimestamp)

## Cargo.toml — feature + 依赖
- [x] C63 `[features]` 含 `sm2 = ["eneros-crypto"]`
- [x] C64 `[dependencies]` 含 `eneros-crypto = { path = "../../security/crypto", default-features = false, optional = true }`
- [x] C65 `default = []` 保持不变（默认不启用 sm2）
- [x] C66 `cyclone-dds = []` 保持不变（v0.75.0 既有）

## 配置文件
- [x] C67 `configs/signing_keys.toml` 存在
- [x] C68 包含字段 `key_id` / `private_key_path` / `public_keys` 数组 / `timestamp_window_ms = 5000`

## 设计文档
- [x] C69 `docs/protocols/message-signing-design.md` 存在
- [x] C70 12 章节完整（版本目标 / 前置依赖 / 交付物 / 数据结构 / 接口 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险 / 偏差声明）
- [x] C71 2 Mermaid 图（签名/验签时序图 + 防重放决策流程图）
- [x] C72 D1~D14 偏差声明表
- [x] C73 文档在 `docs/protocols/` 下（非蓝图 `docs/phase2/`，D2）

## 版本同步
- [x] C74 根 `Cargo.toml` 版本号 `0.78.0`
- [x] C75 `Makefile` 版本号 `0.78.0`（header 注释 + VERSION 变量）
- [x] C76 `.github/workflows/ci.yml` 版本号 `0.78.0`
- [x] C77 `ci/src/gate.rs` clippy 段注释更新 `eneros-agent-bus-dds v0.78.0` 含新类型列表
- [x] C78 `ci/src/gate.rs` test 段注释同上

## 构建校验（§2.4.2 C6~C11）
- [x] C79 `cargo metadata --format-version 1` 成功
- [x] C80 `cargo test -p eneros-agent-bus-dds` 全部通过（63 个测试 + 1 doctest）
- [x] C81 `cargo build -p eneros-agent-bus-dds --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过（默认 feature）
- [x] C82 `cargo build -p eneros-agent-bus-dds --features sm2 --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C83 `cargo fmt -p eneros-agent-bus-dds -- --check` 通过
- [x] C84 `cargo clippy -p eneros-agent-bus-dds --all-targets -- -D warnings` 无 warning（默认 feature）
- [x] C85 `cargo clippy -p eneros-agent-bus-dds --all-features --all-targets -- -D warnings` 无 warning（含 sm2 feature）
- [x] C86 `cargo deny check licenses bans sources` 通过

## 回归
- [x] C87 v0.75.0~v0.77.0 现有 T1~T48 测试仍全绿（无回归）

## no_std 合规
- [x] C88 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C89 无 `panic!` / `todo!` / `unimplemented!`
- [x] C90 子模块（codec.rs / signing.rs）不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
- [x] C91 无 `std::collections::HashMap`（D5：BTreeMap）
- [x] C92 无 `Send + Sync` bound（D7）
- [x] C93 无 `spin::Mutex` 包装（沿用 v0.77.0 D8 风格，&mut self）
- [x] C94 无 `uuid` crate 依赖（D8：MsgId(pub u64)）
- [x] C95 无 `serde` / `serde_json` crate 依赖（D10：&[u8] payload）
- [x] C96 `sm2` feature 启用前不引入 `eneros-crypto`（D14，default build 保持最小依赖）

## 目录规范
- [x] C97 新文件在 `crates/protocols/agent-bus-dds/src/`（扩展现有 crate，D1）
- [x] C98 文档在 `docs/protocols/` 下（D2）
- [x] C99 配置在 `configs/` 下（D3）
- [x] C100 无根目录 crate（除 `ci/`）
- [x] C101 无垃圾文件（target/ / *.elf / *.bin / IDE 缓存）

## 简化设计验证（Karpathy 原则）
- [x] C102 无 `uuid` crate 依赖（D8：MsgId(pub u64)）
- [x] C103 无 `serde` / `serde_json` 依赖（D10：&[u8] payload）
- [x] C104 无 `cdr` / `bincode` 依赖（D6：CodecKind 仅 tag）
- [x] C105 无 `current_timestamp()` 全局函数（D9：now 参数注入）
- [x] C106 无性能基准测试代码（D12：CI 无法验证 ≥1000 sig/s）
- [x] C107 无密钥吊销流程（范围外，延后到 v0.117.0）
- [x] C108 无密钥落盘加密（范围外，延后到 v0.98.0）
- [x] C109 扩展现有 crate 而非新建（D1）
- [x] C110 复用 v0.77.0 `policy::AgentId`（D11，不重复定义）
- [x] C111 `Sm2Signer` 在 feature 后，默认 build 无 eneros-crypto 依赖（D14）

## 破坏性变更
- [x] C112 无破坏性变更（纯增量版本；v0.75.0~v0.77.0 类型签名不变；默认 feature 不变）
