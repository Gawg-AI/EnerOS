# 跨域认证设计

## 概述

EnerOS DID 跨域认证允许多个独立部署的 EnerOS 域之间
建立信任关系，实现 Agent 身份的跨域互认。

## 信任模型

```
Domain A (Trusted)          Domain B (Requesting)
┌─────────────────┐         ┌─────────────────┐
│ Root DID        │         │ Agent DID       │
│   └─ Issuer DID │◄────────│   └─ Cert Chain │
│       └─ Cert   │  trust  │                 │
└─────────────────┘  anchor └─────────────────┘
```

## 认证流程

1. 请求方 Agent 出示 DID 文档 + 证书链
2. 验证方检查域名是否在 `trusted_domains` 列表
3. 验证证书链有效期 (`valid_from` ≤ now ≤ `valid_until`)
4. 验证 SM2 签名完整性
5. 认证通过，建立跨域会话

## API

```rust
// 跨域认证
verifier.cross_domain_auth(doc, domain, trusted_domains)

// 证书链验证
verifier.verify_certificate_chain(chain, current_time)
```

## 约束与偏差

1. 当前为静态信任域列表，生产需动态信任协商
2. 证书链仅验证有效期和结构，完整链签名验证后续实现
3. 未实现 DID 撤销检查 (CRL/OCSP)
4. 跨域通信加密由 mTLS 层负责，本模块仅做身份认证
