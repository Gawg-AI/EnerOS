# Tasks — v0.32.0 PKI 证书基础

> 模块依赖：asn1（基础）→ x509（用 asn1）→ parser/crl（用 x509）→ builder/verify/ca（用 x509+asn1+crl+sm2）
> 所有代码 no_std + alloc，零外部依赖，不修改 v0.31.0 现有模块。

## Wave 1: 基础设施 + ASN.1 + X509 结构

- [x] Task 1: pki 模块骨架 + PkiError + 版本号升级
  - [x] SubTask 1.1: 创建 `crates/security/crypto/src/pki/mod.rs`（模块入口 + `pub mod` 声明占位 + re-exports 占位）
  - [x] SubTask 1.2: 在 `pki/mod.rs` 定义 `PkiError` 枚举（13 变体：InvalidDerFormat/InvalidPemFormat/UnsupportedAlgorithm/SignatureInvalid/CertExpired{not_after}/CertNotYetValid{not_before}/CertRevoked{serial}/UntrustedRoot/ChainTooLong/NoIssuerFound/InvalidKeyUsage/CrlError(String)/Asn1Error(String)）
  - [x] SubTask 1.3: 修改 `crates/security/crypto/src/lib.rs` 添加 `pub mod pki;`
  - [x] SubTask 1.4: 修改 `crates/security/crypto/Cargo.toml` 版本注释（version.workspace 已自动跟随根 Cargo.toml）
  - [x] SubTask 1.5: 修改根 `Cargo.toml` workspace.package.version = "0.32.0"
  - [x] 验证: `cargo build -p eneros-crypto` 编译成功（pki/mod.rs 仅有 PkiError + 占位声明）— 9 tests PASS (4 pki_error + 5 lib doctests), fmt+clippy clean

- [x] Task 2: pki/asn1.rs — ASN.1 DER 编解码器
  - [x] SubTask 2.1: 定义 ASN.1 tag 常量（SEQUENCE=0x30 / SET=0x31 / INTEGER=0x02 / BIT_STRING=0x03 / OCTET_STRING=0x04 / OID=0x06 / UTCTime=0x17 / GENERALIZED_TIME=0x18 / BOOLEAN=0x01 / NULL=0x05 / CONTEXT_SPECIFIC=0xA0..0xA3）
  - [x] SubTask 2.2: 实现 `DerReader`（光标式读取）：read_element() → (tag, content)，支持长度短格式/长格式
  - [x] SubTask 2.3: 实现 `DerWriter`：write_element(tag, content)，长度编码（短/长格式）
  - [x] SubTask 2.4: 实现辅助方法：read_integer() / read_oid() / read_bit_string() / read_octet_string() / read_sequence() / read_set() / read_utctime() / read_generalized_time() / read_boolean() / read_null() / read_context_explicit(n)
  - [x] SubTask 2.5: 实现对应 write 方法
  - [x] SubTask 2.6: 实现 OID 编解码（如 1.2.156.10197.1.301 = SM2 算法 OID）
  - [x] 验证: ASN.1 单元测试（INTEGER/SEQUENCE/OID/UTCTime 编解码往返 + 长格式长度边界）— 36 tests PASS, fmt+clippy clean

