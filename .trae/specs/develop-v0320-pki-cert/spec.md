# v0.32.0 PKI 证书基础 Spec

> 基于 v0.31.0 国密算法库，实现 X.509 证书解析/签发、CA 证书链验证、CRL 吊销列表，为 Phase 2 mTLS 与 Agent DID 提供信任链基础。
>
> 蓝图依据：`蓝图/phase1.md` §v0.32.0（行 4744-5099）
> 工作区规则：`e:\eneros\.trae\rules\记忆.md` §2/§4.3/§5.5
> 前置版本：v0.31.0（国密 SM2/SM3/SM4）✅ 已完成

---

## Why

Edge Box 之间的通信需要双向认证，Agent 身份需要证书验证，PKI 是信任链的基础。v0.31.0 国密算法库已就绪，但仅提供原语，缺少证书层抽象。本版本在 eneros-crypto 内新增 `pki/` 子模块，基于 SM2/SM3 实现 X.509 证书的签发、解析、验证与吊销，解锁 Phase 2 v0.98.0 mTLS 与 v0.169.0 Agent DID。

## What Changes

- **新增** `crates/security/crypto/src/pki/` 子模块（8 个文件）：
  - `mod.rs` — PKI 模块入口 + `PkiError` 错误枚举 + re-exports
  - `asn1.rs` — 最小化 ASN.1 DER 编解码器（X.509 子集，零外部依赖）
  - `x509.rs` — `X509Certificate` / `DistinguishedName` / `SubjectPublicKey` / `SignatureAlgorithm` / `Extension` / `KeyUsage` / `ExtKeyUsage` / `CertRequest`
  - `parser.rs` — `CertParser` trait + DER/PEM 解析（含自实现 Base64）
  - `builder.rs` — 证书签发器（构建 TBS + 调用 Sm2Signer 签名）
  - `verify.rs` — `CertVerifier` 证书链验证（有效期/CRL/签名/信任根）
  - `crl.rs` — `Crl` / `RevokedCert` / `RevocationReason`
  - `ca.rs` — `CaIssuer` CA 管理（签发/吊销/生成 CRL）
