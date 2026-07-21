# v0.31.0 — 国密算法库 Spec

## Why

v0.30.x 完成网络栈安全（防火墙/连接跟踪/DDoS）后，系统具备网络层防护能力，但缺少密码学基础。电力系统关键基础设施要求使用国密算法（GB/T 32918 SM2、GB/T 32905 SM3、GB/T 32907 SM4）以满足等保 2.0 和国密合规要求。

无密码学则 v0.32.0 PKI 证书、v0.39.0 能力 Token、v0.78.0 消息签名、Phase 2 mTLS、v0.113.0 Secure Boot 全部无法实现。本版本是 Phase 1 关键瓶颈版本（阻塞 8+ 下游版本）。

## What Changes

### 新增 `crates/security/crypto/` crate（eneros-crypto）

按工作区规则 §2.3.2，密码学属于跨子系统的安全基础设施，新增 `crates/security/` 子系统目录（与 kernel/hal/runtime/drivers/ai/protocols/agents 平级），首个 crate 为 `crypto`。

**模块结构**：
- `src/lib.rs` — 模块声明 + re-exports + VERSION
- `src/error.rs` — CryptoError 枚举（13 变体）
- `src/constant_time.rs` — 恒定时间比较工具
- `src/bigint.rs` — U256 大整数运算（椭圆曲线基础）
- `src/sm3/mod.rs` — SM3 密码杂凑（GB/T 32905-2016）
- `src/sm4/mod.rs` — SM4 分组密码（GB/T 32907-2016）
- `src/sm4/cbc.rs` — SM4-CBC 模式
- `src/sm4/gcm.rs` — SM4-GCM 认证加密模式
- `src/sm2/mod.rs` — SM2 椭圆曲线密码（GB/T 32918）
- `src/sm2/keypair.rs` — SM2 密钥对生成
- `src/sm2/sign.rs` — SM2 数字签名
- `src/sm2/encrypt.rs` — SM2 公钥加密
- `src/rng/csrng.rs` — 基于 SM3 的 CSRNG（NIST SP 800-90A DRBG）

**测试**：
- `tests/sm3_kat.rs` — SM3 国标测试向量（GB/T 32905-2016 附录 A）
- `tests/sm4_kat.rs` — SM4 国标测试向量（GB/T 32907-2016 附录 A）
- `tests/sm2_kat.rs` — SM2 国标测试向量（GB/T 32918.5-2017）
- 模块内单元测试覆盖大整数运算、CBC/GCM 模式、CSRNG 统计性

**文档**：`docs/security/sm-crypto-design.md` — 国密算法库设计文档（含国标引用、性能基准、内存预算、OOM 策略）

**配置**：无运行时配置（编译时配置）

### 共通变更

- **新增子系统目录** `crates/security/`（首次创建，后续 v0.32.0 PKI、v0.39.0 能力 Token 将归入此子系统）
- **根 Cargo.toml**：members 添加 `"crates/security/crypto"`
- **版本标识**：根 Cargo.toml/Makefile/ci.yml/gate.rs 升至 "0.31.0"
- **BREAKING**：无（纯新增 crate，不修改现有 crate）

## Impact

- **Affected specs**: 解锁 v0.32.0（PKI 证书）、v0.39.0（能力 Token）、v0.78.0（消息签名）、v0.113.0（Secure Boot）、Phase 2 v0.115.0（mTLS）、v0.169.0（Agent DID）
- **Affected code**:
  - 新增 `crates/security/crypto/` crate（约 15 个源文件）
  - 修改根 `Cargo.toml`（members 添加 + 版本号）
  - 修改 `Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`（版本标识）
  - **不修改**：v0.27.0~v0.30.2 的任何源文件（Surgical Changes）

## 设计决策（Karpathy 四原则应用）

### 1. Think Before Coding — 国标合规与测试策略

**问题**：国密算法必须严格遵循国标（GB/T 32905/32907/32918），任何偏差都导致合规失败。

