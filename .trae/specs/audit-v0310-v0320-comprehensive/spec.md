# 审计 v0.31.0 + v0.32.0 综合合规性检查 Spec

> **审计目标**：对 EnerOS v0.31.0（国密算法库）和 v0.32.0（PKI 证书基础）进行彻底的合规性、安全性、正确性检查，发现并修复真实问题。
> **审计原则**：Karpathy 四原则 — Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
> **审计范围**：`crates/security/crypto/` 全部源码 + 测试 + 文档 + 版本标识

## Why

v0.31.0 和 v0.32.0 是 EnerOS 安全基石，后续 v0.39.0（能力 Token）/ v0.78.0（消息签名）/ v0.113.0（Secure Boot）/ v0.115.0（mTLS）/ v0.169.0（Agent DID）全部依赖这两个版本。密码学代码的错误会向下传播到整个安全栈，必须在投入下游开发前彻底验证。

本次审计不是重构，而是**发现真实问题并修复**。遵循 Karpathy "Surgical Changes" 原则：只修复发现的问题，不重构、不优化、不"改进"无关代码。

## What Changes

### 审计维度（5 类）

1. **国标合规性审计**（v0.31.0）
   - SM3 (GB/T 32905-2016) KAT 向量覆盖
   - SM4 (GB/T 32907-2016) ECB/CBC/GCM KAT 向量覆盖
   - SM2 (GB/T 32918.1~5-2017) 签名/加密 KAT 向量覆盖
   - 国标曲线参数正确性

2. **RFC 合规性审计**（v0.32.0）
   - RFC 5280 X.509 证书结构
   - RFC 7468 PEM 格式
   - RFC 4648 Base64 编码
   - ASN.1 DER 编解码（ITU-T X.690）

3. **no_std 合规性审计**（两个版本）
   - `#![cfg_attr(not(test), no_std)]` 继承链
   - 禁止 `use std::*`
   - `alloc::*` / `core::*` 使用规范
   - 禁止 `panic!` / `todo!` / `unimplemented!`

4. **安全性审计**（两个版本）
   - 常数时间比较（侧信道防护）
   - 私钥清零（Drop trait）
   - RNG 熵源安全性
   - 签名验证逻辑（拒绝无效签名）
   - 证书有效期边界检查
   - CRL 吊销检查正确性

5. **代码质量与文档审计**（两个版本）
   - clippy 警告清零
   - cargo deny 合规
   - 测试覆盖完整性（边界条件 + 失败场景）
   - 文档与代码一致性
   - 版本标识一致性

### 修复策略

- **发现即记录**：每个问题记录到 tasks.md
- **Surgical Changes**：只修复发现的问题，不重构
- **回归保护**：修复后 v0.31.0 的 249 tests + v0.32.0 的 402 tests 必须全部继续通过
- **零新增依赖**：修复不得引入新的外部 crate

## Impact

- **Affected specs**: develop-v0310-crypto-sm, develop-v0320-pki-cert
- **Affected code**: `crates/security/crypto/src/` 全部模块
- **Affected tests**: `crates/security/crypto/tests/` + 各模块 `#[cfg(test)]`
- **Affected docs**: `docs/security/sm-crypto-design.md`, `docs/security/pki-design.md`
- **Affected version markers**: Cargo.toml / Makefile / ci.yml / gate.rs / lib.rs

## ADDED Requirements

### Requirement: 国标 KAT 向量覆盖完整性

v0.31.0 的 SM2/SM3/SM4 KAT 测试 SHALL 覆盖国标附录中的全部标准测试向量。缺失的 KAT 向量 MUST 补充。

#### Scenario: SM3 KAT 覆盖
- **WHEN** 检查 `sm3_kat` 测试模块
- **THEN** 至少包含 GB/T 32905-2016 附录 A 的 3 组标准向量（空串/abc/64字节）
- **AND** 每组向量的哈希值与国标附录一致

#### Scenario: SM4 KAT 覆盖
- **WHEN** 检查 `sm4_kat` 测试模块
- **THEN** ECB/CBC/GCM 各模式至少包含 1 组标准加解密向量
- **AND** 密钥/明文/密文与国标附录一致

#### Scenario: SM2 KAT 覆盖
- **WHEN** 检查 `sm2_kat` 测试模块
- **THEN** 至少包含签名生成/验证 + 加密/解密 KAT
- **AND** 曲线参数与 GB/T 32918.5-2017 推荐参数一致

