# v0.78.0 消息序列化与签名 Spec

> **Change-ID**: `develop-v0780-message-signing`
> **版本**: v0.78.0（Phase 2 P2-A 收尾）
> **蓝图来源**: `蓝图/phase2.md` §v0.78.0（行 1037~1280）
> **关联 ADR**: ADR-0003（合规闸门）/ ADR-0004（v1.0.0 重定义）
> **前置版本**: v0.77.0（消息路由器，已完成）/ v0.31.0（国密 SM2/SM3 库，已完成）
> **后续解锁**: v0.98.0（纵向加密 mTLS）/ v0.117.0（审计哈希链复用 SM3）

---

## Why

EnerOS Agent Bus 当前（v0.77.0）已具备路由派发能力，但消息体本身**无完整性保护**：
- 任何能写入 DDS Topic 的实体均可伪造 `DdsSample.payload`
- 跨域联邦场景下，仅靠 mTLS（v0.98.0）无法覆盖应用层消息篡改
- 缺少防重放机制（同一消息可被多次重放执行）

v0.78.0 引入 `SignedEnvelope`（签名信封）+ `MessageSigner` trait + 默认 `MockSigner`（纯 Rust，无外部依赖）+ feature-gated `Sm2Signer`（封装 v0.31.0 国密库），为 Agent Bus 消息提供**应用层签名防线**：mTLS 之外的第二道完整性验证，支持签名/验签/防重放（5s 时间戳窗口）。

## What Changes

### 新增代码

- **新文件** `crates/protocols/agent-bus-dds/src/codec.rs`
  - `CodecKind` 枚举（`Cdr` / `Bincode` / `Json`，仅作为 header 元数据 tag，**不实现实际编解码**，D6）
  - `CodecError` 枚举 + `Display` + `core::error::Error`

- **新文件** `crates/protocols/agent-bus-dds/src/signing.rs`
  - `KeyId(pub u64)` newtype（D13）
  - `MsgId(pub u64)` newtype（D8，替代蓝图 `Uuid::new_v4()`）
  - `EnvelopeHeader` 结构体（7 字段）
  - `SignedEnvelope` 结构体（3 字段：header / payload / signature）
  - `SignError` 枚举（6 变体）+ `Display` + `core::error::Error`
  - `MessageSigner` trait（`sign` / `verify`，无 `Send + Sync`，D7）
  - `MockSigner` 默认实现（derive `Debug, Default`；XOR-style 测试签名）
  - `Sm2Signer` 实现（feature = `"sm2"`，封装 `eneros-crypto`）
  - `KeyStore` 实现（feature = `"sm2"`，`BTreeMap<KeyId, Sm2PublicKey>`）
  - `pack_and_sign()` 自由函数（构造 `SignedEnvelope`）
  - `unpack_and_verify()` 自由函数（验签 + 防重放）

- **新文件** `configs/signing_keys.toml`（密钥配置模板）
- **新文件** `docs/protocols/message-signing-design.md`（12 章节 + 2 Mermaid 图 + D1~D14 偏差声明表）

### 修改代码

- `crates/protocols/agent-bus-dds/src/lib.rs`
  - 新增 `pub mod codec;` + `pub mod signing;`（alphabetical 顺序）
  - 新增 `pub use codec::{CodecError, CodecKind};`
  - 新增 `pub use signing::{pack_and_sign, unpack_and_verify, EnvelopeHeader, KeyId, MessageSigner, MockSigner, MsgId, SignError, SignedEnvelope};`
  - 新增 `#[cfg(feature = "sm2")] pub use signing::{KeyStore, Sm2Signer};`
  - 更新顶部模块文档注释（描述 v0.78.0 签名层）
  - 更新偏差声明表为 D1~D14（v0.78.0）

- `crates/protocols/agent-bus-dds/Cargo.toml`
  - 新增 `[features] sm2 = ["eneros-crypto"]`
  - 新增 `[dependencies] eneros-crypto = { path = "../../security/crypto", default-features = false, optional = true }`