**决策**：
- **国标测试向量是硬性验收标准**：SM3/SM4/SM2 必须通过国标附录 A 的测试向量，否则版本不通过
- **国标引用**：
  - SM3: GB/T 32905-2016 《SM3 密码杂凑算法》
  - SM4: GB/T 32907-2016 《SM4 分组密码算法》
  - SM2: GB/T 32918.1~5-2017 《SM2 椭圆曲线公钥密码算法》
- **SM2 用户 ID**：默认 `"1234567812345678"`（国标默认值，可配置）
- **CSRNG**：基于 SM3 的 DRBG，遵循 NIST SP 800-90A
- **恒定时间**：所有密钥/签名/标签比较使用恒定时间算法（防时序侧信道）
- **密钥清零**：敏感数据（私钥/会话密钥）使用后必须清零（用 `core::ptr::write_volatile` 防编译器优化）

### 2. Simplicity First — 最小可国标合规实现

**决策**：
- **纯软件实现**：不依赖 ARM64 硬件加速指令（Phase 3 考虑），无 C FFI
- **U256 固定大小**：使用 `[u64; 4]` 小端序表示 256 位大整数，不引入堆分配的大数库
- **SM2 推荐曲线**：使用国标 SM2 推荐曲线参数（Fp 域 256 位），不引入通用曲线参数
- **SM4 工作模式**：实现 ECB（基础块）+ CBC（最常用）+ GCM（认证加密），不实现 CTR/OFB/CFB（非必需）
- **CSRNG**：基于 SM3 的确定性随机比特生成器，不引入硬件 TRNG（Phase 3 考虑）
- **性能基准**：no_std 环境用循环计数占位，实机性能验证延后到 QEMU/硬件

### 3. Surgical Changes — 不修改现有源文件

- 仅修改根 `Cargo.toml`（添加 member + 版本号）
- 仅修改 `Makefile`/`ci.yml`/`gate.rs`（版本标识）
- **不修改** v0.27.0~v0.30.2 任何源文件
- **不修改** v0.30.0 的 `crates/drivers/net/src/security/`（那是网络层安全策略，与本密码学库不同）

### 4. Goal-Driven Execution — 国标测试向量是验收标准

**验证标准**：
1. SM3 KAT 通过（GB/T 32905-2016 附录 A 全部向量）
2. SM4 KAT 通过（GB/T 32907-2016 附录 A 全部向量）
3. SM2 KAT 通过（GB/T 32918.5-2017 签名/加密向量）
4. SM2 签名+验签端到端测试通过（自生成密钥对）
5. SM4-CBC 加解密端到端测试通过
6. SM4-GCM 认证加解密端到端测试通过
7. CSRNG 通过基本统计测试（不要求完整 NIST SP 800-22，延后到安全审计）
8. 恒定时间比较测试通过
9. aarch64 交叉编译通过
10. workspace 回归测试全绿

## ADDED Requirements

### Requirement: U256 大整数运算

系统 SHALL 提供 `U256` 类型用于 256 位大整数运算，作为 SM2 椭圆曲线运算的基础。

```rust
#[derive(Clone, Copy, Debug)]
pub struct U256 {
    pub limbs: [u64; 4], // 小端序
}

impl U256 {
    pub const ZERO: U256;
    pub const ONE: U256;
    pub fn from_be_bytes(bytes: &[u8; 32]) -> Self;
    pub fn to_be_bytes(&self) -> [u8; 32];
    pub fn from_hex(hex: &str) -> Result<Self, CryptoError>;
    pub fn add_mod(&self, other: &Self, m: &Self) -> Self;
    pub fn sub_mod(&self, other: &Self, m: &Self) -> Self;
    pub fn mul_mod(&self, other: &Self, m: &Self) -> Self;
    pub fn inv_mod(&self, m: &Self) -> Result<Self, CryptoError>; // 扩展欧几里得
    pub fn cmp(&self, other: &Self) -> core::cmp::Ordering;
    pub fn is_zero(&self) -> bool;
    pub fn bit(&self, index: usize) -> bool;
    pub fn bit_len(&self) -> usize;
}
```

#### Scenario: 模逆运算
- **WHEN** 计算 a^(-1) mod m，其中 a 与 m 互素
- **THEN** 返回正确的逆元

### Requirement: SM3 密码杂凑算法

