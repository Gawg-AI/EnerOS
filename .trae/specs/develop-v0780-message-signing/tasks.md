# Tasks

- [x] Task 1: 新建 codec.rs — CodecKind / CodecError
  - [x] SubTask 1.1: 定义 `CodecKind` 枚举（`Cdr` / `Bincode` / `Json`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 1.2: 实现 `CodecKind::as_u8(&self) -> u8`（Cdr=0 / Bincode=1 / Json=2）与 `from_u8(u8) -> Option<CodecKind>`
  - [x] SubTask 1.3: 定义 `CodecError` 枚举（`Unsupported(CodecKind)` / `InvalidData` / `BufferTooShort`），派生 `Debug, Clone, PartialEq, Eq`；实现 `Display` + `core::error::Error`
  - [x] SubTask 1.4: 注意 — **不实现** `MessageCodec` trait 与实际编解码（D6）

- [x] Task 2: 新建 signing.rs — KeyId / MsgId / EnvelopeHeader / SignedEnvelope / SignError / MessageSigner / MockSigner / pack_and_sign / unpack_and_verify
  - [x] SubTask 2.1: 定义 `KeyId(pub u64)` newtype（D13），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
  - [x] SubTask 2.2: 定义 `MsgId(pub u64)` newtype（D8），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
  - [x] SubTask 2.3: 定义 `EnvelopeHeader` 结构体（`msg_id: MsgId` / `timestamp: u64` / `source: AgentId` / `topic: String` / `qos: u8` / `codec: CodecKind` / `key_id: KeyId`），派生 `Debug, Clone, PartialEq, Eq`；`source` 复用 `crate::policy::AgentId`（D11）
  - [x] SubTask 2.4: 定义 `SignedEnvelope` 结构体（`header: EnvelopeHeader` / `payload: Vec<u8>` / `signature: [u8; 64]`），派生 `Debug, Clone, PartialEq, Eq`
  - [x] SubTask 2.5: 定义 `SignError` 枚举（`EncodeFailed` / `UnknownKey(KeyId)` / `StaleTimestamp` / `SigningFailed` / `VerifyFailed` / `MockError`），派生 `Debug, Clone, PartialEq, Eq`；实现 `Display` + `core::error::Error`
  - [x] SubTask 2.6: 定义 `MessageSigner` trait（`fn sign(&self, header: &EnvelopeHeader, payload: &[u8]) -> Result<[u8; 64], SignError>` + `fn verify(&self, header: &EnvelopeHeader, payload: &[u8], sig: &[u8; 64], now: u64) -> Result<bool, SignError>`），**无** `Send + Sync` bound（D7）
  - [x] SubTask 2.7: 实现 `MockSigner`（derive `Debug, Default`）：`sign()` 返回 `Ok([header.timestamp.to_be_bytes() + payload.len() as u8 + 0..0; 64])`；`verify()` 先校验 `now.abs_diff(header.timestamp) > 5_000` → `Err(StaleTimestamp)`，否则比较签名一致性返回 `Ok(bool)`
  - [x] SubTask 2.8: 实现 `pack_and_sign(signer: &dyn MessageSigner, payload: &[u8], source: AgentId, topic: &str, qos: u8, codec: CodecKind, key_id: KeyId, msg_id: MsgId, now: u64) -> Result<SignedEnvelope, SignError>`（D9/D10：`now` 注入 + payload 为已序列化 `&[u8]`）
  - [x] SubTask 2.9: 实现 `unpack_and_verify(signer: &dyn MessageSigner, envelope: &SignedEnvelope, now: u64) -> Result<bool, SignError>`（委托 `signer.verify()`）

