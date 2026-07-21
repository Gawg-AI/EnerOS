# EnerOS 国密算法库设计文档 (v0.31.0)

> **版本**：v0.31.0
> **crate**：`eneros-crypto` (`crates/security/crypto/`)
> **国标引用**：GB/T 32905-2016, GB/T 32907-2016, GB/T 32918.1~5-2017
> **最后更新**：2026-07-13

## 1. 概述

EnerOS 国密算法库提供纯 Rust 实现的国密算法套件，包括：
- **SM3**：密码杂凑算法（GB/T 32905-2016），256-bit 输出
- **SM4**：分组密码算法（GB/T 32907-2016），128-bit 密钥/分组
- **SM2**：椭圆曲线公钥密码算法（GB/T 32918.1~5-2017），签名+加密
- **CSRNG**：基于 SM3 的密码学安全随机数生成器（NIST SP 800-90A 风格）

### 1.1 设计目标
- **纯 Rust 实现**：无 C FFI、无硬件加速（Phase 3 考虑硬件加速）
- **no_std 合规**：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- **国标合规**：通过国标 KAT (Known Answer Test) 测试向量
- **抗侧信道**：恒定时间比较 (ct_eq)、恒定时间清零 (ct_zeroize)、Montgomery ladder

### 1.2 子系统归属
本 crate 归属 `crates/security/` 子系统，与 `kernel/hal/runtime/drivers/ai/protocols/agents` 平级。
后续 v0.32.0 PKI、v0.39.0 能力 Token 将归入此子系统。

## 2. 架构

```text
┌──────────────────────────────────────────────────────┐
│  eneros-crypto (v0.31.0)                             │
│  ┌──────────┐  ┌──────────┐  ┌────────────────────┐  │
│  │ bigint   │  │ sm3      │  │ sm4                │  │
│  │ (U256)   │  │ (Hash)   │  │ ┌────┐ ┌─────┐     │  │
│  └────┬─────┘  └────┬─────┘  │ │cbc │ │gcm  │     │  │
│       │             │        │ └────┘ └─────┘     │  │
│       │       ┌─────┴─────┐  └────────────────────┘  │
│       │       │ rng       │                          │
│       │       │ (CsRng)   │                          │
│       │       └─────┬─────┘                          │
│       └──────┐      │                                │
│       ┌──────▼──────▼──────┐                         │
│       │ sm2                │                         │
│       │ ┌──────┐ ┌──────┐  │                         │
│       │ │sign  │ │encrypt│  │                        │
│       │ └──────┘ └──────┘  │                         │
│       │ │keypair│          │                         │
│       │ └──────┘           │                         │
│       └────────────────────┘                         │
│  ┌─────────────┐  ┌──────────────┐                   │
│  │error        │  │constant_time │                   │
│  │(CryptoError)│  │(ct_eq/zero)  │                   │
│  └─────────────┘  └──────────────┘                   │
└──────────────────────────────────────────────────────┘
```

### 2.1 模块依赖
- `bigint` (U256): 无依赖，SM2 椭圆曲线运算基础
- `sm3`: 无依赖，独立实现
- `sm4`: 无依赖，独立实现
  - `sm4::cbc`: 依赖 sm4
  - `sm4::gcm`: 依赖 sm4
- `rng::csrng`: 依赖 sm3 (Hash DRBG)
- `sm2::keypair`: 依赖 bigint
- `sm2::sign`: 依赖 sm3 + sm2::keypair + rng
- `sm2::encrypt`: 依赖 sm3 + sm2::keypair + rng

### 2.2 错误处理
`CryptoError` 13 变体，关键方法：
- `is_security_critical()`: 判断是否为安全关键错误（签名/标签/填充验证失败）

## 3. 算法实现

### 3.1 SM3 (GB/T 32905-2016)
- IV: 8 个 32-bit 字
- 消息扩展: W[0..68] + W'[0..64]
- 压缩函数: 64 轮 CF
- 输出: 256-bit (32 字节)
- 性能目标: ≥ 100 MB/s (实机验证延后)