系统 SHALL 实现 GB/T 32905-2016 SM3 密码杂凑算法，输出 256 位杂凑值。

```rust
pub struct Sm3;

impl Sm3 {
    pub fn hash(data: &[u8]) -> [u8; 32];
}

pub struct Sm3Hasher {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    total_len: u64,
}

impl Sm3Hasher {
    pub fn new() -> Self;
    pub fn update(&mut self, data: &[u8]);
    pub fn finalize(self) -> [u8; 32];
}

impl Default for Sm3Hasher { fn default() -> Self { Self::new() } }
```

#### Scenario: SM3 国标测试向量 1
- **WHEN** 输入 "abc"
- **THEN** 输出 66c7f0f4 62eeedd9 d1f2d46b dc10e4e2 4167c487 5cf2f7a2 297da02b 8f4ba8e0

#### Scenario: SM3 国标测试向量 2
- **WHEN** 输入 64 字节消息（"abcd...xyz" 重复填充）
- **THEN** 输出 debe9ff9 2275b8a1 38604889 c18e5a4d 6fdb70e5 387e5765 293dcba3 9c0c5732

### Requirement: SM4 分组密码算法

系统 SHALL 实现 GB/T 32907-2016 SM4 分组密码算法，128 位密钥，128 位分组，32 轮 Feistel 结构。

```rust
pub struct Sm4 {
    round_keys: [u32; 32],
}

impl Sm4 {
    pub fn new(key: &[u8; 16]) -> Self;
    pub fn encrypt_block(&self, block: &[u8; 16]) -> [u8; 16];
    pub fn decrypt_block(&self, block: &[u8; 16]) -> [u8; 16];
}
```

#### Scenario: SM4 国标测试向量
- **WHEN** 密钥 = 0123456789abcdeffedcba9876543210
- **AND** 明文 = 0123456789abcdeffedcba9876543210
- **THEN** 密文 = 681edf34d206965e86b3e94f536e4246

### Requirement: SM4-CBC 模式

系统 SHALL 提供 SM4-CBC 模式，支持变长明文/密文加解密（PKCS#7 填充）。

```rust
pub struct Sm4Cbc {
    cipher: Sm4,
    iv: [u8; 16],
}

impl Sm4Cbc {
    pub fn new(key: &[u8; 16], iv: &[u8; 16]) -> Self;
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError>;
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError>;
}
```

#### Scenario: CBC 加解密一致性
- **WHEN** 加密任意长度明文后立即解密
- **THEN** 输出等于原始明文

### Requirement: SM4-GCM 认证加密模式

系统 SHALL 提供 SM4-GCM 认证加密模式，支持 AAD（附加认证数据）。

```rust
pub struct Sm4Gcm {
    cipher: Sm4,
    nonce: [u8; 12],
}

impl Sm4Gcm {
    pub fn new(key: &[u8; 16], nonce: &[u8; 12]) -> Self;
    pub fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<(Vec<u8>, [u8; 16]), CryptoError>;
    pub fn decrypt(&self, ciphertext: &[u8], aad: &[u8], tag: &[u8; 16]) -> Result<Vec<u8>, CryptoError>;
}
```

#### Scenario: GCM 标签验证失败
- **WHEN** 解密时提供的 tag 与计算的不一致
- **THEN** 返回 CryptoError::AuthTagMismatch

### Requirement: SM2 椭圆曲线运算

系统 SHALL 实现 SM2 推荐曲线（Fp 域 256 位）上的椭圆曲线点运算。

```rust
pub struct Sm2Curve;
impl Sm2Curve {
    pub const P: U256;
    pub const A: U256;
    pub const B: U256;
    pub const N: U256;  // 阶
    pub const GX: U256; // 基点 x
    pub const GY: U256; // 基点 y
}

#[derive(Clone, Debug)]
pub struct EcPoint {
    pub x: U256,
    pub y: U256,
    pub is_infinity: bool,
}

impl EcPoint {
    pub fn generator() -> Self;
    pub fn is_on_curve(&self) -> bool;
    pub fn add(&self, other: &Self) -> Self;
    pub fn double(&self) -> Self;
    pub fn scalar_mult(&self, k: &U256) -> Self;     // Montgomery ladder
    pub fn scalar_base_mult(k: &U256) -> Self;       // k * G
    pub fn to_bytes_uncompressed(&self) -> [u8; 65]; // 04 || x || y
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError>;
}
```

