//! DID 解析器.
//!
//! 将 DID 标识符解析为对应的 DID 文档。
//! 当前实现为本地解析（从 DocumentStore 查找）。

use crate::document::DocumentStore;
use crate::types::{Did, DidDocument};
use crate::DidError;

/// DID 解析器.
///
/// 从本地 DocumentStore 解析 DID 到 DidDocument。
#[derive(Debug)]
pub struct DidResolver<'a> {
    store: &'a DocumentStore,
}

impl<'a> DidResolver<'a> {
    /// 创建解析器，绑定到指定的文档存储.
    pub fn new(store: &'a DocumentStore) -> Self {
        Self { store }
    }

    /// 解析 DID 为 DID 文档.
    ///
    /// # 错误
    /// - [`DidError::NotFound`][]: 本地存储中未找到对应文档
    pub fn resolve(&self, did: &Did) -> Result<&DidDocument, DidError> {
        self.store.get(did)
    }

    /// 解析 DID 字符串为 DID 文档.
    ///
    /// 先解析字符串为 Did，再从存储中查找。
    ///
    /// # 错误
    /// - [`DidError::InvalidDid`][]: DID 字符串格式不合法
    /// - [`DidError::NotFound`][]: 本地存储中未找到对应文档
    pub fn resolve_str(&self, did_str: &str) -> Result<&DidDocument, DidError> {
        let did = crate::identifier::parse_did(did_str)?;
        self.resolve(&did)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AuthMethod;

    fn make_doc(id: &str) -> DidDocument {
        DidDocument {
            id: Did::new("eneros", "main", id),
            public_key: alloc::vec![0x04; 65],
            auth_methods: alloc::vec![AuthMethod::Sm2Signature],
            service_endpoints: alloc::vec![],
            created: 1_700_000_000,
            updated: 1_700_000_000,
        }
    }

    #[test]
    fn test_resolve_existing() {
        let mut store = DocumentStore::new();
        let doc = make_doc("42");
        store.create(doc.clone()).unwrap();

        let resolver = DidResolver::new(&store);
        let did = Did::new("eneros", "main", "42");
        let resolved = resolver.resolve(&did).unwrap();
        assert_eq!(resolved, &doc);
    }

    #[test]
    fn test_resolve_not_found() {
        let store = DocumentStore::new();
        let resolver = DidResolver::new(&store);
        let did = Did::new("eneros", "main", "999");
        assert_eq!(resolver.resolve(&did), Err(DidError::NotFound));
    }

    #[test]
    fn test_resolve_str_valid() {
        let mut store = DocumentStore::new();
        let doc = make_doc("7");
        store.create(doc.clone()).unwrap();

        let resolver = DidResolver::new(&store);
        let resolved = resolver.resolve_str("did:eneros:main:7").unwrap();
        assert_eq!(resolved, &doc);
    }

    #[test]
    fn test_resolve_str_invalid() {
        let store = DocumentStore::new();
        let resolver = DidResolver::new(&store);
        assert_eq!(resolver.resolve_str("invalid"), Err(DidError::InvalidDid));
    }

    #[test]
    fn test_resolve_str_not_found() {
        let store = DocumentStore::new();
        let resolver = DidResolver::new(&store);
        assert_eq!(
            resolver.resolve_str("did:eneros:main:missing"),
            Err(DidError::NotFound)
        );
    }
}
