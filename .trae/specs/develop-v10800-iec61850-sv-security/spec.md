# v0.108.0 IEC 61850 SV + IEC 62351 安全 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.108.0（P2-G 第 4 版，9 节齐全）。新建 crate `crates/protocols/iec61850-sv/`（eneros-iec61850-sv）与 `crates/security/iec62351/`（eneros-iec62351），依赖 eneros-iec61850-model（DaValue 复用）与 eneros-crypto（SM4-GCM / SM3-HMAC 复用）。蓝图检索确认无 v0.108.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

电力采样值（SV，IEC 61850-9-2）是保护/测量的高速数据通道，GOOSE/SV 明文传输不满足电力安全合规（36 号文 / IEC 62351）。v0.107.0 已落地 GOOSE 发布/订阅，v0.31.0 已落地国密 SM2/SM3/SM4，本版实现 SV 接收器 + GOOSE/SV 加密封装，打通联邦安全通信的「采样 + 事件 + 加密」全链路，为 v0.109.0 故障录波提供安全采样数据源。

## What Changes

- **新建** `crates/protocols/iec61850-sv/`（`eneros-iec61850-sv`，no_std + alloc，依赖仅 `eneros-iec61850-model` path 引用）：
  - `src/sv_rx.rs`：`SvSubscriber<T: L2Transport>`（泛型传输注入，D4/D5；EtherType 0x88BA 过滤、APPID/MAC 过滤、SV PDU BER 解码、smpCnt 连续性检测）
  - `src/sv_buffer.rs`：`RingBuffer<T>`（固定容量环形缓冲，溢出覆盖最旧，蓝图 §4.4）
  - `src/lib.rs`：`SvError`（4 变体，D10）+ `L2Transport` trait 复用 GOOSE 版本（D4）+ `MockL2` 复用 GOOSE 版本 + 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新建** `crates/security/iec62351/`（`eneros-iec62351`，no_std + alloc，依赖仅 `eneros-crypto` path 引用）：
  - `src/secure_goose.rs`：`SecureGoose`（SM4-GCM 加密 + SM3-HMAC 认证 + IV 计数器 + SecureFrame 封装/解封）
  - `src/secure_sv.rs`：`SecureSv`（同构于 SecureGoose，独立类型避免混淆，D8）
  - `src/key_mgmt.rs`：`SessionKey` / `KeyMgmt`（密钥轮换、过期检测、多密钥存储，D9）
  - `src/lib.rs`：`SecError`（5 变体，D10）+ 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/iec61850-sv.toml`：`[sv]` 订阅配置模板 + 中文注释 ≥7 点
- **新增** `configs/iec62351.toml`：`[security]` 密钥配置模板 + 中文注释 ≥7 点
- **新增** `docs/protocols/iec61850-sv-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增** `docs/protocols/iec62351-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 40 个单元测试**（src 内嵌 `#[cfg(test)]`：SV 侧 18 个 + IEC62351 侧 22 个）
- 根 `Cargo.toml`：members 追加 `"crates/protocols/iec61850-sv"` 与 `"crates/security/iec62351"` + version 0.107.0 → 0.108.0；`Makefile`（VERSION + 头部注释）/ `ci.yml` 注释 / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：纯新增 crate，既有 crate 零改动（eneros-iec61850-model / eneros-crypto / eneros-iec61850-goose 仅被引用不修改）

## Impact

- Affected specs：develop-v10800-iec61850-sv-security（新建）
- Affected code：`crates/protocols/iec61850-sv/`（新建）、`crates/security/iec62351/`（新建）、`configs/`、`docs/protocols/`、根 4 文件版本号
- 上游：v0.107.0 eneros-iec61850-goose（L2Transport/MockL2 复用）、v0.31.0 eneros-crypto（Sm4Gcm/Sm3Hmac 复用）、v0.105.0 eneros-iec61850-model（DaValue 复用）
- 下游：v0.109.0 故障录波 COMTRADE

## ADDED Requirements

### Requirement: SV 采样值接收（sv_rx.rs）