- [x] Task 3: 新建 signing.rs 的 `sm2` feature-gated 部分 — Sm2Signer + KeyStore
  - [x] SubTask 3.1: `#[cfg(feature = "sm2")]` 实现 `KeyStore` 结构体（`keys: BTreeMap<KeyId, Sm2PublicKey>`，D5），方法 `new()` / `insert(id, pk)` / `get(id) -> Option<&Sm2PublicKey>` / `remove(id) -> Option<Sm2PublicKey>`
  - [x] SubTask 3.2: `#[cfg(feature = "sm2")]` 实现 `Sm2Signer` 结构体（`private_key: Sm2PrivateKey` / `public_key: Sm2PublicKey` / `key_id: KeyId` / `keystore: KeyStore` / `rng: CsRng`），方法 `new(private_key, public_key, key_id) -> Self` / `register_peer_key(&mut self, id, pk)` / `sign()`（调 `eneros_crypto::sm2_sign(&buf, &self.private_key, &self.public_key, &mut self.rng)`，将 `Sm2Signature` 转 `[u8; 64]`）/ `verify()`（先时间戳校验，再查 `keystore`，再调 `sm2_verify`）
  - [x] SubTask 3.3: `#[cfg(feature = "sm2")]` 实现 `MessageSigner` trait for `Sm2Signer`
  - [x] SubTask 3.4: 注意签名时**不预 SM3 哈希** — `eneros_crypto::sm2_sign` 内部已含 SM3 流程，直接传 `header + payload` 拼接 buffer（D14）

- [x] Task 4: 修改 Cargo.toml — 新增 `sm2` feature + 可选依赖
  - [x] SubTask 4.1: 在 `[features]` 添加 `sm2 = ["eneros-crypto"]`（保留 `default = []` 与 `cyclone-dds = []`）
  - [x] SubTask 4.2: 在 `[dependencies]` 添加 `eneros-crypto = { path = "../../security/crypto", default-features = false, optional = true }`

- [x] Task 5: 修改 lib.rs — 模块声明 + 重新导出 + 偏差表 + 测试
  - [x] SubTask 5.1: 添加 `pub mod codec;` + `pub mod signing;`（alphabetical 顺序：codec < config < error < mock < node < policy < qos < registry < router < signing < topic < types）
  - [x] SubTask 5.2: 添加 `pub use codec::{CodecError, CodecKind};`
  - [x] SubTask 5.3: 添加 `pub use signing::{pack_and_sign, unpack_and_verify, EnvelopeHeader, KeyId, MessageSigner, MockSigner, MsgId, SignError, SignedEnvelope};`
  - [x] SubTask 5.4: 添加 `#[cfg(feature = "sm2")] pub use signing::{KeyStore, Sm2Signer};`
  - [x] SubTask 5.5: 更新 `lib.rs` 顶部模块文档注释，描述 v0.78.0 签名层（在 v0.77.0 路由层之上）
  - [x] SubTask 5.6: 更新偏差声明表为 v0.78.0 D1~D14
  - [x] SubTask 5.7: 新增 T49：`CodecKind` 枚举变体（Cdr/Bincode/Json）与 `as_u8()` / `from_u8()` 往返
  - [x] SubTask 5.8: 新增 T50：`CodecError::Display` 输出非空（3 个变体）
  - [x] SubTask 5.9: 新增 T51：`KeyId(pub u64)` newtype 基本访问
  - [x] SubTask 5.10: 新增 T52：`MsgId(pub u64)` newtype 基本访问
  - [x] SubTask 5.11: 新增 T53：`EnvelopeHeader` 构造与字段访问
  - [x] SubTask 5.12: 新增 T54：`SignedEnvelope` 字段访问
  - [x] SubTask 5.13: 新增 T55：`SignError::Display` 输出非空（6 个变体）
  - [x] SubTask 5.14: 新增 T56：`MockSigner::sign()` 返回 Ok(64 字节)
  - [x] SubTask 5.15: 新增 T57：`MockSigner::verify()` 匹配签名返回 Ok(true)
  - [x] SubTask 5.16: 新增 T58：`MockSigner::verify()` 时间戳过期返回 `Err(StaleTimestamp)`
  - [x] SubTask 5.17: 新增 T59：`pack_and_sign()` + MockSigner 成功构造 `SignedEnvelope`
  - [x] SubTask 5.18: 新增 T60：`unpack_and_verify()` + MockSigner 匹配返回 Ok(true)
  - [x] SubTask 5.19: 新增 T61：`unpack_and_verify()` 篡改 payload 返回 Ok(false)
  - [x] SubTask 5.20: 新增 T62：`unpack_and_verify()` 篡改 signature 返回 Ok(false)
  - [x] SubTask 5.21: 新增 T63：`unpack_and_verify()` 时间戳过期返回 `Err(StaleTimestamp)`