#### Scenario: 基点在曲线上
- **WHEN** 检查 SM2 推荐曲线的基点 G
- **THEN** is_on_curve() 返回 true

### Requirement: SM2 密钥对

系统 SHALL 提供 SM2 密钥对生成与管理。

```rust
pub struct Sm2PrivateKey(pub [u8; 32]);
pub struct Sm2PublicKey { pub x: [u8; 32], pub y: [u8; 32] }
pub struct Sm2KeyPair {
    pub private_key: Sm2PrivateKey,
    pub public_key: Sm2PublicKey,
}

impl Sm2KeyPair {
    pub fn generate() -> Result<Self, CryptoError>;
    pub fn from_private_key(sk: &Sm2PrivateKey) -> Result<Self, CryptoError>;
    pub fn public_key_bytes(&self) -> [u8; 65]; // 04 || x || y
}

impl Sm2PrivateKey {
    pub fn as_bytes(&self) -> &[u8; 32];
    pub fn zeroize(&mut self); // 安全清零
}
```

#### Scenario: 从私钥派生公钥
- **WHEN** 给定私钥 sk
- **THEN** from_private_key(sk) 返回的公钥等于 sk * G

### Requirement: SM2 数字签名

系统 SHALL 实现 GB/T 32918.2-2016 SM2 数字签名算法。

```rust
pub struct Sm2Signature {
    pub r: [u8; 32],
    pub s: [u8; 32],
}

pub struct Sm2Signer {
    user_id: Vec<u8>, // 默认 "1234567812345678"
}

impl Sm2Signer {
    pub fn new() -> Self;
    pub fn with_user_id(user_id: &[u8]) -> Self;
    pub fn sign(&self, msg: &[u8], sk: &Sm2PrivateKey) -> Result<Sm2Signature, CryptoError>;
    pub fn verify(&self, msg: &[u8], sig: &Sm2Signature, pk: &Sm2PublicKey) -> Result<bool, CryptoError>;
}

pub fn sm2_sign(msg: &[u8], sk: &Sm2PrivateKey) -> Result<Sm2Signature, CryptoError>;
pub fn sm2_verify(msg: &[u8], sig: &Sm2Signature, pk: &Sm2PublicKey) -> Result<bool, CryptoError>;
```

#### Scenario: 签名与验签
- **WHEN** 用私钥对消息签名
- **AND** 用对应公钥验签
- **THEN** 验签返回 true

#### Scenario: 篡改消息验签失败
- **WHEN** 签名后篡改消息内容
- **THEN** 验签返回 false

### Requirement: SM2 公钥加密

系统 SHALL 实现 GB/T 32918.4-2016 SM2 公钥加密算法。

```rust
pub fn sm2_encrypt(plaintext: &[u8], pk: &Sm2PublicKey) -> Result<Vec<u8>, CryptoError>;
pub fn sm2_decrypt(ciphertext: &[u8], sk: &Sm2PrivateKey) -> Result<Vec<u8>, CryptoError>;
```

**密文格式**：C1 || C3 || C2（国标顺序）
- C1: 65 字节椭圆曲线点（04 || x || y）
- C3: 32 字节 SM3 杂凑值
- C2: 密文数据（与明文等长，XOR 加密）

#### Scenario: 加解密一致性
- **WHEN** 用公钥加密明文，再用私钥解密
- **THEN** 输出等于原始明文

### Requirement: CSRNG 安全随机数生成器

系统 SHALL 提供基于 SM3 的密码学安全随机数生成器（NIST SP 800-90A DRBG）。

```rust
pub struct CsRng {
    state: [u8; 32],
    counter: u64,
}

impl CsRng {
    pub fn new() -> Self;
    pub fn from_seed(seed: &[u8]) -> Self;
    pub fn fill_bytes(&mut self, buf: &mut [u8]);
    pub fn next_u32(&mut self) -> u32;
    pub fn next_u64(&mut self) -> u64;
    pub fn reseed(&mut self, seed: &[u8]);
}

impl Default for CsRng { fn default() -> Self { Self::new() } }
```

