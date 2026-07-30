//! DID 核心类型定义.
//!
//! 定义 W3C DID 规范兼容的核心数据结构，包括 DID 标识符、
//! DID 文档、认证方法、服务端点和证书链。
//!
//! # 参考
//! - W3C DID Core 1.0: <https://www.w3.org/TR/did-core/>

use alloc::string::String;
use alloc::vec::Vec;

/// DID 标识符.
///
/// 格式: `did:<method>:<network>:<id>`
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Did {
    /// DID 方法名 (如 "eneros").
    pub method: String,
    /// 唯一标识符.
    pub id: String,
    /// 网络标识 (如 "main", "test").
    pub network: String,
}

impl Did {
    /// 创建新的 DID 实例.
    pub fn new(method: &str, network: &str, id: &str) -> Self {
        Self {
            method: String::from(method),
            id: String::from(id),
            network: String::from(network),
        }
    }

    /// 格式化为标准 DID 字符串: `did:<method>:<network>:<id>`.
    pub fn to_did_string(&self) -> String {
        alloc::format!("did:{}:{}:{}", self.method, self.network, self.id)
    }
}

/// 认证方法类型.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    /// SM2 数字签名认证.
    Sm2Signature,
    /// SM2 密钥协商认证.
    Sm2KeyAgreement,
}

/// 服务端点.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceEndpoint {
    /// 端点标识.
    pub id: String,
    /// 端点类型 (如 "AgentBus", "MQTT").
    pub endpoint_type: String,
    /// 端点 URL.
    pub url: String,
}

/// DID 文档 (W3C DID Document).
///
/// 包含公钥、认证方法、服务端点等元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DidDocument {
    /// 关联的 DID 标识符.
    pub id: Did,
    /// SM2 公钥 (未压缩格式, 65 字节).
    pub public_key: Vec<u8>,
    /// 支持的认证方法列表.
    pub auth_methods: Vec<AuthMethod>,
    /// 服务端点列表.
    pub service_endpoints: Vec<ServiceEndpoint>,
    /// 创建时间 (Unix 时间戳).
    pub created: u64,
    /// 最后更新时间 (Unix 时间戳).
    pub updated: u64,
}

/// 证书链.
///
/// 用于跨域认证，证明 subject_did 由 issuer_did 签发。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateChain {
    /// 根 DID (信任锚).
    pub root_did: Did,
    /// 签发者 DID.
    pub issuer_did: Did,
    /// 主体 DID.
    pub subject_did: Did,
    /// 签名 (SM2).
    pub signature: Vec<u8>,
    /// 有效期起始 (Unix 时间戳).
    pub valid_from: u64,
    /// 有效期截止 (Unix 时间戳).
    pub valid_until: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_did_creation() {
        let did = Did::new("eneros", "main", "12345");
        assert_eq!(did.method, "eneros");
        assert_eq!(did.network, "main");
        assert_eq!(did.id, "12345");
    }

    #[test]
    fn test_did_to_string() {
        let did = Did::new("eneros", "main", "12345");
        assert_eq!(did.to_did_string(), "did:eneros:main:12345");
    }

    #[test]
    fn test_did_to_string_test_network() {
        let did = Did::new("eneros", "test", "agent-001");
        assert_eq!(did.to_did_string(), "did:eneros:test:agent-001");
    }

    #[test]
    fn test_document_creation() {
        let did = Did::new("eneros", "main", "99");
        let doc = DidDocument {
            id: did.clone(),
            public_key: alloc::vec![0x04; 65],
            auth_methods: alloc::vec![AuthMethod::Sm2Signature],
            service_endpoints: alloc::vec![ServiceEndpoint {
                id: String::from("ep-1"),
                endpoint_type: String::from("AgentBus"),
                url: String::from("agentbus://node-1:9000"),
            }],
            created: 1_700_000_000,
            updated: 1_700_000_000,
        };
        assert_eq!(doc.id, did);
        assert_eq!(doc.public_key.len(), 65);
        assert_eq!(doc.auth_methods.len(), 1);
        assert_eq!(doc.service_endpoints.len(), 1);
    }

    #[test]
    fn test_certificate_chain_creation() {
        let root = Did::new("eneros", "main", "root");
        let issuer = Did::new("eneros", "main", "issuer");
        let subject = Did::new("eneros", "main", "subject");
        let chain = CertificateChain {
            root_did: root,
            issuer_did: issuer,
            subject_did: subject,
            signature: alloc::vec![0u8; 64],
            valid_from: 1_700_000_000,
            valid_until: 1_800_000_000,
        };
        assert_eq!(chain.root_did.id, "root");
        assert_eq!(chain.valid_until - chain.valid_from, 100_000_000);
    }
}