- `Cargo.toml`（workspace 根）版本号 `0.77.0` → `0.78.0`
- `Makefile` 版本号 `0.77.0` → `0.78.0`（header 注释 + VERSION 变量）
- `.github/workflows/ci.yml` 版本号 `0.77.0` → `0.78.0`
- `ci/src/gate.rs` clippy 段 + test 段注释更新 `eneros-agent-bus-dds v0.78.0` 含新类型列表

### 测试（T49~T63，15 个新增）

- T49~T50：`CodecKind` 变体 + `CodecError::Display`
- T51~T52：`KeyId` / `MsgId` newtype
- T53~T54：`EnvelopeHeader` / `SignedEnvelope` 字段访问
- T55：`SignError::Display` 非空
- T56~T58：`MockSigner` 签名/验签/防重放
- T59~T63：`pack_and_sign()` / `unpack_and_verify()` 正反例（篡改 payload / 重放 / 篡改签名）

### 无破坏性变更

- v0.75.0~v0.77.0 现有类型 `DdsSample` / `DdsNode` / `MessageRouter` 等**完全不动**
- 默认 feature `default = []` 保持不变；`sm2` 为 opt-in，不影响 no_std 主线
- v0.77.0 的 `T1~T48` 测试**无回归**

## Impact

- **受影响 spec**:
  - `develop-v0750-agent-bus-dds` — v0.78.0 在此 crate 之上扩展，不修改其类型
  - `develop-v0760-dds-topic-qos` — v0.78.0 不修改 topic/qos
  - `develop-v0770-message-router` — v0.78.0 复用 `policy::AgentId`，不修改 router
  - `develop-v0310-crypto-sm` — v0.78.0 通过 `sm2` feature 调用 `eneros-crypto` API（`sm2_sign` / `sm2_verify` / `Sm2Signature` / `Sm2PrivateKey` / `Sm2PublicKey` / `CsRng`）

