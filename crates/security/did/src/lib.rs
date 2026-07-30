//! EnerOS Agent DID 去中心化身份体系 (v0.169.0).
//!
//! 实现 W3C DID Core 兼容的去中心化身份标识系统，集成 SM2 国密签名
//! 和跨域认证能力，为 EnerOS Agent 提供唯一、可验证的身份。
//!
//! # 架构
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │  eneros-did (v0.169.0)                              │
//! │  ┌───────────┐  ┌───────────┐  ┌────────────────┐  │
//! │  │ types     │  │ identifier│  │ document       │  │
//! │  │ (Did,     │  │ (generate,│  │ (DocumentStore)│  │
//! │  │  Doc,     │  │  parse)   │  │                │  │
//! │  │  Chain)   │  │           │  │                │  │
//! │  └─────┬─────┘  └─────┬─────┘  └───────┬────────┘  │
//! │        │              │                 │           │
//! │        │        ┌─────▼─────────────────▼────┐     │
//! │        │        │ resolver (DidResolver)      │     │
//! │        │        └─────────────┬──────────────┘     │
//! │        │                      │                     │
//! │        │        ┌─────────────▼──────────────┐     │
//! │        └───────►│ verifier (DidVerifier)      │     │
//! │                 │ SM2 签名 + 证书链 + 跨域    │     │
//! │                 └────────────────────────────┘     │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! # 使用示例
//! ```
//! use eneros_did::{DidSystem, DidSystemImpl};
//! use eneros_did::types::{Did, DidDocument, AuthMethod};
//!
//! let mut system = DidSystemImpl::new();
//! let did = system.register_agent(42, "main").unwrap();
//! assert_eq!(did.to_did_string(), "did:eneros:main:42");
//! ```
//!
//! # no_std 合规
//! 所有代码 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 使用 `alloc::vec::Vec`、`alloc::string::String`、`alloc::collections::BTreeMap`，
//! 不使用 `std::*`。
//!
//! # 偏差声明
//! 1. **DID Method 规范**：当前仅实现 `did:eneros` 方法，未注册到 W3C DID Method Registry。
//! 2. **分布式存储**：DID 文档使用内存 BTreeMap 存储，生产环境需对接分布式账本或持久化存储。
//! 3. **密钥管理**：公钥由外部注入，私钥管理由 eneros-crypto 或 TPM 模块负责。
//! 4. **跨域认证**：当前为静态信任域列表，生产环境需对接动态信任协商协议。
//! 5. **证书链**：简化版证书链仅验证有效期和结构完整性，完整密码学链验证后续版本实现。
//! 6. **DID Resolution**：仅支持本地解析，远程 DID Resolution (HTTP/DHT) 后续按需添加。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod document;
pub mod identifier;
pub mod resolver;
pub mod types;
pub mod verifier;

// Re-exports
use alloc::string::String;
use alloc::vec::Vec;

pub use document::DocumentStore;
pub use identifier::{generate_did, parse_did};
pub use resolver::DidResolver;
pub use types::{AuthMethod, CertificateChain, Did, DidDocument, ServiceEndpoint};
pub use verifier::DidVerifier;

/// DID 系统错误类型.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DidError {
    /// DID 或文档未找到.
    NotFound,
    /// DID 格式无效.
    InvalidDid,
    /// 签名无效.
    InvalidSignature,
    /// 证书已过期.
    Expired,
    /// 不受信任的域.
    UntrustedDomain,
    /// DID 已存在.
    AlreadyExists,
    /// 文档内容无效.
    InvalidDocument,
}

/// DID 系统 trait.
///
/// 定义 Agent DID 身份体系的核心操作接口。
pub trait DidSystem {
    /// 注册 Agent 并生成 DID.
    fn register_agent(&mut self, agent_id: u64, network: &str) -> Result<Did, DidError>;

    /// 解析 DID 为 DID 文档.
    fn resolve(&self, did: &Did) -> Result<&DidDocument, DidError>;

    /// 验证消息签名.
    fn verify(&self, did_doc: &DidDocument, msg: &[u8], sig: &[u8]) -> Result<bool, DidError>;

    /// 跨域认证.
    fn cross_domain_auth(
        &self,
        doc: &DidDocument,
        domain: &str,
        trusted_domains: &[String],
    ) -> Result<bool, DidError>;
}

/// DID 系统默认实现.
///
/// 集成 DocumentStore、DidResolver、DidVerifier。
#[derive(Debug)]
pub struct DidSystemImpl {
    store: DocumentStore,
    verifier: DidVerifier,
}

impl DidSystemImpl {
    /// 创建 DID 系统实例.
    pub fn new() -> Self {
        Self {
            store: DocumentStore::new(),
            verifier: DidVerifier::new(),
        }
    }

    /// 获取文档存储的引用.
    pub fn store(&self) -> &DocumentStore {
        &self.store
    }

    /// 获取验证器的引用.
    pub fn verifier(&self) -> &DidVerifier {
        &self.verifier
    }
}

impl Default for DidSystemImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl DidSystem for DidSystemImpl {
    fn register_agent(&mut self, agent_id: u64, network: &str) -> Result<Did, DidError> {
        let did = identifier::generate_did(agent_id, network);
        // 创建默认 DID 文档（公钥待后续注入）
        let doc = DidDocument {
            id: did.clone(),
            public_key: Vec::new(),
            auth_methods: alloc::vec![AuthMethod::Sm2Signature],
            service_endpoints: Vec::new(),
            created: 0,
            updated: 0,
        };
        self.store.create(doc)?;
        Ok(did)
    }

    fn resolve(&self, did: &Did) -> Result<&DidDocument, DidError> {
        self.store.get(did)
    }

    fn verify(&self, did_doc: &DidDocument, msg: &[u8], sig: &[u8]) -> Result<bool, DidError> {
        self.verifier.verify_signature(did_doc, msg, sig)
    }

    fn cross_domain_auth(
        &self,
        doc: &DidDocument,
        domain: &str,
        trusted_domains: &[String],
    ) -> Result<bool, DidError> {
        self.verifier
            .cross_domain_auth(doc, domain, trusted_domains)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_resolve() {
        let mut system = DidSystemImpl::new();
        let did = system.register_agent(100, "main").unwrap();
        assert_eq!(did.to_did_string(), "did:eneros:main:100");

        let doc = system.resolve(&did).unwrap();
        assert_eq!(doc.id, did);
    }

    #[test]
    fn test_register_duplicate() {
        let mut system = DidSystemImpl::new();
        system.register_agent(1, "main").unwrap();
        assert_eq!(
            system.register_agent(1, "main"),
            Err(DidError::AlreadyExists)
        );
    }

    #[test]
    fn test_resolve_not_found() {
        let system = DidSystemImpl::new();
        let did = Did::new("eneros", "main", "ghost");
        assert_eq!(system.resolve(&did), Err(DidError::NotFound));
    }

    #[test]
    fn test_did_error_is_copy() {
        let err = DidError::NotFound;
        let err2 = err;
        assert_eq!(err, err2);
    }
}