The system SHALL provide `SvSubscriber<T: L2Transport>`：`new(app_id, mac, buf_size, transport)` 创建订阅者；`receive(frame)` 接收以太网帧 → 非 0x88BA 返回 Ok(false) → dst MAC 不匹配丢弃 Ok(false) → APPID 不匹配丢弃 Ok(false) → 解码 SV PDU（smpCnt 0x80 / timestamp 0x81 / channels 0x82 含长度）→ smpCnt 跳变 >1 标记 `SampleStatus::SmpJump`（采样丢失，D12）、重复为 `Duplicate`、新采样为 `New` → 有效采样写入环形缓冲返回 Ok(true)；`take_samples()` 返回 Vec<SvSample> 并清空缓冲；`set_callback` 注册后每收到有效采样调用一次。

#### Scenario: SV 帧过滤（蓝图 §4.2）
- **WHEN** 收到 EtherType 非 0x88BA 的帧
- **THEN** `receive` 返回 Ok(false)，不写入缓冲

#### Scenario: 采样丢失检测（蓝图 §4.4，D12）
- **WHEN** 先收 smpCnt=100，再收 smpCnt=103（101/102 丢失）
- **THEN** 第二次 `receive` 返回 Ok(true)，缓冲中样本 `status == SampleStatus::SmpJump`

#### Scenario: 环形缓冲溢出（蓝图 §4.4）
- **WHEN** buf_size=4，连续接收 6 个有效采样
- **THEN** `take_samples()` 返回 4 个样本（最旧 2 个被覆盖），最新 4 个保序

### Requirement: GOOSE/SV 加密封装（secure_goose.rs / secure_sv.rs）

The system SHALL provide `SecureGoose` 与 `SecureSv`（同构）：`new(session_key)` 以会话密钥初始化 SM4-GCM 与 SM3-HMAC；`encrypt(plaintext)` → 生成 12 字节 IV（计数器 + key_id，蓝图 §4.5）→ SM4-GCM 加密 → 计算 HMAC（IV + 密文 + tag）→ 返回 `SecureFrame { key_id, iv, ciphertext, tag, hmac }`；`decrypt(frame)` → 验证 HMAC（常量时间比较，防时序攻击）→ SM4-GCM 解密 → 返回明文；HMAC 不匹配返回 `SecError::HmacMismatch`；tag 不匹配返回 `SecError::DecryptFailed`。

#### Scenario: 加密往返（蓝图 §4.5）
- **WHEN** 对 GOOSE PDU 明文 `encrypt` 后立即 `decrypt`
- **THEN** 解密结果 == 原明文；`frame.key_id == session.key_id`；`frame.iv` 计数器递增

#### Scenario: 篡改检测（蓝图 §6.5 故障注入）
- **WHEN** 翻转 `SecureFrame.ciphertext` 首字节后 `decrypt`
- **THEN** 返回 `Err(SecError::HmacMismatch)`（HMAC 先于解密校验）

#### Scenario: IV 唯一性（蓝图 §4.5）
- **WHEN** 同一 `SecureGoose` 实例连续加密 2 次
- **THEN** 两次 `frame.iv` 不同（计数器递增）

### Requirement: 密钥管理（key_mgmt.rs）

The system SHALL provide `SessionKey { key_id, key_data: [u8;16], mac_key: [u8;32], expiry }` 与 `KeyMgmt { local_keys, key_lifetime }`：`new(key_lifetime)` 创建；`add_key(session)` 存入密钥表；`get_current_key(now)` 返回最近添加且未过期（expiry > now）的密钥，无则返回 `SecError::KeyExpired`；`rotate_keys(now)` 若当前密钥过期则生成新密钥（key_id + 1，expiry = now + key_lifetime，key_data/mac_key 由调用方注入，D9）并设为当前；`get_key(key_id)` 按 ID 查找。

#### Scenario: 密钥轮换（蓝图 §4.4）
- **WHEN** 当前密钥 expiry = 1000，now = 1001 调用 `rotate_keys(now)`
- **THEN** 生成新密钥（key_id 递增），`get_current_key(now)` 返回新密钥