### 3.2 SM4 (GB/T 32907-2016)
- S-Box: 256 字节查找表
- FK/CK: 系统参数/固定参数
- 32 轮 Feistel 结构
- T = L ∘ τ (合成变换)
- 工作模式: ECB (基础块) + CBC (PKCS#7) + GCM (AEAD)

### 3.3 SM2 (GB/T 32918.1~5-2017)
- 曲线: sm2p256v1 (256-bit 素域)
- 基点 G 阶: n
- 点运算: 仿射坐标，点加/点倍/标量乘法 (Montgomery ladder)
- 签名: Z 值计算 → e=SM3(Z‖M) → (r,s)
- 加密: C1‖C3‖C2 (国标顺序)，KDF 密钥派生

### 3.4 CSRNG
- 基于 SM3 的 Hash DRBG
- 状态: V (256-bit) + counter (64-bit)
- 输出: V = SM3(V ‖ counter)
- **警告**: 固定种子，仅测试用，生产需硬件 TRNG

## 4. 安全特性

### 4.1 恒定时间操作
- `ct_eq(a, b)`: XOR 累积比较，防时序侧信道
- `ct_zeroize(buf)`: `write_volatile` + `compiler_fence`，防编译器优化

### 4.2 Montgomery ladder
SM2 标量乘法使用 Montgomery ladder，恒定时间防侧信道攻击。

### 4.3 密钥清零
- `Sm2PrivateKey::Drop` 自动清零
- `CsRng::Drop` 自动清零

## 5. 内存预算

| 组件 | 内存占用 | 说明 |
|------|---------|------|
| Sm3Hasher | 88 bytes | state(32) + buffer(64) + ...  |
| Sm4 | 256 bytes | 32 × u32 轮密钥 |
| Sm4Cbc | 272 bytes | Sm4 + iv(16) |
| Sm4Gcm | 272 bytes | Sm4 + h(16) |
| CsRng | 40 bytes | V(32) + counter(8) |
| EcPoint | 65 bytes | x(32) + y(32) + is_infinity(1) |
| Sm2PrivateKey | 32 bytes | d (U256) |
| Sm2PublicKey | 65 bytes | EcPoint |
| Sm2Signer | 24 bytes | user_id (Vec<u8>) |
| **总计** | **≤ 2 KB** | 不含堆分配 |

### 5.1 OOM 策略
- 密码学操作不分配大块堆内存
- `Vec<u8>` 输出大小与输入成正比
- 极端 OOM 情况下返回 `CryptoError::InternalError`

## 6. 偏差声明

1. **CSRNG 熵源**：固定种子（仅测试），生产需硬件 TRNG
2. **性能基准**：no_std 无系统时钟，循环计数占位，实机验证延后
3. **NIST 测试**：未集成 NIST CAVP，仅使用国标 KAT
4. **SM4 工作模式**：仅 ECB/CBC/GCM，CTR/CFB/OFB 后续按需
5. **SM2 用户 ID**：默认 `"1234567812345678"`，可配置
6. **SM2 压缩点格式**：v0.31.0 仅支持未压缩 (04)，压缩 (02/03) 后续按需

## 7. 测试覆盖

| 模块 | 单元测试 | KAT 集成测试 | 总计 |
|------|---------|-------------|------|
| bigint | 64 | - | 64 |
| sm3 | 22 | 10 | 32 |
| sm4 | 14 | 10 | 24 |
| sm4::cbc | 12 | - | 12 |
| sm4::gcm | 16 | - | 16 |
| rng::csrng | 12 | - | 12 |
| sm2::keypair | 35 | - | 35 |
| sm2::sign | 16 | - | 16 |
| sm2::encrypt | 13 | 15 | 28 |
| constant_time | 6 | - | 6 |
| error | 2 | - | 2 |
| **总计** | **212** | **35** | **247+** |

国标 KAT 硬性验收标准全部通过：
- SM3 "abc" → 66c7f0f4...
- SM4 key=0123...3210 → ct=681edf...
- SM2 签名/加密端到端

## 8. 后续解锁

v0.31.0 完成后解锁：
- v0.32.0 PKI 证书管理（X.509 + SM2 签名）
- v0.39.0 能力 Token（SM2 签名 + SM4 加密）
- v0.57.0 降级规则联动（安全降级使用国密）
- Phase 2 mTLS（双向认证使用 SM2/SM3/SM4）

## 9. 参考

- GB/T 32905-2016 信息安全技术 SM3 密码杂凑算法
- GB/T 32907-2016 信息安全技术 SM4 分组密码算法
- GB/T 32918.1-2017 SM2 第1部分：总则
- GB/T 32918.2-2017 SM2 第2部分：数字签名算法
- GB/T 32918.4-2016 SM2 第4部分：公钥加密算法
- GB/T 32918.5-2017 SM2 第5部分：参数定义
- NIST SP 800-90A Hash DRBG
- GM/T 0003.5-2012 SM2 椭圆曲线公钥密码算法（行业参考）