- [x] Task 3: pki/x509.rs — X509 证书结构 + KeyUsage + Extension + CertRequest
  - [x] SubTask 3.1: 定义 `DistinguishedName`（cn: String / o: Option<String> / ou: Option<String> / c: Option<String>）+ to_rdn_sequence()/from_rdn_sequence()（ASN.1 RDN 编解码）
  - [x] SubTask 3.2: 定义 `SubjectPublicKey` 枚举（Sm2(Sm2PublicKey) / Rsa(Vec<u8>)）+ encode()/decode()
  - [x] SubTask 3.3: 定义 `SignatureAlgorithm` 枚举（Sm2WithSm3 / EcdsaWithSha256）+ oid() 常量
  - [x] SubTask 3.4: 定义 `Extension`（oid: Vec<u8> / critical: bool / value: Vec<u8>）+ KeyUsage 扩展 / ExtKeyUsage 扩展的解析
  - [x] SubTask 3.5: 定义 `KeyUsage` u16 常量（DIGITAL_SIGNATURE=0x01 / KEY_ENCIPHERMENT=0x02 / DATA_ENCIPHERMENT=0x04 / KEY_AGREEMENT=0x08 / KEY_CERT_SIGN=0x10 / CRL_SIGN=0x20）+ contains() 方法
  - [x] SubTask 3.6: 定义 `ExtKeyUsage` 枚举（ServerAuth/ClientAuth/CodeSigning/EmailProtection）+ OID 常量
  - [x] SubTask 3.7: 定义 `X509Certificate`（version/serial_number/subject/issuer/not_before/not_after/public_key/signature_algorithm/signature/extensions）+ encode_tbs()/encode()/decode()
  - [x] SubTask 3.8: 定义 `CertRequest`（subject/public_key/validity_days/key_usage/ext_key_usage）
  - [x] 验证: X509 结构单元测试（DN 编解码 + Extension 解析 + KeyUsage 位操作）— 23 tests PASS, fmt+clippy clean

## Wave 2: 解析器 + CRL（依赖 Wave 1）

- [x] Task 4: pki/parser.rs — DER/PEM 解析器（含 Base64）
  - [x] SubTask 4.1: 自实现 `base64_decode(input) -> Result<Vec<u8>, PkiError>`（标准 Base64 字母表，忽略空白）
  - [x] SubTask 4.2: 自实现 `base64_encode(input) -> String`
  - [x] SubTask 4.3: 定义 `CertParser` trait（parse_der/parse_pem/to_der/to_pem）— 跳过 trait，直接用自由函数（Simplicity First）
  - [x] SubTask 4.4: 实现 `parse_der(der: &[u8]) -> Result<X509Certificate, PkiError>`（用 asn1 + x509.decode()）
  - [x] SubTask 4.5: 实现 `parse_pem(pem: &str) -> Result<X509Certificate, PkiError>`（提取 BEGIN/END 间内容 + base64_decode + parse_der）
  - [x] SubTask 4.6: 实现 `to_der(cert: &X509Certificate) -> Result<Vec<u8>, PkiError>`（cert.encode()）
  - [x] SubTask 4.7: 实现 `to_pem(cert: &X509Certificate) -> Result<String, PkiError>`（base64_encode + 包裹 BEGIN/END CERTIFICATE）
  - [x] 验证: 解析器单元测试（DER 往返 + PEM 往返 + 非法 PEM 失败 + Base64 边界）— 17 tests PASS, fmt+clippy clean

- [x] Task 5: pki/crl.rs — CRL 吊销列表
  - [x] SubTask 5.1: 定义 `RevocationReason` 枚举（Unspecified/KeyCompromise/CACompromise/AffiliationChanged/Superseded/CessationOfOperation/CertificateHold）
  - [x] SubTask 5.2: 定义 `RevokedCert`（serial_number: Vec<u8> / revocation_date: u64 / reason: RevocationReason）
  - [x] SubTask 5.3: 定义 `Crl`（issuer: DistinguishedName / revoked: Vec<RevokedCert> / next_update: u64）
  - [x] SubTask 5.4: 实现 `Crl::is_revoked(&self, serial: &[u8]) -> bool`
  - [x] SubTask 5.5: 实现 `Crl::add_revoked(&mut self, cert: RevokedCert)`
  - [x] SubTask 5.6: 实现 `Crl::encode()/decode()`（ASN.1 DER 编解码，可选：简化版不强制完整 RFC 5280 CRL）
  - [x] 验证: CRL 单元测试（吊销检查 + add/encode/decode 往返）— 15 tests PASS, fmt+clippy clean

## Wave 3: 签发 + 验证 + CA（依赖 Wave 1-2）