#### Scenario: 过期拒绝（蓝图 §4.4）
- **WHEN** 所有密钥均过期
- **THEN** `get_current_key(now)` 返回 `Err(SecError::KeyExpired)`

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/iec61850_sv/` → `crates/protocols/iec61850-sv/`（eneros-iec61850-sv）；蓝图 `crates/iec62351/` → `crates/security/iec62351/`（eneros-iec62351） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；SV 属 protocols，IEC 62351 属 security |
| **D2** | 蓝图 `docs/phase2/sv_security.md` → `docs/protocols/iec61850-sv-design.md` + `docs/protocols/iec62351-design.md` | 记忆 §2.3.3 强制：文档按方向分类；两个 crate 独立文档 |
| **D3** | 蓝图 `tests/sv_secure.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.107.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 删除蓝图 §4.5 `extern "C"` raw socket FFI + unsafe；SV 侧复用 GOOSE 的 `L2Transport` trait + `MockL2`（置于 lib.rs）；真实 raw socket 接线在集成层 | aarch64-unknown-none 无 libc 可链接 extern "C"；项目零 unsafe/零 C FFI 惯例；与 v0.107.0 D4 同先例 |
| **D5** | `SvSubscriber<T: L2Transport>` 泛型化，transport 由 `new` 注入（蓝图内部建 socket 写死） | 可测试性 + 网卡选择属集成层决策（Karpathy Simplicity First） |
| **D6** | 蓝图 §4.1 `RingBuffer { buf: Box<[T]> }` → `Vec<T>` 固定容量（heapless 风格）；`Box` 在 no_std 需全局分配器，Vec 更通用 | no_std 下 `Box<[T]>` 需 `alloc::boxed::Box` 且初始化冗长；`Vec::with_capacity` 更直观（v0.107.0 MockL2 用 Vec 先例） |
| **D7** | 蓝图 §4.5 `Sm4Cipher`/`Sm3Hmac` 自封装 FFI → 直接复用 eneros-crypto 的 `Sm4Gcm`/`Sm3Hmac`（纯 Rust，零 unsafe） | v0.31.0 已落地纯 Rust 实现；蓝图 FFI 代码在 aarch64-unknown-none 无法链接（无 libc）；避免重复造轮子（记忆 §5.5） |
| **D8** | 蓝图 §4.1 `SecureGoose` 单类型 → `SecureGoose` + `SecureSv` 同构双类型（内部均委托公共 `SecureChannel` 私有结构） | GOOSE 与 SV 语义独立（事件 vs 采样），调用方不应混用；公共逻辑抽取私有结构避免重复（Simplicity First） |
| **D9** | 蓝图 §4.1 `KeyMgmt.rotate_keys()` 内部生成密钥 → `rotate_keys(now, new_key_data, new_mac_key)` 由调用方注入密钥材料 | no_std 无系统熵源（CsRng 固定种子仅测试用）；生产环境密钥应由硬件 TRNG/密钥管理系统注入；与 v0.31.0 CaIssuer 外部注入 rng 先例一致 |
| **D10** | 错误模型统一：`SvError` = TransportError / BerDecodeError / InvalidConfig / BufferOverflow（4 变体）；`SecError` = KeyExpired / HmacMismatch / DecryptFailed / EncryptFailed / InvalidKeyId（5 变体） | 蓝图 SocketCreateFailed/SendFailed 随 FFI 删除合并为 TransportError；变体覆盖各失败面（对齐 v0.107.0 D10 精简风格） |
| **D11** | 性能 < 0.5ms（加密延迟）落地为 cfg(test) Instant 断言（MockL2 回路，加密+解密口径，文档声明）；§6.2 真实 GOOSE 端到端加密为实验室硬件项，以 mock 替代 | 无真实网卡硬件（与 v0.107.0 D11 同口径） |
| **D12** | 接收侧 smpCnt 跳变检测以 `SampleStatus`（New/Duplicate/SmpJump）随样本返回；蓝图 §4.4 要求检测跳变但 §4.2 `receive -> Result<(), SvError>` 无承载 → `SvSample.status: SampleStatus` 字段 + `receive -> Result<bool, SvError>` | 蓝图自相矛盾（要求检测但接口无处上报）；接收方必须能区分新采样/重复/丢样 |