#### Scenario: 随机数不重复
- **WHEN** 连续生成 1000 个 32 字节随机数
- **THEN** 无重复

## 错误类型

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    InvalidKeyLength { expected: usize, actual: usize },
    InvalidCiphertext,
    SignatureInvalid,
    InvalidEcPoint,
    PointNotOnCurve,
    InvalidPublicKey,
    DecryptionFailed,
    AuthTagMismatch,
    RngFailed,
    BigIntOverflow,
    InvalidArgument,
    ConstantTimeCheckFailed,
    InvalidHexFormat,
}

impl CryptoError {
    pub fn is_security_critical(&self) -> bool {
        matches!(self,
            CryptoError::SignatureInvalid |
            CryptoError::AuthTagMismatch |
            CryptoError::PointNotOnCurve |
            CryptoError::ConstantTimeCheckFailed)
    }
}
```

## 恒定时间工具

```rust
/// 恒定时间比较两个字节切片（防时序侧信道）
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool;
/// 恒定时间复制（用于密钥清零）
pub fn ct_zeroize(buf: &mut [u8]);
```

## 性能目标（蓝图 §6.3）

| 指标 | 目标 | 验证方式 |
|------|------|---------|
| SM3 吞吐 | ≥ 100 MB/s | QEMU/实机基准（v0.31.x 软件实现） |
| SM4-ECB 吞吐 | ≥ 50 MB/s | QEMU/实机基准 |
| SM2 签名 | < 10ms | QEMU/实机基准 |
| SM2 验签 | < 15ms | QEMU/实机基准 |
| SM2 密钥生成 | < 20ms | QEMU/实机基准 |

**注**：v0.31.0 软件实现交付国标合规功能；性能基准需 QEMU/实机验证，no_std 环境用循环计数占位。性能不达标可在 Phase 3 通过硬件加速指令优化。

## no_std 合规

- 所有源文件 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 使用 `alloc::vec::Vec`（SM2 加密、CBC/GCM 输出需要）
- 不使用 `std::*`、`HashMap`
- CSRNG 在 no_std 环境下用固定种子（生产环境需注入硬件熵源，Phase 3）

## 内存预算声明（§5.6）

| 组件 | 预估内存 | 说明 |
|------|---------|------|
| U256 运算栈 | ≤ 512 B | 每次运算临时变量 |
| SM3 Hasher | 88 B | state(32) + buffer(64) + meta(8) |
| SM4 cipher | 128 B | round_keys[32] |
| SM4-CBC | 144 B | Sm4 + iv |
| SM4-GCM | 144 B | Sm4 + nonce |
| SM2 EcPoint | 65 B | x + y + is_infinity |
| SM2 Signer | 32 B | user_id (默认) |
| CSRNG | 40 B | state(32) + counter(8) |
| **总计（不含堆）** | **≤ 2 KB** | 全部栈分配，无堆增长 |

**OOM 策略**：密码学库本身不触发 OOM；调用方需确保堆有 ≥ 4 KB 用于 SM2 加密输出（Vec<u8>）。

## 偏差声明

1. **CSRNG 熵源**：v0.31.0 软件实现使用固定种子 + SM3 DRBG；生产环境需硬件 TRNG（Phase 3 接入）。CSRNG 默认种子在 `CsRng::new()` 中固定，**禁止用于生产密钥生成**，须用 `CsRng::from_seed(硬件熵)` 初始化。
2. **性能基准**：no_std 环境无法测量真实时间，基准测试用循环计数占位；性能验证延后到 QEMU/实机。
3. **NIST SP 800-22 完整测试**：v0.31.0 仅做基本统计测试（无重复、均匀分布），完整 NIST 随机性测试延后到安全审计。
4. **SM2 用户 ID**：默认 `"1234567812345678"`（国标默认），可通过 `Sm2Signer::with_user_id` 自定义。
5. **SM4 工作模式**：仅实现 ECB/CBC/GCM；CTR/OFB/CFB 非必需，延后到需要时实现。
