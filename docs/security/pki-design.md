# PKI 证书基础设计文档 (v0.32.0)

> **版本**：v0.32.0
> **crate**：eneros-crypto
> **模块**：pki
> **状态**：已实现
> **最后更新**：2026-07-14

## 一、概述

EnerOS PKI 模块提供 X.509 证书的解析、签发、验证与吊销管理功能，基于 v0.31.0 国密 SM2/SM3 算法栈，支持国密证书体系。

### 设计目标
- X.509 v3 证书解析与签发
- SM2-with-SM3 数字签名证书
- 证书链验证（含有效期、CRL 吊销、签名验证）
- CA 管理器（序列号自增、吊销、CRL 生成）
- DER/PEM 编码格式支持

### 依赖
- v0.31.0 国密算法库（SM2/SM3/SM4 + CSRNG）
- 零外部依赖（自研 ASN.1 DER 编解码 + Base64）

## 二、模块架构

```text
pki/
├── mod.rs       — 模块入口 + PkiError 错误类型
├── asn1.rs      — ASN.1 DER 编解码器（DerReader/DerWriter）
├── x509.rs      — X.509 证书结构 + KeyUsage + Extension
├── parser.rs    — DER/PEM 解析器 + Base64 编解码
├── crl.rs       — CRL 吊销列表
├── builder.rs   — 证书签发器（build_tbs/sign_tbs/build_certificate/build_self_signed）
├── verify.rs    — 证书链验证器（CertVerifier）
└── ca.rs        — CA 管理器（CaIssuer）
```

## 三、ASN.1 DER 编解码

### 设计
- DerReader：光标式读取，支持短/长格式长度编码
- DerWriter：构建式写入，自动处理长度编码
- 支持 13 种 tag：BOOLEAN/INTEGER/BIT_STRING/OCTET_STRING/NULL/OID/UTF8_STRING/SEQUENCE/SET/UTC_TIME/GENERALIZED_TIME/CONTEXT_0/CONTEXT_3
- OID 编解码：base-128 编码，如 SM2 OID 1.2.156.10197.1.301

### 时间转换
- no_std 无系统时钟，使用 Howard Hinnant civil_from_days 算法
- UTCTime（YYMMDDHHMMSSZ，2 位年份）↔ Unix 时间戳
- GeneralizedTime（YYYYMMDDHHMMSSZ，4 位年份）↔ Unix 时间戳

## 四、X.509 证书结构

### 字段
- version: u8（0=v1, 1=v2, 2=v3）
- serial_number: Vec<u8>（大端 INTEGER）
- signature_algorithm: SignatureAlgorithm（Sm2WithSm3 / EcdsaWithSha256）
- issuer / subject: DistinguishedName（CN/O/OU/C）
- not_before / not_after: u64（Unix 时间戳）
- public_key: SubjectPublicKey（Sm2 / Rsa 枚举）
- extensions: Vec<Extension>（v3 扩展）
- signature: Vec<u8>（SM2 r‖s，64 字节）

### KeyUsage
- u16 位常量（DIGITAL_SIGNATURE=0x8000, KEY_CERT_SIGN=0x0400, CRL_SIGN=0x0200 等）
- RFC 5280 高位优先编码

### Extension
- oid: Vec<u8>（DER 编码的 OID）
- critical: bool
- value: Vec<u8>（OCTET STRING 内容）

## 五、证书签发流程

1. 构造 CertRequest（subject + public_key + validity_days + key_usage）
2. build_tbs()：构造 TBSCertificate DER（version + serial + sigAlg + issuer + validity + subject + SPKI + extensions）
3. sign_tbs()：用 Sm2Signer::new().sign(tbs, sk, pk, rng) 签名
4. X509Certificate::new()：组装完整证书

### 自签名根证书
- issuer = subject
- 自动添加 KeyUsage: KEY_CERT_SIGN | CRL_SIGN
- serial = [1]

## 六、证书链验证

### CertVerifier
- trusted_roots: Vec<X509Certificate>
- crl: Option<Crl>
- max_chain_length: usize（默认 10）

### 验证流程
1. 有效期：not_before <= now <= not_after
2. CRL 吊销：crl.is_revoked(serial)
3. 签名验证：Sm2Signer::new().verify(tbs, sig, issuer_pk)
4. 链验证：逐级验签 + 末端信任根检查

## 七、CA 管理

### CaIssuer
- ca_cert: X509Certificate（自签名根证书）
- ca_key: Sm2PrivateKey（CA 私钥）
- ca_pk: Sm2PublicKey（CA 公钥，SM2 签名需要）
- serial_counter: u64（从 1 开始自增）
- revoked: Vec<RevokedCert>
- rng: CsRng（内部 RNG）

### 方法
- issue_certificate(req, now)：签发证书，serial 自增
- revoke_certificate(serial, reason, now)：吊销证书
- generate_crl(next_update)：生成 CRL

## 八、偏差声明

1. **时间外部传入**：no_std 无系统时钟，verify/verify_chain 接收外部 now: u64（Unix 时间戳）。
2. **RSA 仅保留枚举**：SubjectPublicKey 含 Rsa 变体但编解码返回 UnsupportedAlgorithm（国密优先）。
3. **CRL 简化版**：不含签名部分（签名验证由 verify 模块负责）；RevocationReason 不写入 DER。
4. **ASN.1 自研**：自研 ASN.1 DER 编解码，不引入外部 crate（保持零依赖）。
5. **KeyUsage u16**：用 u16 手动实现位常量，不引入 bitflags crate。
6. **Base64 自实现**：自实现 Base64 编解码，不引入外部 crate。
7. **时间编码**：UTCTime/GeneralizedTime 选择基于年份（<2050 用 UTCTime），证书有效期通常 <2050。

## 九、内存预算

| 组件 | 预算 | 说明 |
|------|------|------|
| ASN.1 DerReader | ~64 字节 | 光标 + 引用 |
| ASN.1 DerWriter | 动态 | Vec 增长，典型证书 ~500 字节 |
| X509Certificate | ~1 KB | 含 extensions Vec |
| CertVerifier | 动态 | trusted_roots Vec |
| CaIssuer | ~2 KB | 含 ca_cert + revoked Vec |
| 总计（单证书操作） | ≤ 4 KB | 不含堆分配的 Vec 内容 |

### OOM 策略
- 证书解析失败返回 PkiError，不触发 OOM
- 大证书（>10 KB）由调用方预检

## 十、国标/标准引用

- GB/T 32918 信息安全技术 SM2 椭圆曲线公钥密码算法
- GB/T 35275 信息安全技术 SM2 密码算法加密签名消息语法规范
- RFC 5280 Internet X.509 Public Key Infrastructure Certificate and CRL Profile
- RFC 4648 The Base16, Base32, and Base64 Data Encodings

## 十一、测试覆盖

| 模块 | 单元测试 | 集成测试 |
|------|---------|---------|
| asn1.rs | 36 | — |
| x509.rs | 23 | — |
| parser.rs | 17 | — |
| crl.rs | 15 | — |
| builder.rs | 13 | — |
| verify.rs | 19 | — |
| ca.rs | 13 | — |
| **合计** | **136** | **11** |

## 十二、解锁版本

- v0.39.0：能力 Token（SM2 签名 + PKI 已就绪）
- v0.78.0：消息签名（SM2 签名 + PKI 已就绪）
- v0.113.0：Secure Boot（SM2/SM3/SM4 + PKI 已就绪）
- v0.115.0：mTLS 通信安全（PKI 证书链验证已就绪）
- v0.169.0：Agent DID（PKI 身份标识已就绪）