- [x] Task 6: 配置文件
  - [x] SubTask 6.1: 创建 `configs/signing_keys.toml`（TOML 模板：`key_id` / `private_key_path` / `public_keys` 数组 / `timestamp_window_ms = 5000`）

- [x] Task 7: 设计文档
  - [x] SubTask 7.1: 创建 `docs/protocols/message-signing-design.md`（12 章节：版本目标 / 前置依赖 / 交付物 / 数据结构 / 接口 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险 / 偏差声明；2 Mermaid 图：签名/验签时序图 + 防重放决策流程图；D1~D14 偏差声明表）

- [x] Task 8: 版本同步
  - [x] SubTask 8.1: 根 `Cargo.toml` 版本号 `0.77.0` → `0.78.0`
  - [x] SubTask 8.2: `Makefile` 版本号 `0.78.0`（header 注释 + VERSION 变量）
  - [x] SubTask 8.3: `.github/workflows/ci.yml` 版本号 `0.78.0`
  - [x] SubTask 8.4: `ci/src/gate.rs` clippy 段 + test 段注释更新 `eneros-agent-bus-dds v0.78.0` 含新类型列表（CodecKind / CodecError / KeyId / MsgId / EnvelopeHeader / SignedEnvelope / SignError / MessageSigner / MockSigner / pack_and_sign / unpack_and_verify）

- [x] Task 9: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 9.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 9.2: `cargo test -p eneros-agent-bus-dds` 全部通过（63 个测试 + 1 doctest：T1~T63）
  - [x] SubTask 9.3: `cargo build -p eneros-agent-bus-dds --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过（默认 feature）
  - [x] SubTask 9.4: `cargo build -p eneros-agent-bus-dds --features sm2 --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过（sm2 feature 编译）
  - [x] SubTask 9.5: `cargo fmt -p eneros-agent-bus-dds -- --check` 通过
  - [x] SubTask 9.6: `cargo clippy -p eneros-agent-bus-dds --all-targets -- -D warnings` 无 warning（默认 feature）
  - [x] SubTask 9.7: `cargo clippy -p eneros-agent-bus-dds --all-features --all-targets -- -D warnings` 无 warning（含 sm2 feature）
  - [x] SubTask 9.8: `cargo deny check licenses bans sources` 通过
  - [x] SubTask 9.9: 回归 — v0.75.0~v0.77.0 现有 T1~T48 测试仍全绿

# Task Dependencies

- Task 1（codec.rs）必须先完成 — Task 2 的 `EnvelopeHeader.codec: CodecKind` 依赖之
- Task 2（signing.rs 核心）必须先完成 — Task 3 的 `Sm2Signer` 实现 `MessageSigner` trait 依赖之
- Task 3（sm2 feature）依赖 Task 2 完成；与 Task 4 可并行（feature 配置不依赖 impl）
- Task 4（Cargo.toml）依赖 Task 2/3 完成（impl 完成后才能验证 feature）
- Task 5（lib.rs）依赖 Task 1/2/3 完成
- Task 6/7（配置 + 文档）可与 Task 1/2/3 并行
- Task 8（版本同步）依赖 Task 1~7 完成
- Task 9（构建校验）依赖所有前置任务完成