## 接口契约

```rust
// ============ crates/protocols/iec61850-sv/src/lib.rs ============
pub enum SvError {
    TransportError, BerDecodeError, InvalidConfig, BufferOverflow,
}  // Debug/Clone/PartialEq（D10）

pub trait L2Transport {                              // D4（复用 GOOSE 版本）
    fn send(&mut self, frame: &[u8]) -> Result<(), SvError>;
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, SvError>;
}
pub struct MockL2 { /* 帧队列 + 发送记录 + 注入错误 + loopback（测试/集成占位，D4） */ }

// ============ src/sv_rx.rs ============
pub struct SvSample {
    pub smp_cnt: u16,
    pub timestamp: u64,
    pub channels: Vec<f32>,
    pub status: SampleStatus,                        // D12
}  // Debug/Clone/PartialEq

pub enum SampleStatus { New, Duplicate, SmpJump }    // Debug/Clone/Copy/PartialEq（D12）

pub struct SvSubscriber<T: L2Transport> {
    app_id, filter_mac, last_smp_cnt,
    callback: Option<alloc::boxed::Box<dyn Fn(&SvSample)>>,  // 去 Send+Sync bound
    transport, buffer: RingBuffer<SvSample>,
}
impl<T: L2Transport> SvSubscriber<T> {
    pub fn new(app_id: u16, mac: [u8; 6], buf_size: usize, transport: T) -> Result<Self, SvError>;
    pub fn receive(&mut self, frame: &[u8]) -> Result<bool, SvError>;  // true = 写入缓冲
    pub fn take_samples(&mut self) -> Vec<SvSample>;
    pub fn set_callback<F: Fn(&SvSample) + 'static>(&mut self, f: F);
    pub fn last_smp_cnt(&self) -> u16;
    pub fn transport_mut(&mut self) -> &mut T;
}

// ============ src/sv_buffer.rs ============
pub struct RingBuffer<T> {                           // D6
    buf: Vec<T>, head: usize, tail: usize, len: usize,
}
impl<T> RingBuffer<T> {
    pub fn new(capacity: usize) -> Self;
    pub fn push(&mut self, item: T);                 // 满则覆盖最旧（head 前移）
    pub fn drain(&mut self) -> Vec<T>;               // 返回全部并清空
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}

// ============ crates/security/iec62351/src/lib.rs ============
pub enum SecError {
    KeyExpired, HmacMismatch, DecryptFailed, EncryptFailed, InvalidKeyId,
}  // Debug/Clone/PartialEq（D10）

// ============ src/key_mgmt.rs ============
pub struct SessionKey {
    pub key_id: u32,
    pub key_data: [u8; 16],
    pub mac_key: [u8; 32],
    pub expiry: u64,
}  // Debug/Clone/PartialEq（key_data/mac_key 不派生 Debug 防泄露，D9）

pub struct KeyMgmt {
    local_keys: Vec<SessionKey>,                     // D6（Vec 替代 HashMap，no_std）
    key_lifetime: u64,
    next_key_id: u32,
}
impl KeyMgmt {
    pub fn new(key_lifetime: u64) -> Self;
    pub fn add_key(&mut self, session: SessionKey);
    pub fn get_current_key(&self, now: u64) -> Result<&SessionKey, SecError>;
    pub fn rotate_keys(&mut self, now: u64, new_key_data: [u8;16], new_mac_key: [u8;32]) -> Result<(), SecError>;  // D9
    pub fn get_key(&self, key_id: u32) -> Result<&SessionKey, SecError>;
}

// ============ src/secure_goose.rs ============
pub struct SecureFrame {
    pub key_id: u32,
    pub iv: [u8; 12],
    pub ciphertext: Vec<u8>,
    pub tag: [u8; 16],
    pub hmac: [u8; 32],
}  // Debug/Clone/PartialEq

pub struct SecureGoose { /* 内部委托 SecureChannel（D8） */ }
impl SecureGoose {
    pub fn new(session: &SessionKey) -> Self;
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<SecureFrame, SecError>;
    pub fn decrypt(&self, frame: &SecureFrame) -> Result<Vec<u8>, SecError>;
}

// ============ src/secure_sv.rs ============
pub struct SecureSv { /* 同构于 SecureGoose（D8） */ }
impl SecureSv {
    pub fn new(session: &SessionKey) -> Self;
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<SecureFrame, SecError>;
    pub fn decrypt(&self, frame: &SecureFrame) -> Result<Vec<u8>, SecError>;
}
```

