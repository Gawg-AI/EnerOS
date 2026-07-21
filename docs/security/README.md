# EnerOS 安全子系统文档

> 安全相关设计与实现文档

## 文档索引

| 文档 | 版本 | 说明 |
|------|------|------|
| [sm-crypto-design.md](sm-crypto-design.md) | v0.31.0 | 国密算法库设计（SM2/SM3/SM4 + CSRNG） |
| [pki-design.md](pki-design.md) | v0.32.0 | PKI 证书基础设计（X.509 证书解析/签发/验证/CRL） |

## 子系统概述

`crates/security/` 子系统包含 EnerOS 的密码学和安全相关组件：

- **v0.31.0**: `eneros-crypto` — 国密算法库（SM2/SM3/SM4 + CSRNG）
- **v0.32.0** (计划): PKI 证书管理（X.509 + SM2 签名）
- **v0.39.0** (计划): 能力 Token（SM2 签名 + SM4 加密）

## 国标合规

所有密码学算法严格遵循国家标准：
- GB/T 32905-2016 (SM3)
- GB/T 32907-2016 (SM4)
- GB/T 32918.1~5-2017 (SM2)

国标 KAT (Known Answer Test) 测试向量作为硬性验收标准。
