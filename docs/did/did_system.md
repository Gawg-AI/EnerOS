# Agent DID 去中心化身份体系设计

## 概述

EnerOS Agent DID 系统为每个 Agent 提供唯一的去中心化身份标识，
遵循 W3C DID Core 1.0 规范，集成国密 SM2 签名算法。

## DID 格式

```
did:eneros:<network>:<agent_id>
```

- `method`: 固定为 `eneros`
- `network`: 网络标识 (`main` / `test`)
- `agent_id`: Agent 唯一数字标识

## 核心组件

| 组件 | 职责 |
|------|------|
| `types` | DID、DidDocument、CertificateChain 等核心类型 |
| `identifier` | DID 生成与解析 |
| `document` | BTreeMap 文档存储 (CRUD) |
| `resolver` | DID → DidDocument 解析 |
| `verifier` | SM2 签名验证 + 证书链 + 跨域认证 |

## DID Document 结构

```rust
DidDocument {
    id: Did,                    // 关联 DID
    public_key: Vec<u8>,        // SM2 公钥 (65B 未压缩)
    auth_methods: Vec<AuthMethod>,
    service_endpoints: Vec<ServiceEndpoint>,
    created: u64,               // Unix 时间戳
    updated: u64,
}
```

## 签名验证流程

1. 从 DidDocument 提取 SM2 公钥
2. 调用 `eneros_crypto::sm2_verify(msg, sig, pk)`
3. 返回验证结果

## 证书链

```
Root DID → Issuer DID → Subject DID
```

验证内容：有效期、结构完整性、签名长度。

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖（仅 workspace path deps）
- 依赖 `eneros-crypto` 提供 SM2 能力

## 偏差声明

1. 仅实现 `did:eneros` 方法，未注册 W3C DID Method Registry
2. 内存存储，生产需对接分布式账本
3. 证书链为简化版，完整密码学链验证后续实现
4. DID Resolution 仅本地，远程解析后续添加