- **新增** `crates/security/crypto/tests/pki_test.rs` — PKI 集成测试
- **新增** `docs/security/pki-design.md` — PKI 设计文档
- **修改** `crates/security/crypto/src/lib.rs` — 添加 `pub mod pki;` + re-exports
- **修改** `crates/security/crypto/Cargo.toml` — 版本 0.31.0 → 0.32.0
- **修改** 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` — 版本标识更新至 0.32.0
- **不修改** v0.31.0 的 `bigint` / `sm2` / `sm3` / `sm4` / `rng` / `error` / `constant_time` 模块（Surgical Changes）

## Impact

- **Affected specs**：v0.31.0（依赖其 SM2/SM3 原语）；解锁 v0.98.0（mTLS）、v0.169.0（Agent DID）
- **Affected code**：`crates/security/crypto/`（新增 pki 子模块 + lib.rs/Cargo.toml）；根配置文件版本号
- **内存预算**：PKI 模块为调用方库，不常驻内存；单证书解析堆占用 ≤ 4 KB（Vec<u8> for serial/signature/extensions），证书链验证堆占用 ≤ 16 KB（链长度 ≤ 10）。不触发 OOM；调用方需确保堆 ≥ 16 KB
- **SBOM**：零新增外部依赖（保持 eneros-crypto 零依赖）

---

## ADDED Requirements

### Requirement: ASN.1 DER 编解码

系统 SHALL 提供最小化 ASN.1 DER 编解码器，支持 X.509 证书所需的 ASN.1 tag 子集，不依赖外部 crate。

#### Scenario: DER 解码往返
- **WHEN** 对一个 X509Certificate 调用 `to_der()` 编码为 DER 字节
- **AND** 对该 DER 字节调用 `parse_der()` 解码
- **THEN** 解码得到的证书字段值与原证书一致（subject/issuer/serial/public_key/signature）

#### Scenario: 支持的 ASN.1 tag
- **GIVEN** DER 编解码器
- **THEN** 支持 SEQUENCE / SET / INTEGER / BIT STRING / OCTET STRING / OBJECT IDENTIFIER / UTCTime / GeneralizedTime / BOOLEAN / NULL / CONTEXT-SPECIFIC([0]..[3])

### Requirement: X.509 证书结构

系统 SHALL 提供 `X509Certificate` 结构，包含国标 X.509 所需字段，公钥支持 SM2。

#### Scenario: 证书字段
- **GIVEN** X509Certificate 结构
- **THEN** 包含 version / serial_number / subject / issuer / not_before / not_after / public_key / signature_algorithm / signature / extensions 字段

#### Scenario: 公钥类型
- **GIVEN** SubjectPublicKey 枚举
- **THEN** 支持 `Sm2(Sm2PublicKey)` 变体
- **AND** 保留 `Rsa(Vec<u8>)` 变体（解析时返回 `UnsupportedAlgorithm`，不实现 RSA）

### Requirement: DER/PEM 解析器

系统 SHALL 提供 `CertParser` trait，支持 DER 与 PEM 格式的证书编解码。

#### Scenario: PEM 解析
- **WHEN** 调用 `parse_pem(pem_str)` 传入合法 PEM（含 `-----BEGIN CERTIFICATE-----` / Base64 / `-----END CERTIFICATE-----`）
- **THEN** 返回 Ok(X509Certificate)
- **WHEN** 传入非法 PEM（缺失 header/footer 或 Base64 损坏）
- **THEN** 返回 Err(InvalidPemFormat)

#### Scenario: DER 编码
- **WHEN** 调用 `to_der(cert)`
- **THEN** 返回 Ok(Vec<u8>)，字节为合法 DER 编码

### Requirement: 证书签发

系统 SHALL 提供 `CaIssuer`，使用 SM2 私钥签发证书，签名算法为 Sm2WithSm3。

#### Scenario: CA 签发证书
- **GIVEN** CA 证书 + CA 私钥 + CertRequest（subject/public_key/validity_days/key_usage）
- **WHEN** 调用 `ca_issuer.issue_certificate(&req)`
- **THEN** 返回 Ok(X509Certificate)，证书的 issuer = CA subject，signature 由 CA 私钥对 TBS 数据签名
- **AND** 签名使用 v0.31.0 的 Sm2Signer（默认 user_id "1234567812345678"），msg = TBS DER 字节

#### Scenario: 序列号自增
- **GIVEN** CaIssuer 的 serial_counter
- **WHEN** 连续签发多张证书
- **THEN** 每张证书的 serial_number 单调递增，无重复

### Requirement: 证书链验证

系统 SHALL 提供 `CertVerifier`，验证证书链的有效性（有效期 / CRL 吊销 / 签名 / 信任根）。

#### Scenario: 有效证书链
- **GIVEN** 证书链 [leaf, intermediate, root]，root 在信任根列表
- **AND** 所有证书在有效期内、未吊销、签名有效
- **WHEN** 调用 `verify_chain(&chain, now)`，now 为当前 Unix 时间戳
- **THEN** 返回 Ok(())

#### Scenario: 过期证书
- **GIVEN** 证书的 not_after < now
- **WHEN** 调用 verify
- **THEN** 返回 Err(CertExpired { not_after })

#### Scenario: 未到期证书
- **GIVEN** 证书的 not_before > now
- **WHEN** 调用 verify
- **THEN** 返回 Err(CertNotYetValid { not_before })

#### Scenario: 吊销证书
- **GIVEN** 证书的 serial_number 在 CRL 的 revoked 列表中
- **WHEN** 调用 verify（已 set_crl）
- **THEN** 返回 Err(CertRevoked { serial })

#### Scenario: 签名无效
- **GIVEN** 证书的 signature 被篡改
- **WHEN** 调用 verify
- **THEN** 返回 Err(SignatureInvalid)

#### Scenario: 不可信根
- **GIVEN** 链末端证书不在信任根列表
- **WHEN** 调用 verify_chain
- **THEN** 返回 Err(UntrustedRoot)

#### Scenario: 链过长
- **GIVEN** 证书链长度 > 10（max_chain_length）
- **WHEN** 调用 verify_chain
- **THEN** 返回 Err(ChainTooLong)

### Requirement: CRL 吊销列表

系统 SHALL 提供 `Crl` 结构，记录已吊销的证书序列号。

#### Scenario: 吊销检查
- **GIVEN** Crl 包含某 serial_number
- **WHEN** 调用 `crl.is_revoked(&serial)`
- **THEN** 返回 true

#### Scenario: CA 吊销证书
- **GIVEN** CaIssuer
- **WHEN** 调用 `revoke_certificate(serial, reason)`
- **THEN** 内部吊销列表添加该 serial
- **AND** 调用 `generate_crl()` 返回包含该 serial 的 Crl

### Requirement: 时间由调用方传入

系统 SHALL NOT 内部获取系统时间（no_std 无时钟），证书有效期验证接收外部传入的 Unix 时间戳。

#### Scenario: verify 接收 now 参数
- **GIVEN** CertVerifier
- **WHEN** 调用 `verify_chain(&chain, now)` 或 `verify(&cert, now)`
- **THEN** now: u64 为 Unix 时间戳，由调用方提供

### Requirement: KeyUsage 位标志

系统 SHALL 用 u16 位常量实现 KeyUsage，不引入 bitflags crate。

#### Scenario: KeyUsage 标志
- **GIVEN** KeyUsage 常量
- **THEN** 支持 DIGITAL_SIGNATURE(0x01) / KEY_ENCIPHERMENT(0x02) / DATA_ENCIPHERMENT(0x04) / KEY_AGREEMENT(0x08) / KEY_CERT_SIGN(0x10) / CRL_SIGN(0x20)
- **AND** 支持位或组合（如 `KEY_CERT_SIGN | CRL_SIGN`）

---

## MODIFIED Requirements

### Requirement: eneros-crypto crate 版本

crate 版本从 0.31.0 升级至 0.32.0，新增 `pki` 公共模块。VERSION 常量更新为 "0.32.0"。

---

## 偏差声明（对比蓝图代码）

> 蓝图 §4.5 的示例代码含 no_std 不兼容的调用，本 spec 做如下偏差处理（遵循 Karpathy "Think Before Coding" 原则，显式声明）：

1. **时间获取**：蓝图代码 `crate::time::now_unix()` 不存在（eneros-crypto 无 time 模块，no_std 无系统时钟）。偏差：`CertVerifier::verify` / `verify_chain` 接收 `now: u64` 参数，由调用方传入当前 Unix 时间戳。
2. **hex 编码**：蓝图代码 `hex::encode(&cert.serial_number)` 依赖 hex crate。偏差：自实现 `hex_encode(bytes) -> String`（no_std，用 alloc::string::String），不引入 hex crate。
3. **bitflags 宏**：蓝图代码 `bitflags! { pub struct KeyUsage: u16 { ... } }` 依赖 bitflags crate。偏差：用 u16 常量 + 方法手动实现 KeyUsage（`pub const DIGITAL_SIGNATURE: u16 = 0x01;` 等），不引入 bitflags crate。
4. **SM2 签名对接**：蓝图代码 `sm2_compute_z(&issuer.subject, pk)` / `sm2_verify_hash(&e, &sig, pk)` 是示意函数（非标准 Z 值计算）。偏差：使用 v0.31.0 的 `Sm2Signer::verify(msg, sig, pk)`，msg = TBS DER 字节，Z 值用标准默认 user_id（"1234567812345678"），与国标 GB/T 32918.2-2017 一致。
5. **RSA/ECDSA 支持**：蓝图 `SubjectPublicKey::Rsa` / `SignatureAlgorithm::EcdsaWithSha256` 保留枚举变体，但解析/验证时返回 `UnsupportedAlgorithm`，不实现 RSA/ECDSA（遵循 Simplicity First，国密自主可控）。
6. **ASN.1 DER 编解码**：蓝图未提供 ASN.1 实现，标注风险 8.1"ASN.1 DER 编解码复杂"。偏差：自研最小化 DER 编解码器（`pki/asn1.rs`），仅支持 X.509 子集，不引入外部 ASN.1 库（保持零依赖）。
7. **证书扩展**：蓝图 `extensions: Vec<Extension>` 未定义 Extension 结构。偏差：定义 `Extension { oid: Vec<u8>, critical: bool, value: Vec<u8> }`，仅 KeyUsage / ExtKeyUsage 做结构化解析，其他扩展保留 raw DER value（透明传递）。
8. **文档位置**：蓝图写 `docs/pki-design.md`。偏差：按工作区规则 §2.3.3，归入 `docs/security/pki-design.md`（与 v0.31.0 sm-crypto-design.md 同方向）。
9. **测试位置**：蓝图写 `crypto/tests/pki_test.rs`。实际路径 `crates/security/crypto/tests/pki_test.rs`。

---

## no_std 合规

- 所有 PKI 模块代码 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 使用 `alloc::vec::Vec` / `alloc::string::String` / `alloc::format`
- 不使用 `std::*` / `std::time` / `std::collections::HashMap`
- 时间由调用方传入，不在库内获取系统时间
- 零新增外部依赖（保持 eneros-crypto 零依赖）

---

## Karpathy 四原则应用

| 原则 | 应用 |
|------|------|
| **Think Before Coding** | 识别蓝图代码 6 处 no_std 不兼容点（time/hex/bitflags/sm2_compute_z/RSA/ASN.1），显式声明偏差 |
| **Simplicity First** | 仅实现 SM2 签名；RSA/ECDSA 保留枚举返回错误；自实现 hex/Base64/DER 不引入依赖；KeyUsage 用 u16 常量 |
| **Surgical Changes** | 仅新增 pki/ 子模块 + 改 lib.rs 版本号；不修改 v0.31.0 的 sm2/sm3/sm4 等现有模块 |
| **Goal-Driven Execution** | 验收标准：DER 往返 + PEM 解析 + CA 签发→验证通过 + 吊销→失败 + 过期→失败 + 签名篡改→失败 + 证书链验证 |

---

## 验收标准

- [ ] ASN.1 DER 编解码往返一致
- [ ] PEM 解析合法/非法用例正确
- [ ] CA 签发证书 → CertVerifier 验证通过
- [ ] 吊销证书 → 验证返回 CertRevoked
- [ ] 过期证书 → 验证返回 CertExpired
- [ ] 未到期证书 → 验证返回 CertNotYetValid
- [ ] 签名篡改 → 验证返回 SignatureInvalid
- [ ] 不可信根 → 验证返回 UntrustedRoot
- [ ] 证书链验证（leaf → intermediate → root）通过
- [ ] cargo test -p eneros-crypto 全部通过（含 v0.31.0 回归 + v0.32.0 新增）
- [ ] cargo clippy 无 warning
- [ ] cargo fmt --check 通过
- [ ] aarch64 交叉编译通过
- [ ] cargo deny check licenses bans sources 通过