- [x] Task 6: pki/builder.rs — 证书签发器
  - [x] SubTask 6.1: 实现 `build_tbs(req: &CertRequest, issuer: &DistinguishedName, serial: &[u8], now: u64) -> Result<Vec<u8>, PkiError>`（构造 TBS Certificate DER：version + serial + sigAlg + issuer + validity + subject + subjectPKInfo + extensions）
  - [x] SubTask 6.2: 实现 `sign_tbs(tbs: &[u8], sk: &Sm2PrivateKey) -> Result<Sm2Signature, PkiError>`（用 Sm2Signer::new().sign(tbs, sk, pk, rng)）
  - [x] SubTask 6.3: 实现 `build_certificate(req, issuer_dn, issuer_sk, issuer_pk, serial, now, rng) -> Result<X509Certificate, PkiError>`（组装 TBS + 签名 → X509Certificate）
  - [x] SubTask 6.4: 实现 `build_self_signed(req, sk, pk, now, rng) -> Result<X509Certificate, PkiError>`（自签名 CA 根证书，issuer = subject）
  - [x] 验证: 签发器单元测试（自签名根证书 + TBS 编码 + 签名验证）— 13 tests PASS, fmt+clippy clean

- [x] Task 7: pki/verify.rs — 证书链验证
  - [x] SubTask 7.1: 定义 `CertVerifier`（trusted_roots: Vec<X509Certificate> / crl: Option<Crl> / max_chain_length: usize）
  - [x] SubTask 7.2: 实现 `CertVerifier::new(roots)` / `add_trusted_root()` / `set_crl()`
  - [x] SubTask 7.3: 实现 `verify(&self, cert: &X509Certificate, issuer: &X509Certificate, now: u64) -> Result<(), PkiError>`（单证书验证：有效期 + CRL + 签名）
  - [x] SubTask 7.4: 实现 `verify_chain(&self, chain: &[X509Certificate], now: u64) -> Result<(), PkiError>`（链验证：从叶子到根，逐级验签，末端检查信任根）
  - [x] SubTask 7.5: 实现 `verify_signature(cert, issuer)`（用 Sm2Signer::new().verify(tbs, sig, pk)，tbs = cert.encode_tbs()）
  - [x] SubTask 7.6: 实现 `is_trusted_root(&self, cert) -> bool`（按 serial + subject.cn 匹配）
  - [x] 验证: 验证器单元测试（有效链通过 + 过期失败 + 未到期失败 + 吊销失败 + 签名篡改失败 + 不可信根失败 + 链过长失败）— 19 tests PASS, fmt+clippy clean

- [x] Task 8: pki/ca.rs — CA 管理（CaIssuer）
  - [x] SubTask 8.1: 定义 `CaIssuer`（ca_cert: X509Certificate / ca_key: Sm2PrivateKey / ca_pk: Sm2PublicKey / serial_counter: u64 / revoked: Vec<RevokedCert> / rng: CsRng）
  - [x] SubTask 8.2: 实现 `CaIssuer::new(ca_cert, ca_key) -> Result<Self, PkiError>`（从 ca_cert 提取 SM2 公钥，RSA 返回 UnsupportedAlgorithm）
  - [x] SubTask 8.3: 实现 `issue_certificate(&mut self, req: &CertRequest, now: u64) -> Result<X509Certificate, PkiError>`（serial_counter 自增 + build_certificate）
  - [x] SubTask 8.4: 实现 `revoke_certificate(&mut self, serial: &[u8], reason: RevocationReason, now: u64) -> Result<(), PkiError>`
  - [x] SubTask 8.5: 实现 `generate_crl(&self, next_update: u64) -> Result<Crl, PkiError>`
  - [x] 验证: CA 单元测试（签发多证书 serial 递增 + 吊销 + 生成 CRL + 签发后验证通过）— 13 tests PASS, fmt+clippy clean

## Wave 4: lib.rs 完善 + 集成测试 + 文档 + 版本标识

- [x] Task 9: lib.rs 模块完善 + re-exports
  - [x] SubTask 9.1: `pki/mod.rs` 添加所有子模块声明 `pub mod asn1/x509/parser/crl/builder/verify/ca;`
  - [x] SubTask 9.2: `pki/mod.rs` 添加 `pub use` 导出公共类型（X509Certificate/CaIssuer/CertVerifier/Crl/PkiError/CertRequest/KeyUsage 等）
  - [x] SubTask 9.3: `lib.rs` 添加 PKI re-exports（asn1/builder/ca/crl/parser/verify/x509 + PkiError）
  - [x] SubTask 9.4: `lib.rs` 更新 crate 文档注释（架构图添加 pki 模块 + PKI 示例 + 偏差声明 #7-#10）
  - [x] SubTask 9.5: `lib.rs` 更新 `pub const VERSION: &str = "0.32.0"`
  - [x] 验证: `cargo doc` 仅有预存警告（非本次引入）；345 tests PASS, fmt+clippy clean