## 测试规划（iec61850-sv 18 个 + iec62351 22 个，src 内嵌）

| 文件 | 编号 | 数量 | 覆盖 |
|------|------|------|------|
| sv_buffer.rs | RB1~RB6 | 6 | new 空缓冲 / push 未满追加 / push 满覆盖最旧 / drain 返回全部并清空 / len/is_empty 正确 / 溢出后保序 |
| sv_rx.rs | RX7~RX18 | 12 | 有效帧解码 / APPID 不匹配丢弃 / dst MAC 不匹配丢弃 / 非 0x88BA → Ok(false) / smpCnt 跳变 → SmpJump（D12）/ 重复帧 → Duplicate / 截断帧 → BerDecodeError / smpCnt 0x80 解码 / timestamp 0x81 解码 / channels 0x82 含长度解码 / set_callback 被调用 / take_samples 清空缓冲 |
| key_mgmt.rs | KM1~KM8 | 8 | new 空密钥表 / add_key 存储 / get_current_key 命中未过期 / get_current_key 全过期 → KeyExpired / rotate_keys 生成新密钥 / rotate_keys 未过期不轮换 / get_key 按 ID 命中 / get_key miss → InvalidKeyId |
| secure_goose.rs | SG9~SG19 | 11 | 加密往返一致 / frame.key_id 正确 / IV 计数器递增 / 篡改 ciphertext → HmacMismatch / 篡改 tag → HmacMismatch（先校验 HMAC）/ 篡改 hmac → HmacMismatch / 解密空密文 / 加密空明文 / 不同 session 解密失败 / MockL2 回路加密 GOOSE PDU 往返 / 加密延迟 < 0.5ms（D11） |
| secure_sv.rs | SS20~SS22 | 3 | 加密往返一致 / IV 计数器递增 / 篡改检测（与 SG 同构，抽样验证） |

## 配置与文档

- `configs/iec61850-sv.toml`：`[sv]` app_id / dst_mac / buf_size = 16 / 中文注释 ≥7 点（EtherType 0x88BA / L2Transport 抽象 D4 / 环形缓冲溢出策略 §4.4 / 性能 <4ms 口径 D11 / 内存预算声明 / GPU 不适用 §6.6 / 安全加密由 iec62351 提供）
- `configs/iec62351.toml`：`[security]` key_lifetime_ms = 3600000 / initial_key_id = 1 / 中文注释 ≥7 点（SM4-GCM 选型 §5.1 / SM3-HMAC 认证 / 密钥轮换策略 §4.4 / IV 构造规则 §4.5 / 性能 <0.5ms 口径 D11 / 内存预算声明 / GPU 不适用 §6.6）
- `docs/protocols/iec61850-sv-design.md`：12 章节 + ≥2 Mermaid（SV 帧结构图 + smpCnt 状态机图）+ D1~D12 偏差表 + 性能口径声明（D11）
- `docs/protocols/iec62351-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 安全校验流程图重绘 + SecureFrame 结构图）+ D1~D12 偏差表 + 性能口径声明（D11）

## 版本同步

根 `Cargo.toml` version = "0.108.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.108.0 类型清单（SvSubscriber/SvSample/SampleStatus/RingBuffer/SvError/SecureGoose/SecureSv/SecureFrame/SessionKey/KeyMgmt/SecError）。
