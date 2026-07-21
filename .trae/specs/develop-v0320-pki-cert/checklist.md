# Checklist — v0.32.0 PKI 证书基础

> 验证清单：所有检查项必须通过才能标记版本完成。ASN.1 DER 往返 + 证书链验证是硬性验收标准。
> v0.31.0 的 249 tests 必须继续通过（Surgical Changes 回归保护）。

## 一、目录结构校验（§2.4.1）

- [x] **C1 新模块位置**：`crates/security/crypto/src/pki/` 在现有 crypto crate 内，未新增根目录 crate
- [x] **C2 workspace members**：根 `Cargo.toml` members 无需改动（pki 是 crypto 内子模块）
- [x] **C3 跨 crate path 引用**：pki 模块在 crypto crate 内，无新跨 crate 引用
- [x] **C4 文档分类**：`docs/security/pki-design.md` 在 `docs/security/` 下，未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录下无新增 Rust crate 文件夹

## 二、pki 模块骨架校验

- [x] **C6 pki/mod.rs 创建**：模块入口 + `pub mod` 声明 + re-exports
- [x] **C7 PkiError 枚举**：13 变体（InvalidDerFormat/InvalidPemFormat/UnsupportedAlgorithm/SignatureInvalid/CertExpired{not_after}/CertNotYetValid{not_before}/CertRevoked{serial}/UntrustedRoot/ChainTooLong/NoIssuerFound/InvalidKeyUsage/CrlError(String)/Asn1Error(String)）
- [x] **C8 lib.rs 模块声明**：`pub mod pki;` 已添加
- [x] **C9 no_std 合规**：pki 模块 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`

## 三、ASN.1 DER 编解码校验

- [x] **C10 ASN.1 tag 常量**：SEQUENCE/SET/INTEGER/BIT_STRING/OCTET_STRING/OID/UTCTime/GeneralizedTime/BOOLEAN/NULL/CONTEXT_SPECIFIC
- [x] **C11 DerReader**：光标式读取，支持短格式/长格式长度
- [x] **C12 DerWriter**：write_element + 长度编码（短/长格式）
- [x] **C13 辅助读取方法**：read_integer/read_oid/read_bit_string/read_octet_string/read_sequence/read_set/read_utctime/read_generalized_time/read_boolean/read_null/read_context_explicit
- [x] **C14 辅助写入方法**：对应 write 方法
- [x] **C15 OID 编解码**：base-128 编码（如 SM2 OID 1.2.156.10197.1.301）
- [x] **C16 ASN.1 往返测试**：INTEGER/SEQUENCE/OID/UTCTime 编解码往返一致（15+ tests）
- [x] **C17 长格式长度边界**：长度 ≥ 128 时正确使用长格式编码

## 四、X509 证书结构校验

- [x] **C18 DistinguishedName**：cn/o/ou/c 字段 + RDN 编解码
- [x] **C19 SubjectPublicKey**：Sm2(Sm2PublicKey) / Rsa(Vec<u8>) 变体 + encode/decode
- [x] **C20 SignatureAlgorithm**：Sm2WithSm3 / EcdsaWithSha256 + OID 常量
- [x] **C21 Extension**：oid/critical/value 字段
- [x] **C22 KeyUsage**：u16 常量（DIGITAL_SIGNATURE/KEY_ENCIPHERMENT/DATA_ENCIPHERMENT/KEY_AGREEMENT/KEY_CERT_SIGN/CRL_SIGN）+ contains() 方法，未引入 bitflags crate
- [x] **C23 ExtKeyUsage**：ServerAuth/ClientAuth/CodeSigning/EmailProtection + OID
- [x] **C24 X509Certificate**：version/serial_number/subject/issuer/not_before/not_after/public_key/signature_algorithm/signature/extensions 字段
- [x] **C25 X509Certificate::encode_tbs**：返回 TBS DER 字节（用于签名/验签）
- [x] **C26 X509Certificate::encode/decode**：完整证书 DER 编解码
- [x] **C27 CertRequest**：subject/public_key/validity_days/key_usage/ext_key_usage
- [x] **C28 X509 单元测试**：DN 编解码 + Extension 解析 + KeyUsage 位操作（15+ tests）

## 五、DER/PEM 解析器校验

- [x] **C29 base64_decode**：自实现，标准字母表，忽略空白
- [x] **C30 base64_encode**：自实现，标准字母表
- [x] **C31 CertParser trait**：parse_der/parse_pem/to_der/to_pem
- [x] **C32 parse_der**：DER → X509Certificate
- [x] **C33 parse_pem**：提取 BEGIN/END + base64_decode + parse_der
- [x] **C34 to_der**：X509Certificate → DER Vec<u8>
- [x] **C35 to_pem**：base64_encode + 包裹 BEGIN/END CERTIFICATE
- [x] **C36 非法 PEM 失败**：缺失 header/footer 或 Base64 损坏返回 InvalidPemFormat
- [x] **C37 解析器单元测试**：DER 往返 + PEM 往返 + 非法 PEM + Base64 边界（12+ tests）

## 六、CRL 吊销列表校验

- [x] **C38 RevocationReason**：Unspecified/KeyCompromise/CACompromise/AffiliationChanged/Superseded/CessationOfOperation/CertificateHold
- [x] **C39 RevokedCert**：serial_number/revocation_date/reason
- [x] **C40 Crl 结构**：issuer/revoked/next_update
- [x] **C41 Crl::is_revoked**：按 serial_number 检查
- [x] **C42 Crl::add_revoked**：添加吊销记录
- [x] **C43 Crl::encode/decode**：ASN.1 DER 编解码
- [x] **C44 CRL 单元测试**：吊销检查 + add/encode/decode 往返（8+ tests）

## 七、证书签发校验

- [x] **C45 build_tbs**：构造 TBS Certificate DER（version+serial+sigAlg+issuer+validity+subject+subjectPKInfo+extensions）
- [x] **C46 sign_tbs**：用 Sm2Signer::new().sign(tbs, sk) 签名
- [x] **C47 build_certificate**：组装 TBS + 签名 → X509Certificate
- [x] **C48 build_self_signed**：自签名 CA 根证书（issuer = subject）
- [x] **C49 签发器单元测试**：自签名根证书 + TBS 编码 + 签名验证（10+ tests）

## 八、证书链验证校验

- [x] **C50 CertVerifier 结构**：trusted_roots/crl/max_chain_length
- [x] **C51 CertVerifier::new/add_trusted_root/set_crl**：管理方法
- [x] **C52 verify(cert, issuer, now)**：单证书验证（有效期+CRL+签名），接收外部 now
- [x] **C53 verify_chain(chain, now)**：链验证，接收外部 now
- [x] **C54 verify_signature**：用 Sm2Signer::new().verify(tbs, sig, pk)
- [x] **C55 is_trusted_root**：按 serial + subject 匹配
- [x] **C56 有效链通过**：leaf → root 验证 Ok(())
- [x] **C57 过期失败**：not_after < now → CertExpired
- [x] **C58 未到期失败**：not_before > now → CertNotYetValid
- [x] **C59 吊销失败**：serial 在 CRL → CertRevoked
- [x] **C60 签名篡改失败**：signature 被改 → SignatureInvalid
- [x] **C61 不可信根失败**：末端不在信任根 → UntrustedRoot
- [x] **C62 链过长失败**：chain.len() > 10 → ChainTooLong
- [x] **C63 验证器单元测试**：7 类失败 + 成功（15+ tests）

## 九、CA 管理校验

- [x] **C64 CaIssuer 结构**：ca_cert/ca_key/serial_counter/revoked
- [x] **C65 CaIssuer::new**：构造
- [x] **C66 issue_certificate**：serial_counter 自增 + build_certificate
- [x] **C67 revoke_certificate**：添加到 revoked 列表
- [x] **C68 generate_crl**：从 revoked 生成 Crl
- [x] **C69 serial 递增**：连续签发 serial_number 单调递增无重复
- [x] **C70 CA 单元测试**：签发+吊销+生成 CRL+验证（12+ tests）

## 十、lib.rs 模块完善校验

- [x] **C71 pki/mod.rs 子模块声明**：asn1/x509/parser/crl/builder/verify/ca 全部声明
- [x] **C72 pki/mod.rs re-exports**：pub use 导出公共类型
- [x] **C73 lib.rs re-exports**：pub use pki::*
- [x] **C74 lib.rs 文档注释**：架构图含 pki 模块 + 使用示例
- [x] **C75 lib.rs VERSION**：`pub const VERSION: &str = "0.32.0"`
- [x] **C76 cargo doc**：`cargo doc -p eneros-crypto --no-deps` 无警告

## 十一、集成测试校验

- [x] **C77 tests/pki_test.rs 创建**
- [x] **C78 自签名根证书**：生成 + 验证通过
- [x] **C79 CA 签发叶子证书**：verify_chain(leaf, root) 通过
- [x] **C80 吊销证书**：verify 返回 CertRevoked
- [x] **C81 过期证书**：verify 返回 CertExpired
- [x] **C82 未到期证书**：verify 返回 CertNotYetValid
- [x] **C83 签名篡改**：verify 返回 SignatureInvalid
- [x] **C84 不可信根**：verify_chain 返回 UntrustedRoot
- [x] **C85 DER/PEM 往返**：to_der→parse_der / to_pem→parse_pem 还原
- [x] **C86 证书链**：leaf → intermediate → root 验证通过
- [x] **C87 serial 递增**：连续签发 serial 单调递增
- [x] **C88 集成测试通过**：`cargo test --test pki_test -p eneros-crypto`

## 十二、构建校验（§2.4.2）

- [x] **C89 cargo metadata**：`cargo metadata --format-version 1 > /dev/null` 成功
- [x] **C90 cargo build eneros-crypto**：`cargo build -p eneros-crypto` 编译成功
- [x] **C91 cargo test eneros-crypto**：`cargo test -p eneros-crypto` 通过（v0.31.0 249 tests 回归 + v0.32.0 新增 80+ tests）
- [x] **C92 aarch64 交叉编译**：WSL2 `cargo build -p eneros-crypto --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] **C93 cargo fmt**：`cargo fmt --all -- --check` 通过
- [x] **C94 cargo clippy**：`cargo clippy -p eneros-crypto --all-targets -- -D warnings` 无 warning
- [x] **C95 cargo deny check**：licenses/bans/sources 通过；advisories 允许因 GitHub 网络不可达跳过
- [x] **C96 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部 PASS
- [x] **C97 eneros-ci**：`cargo run -p eneros-ci` fmt/clippy/test PASS（audit 允许跳过）