### Requirement: no_std 合规性零违规

所有 `crates/security/crypto/` 下的 `.rs` 文件 SHALL 严格遵守 no_std 合规：
- 仅 `lib.rs` 顶层 `#![cfg_attr(not(test), no_std)]`
- 子模块通过继承获得 no_std，不重复添加
- 禁止 `use std::*`
- 禁止 `panic!` / `todo!` / `unimplemented!` / `unreachable!`（除 `core::panic!` 显式标注外）

#### Scenario: 无 std 违规
- **WHEN** 在 `src/` 下搜索 `use std::`
- **THEN** 返回 0 个匹配

#### Scenario: 无 panic 宏违规
- **WHEN** 在 `src/` 下搜索 `panic!` / `todo!` / `unimplemented!` / `unreachable!`
- **THEN** 返回 0 个匹配（测试代码除外）

### Requirement: 签名验证拒绝无效签名

SM2 签名验证 SHALL 拒绝以下无效签名：
- 签名长度 ≠ 64 字节
- r 或 s 不在 [1, n-1] 范围
- 签名对应错误的公钥
- 签名对应错误的消息

#### Scenario: 无效签名被拒绝
- **WHEN** 用错误的公钥验证签名
- **THEN** 返回 `Ok(false)` 或 `Err`

### Requirement: 证书有效期边界检查

X.509 证书验证 SHALL 正确处理边界：
- `now == not_before` → 有效（含等号）
- `now == not_after` → 有效（含等号，RFC 5280 §4.1.2.5）
- `now < not_before` → CertNotYetValid
- `now > not_after` → CertExpired

#### Scenario: 边界时间验证
- **WHEN** now == not_before
- **THEN** 验证通过（Ok(())）

- **WHEN** now == not_after
- **THEN** 验证通过（Ok(())）

### Requirement: CRL 吊销检查正确性

CRL 吊销检查 SHALL：
- 按 serial_number 精确匹配
- 空 CRL 不触发吊销
- serial_number 长度不同不匹配

#### Scenario: 空 CRL 不吊销
- **WHEN** CRL 为空（revoked = []）
- **AND** 验证任意证书
- **THEN** 不返回 CertRevoked

### Requirement: ASN.1 DER 编解码往返一致性

所有 ASN.1 类型 SHALL 满足 `decode(encode(x)) == x` 往返一致性：
- INTEGER / SEQUENCE / SET / OID / UTCTime / GeneralizedTime
- BIT STRING / OCTET STRING / BOOLEAN / NULL
- Context-specific [0]..[3] EXPLICIT

#### Scenario: DER 往返
- **WHEN** 对任意 DER 值执行 encode → decode
- **THEN** 结果与原值相等

### Requirement: 版本标识一致性

所有版本标识位置 SHALL 一致显示 v0.32.0：
- 根 `Cargo.toml`: `workspace.package.version = "0.32.0"`
- `crates/security/crypto/Cargo.toml`: `version.workspace = true`
- `lib.rs`: `pub const VERSION: &str = "0.32.0"`
- `Makefile`: `VERSION := 0.32.0`
- `.github/workflows/ci.yml`: `Version: v0.32.0`
- `ci/src/gate.rs`: 注释含 `v0.32.0`

#### Scenario: 版本标识一致
- **WHEN** 检查所有版本标识位置
- **THEN** 全部显示 0.32.0，无 0.31.0 残留（除历史注释外）

### Requirement: 回归保护

修复后 SHALL 满足：
- v0.31.0 原有 249 tests 全部继续通过
- v0.32.0 原有 402 tests 全部继续通过
- workspace 回归测试全绿
- aarch64 交叉编译通过
- cargo fmt / clippy / deny 通过

#### Scenario: 回归测试通过
- **WHEN** 执行 `cargo test -p eneros-crypto`
- **THEN** 所有测试通过，无回归

## 审计偏差声明

1. **审计不重构**：即使发现可优化的代码，若功能正确则不修改（Surgical Changes）
2. **KAT 向量来源**：国标附录 KAT 向量以标准文档为准，不引入第三方测试向量
3. **安全审计范围**：仅审计代码级安全，不审计协议级安全（如 SM2 的协议设计）
4. **性能不在审计范围**：性能优化延后到实机验证阶段
5. **依赖审计**：eneros-crypto 保持零外部依赖，不引入任何新 crate