- [x] Task 10: tests/pki_test.rs — PKI 集成测试
  - [x] SubTask 10.1: 测试自签名根证书生成 + 验证通过
  - [x] SubTask 10.2: 测试 CA 签发叶子证书 → verify_chain(leaf, root) 通过
  - [x] SubTask 10.3: 测试 CA 签发 → 吊销 → verify 返回 CertRevoked
  - [x] SubTask 10.4: 测试过期证书 → verify 返回 CertExpired
  - [x] SubTask 10.5: 测试未到期证书 → verify 返回 CertNotYetValid
  - [x] SubTask 10.6: 测试签名篡改 → verify 返回 SignatureInvalid
  - [x] SubTask 10.7: 测试不可信根 → verify_chain 返回 UntrustedRoot
  - [x] SubTask 10.8: 测试 DER/PEM 往返（to_der → parse_der / to_pem → parse_pem）
  - [x] SubTask 10.9: 测试证书链（leaf → intermediate → root）验证通过
  - [x] SubTask 10.10: 测试 serial_number 单调递增
  - [x] 验证: `cargo test --test pki_test -p eneros-crypto` 全部通过 — 11 tests PASS, fmt+clippy clean

- [x] Task 11: 文档 + 版本标识
  - [x] SubTask 11.1: 创建 `docs/security/pki-design.md`（v0.32.0: X.509 结构 + ASN.1 设计 + 证书链验证流程 + 偏差声明 + 内存预算 + 解锁版本）
  - [x] SubTask 11.2: 更新 `docs/security/README.md` 索引添加 pki-design.md
  - [x] SubTask 11.3: `Makefile` VERSION := 0.32.0 + 更新 crypto-build 目标注释
  - [x] SubTask 11.4: `.github/workflows/ci.yml` Version: v0.32.0 + 更新 crypto 构建步骤描述
  - [x] SubTask 11.5: `ci/src/gate.rs` 注释更新 v0.31.0 → v0.32.0（2 处）
  - [x] 验证: `cargo build -p eneros-ci` 通过；文档位于 `docs/security/`；版本号一致性

## Wave 5: 构建与质量验证（依赖全部）

- [x] Task 12: 构建与质量验证
  - [x] SubTask 12.1: `cargo fmt --all -- --check` 通过 — FMT PASS
  - [x] SubTask 12.2: `cargo clippy -p eneros-crypto --all-targets -- -D warnings` 通过 — 0 warnings
  - [x] SubTask 12.3: `cargo test -p eneros-crypto` 通过 — 402 tests PASS（345 unit + 11 pki_test + 15 sm2_kat + 10 sm3_kat + 10 sm4_kat + 11 doctests）
  - [x] SubTask 12.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过 — 全绿, 0 failed
  - [x] SubTask 12.5: `cargo run -p eneros-ci` — Overall: PASS（fmt 386ms / clippy 1800ms / audit 7023ms / test 10393ms 全 PASS）
  - [x] SubTask 12.6: aarch64 交叉编译通过 — WSL2 Ubuntu-22.04, Finished dev profile in 1.61s, exit 0
  - [x] SubTask 12.7: `cargo deny check licenses bans sources` 通过 — bans ok, licenses ok, sources ok
  - [x] 验证: 所有代码相关检查项 PASS

# Task Dependencies