- **受影响代码**:
  - `crates/protocols/agent-bus-dds/src/codec.rs`（新建）
  - `crates/protocols/agent-bus-dds/src/signing.rs`（新建）
  - `crates/protocols/agent-bus-dds/src/lib.rs`（修改：模块声明 + 导出 + 偏差表 + 测试）
  - `crates/protocols/agent-bus-dds/Cargo.toml`（修改：新增 `sm2` feature）
  - `configs/signing_keys.toml`（新建）
  - `docs/protocols/message-signing-design.md`（新建）
  - `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）

## ADDED Requirements

### Requirement: 消息签名信封

系统 SHALL 提供 `SignedEnvelope` 类型，封装业务 payload、header 与 64 字节签名，作为 Agent Bus 跨域消息的标准传输载体。

#### Scenario: 构造签名信封
- **WHEN** 调用 `pack_and_sign(signer, payload_bytes, source, topic, qos, codec, key_id, msg_id, now)` 
- **THEN** 返回 `SignedEnvelope { header, payload, signature }`，header 字段完整填充，signature 为 64 字节

#### Scenario: 验证签名信封
- **WHEN** 调用 `unpack_and_verify(signer, envelope, now)` 且 signature 与 header+payload 匹配
- **THEN** 返回 `Ok(true)`

#### Scenario: 篡改 payload 验签失败
- **WHEN** 修改 `envelope.payload` 后调用 `unpack_and_verify`
- **THEN** 返回 `Ok(false)` 或 `Err(VerifyFailed)`

#### Scenario: 防重放 — 时间戳过期
- **WHEN** `now.abs_diff(header.timestamp) > 5_000`（5 秒窗口外）
- **THEN** 返回 `Err(SignError::StaleTimestamp)`

### Requirement: 签名算法可插拔

系统 SHALL 提供 `MessageSigner` trait，允许通过 `MockSigner`（默认，纯 Rust，无依赖）或 `Sm2Signer`（feature = `"sm2"`，封装 `eneros-crypto`）注入具体签名实现。

#### Scenario: 默认 MockSigner 可用
- **WHEN** 在 `default` feature 下编译
- **THEN** `MockSigner` 实例化成功，`pack_and_sign()` / `unpack_and_verify()` 可调用

#### Scenario: SM2 签名器可选启用
- **WHEN** 启用 `sm2` feature（`cargo build --features sm2`）
- **THEN** `Sm2Signer` / `KeyStore` 可用，签名通过 `eneros-crypto::sm2_sign` 实现

### Requirement: 防重放时间戳窗口

系统 SHALL 在 `MessageSigner::verify()` 中接受 `now: u64` 参数（由调用方注入当前时钟），并拒绝 `|now - header.timestamp| > 5_000` 的消息。

#### Scenario: 时钟注入
- **WHEN** no_std 环境无系统时钟
- **THEN** 调用方负责传入 `now: u64`（毫秒），无需 `current_timestamp()` 全局函数

## MODIFIED Requirements

### Requirement: eneros-agent-bus-dds crate 文档

crate 顶部文档注释扩展为描述 v0.78.0 签名层（在 v0.77.0 路由层之上），偏差声明表扩展为 D1~D14。

## REMOVED Requirements

### Requirement: 蓝图 §4.2 `Send + Sync` bound
**Reason**: no_std 单线程环境，无需 `Send + Sync`（D7，沿用 v0.59.0/v0.64.0/v0.72.0/v0.77.0 先例）
**Migration**: `MessageCodec` / `MessageSigner` trait 无 `Send + Sync` bound

### Requirement: 蓝图 §4.2 `impl Serialize` / `DeserializeOwned` 泛型约束
**Reason**: 引入 `serde` 会破坏 no_std 约束（serde_derive 依赖 proc-macro 在 no_std 路径下不可用），且增加 Cargo 依赖
**Migration**: `pack_and_sign()` 接受已序列化的 `&[u8]` payload，业务侧自行序列化（D10）

### Requirement: 蓝图 §4.1 `Uuid::new_v4()` 作为 `msg_id`
**Reason**: `uuid` crate 引入额外依赖，且 v4 随机 Uuid 需要系统 RNG（no_std 限制）
**Migration**: `MsgId(pub u64)` newtype，由调用方分配（自增计数器或时间戳派生，D8）

### Requirement: 蓝图 §4.4 `current_timestamp()` 全局函数
**Reason**: no_std 无系统时钟，且 `current_timestamp()` 隐含全局状态
**Migration**: `verify()` 与 `pack_and_sign()` 显式接受 `now: u64` 参数（D9）

### Requirement: 蓝图 §4.2 `HashMap<KeyId, Sm2PublicKey>`
**Reason**: no_std 合规（项目规则 §4.3 / §8.1 禁忌 #3）
**Migration**: `BTreeMap<KeyId, Sm2PublicKey>`（D5，沿用 v0.76.0 D1 先例）

### Requirement: 蓝图 §4.2 实际 CDR/Bincode/JSON 编解码
**Reason**: 引入 `cdr` / `bincode` / `serde_json` 三个 crate 会大幅膨胀依赖树，且 v0.78.0 核心目标是"签名信封"而非"序列化框架"
**Migration**: `CodecKind` 仅作为 header 元数据 tag（`u8` 表示），业务侧自行选择序列化库（D6）

### Requirement: 蓝图 §6.3 性能基准（≥1000 sig/s / ≥1M encode/s）
**Reason**: CI 无法在无硬件加速环境下稳定验证软实现性能指标
**Migration**: 仅保留正确性测试，性能延后到 v0.158.0 硬件加速后验证（D12）

### Requirement: 蓝图 §4.5 `sm2_sign(&self.private_key, &digest)` 2 参数 API
**Reason**: 实际 `eneros-crypto::sm2_sign(msg, sk, pk, rng)` 需要 4 参数（公钥用于计算 Z 值，RNG 用于生成 k），蓝图 API 与实际不符
**Migration**: `Sm2Signer` 内部持有 `(Sm2PrivateKey, Sm2PublicKey)` 密钥对 + `CsRng`，签名时直接传 `header+payload` 拼接 buffer（无需预 SM3 哈希，因为 `sm2_sign` 内部已含 SM3 流程，D14）

---

## 偏差声明（D1~D14）

| 偏差 | 说明 |
|------|------|
| **D1** | 扩展 v0.75.0 `eneros-agent-bus-dds` crate（不新建 crate）；签名层与 DDS 同属协议层（项目规则 §2.3.1，沿用 v0.77.0 D1） |
| **D2** | 文档位于 `docs/protocols/message-signing-design.md`（项目规则 §2.3.3，非蓝图 `docs/phase2/message_signing.md`） |
| **D3** | 配置位于 `configs/signing_keys.toml`（项目规则 §2.3，非蓝图 `config/`） |
| **D4** | 测试内嵌 `src/lib.rs` T49~T63（沿用 v0.75.0~v0.77.0 模式，非蓝图 `tests/signing_verify.rs` / `tests/codec_bench.rs`） |
| **D5** | `KeyStore.keys: BTreeMap<KeyId, Sm2PublicKey>` 替代 `HashMap`（no_std 合规，v0.76.0 D1 先例） |
| **D6** | `CodecKind` 仅作为 header 元数据 tag，**不实现**实际 CDR/Bincode/JSON 编解码（避免引入 `cdr` / `bincode` / `serde_json` 三个 crate；扩展 v0.76.0 D6） |
| **D7** | `MessageCodec` / `MessageSigner` trait 无 `Send + Sync` bound（no_std 单线程，v0.59.0/v0.64.0/v0.72.0/v0.77.0 先例） |
| **D8** | `MsgId(pub u64)` newtype 替代 `Uuid::new_v4()`（无 `uuid` crate 依赖；Karpathy 简化） |
| **D9** | `verify()` / `pack_and_sign()` 显式接受 `now: u64` 参数，无 `current_timestamp()` 全局函数（no_std 无系统时钟） |
| **D10** | `pack_and_sign(payload: &[u8])` 接受已序列化字节，无 `impl Serialize` 泛型约束（避免 `serde` 依赖） |
| **D11** | 复用 `policy::AgentId(pub u64)`（v0.77.0 已定义），不重复定义 |
| **D12** | 不实现性能基准测试（CI 无法稳定验证 ≥1000 sig/s），仅保留正确性测试；性能延后到 v0.158.0 硬件加速 |
| **D13** | `KeyId(pub u64)` newtype（与 `AgentId` 解耦：密钥轮换允许 key_id ≠ agent_id） |
| **D14** | `MessageSigner` trait + `MockSigner` 默认实现；`Sm2Signer` + `KeyStore` 在 `sm2` feature 后（默认 build 不引入 `eneros-crypto`，保持 no_std 最小依赖） |

---

## 范围外（Out of Scope）

- **CDR/Bincode 实际编解码**：D6，仅保留 `CodecKind` tag
- **密钥吊销流程**：蓝图 §6.5 故障注入测试，延后到 v0.117.0 审计哈希链
- **密钥落盘加密**：蓝图 §8.5 坑点，延后到 v0.98.0 mTLS
- **gPTP 时间同步**：蓝图 §8.1 风险，由 v0.79.0 提供（本版本接受 `now: u64` 注入）
- **DDS write 前置签名集成**：蓝图 §5.3 实现路径第 3 步，由调用方（v0.89.0 联邦消息总线）集成
- **审计哈希链**：v0.117.0 复用 `sm3_hash`，本版本不实现