## 十三、文档与规范校验

- [x] **C98 pki-design.md**：v0.32.0 设计文档含 X.509 结构 + ASN.1 设计 + 验证流程 + 偏差声明 + 内存预算 + 解锁版本
- [x] **C99 docs/security/README.md**：索引更新含 pki-design.md
- [x] **C100 文档位置**：文档在 `docs/security/` 下，未放 `docs/` 根
- [x] **C101 无垃圾文件**：`git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪

## 十四、版本标识校验

- [x] **C102 根 Cargo.toml**：workspace.package.version = "0.32.0"
- [x] **C103 eneros-crypto Cargo.toml**：version.workspace = true（跟随根版本）
- [x] **C104 lib.rs VERSION**：`pub const VERSION: &str = "0.32.0"`
- [x] **C105 Makefile**：VERSION := 0.32.0
- [x] **C106 ci.yml**：Version: v0.32.0
- [x] **C107 gate.rs**：注释含 v0.32.0 说明

## 十五、设计原则合规

- [x] **C108 Karpathy Think Before Coding**：9 处偏差声明（time/hex/bitflags/sm2_compute_z/RSA/ASN.1/Extension/文档位置/测试位置）已在 spec.md 显式记录
- [x] **C109 Karpathy Simplicity First**：仅 SM2 签名；RSA/ECDSA 保留枚举返回错误；自实现 hex/Base64/DER；KeyUsage 用 u16 常量；零新增依赖
- [x] **C110 Karpathy Surgical Changes**：v0.31.0 的 sm2/sm3/sm4/bigint/rng/error/constant_time 源文件未修改；仅新增 pki/ + 改 lib.rs/Cargo.toml 版本号
- [x] **C111 Karpathy Goal-Driven Execution**：ASN.1 往返 + 证书链验证 + 7 类失败场景是验收标准
- [x] **C112 ADR 合规**：未引入自研重复组件；PKI 基于国密 SM2，遵循蓝图 §5.1 选型（国密自主可控）
- [x] **C113 偏差声明**：spec.md 记录 9 项偏差，对比蓝图代码的 no_std 不兼容点

## 十六、内存预算声明（§5.6）

- [x] **C114 内存预算声明**：设计文档声明 PKI 模块堆占用（单证书 ≤ 4 KB，证书链验证 ≤ 16 KB）
- [x] **C115 OOM 策略**：PKI 不触发 OOM；调用方需确保堆 ≥ 16 KB

## 十七、no_std 合规校验

- [x] **C116 PKI 模块 no_std**：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] **C117 无 std:: 使用**：不使用 std::time/std::collections::HashMap/std::net
- [x] **C118 时间外部传入**：verify/verify_chain 接收 now: u64，库内不获取系统时间
- [x] **C119 零外部依赖**：eneros-crypto Cargo.toml [dependencies] 为空

## 十八、v0.31.0 回归保护

- [x] **C120 v0.31.0 源文件未修改**：bigint/sm2/sm3/sm4/rng/error/constant_time 模块源文件无改动
- [x] **C121 v0.31.0 测试回归**：249 tests 全部继续通过（204 unit + 15 sm2_kat + 10 sm3_kat + 10 sm4_kat + 10 doctests）
- [x] **C122 v0.31.0 KAT 回归**：sm3_kat/sm4_kat/sm2_kat 测试文件未修改且通过

## 十九、后续版本解锁

- [x] **C123 解锁 Phase 2 v0.98.0**：mTLS 通信安全（X509 证书 + 证书链验证已就绪）
- [x] **C124 解锁 v0.169.0**：Agent DID（SM2 密钥对 + X509 证书已就绪）