## 依赖链
- Task 1（骨架）无依赖
- Task 2（asn1）依赖 Task 1
- Task 3（x509）依赖 Task 2（用 asn1）
- Task 4（parser）依赖 Task 2 + Task 3
- Task 5（crl）依赖 Task 3（用 DistinguishedName）
- Task 6（builder）依赖 Task 3 + Task 2（用 x509 + asn1 编码 TBS）
- Task 7（verify）依赖 Task 3 + Task 5 + sm2（验签）
- Task 8（ca）依赖 Task 6 + Task 5（用 builder + crl）
- Task 9（lib.rs）依赖 Task 2-8
- Task 10（集成测试）依赖 Task 9
- Task 11（文档+版本标识）可与 Task 6-10 并行
- Task 12（验证）依赖全部完成

## 并行化建议
- **Wave 1（串行）**：Task 1 → Task 2 → Task 3（asn1 是基础，x509 依赖它）
- **Wave 2（并行）**：Task 4（parser）+ Task 5（crl）
- **Wave 3（部分并行）**：Task 6（builder）→ Task 7（verify）+ Task 8（ca 可与 verify 并行）
- **Wave 4（并行）**：Task 9（lib.rs）+ Task 10（测试）+ Task 11（文档+版本标识）
- **Wave 5**: Task 12（验证）

# 关键技术要点

## ASN.1 DER 编解码要点
- 长度编码：短格式（≤127，1 字节）/ 长格式（≥128，首字节 0x80|len_bytes + 后续 len_bytes 字节大端长度）
- INTEGER：大端补码，正整数首位 < 0x80 否则前导 0x00
- OID：首字节 = 40*X + Y，后续用 base-128 编码（每字节最高位 1 表示继续，末字节最高位 0）
- UTCTime：YYMMDDHHMMSSZ（2 位年份，<50 → 20XX，≥50 → 19XX）
- GeneralizedTime：YYYYMMDDHHMMSSZ（4 位年份）
- SM2 OID：1.2.156.10197.1.301 → 编码 2A 81 1C CF 55 01 82 1D

## X.509 证书 DER 结构（RFC 5280 简化）
```
Certificate ::= SEQUENCE {
    tbsCertificate       TBSCertificate,
    signatureAlgorithm   AlgorithmIdentifier,
    signatureValue       BIT STRING
}
TBSCertificate ::= SEQUENCE {
    version         [0] EXPLICIT Version DEFAULT v1,
    serialNumber         CertificateSerialNumber,  -- INTEGER
    signature            AlgorithmIdentifier,       -- SEQUENCE{OID, params?}
    issuer               Name,                       -- RDNSequence
    validity             Validity,                   -- SEQUENCE{notBefore, notAfter}
    subject              Name,
    subjectPublicKeyInfo SubjectPublicKeyInfo,       -- SEQUENCE{alg, BIT STRING}
    extensions      [3] EXPLICIT Extensions OPTIONAL
}
```

## SM2 证书签名对接（v0.31.0）
- TBS = cert.encode_tbs() 返回的 DER 字节
- 签名：`Sm2Signer::new().sign(tbs, sk)` → Sm2Signature(r, s)
- 验签：`Sm2Signer::new().verify(tbs, &sig, pk)` → bool
- Z 值用标准默认 user_id "1234567812345678"（Sm2Signer::new() 默认）
- 注意：国标 SM2 签名对消息 M 计算 e=SM3(Z‖M)，证书场景 M=TBS

## 时间处理（no_std）
- not_before / not_after / now 均为 u64 Unix 时间戳（秒）
- CertVerifier::verify(cert, now) / verify_chain(chain, now) 接收外部 now
- CaIssuer::issue_certificate(req, now) 用 now 计算 not_before = now, not_after = now + validity_days*86400

## no_std 合规
- 所有 PKI 源文件 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 使用 `alloc::vec::Vec` / `alloc::string::String` / `alloc::format`
- 不使用 `std::*` / `std::time` / `std::collections::HashMap`
- 零外部依赖（保持 eneros-crypto 零依赖）

## 测试策略
- **ASN.1 DER 往返是基础验收**：编码后解码得到相同结构
- **证书链验证是核心验收**：签发→验证通过，各类失败场景返回正确错误
- **集成测试**：tests/pki_test.rs 端到端测试（自签根 → CA 签发 → 验证 → 吊销 → 失败）
- **回归测试**：v0.31.0 的 249 tests 必须继续通过（Surgical Changes 保证）
