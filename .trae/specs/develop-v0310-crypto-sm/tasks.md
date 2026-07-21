# Tasks — v0.31.0 国密算法库

## Wave 1: 基础设施 + 无依赖模块（可并行）

- [x] Task 1: crypto crate 骨架 + 版本标识
  - [x] SubTask 1.1: 创建 `crates/security/crypto/Cargo.toml`（name="eneros-crypto", version="0.31.0", 无外部依赖）
  - [x] SubTask 1.2: 创建 `crates/security/crypto/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + VERSION="0.31.0" + 模块声明占位）
  - [x] SubTask 1.3: 创建 `crates/security/crypto/src/error.rs`（CryptoError 13 变体 + is_security_critical()）
  - [x] SubTask 1.4: 创建 `crates/security/crypto/src/constant_time.rs`（ct_eq + ct_zeroize，使用 `core::ptr::write_volatile`）
  - [x] SubTask 1.5: 修改根 `Cargo.toml`：members 添加 `"crates/security/crypto"` + workspace.package.version = "0.31.0"
  - [x] 验证: `cargo build -p eneros-crypto` 编译成功（8 tests + 2 doctests PASS，clippy 0 warnings）

- [x] Task 2: bigint.rs — U256 大整数运算
  - [x] SubTask 2.1: 定义 `U256` 结构体（`limbs: [u64; 4]` 小端序）+ ZERO/ONE 常量
  - [x] SubTask 2.2: 实现 `from_be_bytes` / `to_be_bytes` / `from_hex`（解析 64 字符十六进制）
  - [x] SubTask 2.3: 实现 `cmp` / `is_zero` / `bit` / `bit_len`
  - [x] SubTask 2.4: 实现 `add_mod` / `sub_mod`（模运算，使用 Barrett 或基础模减）
  - [x] SubTask 2.5: 实现 `mul_mod`（256×256→512 位中间结果，模 P/N）
  - [x] SubTask 2.6: 实现 `inv_mod`（扩展欧几里得算法）
  - [x] 验证: U256 单元测试（加减乘模逆、hex 解析、bit 操作）(20+ tests) — 64 tests PASS, fmt+clippy clean

- [x] Task 3: sm3/mod.rs — SM3 密码杂凑
  - [x] SubTask 3.1: 定义 IV / FF / GG / T / P0 / P1 常量与函数（按 GB/T 32905-2016）
  - [x] SubTask 3.2: 实现 `message_expand(block) -> ([u32; 68], [u32; 64])`（W 和 W'）
  - [x] SubTask 3.3: 实现 `compress(v: &mut [u32; 8], block: &[u8; 64])`（64 轮压缩函数 CF）
  - [x] SubTask 3.4: 定义 `Sm3Hasher` 结构体 + `new()` / `update()` / `finalize()`（含填充逻辑）
  - [x] SubTask 3.5: 实现 `Sm3::hash(data) -> [u8; 32]` 便捷函数
  - [x] 验证: SM3 国标 KAT（"abc" 和 64 字节消息）+ 流式 update 测试 (15+ tests) — 22 tests PASS, fmt+clippy clean

- [x] Task 4: sm4/mod.rs — SM4 分组密码
  - [x] SubTask 4.1: 定义 SBOX (256 字节) / FK / CK 常量（按 GB/T 32907-2016）
  - [x] SubTask 4.2: 实现 `tau` / `l_transform` / `t_transform` / `l_prime` / `t_prime` 变换函数
  - [x] SubTask 4.3: 实现 `key_expand(key: &[u8; 16]) -> [u32; 32]`（32 轮密钥扩展）
  - [x] SubTask 4.4: 实现 `encrypt_block(rk, block)` / `decrypt_block(rk, block)`（32 轮 Feistel + 反序变换 R）
  - [x] SubTask 4.5: 定义 `Sm4` 结构体 + `new()` / `encrypt_block()` / `decrypt_block()`
  - [x] 验证: SM4 国标 KAT（key=0123...3210, plaintext=0123...3210 → ciphertext=681edf...4246）+ 加解密一致性 (10+ tests) — 14 tests PASS, fmt+clippy clean

## Wave 2: SM4 工作模式 + CSRNG（依赖 Wave 1）

- [x] Task 5: sm4/cbc.rs — SM4-CBC 模式
  - [x] SubTask 5.1: 定义 `Sm4Cbc` 结构体（cipher: Sm4, iv: [u8; 16]）
  - [x] SubTask 5.2: 实现 `new(key, iv)` 构造函数
  - [x] SubTask 5.3: 实现 `encrypt(plaintext) -> Result<Vec<u8>, CryptoError>`（PKCS#7 填充 + CBC 链接）
  - [x] SubTask 5.4: 实现 `decrypt(ciphertext) -> Result<Vec<u8>, CryptoError>`（去填充 + 验证 PKCS#7）
  - [x] 验证: CBC 加解密一致性 + PKCS#7 填充边界测试 (10+ tests) — 12 tests PASS, fmt+clippy clean

- [x] Task 6: sm4/gcm.rs — SM4-GCM 认证加密
  - [x] SubTask 6.1: 定义 `Sm4Gcm` 结构体（cipher: Sm4, nonce: [u8; 12]）
  - [x] SubTask 6.2: 实现 GHASH 函数（基于 GF(2^128) 多项式乘法）
  - [x] SubTask 6.3: 实现 `encrypt(plaintext, aad) -> Result<(Vec<u8>, [u8; 16]), CryptoError>`（CTR 模式加密 + GHASH 认证）
  - [x] SubTask 6.4: 实现 `decrypt(ciphertext, aad, tag) -> Result<Vec<u8>, CryptoError>`（恒定时间 tag 比较）
  - [x] 验证: GCM 加解密一致性 + tag 篡改失败测试 + AAD 测试 (15+ tests) — 16 tests PASS, fmt+clippy clean

- [x] Task 7: rng/csrng.rs — CSRNG 安全随机数生成器
  - [x] SubTask 7.1: 定义 `CsRng` 结构体（state: [u8; 32], counter: u64）
  - [x] SubTask 7.2: 实现 `new()`（固定种子，标注禁止用于生产）+ `from_seed(seed)` 构造函数
  - [x] SubTask 7.3: 实现 `fill_bytes(buf)`（基于 SM3 DRBG，counter 作为输入）
  - [x] SubTask 7.4: 实现 `next_u32()` / `next_u64()` / `reseed(seed)`
  - [x] 验证: CSRNG 不重复测试（1000 个 32 字节）+ 确定性测试（同种子同输出）(10+ tests) — 12 tests PASS, fmt+clippy clean

## Wave 3: SM2 椭圆曲线运算（依赖 Wave 1 Task 2）

- [x] Task 8: sm2/mod.rs + sm2/keypair.rs — SM2 曲线与密钥对
  - [x] SubTask 8.1: 定义 `Sm2Curve` 常量（P/A/B/N/GX/GY，按 GB/T 32918.1-2017）
  - [x] SubTask 8.2: 定义 `EcPoint` 结构体（x: U256, y: U256, is_infinity: bool）+ `generator()`
  - [x] SubTask 8.3: 实现 `is_on_curve()`（验证 y^2 ≡ x^3 + ax + b mod p）
  - [x] SubTask 8.4: 实现 `add()`（点加：处理无穷远点、相同点、相反点）
  - [x] SubTask 8.5: 实现 `double()`（点倍：斜率 λ = (3x^2 + a) / (2y) mod p）
  - [x] SubTask 8.6: 实现 `scalar_mult(k)`（Montgomery ladder，恒定时间防侧信道）
  - [x] SubTask 8.7: 实现 `scalar_base_mult(k)`（k * G，使用 scalar_mult）
  - [x] SubTask 8.8: 实现 `to_bytes_uncompressed()`（04 || x || y）+ `from_bytes()`（解析 04/02/03 前缀）
  - [x] SubTask 8.9: 定义 `Sm2PrivateKey` / `Sm2PublicKey` / `Sm2KeyPair` + `generate()` / `from_private_key()` / `public_key_bytes()`
  - [x] SubTask 8.10: 实现 `Sm2PrivateKey::zeroize()`（用 ct_zeroize）
  - [x] 验证: 基点在曲线上 + 点加/倍/标量乘法 + 密钥对派生 + KAT 向量 (20+ tests) — 35 tests PASS（修正了 Gx/Gy 常量错误），fmt+clippy clean

- [x] Task 9: sm2/sign.rs — SM2 数字签名
  - [x] SubTask 9.1: 定义 `Sm2Signature`（r: [u8; 32], s: [u8; 32]）+ `Sm2Signer`（user_id: Vec<u8>）
  - [x] SubTask 9.2: 实现 `Sm2Signer::new()`（默认 user_id = "1234567812345678"）+ `with_user_id()`
  - [x] SubTask 9.3: 实现 Z 值计算：`compute_z(pk, user_id) -> [u8; 32]`（SM3(ENTL‖ID‖a‖b‖Gx‖Gy‖Px‖Py)）
  - [x] SubTask 9.4: 实现 `sign(msg, sk) -> Result<Sm2Signature, CryptoError>`（e=SM3(Z‖M)，循环生成 k 直到 r/s 有效）
  - [x] SubTask 9.5: 实现 `verify(msg, sig, pk) -> Result<bool, CryptoError>`（恒定时间比较）
  - [x] SubTask 9.6: 实现便捷函数 `sm2_sign` / `sm2_verify`（使用默认 Sm2Signer）
  - [x] 验证: 签名+验签端到端 + 篡改消息验签失败 + 篡改签名验签失败 + KAT 向量 (15+ tests) — 16 tests PASS, fmt+clippy clean

- [x] Task 10: sm2/encrypt.rs — SM2 公钥加密
  - [x] SubTask 10.1: 实现 `sm2_encrypt(plaintext, pk) -> Result<Vec<u8>, CryptoError>`
    - 生成随机 k，计算 C1 = k*G
    - 计算 (x2, y2) = k*P
    - 计算 C3 = SM3(x2 ‖ plaintext ‖ y2)
    - 计算 C2 = plaintext XOR KDF(x2‖y2, len(plaintext))
    - 输出 C1 ‖ C3 ‖ C2（国标顺序）
  - [x] SubTask 10.2: 实现 `sm2_decrypt(ciphertext, sk) -> Result<Vec<u8>, CryptoError>`
    - 解析 C1/C3/C2
    - 计算 (x2, y2) = sk*C1
    - 先恢复 M' = C2 XOR KDF(x2‖y2, len(C2))，再计算 C3' = SM3(x2 ‖ M' ‖ y2)，恒定时间比较 C3' == C3（修正了原描述的算法顺序）
  - [x] SubTask 10.3: 实现 KDF（基于 SM3 的密钥派生函数，国标 GB/T 32918.4-2017）
  - [x] 验证: 加解密一致性 + 篡改密文解密失败 + 空明文测试 + KAT 向量 (10+ tests) — 13 tests PASS, fmt+clippy clean

## Wave 4: lib.rs 完善 + 国标 KAT 集成测试（依赖 Wave 1-3）

- [x] Task 11: lib.rs 模块完善 + re-exports
  - [x] SubTask 11.1: 添加所有模块声明 `pub mod error / constant_time / bigint / sm3 / sm4 / sm2 / rng;`
  - [x] SubTask 11.2: 添加 `pub sm4::cbc; pub sm4::gcm;` `pub sm2::keypair / sign / encrypt;` `pub rng::csrng;`
  - [x] SubTask 11.3: 添加 `pub use` 导出所有公共类型
  - [x] SubTask 11.4: 添加 crate 文档注释（架构图 + 使用示例 + 偏差声明 + 国标引用）
  - [x] 验证: `cargo doc -p eneros-crypto --no-deps` 生成文档无警告 — 10 doctests PASS

- [x] Task 12: tests/sm3_kat.rs — SM3 国标测试向量
  - [x] SubTask 12.1: 添加测试向量 1（"abc" → 66c7f0f4...）
  - [x] SubTask 12.2: 添加测试向量 2（64 字节消息 → debe9ff9...）
  - [x] SubTask 12.3: 添加流式 update 测试（分块 vs 整块结果一致）
  - [x] 验证: `cargo test --test sm3_kat -p eneros-crypto` 全部通过 — 10 tests PASS, fmt+clippy clean

- [x] Task 13: tests/sm4_kat.rs — SM4 国标测试向量
  - [x] SubTask 13.1: 添加 SM4-ECB KAT（key=0123...3210, pt=0123...3210 → ct=681edf...4246）
  - [x] SubTask 13.2: 添加 SM4 加解密一致性测试（同密钥同明文加解密还原）
  - [x] SubTask 13.3: 添加 SM4-CBC KAT（如有国标向量）+ PKCS#7 填充测试
  - [x] SubTask 13.4: 添加 SM4-GCM 测试向量（NIST GCM 测试向量改编）
  - [x] 验证: `cargo test --test sm4_kat -p eneros-crypto` 全部通过 — 10 tests PASS, fmt+clippy clean

- [x] Task 14: tests/sm2_kat.rs — SM2 国标测试向量
  - [x] SubTask 14.1: 添加 SM2 签名 KAT（GB/T 32918.5-2017 签名向量）
  - [x] SubTask 14.2: 添加 SM2 加密 KAT（GB/T 32918.5-2017 加密向量）
  - [x] SubTask 14.3: 添加 SM2 密钥对派生测试（私钥 → 公钥 = sk*G）
  - [x] 验证: `cargo test --test sm2_kat -p eneros-crypto` 全部通过 — 15 tests PASS, fmt+clippy clean

## Wave 5: 文档 + 配置 + 版本标识（依赖 Wave 1-4）

- [x] Task 15: 文档创建
  - [x] SubTask 15.1: 创建 `docs/security/sm-crypto-design.md`（v0.31.0: 国标引用 + 算法说明 + 性能基准 + 内存预算 + OOM 策略 + 偏差声明）
  - [x] SubTask 15.2: 在 `docs/security/` 下创建 `README.md` 索引（首次创建此目录）
  - [x] 验证: 文档位于 `docs/security/`（§2.3.3 文档分类）

- [x] Task 16: 版本标识更新
  - [x] SubTask 16.1: 根 `Cargo.toml` workspace.package.version = "0.31.0"（Task 1 已改）
  - [x] SubTask 16.2: `Makefile` VERSION := 0.31.0 + 添加 crypto-build/crypto-test 目标
  - [x] SubTask 16.3: `.github/workflows/ci.yml` Version: v0.31.0 + 添加 eneros-crypto 到构建步骤
  - [x] SubTask 16.4: `ci/src/gate.rs` 注释添加 v0.31.0 说明 + eneros-crypto crate 配置
  - [x] 验证: 版本号一致性

## Wave 6: 构建与质量验证（依赖全部）

- [x] Task 17: 构建与质量验证
  - [x] SubTask 17.1: `cargo fmt --all -- --check` 通过 — exit 0
  - [x] SubTask 17.2: `cargo clippy -p eneros-crypto --all-targets -- -D warnings` 通过 — exit 0, 0 warnings
  - [x] SubTask 17.3: `cargo test -p eneros-crypto` 通过 — 249 tests PASS（204 unit + 15 sm2_kat + 10 sm3_kat + 10 sm4_kat + 10 doctests）
  - [x] SubTask 17.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过 — 全绿, 0 failed
  - [x] SubTask 17.5: `cargo run -p eneros-ci` — Overall: FAIL 仅因 audit 步骤（fmt/clippy/test 全 PASS）；audit 失败原因为 GitHub 网络不可达（`Recv failure: Connection was reset` fetch advisory-db 失败），与 v0.29.0/v0.30.x 相同环境问题，非代码问题，符合 spec 预期"audit 允许因 GitHub 网络不可达跳过"
  - [x] SubTask 17.6: aarch64 交叉编译通过 — WSL2 Ubuntu-22.04, `Finished dev profile in 1.49s`, exit 0
  - [x] SubTask 17.7: `cargo deny check licenses bans sources` 通过 — bans ok, licenses ok, sources ok（有 heapless/shlex duplicate warning，非 error）
  - [x] 验证: 所有代码相关检查项 PASS（audit 除外，环境问题）

# Task Dependencies

## 依赖链
- Task 1（骨架）无依赖
- Task 2（bigint）依赖 Task 1
- Task 3（sm3）依赖 Task 1
- Task 4（sm4）依赖 Task 1
- Task 5（sm4-cbc）依赖 Task 4
- Task 6（sm4-gcm）依赖 Task 4
- Task 7（csrng）依赖 Task 3（用 SM3）
- Task 8（sm2 曲线+密钥对）依赖 Task 2（用 U256）
- Task 9（sm2 签名）依赖 Task 3（用 SM3）+ Task 8（用 EcPoint）
- Task 10（sm2 加密）依赖 Task 3（用 SM3）+ Task 7（用 CSRNG）+ Task 8（用 EcPoint）
- Task 11（lib.rs 完善）依赖 Task 2-10
- Task 12（sm3 KAT）依赖 Task 3
- Task 13（sm4 KAT）依赖 Task 4-6
- Task 14（sm2 KAT）依赖 Task 8-10
- Task 15（文档）依赖 Task 11
- Task 16（版本标识）可与 Task 2-14 并行
- Task 17（验证）依赖全部完成

## 并行化建议

- **Wave 1（并行）**：Task 1（骨架）+ Task 2（bigint）+ Task 3（sm3）+ Task 4（sm4）
  - 注：Task 1 必须最先完成（其他 Task 依赖 Cargo.toml）
  - 实际执行：先 Task 1，然后 Task 2/3/4 并行
- **Wave 2（并行）**：Task 5（cbc）+ Task 6（gcm）+ Task 7（csrng）
- **Wave 3（串行）**：Task 8（曲线+密钥对）→ Task 9（签名）→ Task 10（加密）
  - 注：Task 9/10 都依赖 Task 8，但 Task 9 不依赖 Task 10，可部分并行
- **Wave 4（并行）**：Task 11（lib.rs）+ Task 12（sm3 KAT）+ Task 13（sm4 KAT）+ Task 14（sm2 KAT）
- **Wave 5（并行）**：Task 15（文档）+ Task 16（版本标识）
- **Wave 6**: Task 17（验证）

# 关键技术要点

## U256 大整数运算

```rust
// 小端序 [u64; 4]，limbs[0] 是最低位
// 模乘：256×256→512 位中间结果，需要 8 个 u64 临时存储
// 模逆：扩展欧几里得算法，迭代实现避免递归栈溢出
```

## SM3 实现要点

- IV: `[0x7380166F, 0x4914B2B9, 0x172442D7, 0xDA8A0600, 0xA96F30BC, 0x163138AA, 0xE38DEE4D, 0xB0FB0E4E]`
- T(j): j<16 → 0x79CC4519, j≥16 → 0x7A879D8A
- 消息扩展：W[0..68] + W'[0..64]
- 压缩函数：64 轮，每轮更新 A/B/C/D/E/F/G/H

## SM4 实现要点

- SBOX: 256 字节固定表
- FK: `[0xA3B1BAC6, 0x56AA3350, 0x677D9197, 0xB27022DC]`
- CK: 32 个常量
- 32 轮 Feistel，解密用逆序轮密钥
- 合成变换 T = L ∘ τ

## SM2 椭圆曲线参数

```
P  = FFFFFFFE FFFFFFFF FFFFFFFF FFFFFFFF FFFFFFFF 00000000 FFFFFFFF FFFFFFFF
A  = FFFFFFFE FFFFFFFF FFFFFFFF FFFFFFFF FFFFFFFF 00000000 FFFFFFFF FFFFFFFC
B  = 28E9FA9E 9D9F5E34 4D5A9E4B CF6509A7 F39789F5 15AB8F92 DDBCBD41 4D940E93
N  = FFFFFFFE FFFFFFFF FFFFFFFF FFFFFFFF 7203DF6B 21C6052B 53BBF409 39D54123
Gx = 32C4AE2C 1F198119 5F990446 6A39C994 8FE30BBF 2660BE17 15A45893 34C74C7
Gy = 0BC37362 32A89259 E5A0C1A1 B79F23FE 5D5BD11D BC2A9F7D 3B4C77F5 2C9E68BA
```

## SM2 签名算法

1. 计算 Z = SM3(ENTL ‖ ID ‖ a ‖ b ‖ Gx ‖ Gy ‖ Px ‖ Py)
2. 计算 e = SM3(Z ‖ M)
3. 生成随机 k ∈ [1, n-1]
4. 计算 (x1, y1) = k*G
5. 计算 r = (e + x1) mod n，若 r=0 或 r+k=n 则重试
6. 计算 s = ((1+d)^(-1) * (k - r*d)) mod n，若 s=0 则重试
7. 输出 (r, s)

## SM2 加密算法

1. 生成随机 k ∈ [1, n-1]
2. 计算 C1 = k*G（椭圆曲线点）
3. 计算 (x2, y2) = k*P
4. 计算 C3 = SM3(x2 ‖ M ‖ y2)
5. 计算 C2 = M XOR KDF(x2 ‖ y2, len(M))
6. 输出 C1 ‖ C3 ‖ C2

## KDF（密钥派生函数）

```
输入：Z（共享秘密）、klen（输出比特长度）
输出：K（密钥比特串）
1. ct = 0x00000001
2. for i = 1 to ⌈klen/v⌉:
   K_i = SM3(Z ‖ ct)
   ct = ct + 1
3. K = K_1 ‖ K_2 ‖ ... ‖ K_⌈klen/v⌉（截断到 klen 比特）
```

## 恒定时间实现

```rust
// 恒定时间比较
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// 恒定时间清零（防编译器优化）
pub fn ct_zeroize(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        unsafe { core::ptr::write_volatile(byte, 0); }
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
```

## no_std 合规

- 所有源文件 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 使用 `alloc::vec::Vec`（SM2 加密输出、CBC/GCM 输出）
- 不使用 `std::*`、`HashMap`、`std::time`
- CSRNG 在 no_std 下用固定种子（生产需硬件熵源）

## 测试策略

- **国标 KAT 是硬性验收标准**：SM3/SM4/SM2 必须通过国标测试向量
- **单元测试**：模块内测试（`#[cfg(test)] mod tests`），覆盖边界情况
- **集成测试**：`tests/` 目录下的 KAT 测试文件
- **恒定时间测试**：验证 ct_eq 在不同输入下执行时间一致（基本测试）
- **性能基准**：no_std 用循环计数占位，实机验证延后
